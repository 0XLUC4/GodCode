//! The values written in a `.env` file.
//!
//! One `NAME=value` a line, `export ` before it or not. A value is written bare, between
//! double quotes (escapes read, several lines allowed) or between single quotes (taken as
//! written). `$NAME`, `${NAME}` and `${NAME:-default}` stand for a value read earlier in
//! the file, but between single quotes.

/// Whether a value may be named so: letters, digits, `_`, `.` and `-`, no digit first.
fn named(name: &str) -> bool {
    let allowed = |ch: char| ch.is_ascii_alphanumeric() || "_.-".contains(ch);
    !name.starts_with(|ch: char| ch.is_ascii_digit())
        && !name.is_empty()
        && name.chars().all(allowed)
}

/// The name a line gives a value, and what it writes after the `=`.
fn assignment(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    if line.starts_with('#') {
        return None;
    }
    let exported = line.strip_prefix("export");
    let line = exported
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .unwrap_or(line);
    let (name, written) = line.split_once('=')?;
    Some((name.trim(), written))
}

/// A bare value: without the space around it, nor the comment that follows white space.
fn bare(written: &str) -> &str {
    let comment = written
        .match_indices('#')
        .map(|(at, _)| at)
        .find(|&at| written[..at].ends_with(char::is_whitespace));
    written[..comment.unwrap_or(written.len())].trim()
}

/// Reads what follows a `$`: the value it names is added to `value`, or the `$` alone when
/// it names none. What is left to read is given back.
fn reference<'a>(rest: &'a str, known: &[(String, String)], value: &mut String) -> &'a str {
    let find = |name: &str| {
        let known = known.iter().find(|(known, _)| known == name);
        known.map(|(_, value)| value.as_str())
    };
    // `${NAME}` and `${NAME:-default}`, closed before the line or the quotes end.
    let braced = rest
        .strip_prefix('{')
        .and_then(|inner| inner.split_once('}'))
        .filter(|(inner, _)| !inner.contains(['"', '\n']));
    if let Some((inner, after)) = braced {
        let (name, default) = match inner.split_once(":-") {
            Some((name, default)) => (name, Some(default)),
            None => (inner, None),
        };
        // The default stands for a value that is not there, or is empty.
        let found = find(name)
            .filter(|found| default.is_none() || !found.is_empty())
            .or(default);
        if let (true, Some(found)) = (named(name), found) {
            value.push_str(found);
            return after;
        }
    }
    // Without braces a name ends at the first dot or dash: `$HOST.example.com`.
    let word = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    let end = rest.find(|ch| !word(ch)).unwrap_or(rest.len());
    if let (true, Some(found)) = (named(&rest[..end]), find(&rest[..end])) {
        value.push_str(found);
        return &rest[end..];
    }
    value.push('$');
    rest
}

/// A bare value, the values it names standing in.
fn expanded(written: &str, known: &[(String, String)]) -> String {
    let mut value = String::new();
    let mut rest = written;
    while let Some((before, after)) = rest.split_once('$') {
        value.push_str(before);
        rest = reference(after, known, &mut value);
    }
    value.push_str(rest);
    value
}

/// A value between double quotes, read from after the opening one: the value, and what
/// follows the closing one. None when they are never closed.
fn double<'a>(mut rest: &'a str, known: &[(String, String)]) -> Option<(String, &'a str)> {
    let mut value = String::new();
    loop {
        let mut chars = rest.chars();
        match chars.next()? {
            '"' => return Some((value, chars.as_str())),
            '$' => {
                rest = reference(chars.as_str(), known, &mut value);
                continue;
            }
            '\\' => match chars.next()? {
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                escaped @ ('\\' | '"' | '$') => value.push(escaped),
                // Any other is no escape: it stays as written.
                other => value.extend(['\\', other]),
            },
            ch => value.push(ch),
        }
        rest = chars.as_str();
    }
}

/// The values of a `.env` file, in the order written. A name written twice keeps its first
/// place and takes its last value.
///
/// A line that gives no value to a well formed name is skipped. Quotes that are never
/// closed are read as part of a bare value, which ends with its line.
pub fn values(text: &str) -> Vec<(String, String)> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut rest = text.as_str();
    let mut values: Vec<(String, String)> = Vec::new();
    while !rest.is_empty() {
        let line = rest.split('\n').next().unwrap_or(rest);
        let end = &rest[line.len()..];
        let Some((name, written)) = assignment(line) else {
            rest = end.strip_prefix('\n').unwrap_or(end);
            continue;
        };
        // A value between quotes may go on past the end of its line.
        let opened = written.trim_start();
        let from = &rest[line.len() - opened.len()..];
        let quoted = match from.chars().next() {
            Some('"') => double(&from[1..], &values),
            Some('\'') => from[1..]
                .split_once('\'')
                .map(|(value, after)| (value.to_owned(), after)),
            _ => None,
        };
        let (value, after) = quoted.unwrap_or_else(|| (expanded(bare(written), &values), end));
        // What follows a closing quote on its line is not read.
        rest = after.split_once('\n').map_or("", |(_, next)| next);
        if !named(name) {
            continue;
        }
        match values.iter_mut().find(|(known, _)| known == name) {
            Some(known) => known.1 = value,
            None => values.push((name.to_owned(), value)),
        }
    }
    values
}

/// Among the file names of a folder, those that are env files, in the order they are
/// offered: `.env`, `.env.local`, then the other `.env.*` by name. The ones that only show
/// what to write (`.env.example`, `.env.sample`, `.env.template`) are left out.
pub fn files(names: &[String]) -> Vec<String> {
    let shown = [".env.example", ".env.sample", ".env.template", ".env."];
    let env = |name: &&String| {
        (*name == ".env" || name.starts_with(".env.")) && !shown.contains(&name.as_str())
    };
    let mut files: Vec<String> = names.iter().filter(env).cloned().collect();
    files.sort_by_key(|name| (name != ".env", name != ".env.local", name.clone()));
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(values: &[(&str, &str)]) -> Vec<(String, String)> {
        let owned = |(name, value): &(&str, &str)| (name.to_string(), value.to_string());
        values.iter().map(owned).collect()
    }

    #[test]
    fn values_are_read_in_the_order_written() {
        let text = "# database\n\nHOST=localhost\nexport PORT=5432\n  NAME = app db  \nEMPTY=\n\
                    export\tTABBED=1\nSPACED =\n  # indented=comment\nLAST=a=b";
        assert_eq!(
            values(text),
            pairs(&[
                ("HOST", "localhost"),
                ("PORT", "5432"),
                ("NAME", "app db"),
                ("EMPTY", ""),
                ("TABBED", "1"),
                ("SPACED", ""),
                ("LAST", "a=b"),
            ])
        );
        assert_eq!(values(""), []);
        assert_eq!(values("\n\n# nothing\n"), []);
    }

    #[test]
    fn a_comment_after_white_space_ends_a_bare_value() {
        let text = "A=one # two\nB=one#two\nC=http://a.test/#top\nD= # nothing\nE=#hash\n\
                    F=a\t#tab\nG=a # b # c";
        assert_eq!(
            values(text),
            pairs(&[
                ("A", "one"),
                ("B", "one#two"),
                ("C", "http://a.test/#top"),
                ("D", ""),
                ("E", "#hash"),
                ("F", "a"),
                ("G", "a"),
            ])
        );
    }

    #[test]
    fn a_value_between_double_quotes_has_its_escapes_read_and_may_span_lines() {
        let text = "A=\"say \\\"hi\\\" \\\\ done\"\nB=\"a\\tb\\nc\\rd\"\nC=\"# kept\" # dropped\n\
                    D=\"line one\nline two\n\"\nE=\"\"\nF = \"  padded  \"\nG=\"other \\x escape\"\n\
                    H=\"it's\"";
        assert_eq!(
            values(text),
            pairs(&[
                ("A", "say \"hi\" \\ done"),
                ("B", "a\tb\nc\rd"),
                ("C", "# kept"),
                ("D", "line one\nline two\n"),
                ("E", ""),
                ("F", "  padded  "),
                ("G", "other \\x escape"),
                ("H", "it's"),
            ])
        );
    }

    #[test]
    fn a_value_between_single_quotes_is_taken_as_written() {
        let text = "HOST=h\nA='$HOST and ${HOST} \\n \"literal\"'\nB='two\nlines'\nC='# kept' # dropped\n\
                    D=''\nexport E = ' padded '";
        assert_eq!(
            values(text),
            pairs(&[
                ("HOST", "h"),
                ("A", "$HOST and ${HOST} \\n \"literal\""),
                ("B", "two\nlines"),
                ("C", "# kept"),
                ("D", ""),
                ("E", " padded "),
            ])
        );
    }

    #[test]
    fn line_ends_of_windows_and_a_byte_order_mark_are_read() {
        let text =
            "\u{feff}A=1\r\n\r\n# note\r\nB = two words \r\nC=\"one\r\ntwo\"\r\nD='x'\r\nE=\r\n";
        assert_eq!(
            values(text),
            pairs(&[
                ("A", "1"),
                ("B", "two words"),
                ("C", "one\ntwo"),
                ("D", "x"),
                ("E", ""),
            ])
        );
    }

    #[test]
    fn a_name_written_twice_keeps_its_first_place_and_its_last_value() {
        let text = "A=1\nB=2\nA=3\nC=4\nB=\"5\"";
        assert_eq!(values(text), pairs(&[("A", "3"), ("B", "5"), ("C", "4")]));
    }

    #[test]
    fn a_line_that_gives_no_value_to_a_well_formed_name_is_skipped() {
        let text = "just some text\nexport ONLY\n=nameless\n1ST=digit first\nMY VAR=space\n\
                    KEY!=bang\n\u{e9}t\u{e9}=accent\nexport = lone\n# COMMENTED=1\n\
                    my.key-1=kept\n_under=kept\n-dash=kept\nexport=kept";
        assert_eq!(
            values(text),
            pairs(&[
                ("my.key-1", "kept"),
                ("_under", "kept"),
                ("-dash", "kept"),
                ("export", "kept"),
            ])
        );
        // The value of a name that is skipped is gone past whole, its other lines too.
        let spanning = "BAD NAME=\"one\nINSIDE=1\"\nREAL=1";
        assert_eq!(values(spanning), pairs(&[("REAL", "1")]));
    }

    #[test]
    fn values_read_earlier_stand_in_for_their_names() {
        let text = "HOST=a.test\nPORT=8080\nURL=http://${HOST}:$PORT/x\nQUOTED=\"${HOST}/$PORT\"\n\
                    LITERAL='${HOST}'\nESCAPED=\"\\$HOST costs $5\"\nDOTTED=$HOST.example\n\
                    my.key=dot\nBRACED=${my.key}$my.key\nLATER=$AFTER\nAFTER=1\nSELF=a\nSELF=$SELF:b";
        assert_eq!(
            values(text),
            pairs(&[
                ("HOST", "a.test"),
                ("PORT", "8080"),
                ("URL", "http://a.test:8080/x"),
                ("QUOTED", "a.test/8080"),
                ("LITERAL", "${HOST}"),
                ("ESCAPED", "$HOST costs $5"),
                ("DOTTED", "a.test.example"),
                ("my.key", "dot"),
                // Without braces the name ends at the dot, and `my` names nothing.
                ("BRACED", "dot$my.key"),
                // A value read further down is not known yet.
                ("LATER", "$AFTER"),
                ("AFTER", "1"),
                ("SELF", "a:b"),
            ])
        );
    }

    #[test]
    fn a_name_that_names_nothing_is_left_as_written() {
        let text = "A=$NOPE ${NOPE} $ ${ ${} $1 end$\nB=\"$NOPE ${NOPE} $ ${ ${} end$\"\nC=${A\nD=\"${A\"\nE=}";
        let read = values(text);
        assert_eq!(read[0].1, "$NOPE ${NOPE} $ ${ ${} $1 end$");
        assert_eq!(read[1].1, "$NOPE ${NOPE} $ ${ ${} end$");
        // Braces closed past the line or past the quotes are no name.
        assert_eq!(read[2..], pairs(&[("C", "${A"), ("D", "${A"), ("E", "}")]));
    }

    #[test]
    fn a_default_stands_for_a_value_that_is_missing_or_empty() {
        let text = "HOST=a.test\nEMPTY=\nA=${NOPE:-fallback}\nB=${HOST:-fallback}\nC=${EMPTY:-fallback}\n\
                    D=[${EMPTY}]\nE=\"${NOPE:-two words}\"\nF=${NOPE:-}\nG=${NOPE:-$HOST}\nH=${bad name:-x}";
        assert_eq!(
            values(text)[2..],
            pairs(&[
                ("A", "fallback"),
                ("B", "a.test"),
                ("C", "fallback"),
                ("D", "[]"),
                ("E", "two words"),
                ("F", ""),
                // A default is taken as written: no name stands in inside it.
                ("G", "$HOST"),
                ("H", "${bad name:-x}"),
            ])
        );
    }

    #[test]
    fn quotes_never_closed_are_part_of_a_bare_value() {
        let text = "A=\"open\nB=2\nC='open # note\nD=4";
        assert_eq!(
            values(text),
            pairs(&[("A", "\"open"), ("B", "2"), ("C", "'open"), ("D", "4")])
        );
        assert_eq!(values("A=\"tail\\"), pairs(&[("A", "\"tail\\")]));
        // The same quote further down closes them all the same, lines in between taken in.
        assert_eq!(
            values("A=\"open\nB=\"2\"\nC=3"),
            pairs(&[("A", "open\nB="), ("C", "3")])
        );
    }

    #[test]
    fn malformed_text_is_read_without_trouble() {
        for text in [
            "=",
            "==",
            "\"",
            "'",
            "A=\"",
            "A='",
            "A=\"\\",
            "A=$",
            "A=${",
            "A=${}",
            "A=${:-",
            "A=\"${",
            "export",
            "export ",
            "export =",
            "\u{feff}",
            "\r",
            "\r\n",
            "\n=\n",
            "#",
            "A=\u{e9}\"\u{4e16}",
            "\u{4e16}=\u{754c}",
            "A=$\u{e9}",
            "A=\"\\\u{e9}\"",
        ] {
            values(text);
        }
        // A file cut anywhere, read from anywhere.
        let text = "\u{feff}# caf\u{e9}\r\nexport A = \"l'\u{e9}t\u{e9} \\\"x\\\" ${B:-d\u{e9}} $A\nmore\" # c\r\n\
                    B='\u{4e16}\u{754c}\n$A' \nC=$A${B}${C:-\u{e9}} #\u{e9}\n D\u{e9}=1\n=\n$=${\n";
        for (at, _) in text.char_indices() {
            values(&text[..at]);
            values(&text[at..]);
        }
        assert_eq!(values(text).len(), 3);
    }

    #[test]
    fn env_files_are_offered_in_order() {
        let names = [
            ".env.production",
            "README.md",
            ".env.local",
            ".env.example",
            ".env",
            ".envrc",
            ".env.development",
            "app.env",
            ".env.sample",
            ".env.template",
            ".env.",
            ".env.Staging",
            "env",
        ]
        .map(String::from);
        assert_eq!(
            files(&names),
            [
                ".env",
                ".env.local",
                ".env.Staging",
                ".env.development",
                ".env.production"
            ]
        );
        assert_eq!(files(&[]), [] as [&str; 0]);
        assert_eq!(
            files(&["src".to_owned(), ".env.example".to_owned()]),
            [] as [&str; 0]
        );
    }
}
