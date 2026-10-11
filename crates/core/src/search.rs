//! Finding text: in the open file and across the files of a folder.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{byte_index, decode};

/// Folders never searched nor listed in "go to file": build output and dependencies.
pub const SKIPPED_DIRS: [&str; 9] = [
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    ".venv",
    "__pycache__",
    ".next",
    ".idea",
];
/// Files above this size are not searched.
const MAX_SEARCHED_BYTES: u64 = 2 * 1024 * 1024;
const MAX_FILES: usize = 50_000;
const PREVIEW_CHARS: usize = 200;

/// One occurrence in a text; positions in chars, `line` and `column` counted from 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub start: usize,
    pub len: usize,
    pub line: usize,
    pub column: usize,
}

/// One occurrence in a file of the folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    /// Position in the file's text.
    pub at: Match,
    /// The line holding the occurrence, trimmed.
    pub preview: String,
}

/// Lowercase when case is ignored, keeping byte offsets valid in the original.
fn fold(text: &str, match_case: bool) -> Cow<'_, str> {
    if match_case {
        return Cow::Borrowed(text);
    }
    let lower = text.to_lowercase();
    if lower.len() == text.len() {
        Cow::Owned(lower)
    } else {
        Cow::Owned(text.to_ascii_lowercase())
    }
}

/// Byte offsets in `line` where `needle` (already folded) occurs, without overlaps.
fn offsets(line: &str, needle: &str, match_case: bool) -> Vec<usize> {
    let hay = fold(line, match_case);
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(position) = hay[from..].find(needle) {
        let at = from + position;
        if line.is_char_boundary(at) && line.is_char_boundary(at + needle.len()) {
            found.push(at);
        }
        from = at + needle.len();
    }
    found
}

/// Every occurrence of `query` in `text`. Queries do not span lines.
pub fn find(text: &str, query: &str, match_case: bool) -> Vec<Match> {
    if query.is_empty() || query.contains('\n') {
        return Vec::new();
    }
    let needle = fold(query, match_case);
    let len = query.chars().count();
    let mut matches = Vec::new();
    let mut line_start = 0;
    for (line, content) in text.split('\n').enumerate() {
        let mut column = 0;
        let mut counted = 0;
        for at in offsets(content, &needle, match_case) {
            column += content[counted..at].chars().count();
            counted = at;
            matches.push(Match {
                start: line_start + column,
                len,
                line,
                column,
            });
        }
        line_start += column + content[counted..].chars().count() + 1;
    }
    matches
}

/// Every occurrence of `word` in `text` that is a whole word, not a part of a longer one.
pub fn find_word(text: &str, word: &str) -> Vec<Match> {
    let is_word = |ch: &char| ch.is_alphanumeric() || *ch == '_';
    let chars: Vec<char> = text.chars().collect();
    let mut found = find(text, word, true);
    found.retain(|at| {
        let before = at.start.checked_sub(1).and_then(|index| chars.get(index));
        let after = chars.get(at.start + at.len);
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    });
    found
}

/// `text` with every occurrence of `query` replaced, and how many there were.
pub fn replace_all(text: &str, query: &str, with: &str, match_case: bool) -> (String, usize) {
    let matches = find(text, query, match_case);
    let mut replaced = String::with_capacity(text.len());
    let mut copied = 0;
    let mut byte = 0;
    let mut index = 0;
    for found in &matches {
        // Walk forward from the previous occurrence instead of from the start each time.
        byte += byte_index(&text[byte..], found.start - index);
        index = found.start;
        replaced.push_str(&text[copied..byte]);
        replaced.push_str(with);
        copied = byte + byte_index(&text[byte..], found.len);
    }
    replaced.push_str(&text[copied..]);
    (replaced, matches.len())
}

/// `text` with every whole word `old` replaced by `new`, and how many there were.
pub fn rename_word(text: &str, old: &str, new: &str) -> (String, usize) {
    let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
    let mut renamed = String::with_capacity(text.len());
    let mut count = 0;
    let mut word_start = None;
    // One char past the end closes a word that ends the text.
    for (index, ch) in text.char_indices().chain([(text.len(), '\n')]) {
        if is_word(ch) {
            word_start.get_or_insert(index);
            continue;
        }
        if let Some(start) = word_start.take() {
            let word = &text[start..index];
            if word == old {
                count += 1;
                renamed.push_str(new);
            } else {
                renamed.push_str(word);
            }
        }
        if index < text.len() {
            renamed.push(ch);
        }
    }
    (renamed, count)
}

/// Every file under `root`, sorted, leaving out [`SKIPPED_DIRS`] and links.
/// Stops early when `cancelled` is set.
pub fn files(root: &Path, cancelled: &AtomicBool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        if cancelled.load(Ordering::Relaxed) || found.len() >= MAX_FILES {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            if kind.is_dir() {
                if !SKIPPED_DIRS.iter().any(|skipped| name == *skipped) {
                    pending.push(entry.path());
                }
            } else if kind.is_file() {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    found
}

/// Occurrences of `query` in the file at `path`; none for binary, huge or unreadable files.
pub fn in_file(path: &Path, query: &str, match_case: bool) -> Vec<Hit> {
    let small = std::fs::metadata(path).is_ok_and(|meta| meta.len() <= MAX_SEARCHED_BYTES);
    if !small {
        return Vec::new();
    }
    let Some(decoded) = std::fs::read(path).ok().and_then(|bytes| decode(&bytes)) else {
        return Vec::new();
    };
    let lines: Vec<&str> = decoded.text.split('\n').collect();
    find(&decoded.text, query, match_case)
        .into_iter()
        .map(|at| Hit {
            path: path.to_path_buf(),
            preview: lines[at.line].trim().chars().take(PREVIEW_CHARS).collect(),
            at,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_carry_char_positions() {
        let found = find("één Een\nxx een", "een", false);
        let starts: Vec<usize> = found.iter().map(|found| found.start).collect();
        assert_eq!(starts, [4, 11]);
        assert_eq!((found[1].line, found[1].column, found[1].len), (1, 3, 3));
    }

    #[test]
    fn whole_words_are_renamed() {
        let (text, count) = rename_word("id = id_of(x); // id\nvalid", "id", "key");
        assert_eq!(text, "key = id_of(x); // key\nvalid");
        assert_eq!(count, 2);
        assert_eq!(rename_word("été", "été", "hiver"), ("hiver".to_owned(), 1));
    }

    #[test]
    fn case_can_matter() {
        assert_eq!(find("Foo foo FOO", "foo", true).len(), 1);
        assert_eq!(find("Foo foo FOO", "foo", false).len(), 3);
        assert!(find("abc", "", false).is_empty());
    }

    #[test]
    fn replace_all_rewrites_every_occurrence() {
        let (text, count) = replace_all("é cat, Cat\ncat", "cat", "dog", false);
        assert_eq!(text, "é dog, dog\ndog");
        assert_eq!(count, 3);
        assert_eq!(
            replace_all("same", "zzz", "x", true),
            ("same".to_owned(), 0)
        );
    }

    #[test]
    fn folders_are_walked_and_searched() {
        let root = std::env::temp_dir().join(format!("godcode-search-{}", std::process::id()));
        let nested = root.join("src");
        std::fs::create_dir_all(&nested).expect("folder");
        std::fs::create_dir_all(root.join("target")).expect("folder");
        std::fs::write(nested.join("a.rs"), "fn main() {}\n  // main here\n").expect("file");
        std::fs::write(root.join("target").join("b.rs"), "main").expect("file");
        std::fs::write(root.join("blob.bin"), b"main\x00").expect("file");

        let listed = files(&root, &AtomicBool::new(false));
        assert_eq!(listed, [root.join("blob.bin"), nested.join("a.rs")]);
        let hits: Vec<Hit> = listed
            .iter()
            .flat_map(|path| in_file(path, "main", true))
            .collect();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].preview, "// main here");
        assert_eq!(hits[1].at.line, 1);
        std::fs::remove_dir_all(&root).expect("cleanup");
    }
}
