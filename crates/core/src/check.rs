//! Problems in a file: brackets checked while typing, and what compilers report.

use std::path::PathBuf;

use crate::byte_index;
use crate::syntax::{self, Language, Token};

/// Bracket checks stop after this many problems: one slip must not flood the list.
const MAX_BRACKET_PROBLEMS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// Something wrong at a place in a text; `line` and `column` in chars, counted from 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub line: usize,
    pub column: usize,
    pub severity: Severity,
    pub message: String,
}

/// A problem a compiler reported, in the file it named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// As printed: often relative to where the compiler ran.
    pub path: PathBuf,
    pub problem: Problem,
}

/// Brackets outside strings and comments, with their byte offsets.
fn code_brackets(text: &str, language: &Language) -> Vec<(usize, u8)> {
    let mut found = Vec::new();
    let mut start = 0;
    for span in syntax::highlight(text, language) {
        if !matches!(span.token, Token::String | Token::Comment) {
            let bytes = &text.as_bytes()[start..span.end];
            for (offset, byte) in bytes.iter().enumerate() {
                if b"()[]{}".contains(byte) {
                    found.push((start + offset, *byte));
                }
            }
        }
        start = span.end;
    }
    found
}

fn opens(bracket: u8) -> bool {
    b"([{".contains(&bracket)
}

fn closer_of(opener: u8) -> u8 {
    match opener {
        b'(' => b')',
        b'[' => b']',
        _ => b'}',
    }
}

fn position(text: &str, byte: usize) -> (usize, usize) {
    let before = &text[..byte];
    let line = before.matches('\n').count();
    let column = before
        .rsplit('\n')
        .next()
        .map_or(0, |tail| tail.chars().count());
    (line, column)
}

/// Brackets that are never closed, never opened, or closed by the wrong kind.
/// Empty for languages whose strings the lexer cannot tell reliably.
pub fn brackets(text: &str, language: &Language) -> Vec<Problem> {
    if !language.checks_brackets() {
        return Vec::new();
    }
    let problem = |byte: usize, message: String| {
        let (line, column) = position(text, byte);
        Problem {
            line,
            column,
            severity: Severity::Error,
            message,
        }
    };
    let mut open: Vec<(usize, u8)> = Vec::new();
    let mut problems = Vec::new();
    for (at, bracket) in code_brackets(text, language) {
        if problems.len() >= MAX_BRACKET_PROBLEMS {
            return problems;
        }
        if opens(bracket) {
            open.push((at, bracket));
            continue;
        }
        match open.last() {
            Some((_, opener)) if closer_of(*opener) == bracket => {
                open.pop();
            }
            Some((opened, opener)) => {
                let line = position(text, *opened).0 + 1;
                problems.push(problem(
                    at,
                    format!(
                        "Expected `{}` to close `{}` from line {line}, found `{}`",
                        closer_of(*opener) as char,
                        *opener as char,
                        bracket as char
                    ),
                ));
                // When an outer bracket matches, the inner ones are the unclosed ones.
                if let Some(depth) = open
                    .iter()
                    .rposition(|(_, opener)| closer_of(*opener) == bracket)
                {
                    open.truncate(depth);
                }
            }
            None => problems.push(problem(at, format!("Unexpected `{}`", bracket as char))),
        }
    }
    let room = MAX_BRACKET_PROBLEMS - problems.len();
    for (at, opener) in open.into_iter().take(room) {
        problems.push(problem(at, format!("Unclosed `{}`", opener as char)));
    }
    problems.sort_by_key(|problem| (problem.line, problem.column));
    problems
}

/// The bracket next to the caret and its partner, as char indices.
pub fn matching_bracket(text: &str, language: &Language, caret: usize) -> Option<(usize, usize)> {
    let caret = byte_index(text, caret);
    let list = code_brackets(text, language);
    let at = |byte: usize| list.iter().position(|(found, _)| *found == byte);
    // The bracket after the caret first, then the one before it.
    let index = at(caret).or_else(|| at(caret.checked_sub(1)?))?;
    let (start, bracket) = list[index];
    let forward = opens(bracket);
    let others: Box<dyn Iterator<Item = &(usize, u8)>> = if forward {
        Box::new(list[index..].iter())
    } else {
        Box::new(list[..=index].iter().rev())
    };
    let mut depth = 0;
    for (other, kind) in others {
        depth += if opens(*kind) == forward { 1 } else { -1 };
        if depth == 0 {
            let chars = |byte: usize| text[..byte].chars().count();
            return Some((chars(start), chars(*other)));
        }
    }
    None
}

/// The innermost bracket pair strictly around the chars `start..end`, as the char indices
/// of its two brackets.
pub fn enclosing_brackets(
    text: &str,
    language: &Language,
    start: usize,
    end: usize,
) -> Option<(usize, usize)> {
    let (start, end) = (byte_index(text, start), byte_index(text, end));
    let mut open: Vec<usize> = Vec::new();
    for (at, bracket) in code_brackets(text, language) {
        if opens(bracket) {
            open.push(at);
        } else if let Some(opened) = open.pop()
            && opened < start
            && at >= end
        {
            let chars = |byte: usize| text[..byte].chars().count();
            return Some((chars(opened), chars(at)));
        }
    }
    None
}

/// Leading digits of `text` as a number, and what follows them.
fn number(text: &str) -> Option<(usize, &str)> {
    let digits = text.len()
        - text
            .trim_start_matches(|ch: char| ch.is_ascii_digit())
            .len();
    let value = text[..digits].parse().ok()?;
    Some((value, &text[digits..]))
}

fn report(path: &str, line: usize, column: usize, message: &str) -> Report {
    let message = message.trim();
    let severity = if message.to_lowercase().starts_with("warning") {
        Severity::Warning
    } else {
        Severity::Error
    };
    Report {
        path: PathBuf::from(path.trim()),
        problem: Problem {
            line: line.saturating_sub(1),
            column: column.saturating_sub(1),
            severity,
            message: message.to_owned(),
        },
    }
}

fn looks_like_a_file(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    !path.trim().is_empty() && name.contains('.')
}

/// `path:line:column: message`, `path:line: message` or `path(line,column): message`.
fn located(line: &str) -> Option<Report> {
    if let Some((head, message)) = line.split_once("): ")
        && let Some((path, place)) = head.rsplit_once('(')
        && let Some((row, column)) = place.split_once(',')
        && let (Ok(row), Ok(column)) = (row.parse(), column.parse())
        && looks_like_a_file(path)
    {
        return Some(report(path, row, column, message));
    }
    // Index 1 is the colon of a Windows drive letter.
    for (index, _) in line.match_indices(':').filter(|(index, _)| *index > 1) {
        let Some((row, rest)) = number(&line[index + 1..]) else {
            continue;
        };
        let (column, rest) = rest.strip_prefix(':').and_then(number).unwrap_or((1, rest));
        let path = &line[..index];
        if let Some(message) = rest.strip_prefix(':')
            && !message.trim().is_empty()
            && looks_like_a_file(path)
        {
            return Some(report(path, row, column, message));
        }
    }
    None
}

/// A place announced alone, the message coming on a later line (Python, Node).
fn place(line: &str) -> Option<(String, usize)> {
    if let Some(rest) = line.strip_prefix("File \"") {
        let (path, rest) = rest.split_once('"')?;
        let (row, _) = number(rest.strip_prefix(", line ")?)?;
        return Some((path.to_owned(), row));
    }
    let (path, row) = line.rsplit_once(':')?;
    let row = row.parse().ok()?;
    looks_like_a_file(path).then(|| (path.to_owned(), row))
}

/// `SyntaxError: message` and its cousins.
fn is_error(line: &str) -> bool {
    line.split_once(": ")
        .is_some_and(|(kind, _)| kind.ends_with("Error") && !kind.contains(' '))
}

/// The problems in the output of a compiler or linter.
pub fn parse_output(output: &str) -> Vec<Report> {
    let mut reports = Vec::new();
    let mut announced: Option<(String, usize)> = None;
    for line in output.lines().map(str::trim) {
        if let Some(found) = located(line) {
            reports.push(found);
            announced = None;
        } else if let Some(found) = place(line) {
            announced = Some(found);
        } else if is_error(line)
            && let Some((path, row)) = announced.take()
        {
            reports.push(report(&path, row, 1, line));
        }
    }
    reports
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn language(file: &str) -> &'static Language {
        Language::of(Path::new(file))
    }

    fn messages(text: &str, file: &str) -> Vec<(usize, usize, String)> {
        brackets(text, language(file))
            .into_iter()
            .map(|problem| (problem.line, problem.column, problem.message))
            .collect()
    }

    #[test]
    fn balanced_code_has_no_problem() {
        let rust = "fn a<'a>(x: &'a [u8]) -> char { let s = r#\"}(\"#; '{' } // )";
        assert!(messages(rust, "a.rs").is_empty());
        let js = "const re = /[(\\/]/g; f(\"{\", `(${x}`, a / b); /* ] */";
        assert!(messages(js, "a.js").is_empty());
        assert!(messages("print('(')  # ]\nx = [1, {2: (3)}]", "a.py").is_empty());
    }

    #[test]
    fn broken_brackets_are_reported_where_they_are() {
        assert_eq!(
            messages("fn a() {\n    b(1;\n}", "a.rs"),
            [(
                2,
                0,
                "Expected `)` to close `(` from line 2, found `}`".to_owned()
            )]
        );
        assert_eq!(
            messages("x = 1)", "a.py"),
            [(0, 5, "Unexpected `)`".to_owned())]
        );
        assert_eq!(
            messages("é = [", "a.py"),
            [(0, 4, "Unclosed `[`".to_owned())]
        );
        // Markdown is prose: nothing to check.
        assert!(messages("a ( b", "a.md").is_empty());
    }

    #[test]
    fn brackets_find_their_partner() {
        let text = "f(a[1], \")\")";
        let rust = language("a.rs");
        assert_eq!(matching_bracket(text, rust, 1), Some((1, 11)));
        assert_eq!(matching_bracket(text, rust, 12), Some((11, 1)));
        assert_eq!(matching_bracket(text, rust, 4), Some((3, 5)));
        assert_eq!(matching_bracket(text, rust, 9), None);
    }

    #[test]
    fn compiler_output_is_understood() {
        let output = "\
src\\main.rs:2:5: error[E0425]: cannot find value `x`
C:\\work\\lib.rs:10:1: warning: unused variable
error: could not compile `demo`
src/app.ts(4,9): error TS2304: Cannot find name 'y'.
./main.go:7:2: undefined: z
  File \"C:\\py\\ast.py\", line 50, in parse
  File \"bad.py\", line 3
    def f(
SyntaxError: '(' was never closed
C:\\js\\bad.js:8
SyntaxError: Unexpected token ')'";
        let found: Vec<(String, usize, usize, Severity)> = parse_output(output)
            .into_iter()
            .map(|report| {
                (
                    report.path.display().to_string(),
                    report.problem.line,
                    report.problem.column,
                    report.problem.severity,
                )
            })
            .collect();
        let error = Severity::Error;
        assert_eq!(
            found,
            [
                ("src\\main.rs".to_owned(), 1, 4, error),
                ("C:\\work\\lib.rs".to_owned(), 9, 0, Severity::Warning),
                ("src/app.ts".to_owned(), 3, 8, error),
                ("./main.go".to_owned(), 6, 1, error),
                ("bad.py".to_owned(), 2, 0, error),
                ("C:\\js\\bad.js".to_owned(), 7, 0, error),
            ]
        );
    }
}
