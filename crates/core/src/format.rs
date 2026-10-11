//! Reformat Code: line breaks and indentation rebuilt from the structure of the text.
//!
//! Only white space is added or removed, and never inside a string. A minified file comes
//! out one statement, one rule or one tag per line.

use crate::syntax::{self, Language, Token};

/// Languages whose blocks are braces: their lines are broken and indented from the brackets.
const BRACED: [&str; 13] = [
    "JavaScript",
    "TypeScript",
    "CSS",
    "JSON",
    "Rust",
    "C",
    "C++",
    "C#",
    "Java",
    "Kotlin",
    "Swift",
    "PHP",
    "Go",
];
/// A `{…}` longer than this never stays on one line.
const INLINE_BYTES: usize = 60;
/// A tag, its text and its closing tag stay on one line up to this length.
const COMPACT_BYTES: usize = 120;

/// HTML elements that flow with the text around them: they never start a line of their own.
const INLINE_TAGS: [&str; 26] = [
    "a", "abbr", "b", "bdi", "bdo", "br", "cite", "code", "em", "i", "img", "kbd", "mark", "q",
    "s", "samp", "small", "span", "strong", "sub", "sup", "time", "u", "var", "wbr", "del",
];
/// HTML elements without content nor closing tag.
pub(crate) const VOID_TAGS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];
/// HTML elements whose content is not markup.
const RAW_TAGS: [&str; 4] = ["script", "style", "pre", "textarea"];
/// HTML elements whose closing tag may be left out: the next one of the group closes them.
const IMPLIED_END: [&[&str]; 5] = [&["li"], &["p"], &["tr"], &["td", "th"], &["dt", "dd"]];

/// Whether [`format`] rebuilds the lines of this language, beyond tidying their ends.
pub fn restructures(language: &Language) -> bool {
    language.is_markup() || BRACED.contains(&language.name)
}

/// `text` reformatted, indenting with `unit`.
pub fn format(text: &str, language: &Language, unit: &str) -> String {
    let body = if language.is_markup() {
        markup(text, language.name == "HTML", unit)
    } else if BRACED.contains(&language.name) {
        braces(text, language, unit, 0)
    } else if language.name == "Markdown" {
        // Two spaces at the end of a line are a line break there.
        text.to_owned()
    } else {
        text.lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let body = body.trim_end();
    if body.is_empty() {
        String::new()
    } else {
        format!("{body}\n")
    }
}

/// Bytes of the longest line of `text`.
pub fn longest_line(text: &str) -> usize {
    text.split('\n').map(str::len).max().unwrap_or(0)
}

// ------------------------------------------------------------------ braces

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Space,
    /// A string or a comment: copied as it is.
    Opaque {
        comment: bool,
        line_comment: bool,
    },
    Word,
    Punct,
    Open(u8),
    Close(u8),
    Semi,
    Comma,
}

#[derive(Clone, Copy, Debug)]
struct Atom {
    start: usize,
    end: usize,
    kind: Kind,
}

/// What goes between the last piece written and the next one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gap {
    None,
    Space,
    /// The white space of the source, bytes `start..end`.
    Keep(usize, usize),
    /// A new line, unless a comment follows on the same line.
    Soft,
    /// After a closing brace: a new line before a statement, nothing before `)` or `;`.
    AfterBlock,
    Line {
        blank: bool,
    },
}

/// A bracket still open.
struct Open {
    bracket: u8,
    /// It closes on the line it opens on.
    inline: bool,
    /// An object literal: one member per line.
    object: bool,
    /// A `case` was seen in it: what follows the cases is one level deeper.
    cases: bool,
    /// The output line it opened on: brackets opened on one line indent once, as in `f({`.
    line: usize,
}

/// How deep a line is inside `stack`, not counting brackets opened on `closing` line.
fn levels_in(stack: &[Open], closing: Option<usize>) -> usize {
    let mut levels = 0;
    let mut last = None;
    for open in stack {
        if last != Some(open.line) && closing != Some(open.line) {
            levels += 1;
        }
        last = Some(open.line);
        levels += usize::from(open.cases);
    }
    levels
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '$'
}

/// 0 for white space, 1 for word chars, 2 for operators, 3 for what stands alone.
fn class(ch: char) -> u8 {
    match ch {
        _ if ch.is_whitespace() => 0,
        _ if is_word_char(ch) => 1,
        '(' | '[' | '{' | ')' | ']' | '}' | ';' | ',' => 3,
        _ => 2,
    }
}

fn atoms(text: &str, language: &Language) -> Vec<Atom> {
    let mut found = Vec::new();
    let mut start = 0;
    for span in syntax::highlight(text, language) {
        let piece = &text[start..span.end];
        if matches!(span.token, Token::String | Token::Comment) {
            let comment = span.token == Token::Comment;
            found.push(Atom {
                start,
                end: span.end,
                kind: Kind::Opaque {
                    comment,
                    line_comment: comment && language.starts_line_comment(piece),
                },
            });
            start = span.end;
            continue;
        }
        let mut chars = piece.char_indices().peekable();
        while let Some((offset, ch)) = chars.next() {
            let kind = match ch {
                '(' | '[' | '{' => Kind::Open(ch as u8),
                ')' | ']' | '}' => Kind::Close(ch as u8),
                ';' => Kind::Semi,
                ',' => Kind::Comma,
                _ if ch.is_whitespace() => Kind::Space,
                _ if is_word_char(ch) => Kind::Word,
                _ => Kind::Punct,
            };
            let mut end = offset + ch.len_utf8();
            // Runs of spaces, of word chars and of operators are one piece each.
            if class(ch) != 3 {
                while let Some((next, same)) = chars.next_if(|(_, next)| class(*next) == class(ch))
                {
                    end = next + same.len_utf8();
                }
            }
            found.push(Atom {
                start: start + offset,
                end: start + end,
                kind,
            });
        }
        start = span.end;
    }
    found
}

/// The brace opened by atom `at` closes soon, on its line, around something simple.
fn closes_inline(text: &str, list: &[Atom], at: usize) -> bool {
    let mut bytes = 0;
    for atom in &list[at + 1..] {
        let piece = &text[atom.start..atom.end];
        // White space does not count: formatting adds some, and must not change the answer.
        if atom.kind != Kind::Space {
            bytes += piece.len();
        }
        if bytes > INLINE_BYTES {
            return false;
        }
        match atom.kind {
            Kind::Close(b'}') => return true,
            Kind::Open(b'{') | Kind::Semi => return false,
            Kind::Opaque { line_comment, .. } if line_comment || piece.contains('\n') => {
                return false;
            }
            Kind::Space if piece.contains('\n') => return false,
            _ => {}
        }
    }
    false
}

/// Start a new line `levels` deep, dropping the spaces that ended the last one.
fn break_line(out: &mut String, blank: bool, levels: usize, unit: &str) {
    out.truncate(out.trim_end_matches([' ', '\t']).len());
    if out.is_empty() {
        return;
    }
    out.push('\n');
    if blank {
        out.push('\n');
    }
    for _ in 0..levels {
        out.push_str(unit);
    }
}

/// Copy a comment; when it starts its line, its other lines move along with the first.
fn push_comment(out: &mut String, text: &str, atom: &Atom) {
    let piece = &text[atom.start..atom.end];
    let line_start = text[..atom.start].rfind('\n').map_or(0, |index| index + 1);
    let old = &text[line_start..atom.start];
    let line_now = out.rfind('\n').map_or(0, |index| index + 1);
    let new = out[line_now..].to_owned();
    let blank = |lead: &str| lead.chars().all(char::is_whitespace);
    if !blank(old) || !blank(&new) || !piece.contains('\n') {
        out.push_str(piece);
        return;
    }
    for (index, line) in piece.split('\n').enumerate() {
        if index == 0 {
            out.push_str(line);
            continue;
        }
        out.push('\n');
        let line = line.strip_prefix(old).unwrap_or_else(|| line.trim_start());
        if !line.is_empty() {
            out.push_str(&new);
        }
        out.push_str(line);
    }
}

/// Reformat brace-delimited code; every line but the first starts `base` levels deep.
fn braces(text: &str, language: &Language, unit: &str, base: usize) -> String {
    let css = language.name == "CSS";
    let json = language.name == "JSON";
    let list = atoms(text, language);
    let piece_of = |atom: &Atom| &text[atom.start..atom.end];
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    let mut stack: Vec<Open> = Vec::new();
    let mut gap = Gap::None;
    // The last piece written.
    let mut prev: Option<Atom> = None;
    // Lines written so far, and the line the bracket being closed was opened on.
    let mut line = 0;

    for (index, atom) in list.iter().enumerate() {
        let piece = piece_of(atom);
        if atom.kind == Kind::Space {
            let lines = piece.matches('\n').count();
            if lines > 0 {
                let blank = lines > 1 || gap == (Gap::Line { blank: true });
                gap = Gap::Line { blank };
            } else if matches!(gap, Gap::None | Gap::Space) {
                gap = Gap::Keep(atom.start, atom.end);
            }
            continue;
        }
        let is_comment = matches!(atom.kind, Kind::Opaque { comment: true, .. });
        let is_word = atom.kind == Kind::Word;

        // Closing brackets leave their level before their own line is indented.
        let mut closed_block = false;
        let mut closing = None;
        if let Kind::Close(bracket) = atom.kind {
            let opener = match bracket {
                b')' => b'(',
                b']' => b'[',
                _ => b'{',
            };
            if let Some(depth) = stack.iter().rposition(|open| open.bracket == opener) {
                closed_block = bracket == b'}' && !stack[depth].inline;
                closing = Some(stack[depth].line);
                if closed_block && !matches!(gap, Gap::Line { .. }) {
                    let empty = prev.is_some_and(|before| before.kind == Kind::Open(b'{'));
                    gap = if empty {
                        Gap::None
                    } else {
                        Gap::Line { blank: false }
                    };
                }
                stack.truncate(depth);
            }
        }

        gap = match gap {
            Gap::Soft if is_comment => Gap::Space,
            Gap::Soft => Gap::Line { blank: false },
            Gap::AfterBlock => match atom.kind {
                _ if is_comment => Gap::Space,
                Kind::Close(_) | Kind::Semi | Kind::Comma => Gap::None,
                _ if css => Gap::Line { blank: false },
                Kind::Word if matches!(piece, "else" | "catch" | "finally") => Gap::Space,
                Kind::Word | Kind::Opaque { .. } | Kind::Open(b'{') => Gap::Line { blank: false },
                _ => Gap::None,
            },
            Gap::Space if matches!(atom.kind, Kind::Close(_) | Kind::Semi | Kind::Comma) => {
                Gap::None
            }
            // A brace opening a block stands off what announces it.
            Gap::None if atom.kind == Kind::Open(b'{') => match prev {
                Some(before)
                    if matches!(before.kind, Kind::Word | Kind::Close(b')' | b']'))
                        || piece_of(&before).ends_with("=>") =>
                {
                    Gap::Space
                }
                _ => Gap::None,
            },
            other => other,
        };

        match gap {
            Gap::None | Gap::Soft | Gap::AfterBlock => {}
            Gap::Space => out.push(' '),
            Gap::Keep(start, end) => out.push_str(&text[start..end]),
            Gap::Line { blank } => {
                let switch = !css && !json;
                if switch
                    && is_word
                    && piece == "case"
                    && let Some(open) = stack.last_mut().filter(|open| open.bracket == b'{')
                {
                    open.cases = true;
                }
                let in_cases = stack.last().is_some_and(|open| open.cases);
                let mut levels = base + levels_in(&stack, closing);
                if in_cases && is_word && matches!(piece, "case" | "default") {
                    levels -= 1;
                }
                // A line that goes on with the statement above sits one level deeper.
                let follows_operator = prev.is_some_and(|before| {
                    let before_piece = piece_of(&before);
                    before.kind == Kind::Punct
                        && (before_piece.ends_with('=')
                            || matches!(before_piece, "+" | "&&" | "||" | "??" | "=>" | "?"))
                });
                let leads_operator = atom.kind == Kind::Punct
                    && ((piece.starts_with('.') && !piece.starts_with(".."))
                        || piece.starts_with('?')
                        || piece.starts_with("->")
                        || matches!(piece, ":" | "&&" | "||" | "|>"));
                if switch && (follows_operator || leads_operator) {
                    levels += 1;
                }
                break_line(&mut out, blank, levels, unit);
                line += 1;
            }
        }

        if is_comment {
            push_comment(&mut out, text, atom);
        } else {
            out.push_str(piece);
        }
        if piece.contains('\n') {
            line += 1;
        }

        gap = match atom.kind {
            Kind::Open(bracket) => {
                let before = prev.map(|before| (before.kind, piece_of(&before)));
                let announces_block = before.is_some_and(|(kind, piece)| {
                    kind == Kind::Close(b')')
                        || piece.ends_with("=>")
                        || (kind == Kind::Word
                            && matches!(piece, "else" | "try" | "do" | "finally"))
                });
                let object = bracket == b'{'
                    && !css
                    && match before {
                        None => json,
                        Some((Kind::Open(b'(' | b'[') | Kind::Comma, _)) => true,
                        Some((Kind::Punct, piece)) => {
                            !piece.ends_with("=>")
                                && !piece.ends_with("::")
                                && piece.ends_with(['=', ':', '?', '&', '|'])
                        }
                        Some((Kind::Word, piece)) => matches!(piece, "return" | "default"),
                        Some(_) => false,
                    };
                let spaced = list
                    .get(index + 1)
                    .is_some_and(|next| matches!(next.kind, Kind::Space | Kind::Close(b'}')));
                let inline = bracket != b'{'
                    || (!css && closes_inline(text, &list, index) && (!announces_block || spaced));
                stack.push(Open {
                    bracket,
                    inline,
                    object,
                    cases: false,
                    line,
                });
                if inline { Gap::None } else { Gap::Soft }
            }
            Kind::Close(b'}') if closed_block => Gap::AfterBlock,
            Kind::Close(b'}') => match list.get(index + 1) {
                Some(next) if next.kind == Kind::Word => Gap::Space,
                _ => Gap::None,
            },
            Kind::Semi => match stack.last() {
                None => Gap::Soft,
                Some(open) if open.bracket == b'{' && !open.inline => Gap::Soft,
                Some(_) => Gap::Space,
            },
            Kind::Comma => match stack.last() {
                Some(open) if open.bracket == b'{' && !open.inline && open.object => Gap::Soft,
                _ => Gap::Space,
            },
            _ => Gap::None,
        };
        prev = Some(*atom);
    }
    out
}

// ------------------------------------------------------------------ markup

enum Node<'a> {
    /// A comment, a doctype, a processing instruction: a line of its own.
    Aside(&'a str),
    Open {
        raw: &'a str,
        name: String,
        /// Nothing to close: `<br>`, `<path/>`.
        childless: bool,
    },
    Close {
        raw: &'a str,
        name: String,
    },
    Text(&'a str),
    /// The content of a `<script>`, `<pre>`… element.
    Raw(&'a str),
}

fn starts_tag(text: &str, at: usize) -> bool {
    let mut after = text[at..].chars().skip(1);
    match after.next() {
        Some('/') => after.next().is_some_and(char::is_alphabetic),
        Some(ch) => ch.is_alphabetic() || ch == '!' || ch == '?',
        None => false,
    }
}

/// Byte after the `>` that ends the tag opened at `start`, quotes skipped.
fn tag_end(text: &str, start: usize) -> usize {
    let mut quote = None;
    for (offset, ch) in text[start..].char_indices() {
        match (quote, ch) {
            (None, '"' | '\'') => quote = Some(ch),
            (Some(open), _) if open == ch => quote = None,
            (None, '>') => return start + offset + 1,
            _ => {}
        }
    }
    text.len()
}

fn tag_name(raw: &str) -> String {
    raw.trim_start_matches(['<', '/'])
        .chars()
        .take_while(|ch| !ch.is_whitespace() && *ch != '>' && *ch != '/')
        .collect()
}

fn nodes(text: &str, html: bool) -> Vec<Node<'_>> {
    let find = |from: usize, needle: &str| text[from..].find(needle).map(|index| from + index);
    let mut found = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let rest = &text[at..];
        if !rest.starts_with('<') || !starts_tag(text, at) {
            // Text runs up to the next `<` that opens a tag.
            let mut end = at + rest.chars().next().map_or(1, char::len_utf8);
            loop {
                match find(end, "<") {
                    Some(next) if starts_tag(text, next) => {
                        end = next;
                        break;
                    }
                    Some(next) => end = next + 1,
                    None => {
                        end = text.len();
                        break;
                    }
                }
            }
            found.push(Node::Text(&text[at..end]));
            at = end;
            continue;
        }
        let closing = [("<!--", "-->"), ("<![CDATA[", "]]>"), ("<?", "?>")]
            .into_iter()
            .find(|(open, _)| rest.starts_with(open));
        if let Some((open, close)) = closing {
            let end = find(at + open.len(), close).map_or(text.len(), |index| index + close.len());
            found.push(Node::Aside(&text[at..end]));
            at = end;
            continue;
        }
        let end = tag_end(text, at);
        let raw = &text[at..end];
        at = end;
        if raw.starts_with("<!") {
            found.push(Node::Aside(raw));
            continue;
        }
        let mut name = tag_name(raw);
        if html {
            name = name.to_ascii_lowercase();
        }
        if raw.starts_with("</") {
            found.push(Node::Close { raw, name });
            continue;
        }
        let childless = raw.ends_with("/>") || (html && VOID_TAGS.contains(&name.as_str()));
        let is_raw = html && !childless && RAW_TAGS.contains(&name.as_str());
        if is_raw {
            let lower = text[at..].to_ascii_lowercase();
            let close = lower
                .find(&format!("</{name}"))
                .map_or(text.len(), |index| at + index);
            found.push(Node::Open {
                raw,
                name,
                childless,
            });
            found.push(Node::Raw(&text[at..close]));
            at = close;
        } else {
            found.push(Node::Open {
                raw,
                name,
                childless,
            });
        }
    }
    found
}

fn is_inline(node: &Node<'_>, html: bool) -> bool {
    match node {
        Node::Text(_) => true,
        Node::Open { name, .. } | Node::Close { name, .. } => {
            html && INLINE_TAGS.contains(&name.as_str())
        }
        Node::Aside(_) | Node::Raw(_) => false,
    }
}

/// Index of the first node at or after `from` that does not flow with the text.
fn run_end(list: &[Node<'_>], from: usize, html: bool) -> usize {
    list[from..]
        .iter()
        .position(|node| !is_inline(node, html))
        .map_or(list.len(), |offset| from + offset)
}

/// The text and inline tags of `run` on one line, runs of white space being one space.
fn flow(run: &[Node<'_>]) -> String {
    let mut line = String::new();
    for node in run {
        match node {
            Node::Text(text) => {
                let words: Vec<&str> = text.split_whitespace().collect();
                if text.starts_with(char::is_whitespace) {
                    line.push(' ');
                }
                line.push_str(&words.join(" "));
                if !words.is_empty() && text.ends_with(char::is_whitespace) {
                    line.push(' ');
                }
            }
            Node::Open { raw, .. }
            | Node::Close { raw, .. }
            | Node::Aside(raw)
            | Node::Raw(raw) => {
                line.push_str(raw);
            }
        }
    }
    line.trim().to_owned()
}

/// The language of what a `<script>` or `<style>` tag holds; `None` to leave it alone.
fn embedded(name: &str, raw: &str) -> Option<&'static Language> {
    let tag = raw.to_ascii_lowercase();
    match name {
        "style" => Language::named("CSS"),
        "script" if tag.contains("json") => Language::named("JSON"),
        "script"
            if !tag.contains("type=")
                || ["javascript", "module", "babel"]
                    .iter()
                    .any(|kind| tag.contains(kind)) =>
        {
            Language::named("JavaScript")
        }
        _ => None,
    }
}

/// Reformat HTML (`html`) or XML: one block tag per line, indented by depth.
fn markup(text: &str, html: bool, unit: &str) -> String {
    let list = nodes(text, html);
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    // Names of the elements still open.
    let mut open: Vec<&str> = Vec::new();
    let line = |out: &mut String, depth: usize, content: &str| {
        if !out.is_empty() {
            out.push('\n');
        }
        for _ in 0..depth {
            out.push_str(unit);
        }
        out.push_str(content);
    };
    let mut index = 0;
    while index < list.len() {
        let node = &list[index];
        if is_inline(node, html) {
            let end = run_end(&list, index, html);
            let content = flow(&list[index..end]);
            if !content.is_empty() {
                line(&mut out, open.len(), &content);
            }
            index = end;
            continue;
        }
        index += 1;
        match node {
            Node::Text(_) | Node::Raw(_) => {}
            Node::Aside(raw) => line(&mut out, open.len(), raw),
            Node::Close { raw, name } => {
                if let Some(depth) = open.iter().rposition(|known| known == name) {
                    open.truncate(depth);
                }
                line(&mut out, open.len(), raw);
            }
            Node::Open {
                raw,
                childless: true,
                ..
            } => line(&mut out, open.len(), raw),
            Node::Open { raw, name, .. } => {
                let group = IMPLIED_END
                    .iter()
                    .find(|group| group.contains(&name.as_str()));
                if html
                    && let Some(group) = group
                    && open.last().is_some_and(|last| group.contains(last))
                {
                    open.pop();
                }
                let depth = open.len();
                if let Some(Node::Raw(content)) = list.get(index) {
                    let close = match list.get(index + 1) {
                        Some(Node::Close { raw, .. }) => raw,
                        _ => "",
                    };
                    index += 1 + usize::from(!close.is_empty());
                    match embedded(name, raw) {
                        Some(_) if content.trim().is_empty() => {
                            line(&mut out, depth, &format!("{raw}{close}"));
                        }
                        Some(language) => {
                            line(&mut out, depth, raw);
                            let code = braces(content.trim(), language, unit, depth + 1);
                            line(&mut out, depth + 1, &code);
                            line(&mut out, depth, close);
                        }
                        // Every space counts in there.
                        None => line(&mut out, depth, &format!("{raw}{content}{close}")),
                    }
                    continue;
                }
                // `<h1>Title</h1>` reads best whole.
                let end = run_end(&list, index, html);
                if let Some(Node::Close {
                    raw: close,
                    name: closed,
                }) = list.get(end)
                    && closed == name
                {
                    let whole = format!("{raw}{}{close}", flow(&list[index..end]));
                    if whole.len() <= COMPACT_BYTES && !whole.contains('\n') {
                        line(&mut out, depth, &whole);
                        index = end + 1;
                        continue;
                    }
                }
                line(&mut out, depth, raw);
                open.push(name);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn formatted(text: &str, file: &str) -> String {
        let language = Language::of(Path::new(file));
        let once = format(text, language, "  ");
        let squeeze = |text: &str| text.split_whitespace().collect::<String>();
        assert_eq!(squeeze(&once), squeeze(text), "only white space changes");
        assert_eq!(
            format(&once, language, "  "),
            once,
            "formatting twice changes nothing"
        );
        once
    }

    #[test]
    fn minified_html_gets_one_tag_per_line() {
        let text = "<!doctype html><html><head><title>Hi</title><meta charset=\"utf-8\"></head>\
                    <body><div class=\"a\"><p>Hello <b>you</b> there</p><br><ul><li>one<li>two</ul>\
                    </div></body></html>";
        assert_eq!(
            formatted(text, "index.html"),
            "<!doctype html>\n<html>\n  <head>\n    <title>Hi</title>\n    <meta charset=\"utf-8\">\n  \
             </head>\n  <body>\n    <div class=\"a\">\n      <p>Hello <b>you</b> there</p>\n      \
             <br>\n      <ul>\n        <li>\n          one\n        <li>\n          two\n      \
             </ul>\n    </div>\n  \
             </body>\n</html>\n"
        );
    }

    #[test]
    fn scripts_and_styles_are_formatted_and_pre_is_kept() {
        let text = "<style>a{color:red;margin:0}</style><pre> a\n  b </pre>\
                    <script>function f(a){if(a){return 1}else{g(a,2)}}</script>";
        assert_eq!(
            formatted(text, "x.html"),
            "<style>\n  a {\n    color:red;\n    margin:0\n  }\n</style>\n<pre> a\n  b </pre>\n\
             <script>\n  function f(a) {\n    if(a) {\n      return 1\n    } else {\n      \
             g(a, 2)\n    }\n  }\n</script>\n"
        );
    }

    #[test]
    fn minified_css_and_json_are_expanded() {
        assert_eq!(
            formatted(
                "/*! x */a,b>c{color:red}@media(min-width:1px){.d{e:f}}",
                "x.css"
            ),
            "/*! x */a, b>c {\n  color:red\n}\n@media(min-width:1px) {\n  .d {\n    e:f\n  }\n}\n"
        );
        assert_eq!(
            formatted(
                "{\"a\":1,\"b\":{\"c\":[1,2],\"d\":\"x,y;{\"},\"e\":{}}",
                "x.json"
            ),
            "{\n  \"a\":1,\n  \"b\":{\"c\":[1, 2], \"d\":\"x,y;{\"},\n  \"e\":{}\n}\n"
        );
    }

    #[test]
    fn formatted_code_keeps_its_shape() {
        let rust = "fn main() {\n    let total = items\n        .iter()\n        .sum();\n    \
                    // done\n    match total {\n        0 => {}\n        _ => run(total, 1),\n    \
                    }\n}\n";
        assert_eq!(format(rust, Language::of(Path::new("a.rs")), "    "), rust);
        let script = "import { a, b } from \"x\";\n\nswitch (a) {\n  case 1:\n    b();\n    \
                      break;\n  default:\n    for (let i = 0; i < 2; i++) {\n      b(i);\n    \
                      }\n}\nconst o = {\n  /**\n   * Doc.\n   */\n  key: `a;{\n  b`,\n};\n";
        assert_eq!(
            format(script, Language::of(Path::new("a.js")), "  "),
            script
        );
    }

    #[test]
    fn wrong_indentation_is_rebuilt() {
        assert_eq!(
            formatted(
                "if (a) {\nb();   \n      } else {\n\n\n  c(); // why\n}",
                "a.js"
            ),
            "if (a) {\n  b();\n} else {\n\n  c(); // why\n}\n"
        );
        assert_eq!(formatted("a = 1   \n\n\n", "a.py"), "a = 1\n");
    }
}
