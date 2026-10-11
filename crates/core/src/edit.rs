//! Code-aware edits: new line with indent, indent / outdent, comment toggling.
//!
//! Selections are sorted char ranges; each edit returns the selection to show afterwards.

use std::ops::Range;

use crate::byte_index;
use crate::check;
use crate::syntax::Language;

fn char_index(text: &str, byte: usize) -> usize {
    text[..byte].chars().count()
}

fn line_start(text: &str, byte: usize) -> usize {
    text[..byte].rfind('\n').map_or(0, |index| index + 1)
}

fn line_end(text: &str, byte: usize) -> usize {
    text[byte..]
        .find('\n')
        .map_or(text.len(), |index| byte + index)
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// The name of the element a line ends by opening: `<div class="a">`, not `</div>`,
/// `<br>` or `<img />`, which hold nothing.
fn opened_tag(line: &str) -> Option<&str> {
    let inside = line.trim_end().strip_suffix('>')?;
    let tag = &inside[inside.rfind('<')? + 1..];
    if tag.contains('>') || tag.ends_with('/') || tag.starts_with(['/', '!', '?']) {
        return None;
    }
    let named = |ch: char| ch.is_alphanumeric() || matches!(ch, '-' | ':' | '.');
    let name = tag.split(|ch| !named(ch)).next()?;
    let void = crate::format::VOID_TAGS.contains(&name.to_ascii_lowercase().as_str());
    (!name.is_empty() && !void).then_some(name)
}

/// Break the line at the selection, keeping the indent of the line and adding one level
/// after an opening bracket or a colon, and in `markup` after a tag that opens an element.
/// Returns the caret.
pub fn newline(text: &mut String, selection: Range<usize>, unit: &str, markup: bool) -> usize {
    let start = byte_index(text, selection.start);
    let end = byte_index(text, selection.end);
    let line = &text[line_start(text, start)..start];
    let lead: String = line
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect();
    // A comment after what opens a block does not hide that it opens one.
    let code = [" //", " #", "\t//", "\t#"]
        .iter()
        .filter_map(|marker| Some(line[..line.rfind(marker)?].trim_end()))
        .find(|code| code.ends_with(['{', '[', '(', ':']))
        .unwrap_or(line.trim_end());
    let before = code.chars().next_back();
    let after = text[end..].chars().next();
    let element = markup.then(|| opened_tag(line)).flatten();
    let mut inserted = format!("\n{lead}");
    if matches!(before, Some('{' | '[' | '(' | ':')) || element.is_some() {
        inserted.push_str(unit);
    }
    let caret = selection.start + inserted.chars().count();
    // Between a pair of brackets, or the two tags of an element, what closes goes on its
    // own line.
    let closed_here = element.is_some_and(|name| {
        let closing = text[end..].strip_prefix("</");
        closing.is_some_and(|rest| {
            rest.get(..name.len())
                .is_some_and(|found| found.eq_ignore_ascii_case(name))
                && rest[name.len()..].starts_with('>')
        })
    });
    if closed_here
        || matches!(
            (before, after),
            (Some('{'), Some('}')) | (Some('['), Some(']')) | (Some('('), Some(')'))
        )
    {
        inserted.push('\n');
        inserted.push_str(&lead);
    }
    text.replace_range(start..end, &inserted);
    caret
}

/// Byte range of the whole lines the selection touches, without the last line end.
fn whole_lines(text: &str, selection: &Range<usize>) -> Range<usize> {
    let start = byte_index(text, selection.start);
    let mut end = byte_index(text, selection.end);
    // A selection ending at the start of a line does not take that line.
    if end > start && text[..end].ends_with('\n') {
        end -= 1;
    }
    line_start(text, start)..line_end(text, end)
}

/// Copy the touched lines below themselves; the selection moves to the copy.
pub fn duplicate_lines(text: &mut String, selection: Range<usize>) -> Range<usize> {
    let block = whole_lines(text, &selection);
    let copy = format!("\n{}", &text[block.clone()]);
    let shift = copy.chars().count();
    text.insert_str(block.end, &copy);
    selection.start + shift..selection.end + shift
}

/// Remove the touched lines; the caret goes to the start of the line that takes their place.
pub fn delete_lines(text: &mut String, selection: Range<usize>) -> Range<usize> {
    let block = whole_lines(text, &selection);
    // Take a line end too: the one after, or on the last line the one before.
    let removed = if block.end < text.len() {
        block.start..block.end + 1
    } else {
        block.start.saturating_sub(1)..block.end
    };
    text.replace_range(removed.clone(), "");
    let caret = char_index(text, line_start(text, removed.start));
    caret..caret
}

/// Swap the touched lines with the line above or below; `None` at the edge of the text.
pub fn move_lines(text: &mut String, selection: Range<usize>, up: bool) -> Option<Range<usize>> {
    let block = whole_lines(text, &selection);
    if up {
        let above_end = block.start.checked_sub(1)?;
        let above = line_start(text, above_end)..above_end;
        let shift = text[above.clone()].chars().count() + 1;
        let moved = format!("{}\n{}", &text[block.clone()], &text[above.clone()]);
        text.replace_range(above.start..block.end, &moved);
        Some(selection.start - shift..selection.end - shift)
    } else {
        if block.end >= text.len() {
            return None;
        }
        let below = block.end + 1..line_end(text, block.end + 1);
        let shift = text[below.clone()].chars().count() + 1;
        let moved = format!("{}\n{}", &text[below.clone()], &text[block.clone()]);
        text.replace_range(block.start..below.end, &moved);
        Some(selection.start + shift..selection.end + shift)
    }
}

/// Replace the whole lines the selection touches by `change(lines)`.
fn rewrite(
    text: &mut String,
    selection: Range<usize>,
    change: impl FnOnce(&str) -> String,
) -> Range<usize> {
    let block = whole_lines(text, &selection);
    let first = char_index(text, block.start);
    let old = &text[block.clone()];
    let old_len = old.chars().count();
    let new = change(old);
    let new_len = new.chars().count();
    text.replace_range(block, &new);
    if selection.is_empty() {
        let caret = (selection.start + new_len)
            .saturating_sub(old_len)
            .max(first);
        caret..caret
    } else {
        first..first + new_len
    }
}

fn map_lines(block: &str, change: impl Fn(&str) -> String) -> String {
    block.split('\n').map(change).collect::<Vec<_>>().join("\n")
}

/// Tab: insert one indent at the caret, or indent every line of a multi-line selection.
pub fn indent(text: &mut String, selection: Range<usize>, unit: &str) -> Range<usize> {
    let bytes = byte_index(text, selection.start)..byte_index(text, selection.end);
    if !text[bytes.clone()].contains('\n') {
        text.replace_range(bytes, unit);
        let caret = selection.start + unit.chars().count();
        return caret..caret;
    }
    rewrite(text, selection, |block| {
        map_lines(block, |line| {
            if is_blank(line) {
                line.to_owned()
            } else {
                format!("{unit}{line}")
            }
        })
    })
}

/// Shift+Tab: remove one indent level from every line the selection touches.
pub fn outdent(text: &mut String, selection: Range<usize>, unit: &str) -> Range<usize> {
    let width = if unit.starts_with(' ') { unit.len() } else { 4 };
    rewrite(text, selection, |block| {
        map_lines(block, |line| {
            if let Some(rest) = line.strip_prefix('\t') {
                return rest.to_owned();
            }
            let spaces = line.len() - line.trim_start_matches(' ').len();
            line[spaces.min(width)..].to_owned()
        })
    })
}

/// Comment the touched lines with `prefix`, or uncomment them when they all are commented.
pub fn toggle_comment(text: &mut String, selection: Range<usize>, prefix: &str) -> Range<usize> {
    rewrite(text, selection, |block| {
        let code = || block.split('\n').filter(|line| !is_blank(line));
        let commented =
            code().next().is_some() && code().all(|line| line.trim_start().starts_with(prefix));
        if commented {
            return map_lines(block, |line| {
                let Some(at) = line.find(prefix) else {
                    return line.to_owned();
                };
                let rest = &line[at + prefix.len()..];
                format!("{}{}", &line[..at], rest.strip_prefix(' ').unwrap_or(rest))
            });
        }
        // Line the markers up on the least indented line.
        let column = code()
            .map(|line| line.len() - line.trim_start().len())
            .min()
            .unwrap_or(0);
        let every_line = code().next().is_none();
        map_lines(block, |line| {
            if is_blank(line) && !every_line {
                line.to_owned()
            } else {
                let column = column.min(line.len());
                format!("{}{prefix} {}", &line[..column], &line[column..])
            }
        })
    })
}

/// Join the touched lines into one, or the caret's line with the next: the indent of the
/// joined lines becomes one space.
pub fn join_lines(text: &mut String, selection: Range<usize>) -> Range<usize> {
    let mut block = whole_lines(text, &selection);
    if !text[block.clone()].contains('\n') {
        if block.end >= text.len() {
            return selection;
        }
        block.end = line_end(text, block.end + 1);
    }
    let first = char_index(text, block.start);
    let mut lines = text[block.clone()].split('\n');
    let mut joined = lines.next().unwrap_or("").trim_end().to_owned();
    // The caret ends where the first join was made.
    let caret = first + joined.chars().count();
    for line in lines.map(str::trim).filter(|line| !line.is_empty()) {
        joined.push(' ');
        joined.push_str(line);
    }
    text.replace_range(block, &joined);
    caret..caret
}

/// Upper-case the selection, or lower-case it when it already is upper-case.
pub fn toggle_case(text: &mut String, selection: Range<usize>) -> Range<usize> {
    let bytes = byte_index(text, selection.start)..byte_index(text, selection.end);
    let selected = &text[bytes.clone()];
    let changed = if selected.chars().any(char::is_lowercase) {
        selected.to_uppercase()
    } else {
        selected.to_lowercase()
    };
    let end = selection.start + changed.chars().count();
    text.replace_range(bytes, &changed);
    selection.start..end
}

/// The next larger thing around the selection: the word, what the nearest brackets hold,
/// those with their brackets, the lines, then the whole text.
pub fn expand_selection(text: &str, language: &Language, selection: &Range<usize>) -> Range<usize> {
    let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
    let chars: Vec<char> = text.chars().collect();
    let mut word = selection.clone();
    while word.start > 0 && is_word(chars[word.start - 1]) {
        word.start -= 1;
    }
    while word.end < chars.len() && is_word(chars[word.end]) {
        word.end += 1;
    }
    if word != *selection && chars[word.clone()].iter().all(|ch| is_word(*ch)) {
        return word;
    }
    if let Some((open, close)) =
        check::enclosing_brackets(text, language, selection.start, selection.end)
    {
        let inside = open + 1..close;
        return if inside == *selection {
            open..close + 1
        } else {
            inside
        };
    }
    let block = whole_lines(text, selection);
    let lines = char_index(text, block.start)..char_index(text, block.end);
    if lines != *selection && lines.start <= selection.start && lines.end >= selection.end {
        return lines;
    }
    0..chars.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_join_and_case_toggles() {
        let mut text = "let a = [\n    1,\n    2,\n];\nend".to_owned();
        assert_eq!(join_lines(&mut text, 0..25), 9..9);
        assert_eq!(text, "let a = [ 1, 2, ];\nend");
        assert_eq!(join_lines(&mut text, 3..3), 18..18);
        assert_eq!(text, "let a = [ 1, 2, ]; end");
        assert_eq!(join_lines(&mut text, 3..3), 3..3);

        let mut text = "say hello".to_owned();
        assert_eq!(toggle_case(&mut text, 4..9), 4..9);
        assert_eq!(text, "say HELLO");
        toggle_case(&mut text, 4..9);
        assert_eq!(text, "say hello");
    }

    #[test]
    fn the_selection_grows_step_by_step() {
        let text = "fn a() {\n    call(one, two);\n}";
        let language = Language::of(std::path::Path::new("a.rs"));
        let grow = |selection: Range<usize>| expand_selection(text, language, &selection);
        assert_eq!(grow(19..19), 18..21, "the word");
        assert_eq!(grow(18..21), 18..26, "inside the brackets");
        assert_eq!(grow(18..26), 17..27, "with the brackets");
        assert_eq!(grow(17..27), 8..29, "inside the braces");
        assert_eq!(grow(8..29), 7..30, "with the braces");
        assert_eq!(grow(7..30), 0..30, "the lines");
        assert_eq!(grow(0..30), 0..30);
    }

    #[test]
    fn lines_are_duplicated_deleted_and_moved() {
        let mut text = "one\ntwo\nthree".to_owned();
        assert_eq!(duplicate_lines(&mut text, 5..5), 9..9);
        assert_eq!(text, "one\ntwo\ntwo\nthree");

        assert_eq!(delete_lines(&mut text, 5..9), 4..4);
        assert_eq!(text, "one\nthree");
        assert_eq!(delete_lines(&mut text, 9..9), 0..0);
        assert_eq!(text, "one");

        let mut text = "a\nbb\nccc".to_owned();
        assert_eq!(move_lines(&mut text, 3..3, true), Some(1..1));
        assert_eq!(text, "bb\na\nccc");
        assert_eq!(move_lines(&mut text, 1..4, false), Some(5..8));
        assert_eq!(text, "ccc\nbb\na");
        assert_eq!(move_lines(&mut text, 0..0, true), None);
        assert_eq!(move_lines(&mut text, 8..8, false), None);
    }

    #[test]
    fn a_new_line_keeps_the_indent() {
        let mut text = "    let a = 1;".to_owned();
        let caret = newline(&mut text, 14..14, "    ", false);
        assert_eq!(text, "    let a = 1;\n    ");
        assert_eq!(caret, 19);
    }

    #[test]
    fn a_new_line_after_a_bracket_indents_and_splits_the_pair() {
        let mut text = "fn main() {}".to_owned();
        let caret = newline(&mut text, 11..11, "    ", false);
        assert_eq!(text, "fn main() {\n    \n}");
        assert_eq!(caret, 16);

        let mut text = "if ok:".to_owned();
        newline(&mut text, 6..6, "  ", false);
        assert_eq!(text, "if ok:\n  ");
    }

    #[test]
    fn a_new_line_after_a_tag_indents_and_splits_the_element() {
        let broken = |text: &str, at: usize| {
            let mut text = text.to_owned();
            let caret = newline(&mut text, at..at, "  ", true);
            (text, caret)
        };
        // Between the two tags of an element: the closing one goes under the opening one.
        assert_eq!(
            broken("<body></body>", 6),
            ("<body>\n  \n</body>".to_owned(), 9)
        );
        assert_eq!(
            broken("  <div class=\"a\"></DIV>", 17),
            ("  <div class=\"a\">\n    \n  </DIV>".to_owned(), 22)
        );
        // After a tag that opens, one level deeper; after any other, the same level.
        assert_eq!(broken("<ul>", 4), ("<ul>\n  ".to_owned(), 7));
        assert_eq!(broken("<ul><li>a</li>", 14).0, "<ul><li>a</li>\n");
        assert_eq!(broken("  <br>", 6).0, "  <br>\n  ");
        assert_eq!(broken("<img src=\"a\" />", 16).0, "<img src=\"a\" />\n");
        assert_eq!(broken("<!doctype html>", 15).0, "<!doctype html>\n");
        // What holds another element still splits from its own closing tag only.
        assert_eq!(broken("<p></b>", 3).0, "<p>\n  </b>");
        // A comment after the bracket changes nothing; one after anything else neither.
        let mut noted = "fn a() { // why".to_owned();
        newline(&mut noted, 15..15, "    ", false);
        assert_eq!(noted, "fn a() { // why\n    ");
        let mut python = "if ok:  # sure".to_owned();
        newline(&mut python, 14..14, "    ", false);
        assert_eq!(python, "if ok:  # sure\n    ");
        let mut plain = "let a = 1; // see http://x".to_owned();
        newline(&mut plain, 26..26, "    ", false);
        assert_eq!(plain, "let a = 1; // see http://x\n");
        // In code a `>` ends no tag.
        let mut code = "let a: Vec<String>".to_owned();
        newline(&mut code, 18..18, "    ", false);
        assert_eq!(code, "let a: Vec<String>\n");
    }

    #[test]
    fn tab_inserts_at_the_caret_and_indents_selected_lines() {
        let mut text = "ab".to_owned();
        assert_eq!(indent(&mut text, 1..1, "  "), 3..3);
        assert_eq!(text, "a  b");

        let mut text = "one\n\ntwo\nthree".to_owned();
        let selection = indent(&mut text, 1..7, "\t");
        assert_eq!(text, "\tone\n\n\ttwo\nthree");
        assert_eq!(selection, 0..10);
    }

    #[test]
    fn outdent_removes_one_level() {
        let mut text = "    a\n  b\n\tc\nd".to_owned();
        outdent(&mut text, 0..13, "    ");
        assert_eq!(text, "a\nb\nc\nd");

        let mut text = "        deep".to_owned();
        assert_eq!(outdent(&mut text, 10..10, "    "), 6..6);
        assert_eq!(text, "    deep");
    }

    #[test]
    fn comments_toggle_on_and_off() {
        let mut text = "fn a() {\n    b();\n\n    c();\n}".to_owned();
        let selection = toggle_comment(&mut text, 10..27, "//");
        assert_eq!(text, "fn a() {\n    // b();\n\n    // c();\n}");
        toggle_comment(&mut text, selection, "//");
        assert_eq!(text, "fn a() {\n    b();\n\n    c();\n}");
    }

    #[test]
    fn a_caret_comments_its_line_and_follows_the_text() {
        let mut text = "x = 1".to_owned();
        assert_eq!(toggle_comment(&mut text, 2..2, "#"), 4..4);
        assert_eq!(text, "# x = 1");
        assert_eq!(toggle_comment(&mut text, 4..4, "#"), 2..2);
        assert_eq!(text, "x = 1");
    }
}
