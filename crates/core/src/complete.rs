//! Completion of the word being typed: snippets, keywords and words of the file.

use std::collections::HashSet;

use crate::byte_index;
use crate::syntax::Language;

const MAX_COMPLETIONS: usize = 30;
/// Shorter words are quicker typed than picked.
const MIN_WORD_CHARS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Snippet,
    Keyword,
    Word,
    /// A name the language server knows: a method, a field, a type.
    Symbol,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    /// What is inserted. In snippets a tab is one indent level and `$0` where the caret ends.
    pub body: String,
    pub kind: Kind,
}

type Snippets = &'static [(&'static str, &'static str)];

const C_LIKE: Snippets = &[
    ("for", "for (int i = 0; i < $0; i++) {\n\t\n}"),
    ("if", "if ($0) {\n\t\n}"),
    ("while", "while ($0) {\n\t\n}"),
];

const SCRIPT: Snippets = &[
    ("function", "function $0() {\n\t\n}"),
    ("for", "for (let i = 0; i < $0; i++) {\n\t\n}"),
    ("forof", "for (const item of $0) {\n\t\n}"),
    ("if", "if ($0) {\n\t\n}"),
    ("class", "class $0 {\n\tconstructor() {\n\t\t\n\t}\n}"),
    ("log", "console.log($0);"),
    ("try", "try {\n\t$0\n} catch (error) {\n\t\n}"),
];

/// Snippets by language name.
const SNIPPETS: &[(&str, Snippets)] = &[
    (
        "Rust",
        &[
            ("fn", "fn $0() {\n\t\n}"),
            ("for", "for $0 in  {\n\t\n}"),
            ("if", "if $0 {\n\t\n}"),
            ("impl", "impl $0 {\n\t\n}"),
            ("struct", "struct $0 {\n\t\n}"),
            ("enum", "enum $0 {\n\t\n}"),
            ("match", "match $0 {\n\t_ => {}\n}"),
            ("println", "println!(\"$0\");"),
            ("test", "#[test]\nfn $0() {\n\t\n}"),
        ],
    ),
    (
        "Python",
        &[
            ("def", "def $0():\n\tpass"),
            ("class", "class $0:\n\tdef __init__(self):\n\t\tpass"),
            ("for", "for $0 in :\n\tpass"),
            ("if", "if $0:\n\tpass"),
            ("ifmain", "if __name__ == \"__main__\":\n\t$0"),
            ("print", "print($0)"),
            ("try", "try:\n\t$0\nexcept Exception as error:\n\tpass"),
        ],
    ),
    ("JavaScript", SCRIPT),
    ("TypeScript", SCRIPT),
    (
        "Go",
        &[
            ("func", "func $0() {\n\t\n}"),
            ("for", "for $0 {\n\t\n}"),
            ("if", "if $0 {\n\t\n}"),
            ("iferr", "if err != nil {\n\treturn $0\n}"),
        ],
    ),
    (
        "HTML",
        &[(
            "html5",
            "<!doctype html>\n<html lang=\"en\">\n<head>\n\t<meta charset=\"utf-8\">\n\t<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\t<title>$0</title>\n</head>\n<body>\n\t\n</body>\n</html>",
        )],
    ),
    ("C", C_LIKE),
    ("C++", C_LIKE),
    ("Java", C_LIKE),
    ("C#", C_LIKE),
];

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Char index where the word ending at `caret` starts.
pub fn word_start(text: &str, caret: usize) -> usize {
    let before = &text[..byte_index(text, caret)];
    caret
        - before
            .chars()
            .rev()
            .take_while(|ch| is_word_char(*ch))
            .count()
}

/// What the word ending at `caret` could become, best first. Empty when no word is there.
pub fn complete(text: &str, caret: usize, language: &Language) -> Vec<Completion> {
    let end = byte_index(text, caret);
    let prefix = &text[byte_index(text, word_start(text, caret))..end];
    if prefix.is_empty() || prefix.starts_with(|ch: char| ch.is_ascii_digit()) {
        return Vec::new();
    }
    let wanted = prefix.to_lowercase();
    let fits = |label: &str| label.to_lowercase().starts_with(&wanted);
    let completion = |label: &str, body: &str, kind| Completion {
        label: label.to_owned(),
        body: body.to_owned(),
        kind,
    };

    let snippets = SNIPPETS
        .iter()
        .find(|(name, _)| *name == language.name)
        .map_or(&[][..], |(_, snippets)| snippets);
    let mut found: Vec<Completion> = snippets
        .iter()
        .filter(|(label, _)| fits(label))
        .map(|(label, body)| completion(label, body, Kind::Snippet))
        .collect();

    // A word typed in full has nothing left to complete.
    let mut seen: HashSet<&str> = HashSet::from([prefix]);
    for keyword in language.keywords() {
        if fits(keyword) && seen.insert(keyword) {
            found.push(completion(keyword, keyword, Kind::Keyword));
        }
    }
    let mut words: Vec<&str> = text
        .split(|ch: char| !is_word_char(ch))
        .filter(|word| word.chars().count() >= MIN_WORD_CHARS && fits(word))
        .filter(|word| seen.insert(word))
        .collect();
    words.sort_by_key(|word| (word.len(), *word));
    found.extend(
        words
            .into_iter()
            .map(|word| completion(word, word, Kind::Word)),
    );
    found.truncate(MAX_COMPLETIONS);
    found
}

/// What a tag may be named with.
fn in_tag_name(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '-' | ':' | '.')
}

/// The tags named `name` in `text`, in order: `true` for one that opens an element, `false`
/// for one that closes it. A tag that closes itself (`<a />`) is neither.
fn tags_named<'a>(text: &'a str, name: &'a str) -> impl Iterator<Item = bool> + 'a {
    text.match_indices('<').filter_map(move |(at, _)| {
        let rest = &text[at + 1..];
        let (closes, rest) = match rest.strip_prefix('/') {
            Some(rest) => (true, rest),
            None => (false, rest),
        };
        let found = rest.get(..name.len())?;
        let longer = rest[name.len()..].chars().next().is_some_and(in_tag_name);
        if !found.eq_ignore_ascii_case(name) || longer {
            return None;
        }
        let empty = rest.find('>').is_some_and(|end| rest[..end].ends_with('/'));
        (closes || !empty).then_some(!closes)
    })
}

/// The closing tag to write after a `>` typed between `before` and `after`, when that `>`
/// ends an opening tag that wants one: not when a closing tag after it is already waiting
/// for it.
pub fn closing_tag(before: &str, after: &str) -> Option<String> {
    let opened = before.rfind('<')?;
    let tag = &before[opened + 1..];
    if tag.contains('>') || tag.ends_with('/') {
        return None;
    }
    let name: String = tag.chars().take_while(|ch| in_tag_name(*ch)).collect();
    let void = crate::format::VOID_TAGS.contains(&name.to_ascii_lowercase().as_str());
    if name.is_empty() || void {
        return None;
    }
    // Elements of that name still open before this one: each has a closing tag coming.
    let open = tags_named(&before[..opened], &name).fold(0_usize, |open, opens| match opens {
        true => open + 1,
        false => open.saturating_sub(1),
    });
    // Closing tags after it that close nothing opened there: one more of them than of
    // those elements, and it is the one this tag was missing.
    let (mut depth, mut spare) = (0_usize, 0_usize);
    for opens in tags_named(after, &name) {
        match (opens, depth) {
            (true, _) => depth += 1,
            (false, 0) => spare += 1,
            (false, _) => depth -= 1,
        }
    }
    (spare <= open).then(|| format!("</{name}>"))
}

/// The element still open at the end of `before`: its name, and the byte its opening tag
/// starts at.
pub fn unclosed_tag(before: &str) -> Option<(String, usize)> {
    let mut open: Vec<(String, usize)> = Vec::new();
    for (at, _) in before.match_indices('<') {
        let rest = &before[at + 1..];
        let (closes, rest) = match rest.strip_prefix('/') {
            Some(rest) => (true, rest),
            None => (false, rest),
        };
        let name: String = rest.chars().take_while(|ch| in_tag_name(*ch)).collect();
        // A tag still being written closes and opens nothing yet.
        let Some(end) = rest.find('>').filter(|_| !name.is_empty()) else {
            continue;
        };
        let same = |(other, _): &(String, usize)| other.eq_ignore_ascii_case(&name);
        if closes {
            // What was left open inside it is closed with it.
            if let Some(found) = open.iter().rposition(same) {
                open.truncate(found);
            }
        } else if !rest[..end].ends_with('/')
            && !crate::format::VOID_TAGS.contains(&name.to_ascii_lowercase().as_str())
        {
            open.push((name, at));
        }
    }
    open.pop()
}

/// Replace the word ending at `caret` by `completion`. Returns the new caret.
pub fn apply(text: &mut String, caret: usize, completion: &Completion, unit: &str) -> usize {
    let start = word_start(text, caret);
    let bytes = byte_index(text, start)..byte_index(text, caret);
    // The lines of a snippet continue the indent of the line it starts on.
    let line_start = text[..bytes.start].rfind('\n').map_or(0, |index| index + 1);
    let lead: String = text[line_start..]
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect();
    let body = completion
        .body
        .replace('\t', unit)
        .replace('\n', &format!("\n{lead}"));
    let (body, offset) = match body.find("$0") {
        Some(at) => (body.replacen("$0", "", 1), body[..at].chars().count()),
        None => {
            let len = body.chars().count();
            (body, len)
        }
    };
    text.replace_range(bytes, &body);
    start + offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn labels(text: &str, file: &str) -> Vec<(String, Kind)> {
        complete(text, text.chars().count(), Language::of(Path::new(file)))
            .into_iter()
            .map(|completion| (completion.label, completion.kind))
            .collect()
    }

    #[test]
    fn snippets_keywords_then_words_are_offered() {
        let found = labels("let matcher = 1;\nlet mate = 2;\nma", "a.rs");
        assert_eq!(
            found,
            [
                ("match".to_owned(), Kind::Snippet),
                ("match".to_owned(), Kind::Keyword),
                ("mate".to_owned(), Kind::Word),
                ("matcher".to_owned(), Kind::Word),
            ]
        );
    }

    #[test]
    fn nothing_is_offered_without_a_word_or_for_a_finished_one() {
        assert!(labels("total = 1\n", "a.py").is_empty());
        assert!(labels("total = 12", "a.py").is_empty());
        assert!(labels("total = 1\ntotal", "a.py").is_empty());
        assert_eq!(
            labels("étage = 1\nét", "a.py"),
            [("étage".to_owned(), Kind::Word)]
        );
    }

    #[test]
    fn a_snippet_is_indented_and_places_the_caret() {
        let mut text = "class A:\n    de".to_owned();
        let found = complete(&text, 15, Language::of(Path::new("a.py")));
        assert_eq!(found[0].label, "def");
        let caret = apply(&mut text, 15, &found[0], "    ");
        assert_eq!(text, "class A:\n    def ():\n        pass");
        assert_eq!(caret, 17);
    }

    #[test]
    fn an_opening_tag_gets_its_closing_tag() {
        assert_eq!(
            closing_tag("<div class=\"a\"", "").as_deref(),
            Some("</div>")
        );
        assert_eq!(closing_tag("<p>text <b", "</p>").as_deref(), Some("</b>"));
        let none = [
            ("<br", ""),
            ("</div", ""),
            ("<img /", ""),
            ("a ", ""),
            ("<b", "</b>"),
        ];
        for (before, after) in none {
            assert_eq!(closing_tag(before, after), None, "{before}");
        }
        // A closing tag already there, further down, is the one: no second one is written.
        assert_eq!(
            closing_tag("<html>\n<body", "\n<p>a</p>\n</body>\n</html>"),
            None
        );
        assert_eq!(closing_tag("<BODY", "\n</body>"), None);
        // One that closes an element opened before is not for this tag.
        assert_eq!(
            closing_tag("<div>\n  <div", "\n</div>").as_deref(),
            Some("</div>")
        );
        assert_eq!(closing_tag("<div>\n  <div", "\n  </div>\n</div>"), None);
        // Nor is one that closes an element opened after it.
        assert_eq!(closing_tag("<p", "<p>a</p>").as_deref(), Some("</p>"));
        // A name that only starts the same is another element.
        assert_eq!(closing_tag("<b", "</body>").as_deref(), Some("</b>"));
    }

    #[test]
    fn the_element_still_open_is_found() {
        let text = "<html>\n  <body>\n    <p>a <b>b</b><br></p>\n    <img src=\"x\" />\n    ";
        assert_eq!(unclosed_tag(text), Some(("body".to_owned(), 9)));
        assert_eq!(unclosed_tag("<ul><li>a"), Some(("li".to_owned(), 4)));
        // A closing tag closes what was left open inside its element too.
        assert_eq!(unclosed_tag("<ul><li>a</ul>"), None);
        assert_eq!(unclosed_tag("<!doctype html>\n"), None);
        assert_eq!(unclosed_tag("<div"), None);
    }

    #[test]
    fn a_word_replaces_what_was_typed() {
        let mut text = "counter = 0\ncou + 1".to_owned();
        let found = complete(&text, 15, Language::of(Path::new("a.py")));
        let caret = apply(&mut text, 15, &found[0], "    ");
        assert_eq!(text, "counter = 0\ncounter + 1");
        assert_eq!(caret, 19);
    }
}
