//! Markdown as the preview shows it: blocks with the line they start on, and the styles
//! inside a line of text.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: usize,
        text: String,
    },
    Paragraph(String),
    /// A list item: how deep it is nested, its number in a numbered list, its box in a
    /// task list.
    Item {
        depth: usize,
        number: Option<String>,
        done: Option<bool>,
        text: String,
    },
    Code {
        language: String,
        text: String,
    },
    Quote(String),
    Rule,
    /// Rows of cells, the first being the header.
    Table(Vec<Vec<String>>),
}

/// A block and the line, counted from 0, it starts on.
pub type Placed = (usize, Block);

/// A stretch of text in one style.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: bool,
}

/// What the line being read continues, if anything.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Open {
    Nothing,
    Paragraph,
    Quote,
    Item,
    Table,
}

/// Marker and text of a list item: `- text`, `3. text`, `- [x] text`.
fn item(line: &str) -> Option<(Option<String>, Option<bool>, &str)> {
    let digits = line.len()
        - line
            .trim_start_matches(|ch: char| ch.is_ascii_digit())
            .len();
    let (number, rest) = if digits > 0 {
        let rest = line[digits..].strip_prefix(['.', ')'])?;
        (Some(line[..digits].to_owned()), rest)
    } else {
        (None, line.strip_prefix(['-', '*', '+'])?)
    };
    let text = rest.strip_prefix(' ')?.trim_start();
    let task = [("[ ] ", false), ("[x] ", true), ("[X] ", true)]
        .into_iter()
        .find_map(|(mark, done)| Some((done, text.strip_prefix(mark)?)));
    Some(match task {
        Some((done, text)) => (number, Some(done), text),
        None => (number, None, text),
    })
}

fn cells(row: &str) -> Vec<String> {
    let row = row.strip_prefix('|').unwrap_or(row);
    let row = row.strip_suffix('|').unwrap_or(row);
    row.split('|').map(|cell| cell.trim().to_owned()).collect()
}

/// The blocks of `text`, in order.
pub fn blocks(text: &str) -> Vec<Placed> {
    let mut found: Vec<Placed> = Vec::new();
    let mut open = Open::Nothing;
    // The fence being read: its line, its language, its lines.
    let mut fence: Option<(usize, String, Vec<&str>)> = None;
    for (line, content) in text.split('\n').enumerate() {
        let trimmed = content.trim();
        if let Some((start, language, lines)) = &mut fence {
            if trimmed.starts_with("```") {
                let code = Block::Code {
                    language: std::mem::take(language),
                    text: lines.join("\n"),
                };
                found.push((*start, code));
                fence = None;
            } else {
                lines.push(content);
            }
            continue;
        }
        let mut next = Open::Nothing;
        if let Some(language) = trimmed.strip_prefix("```") {
            fence = Some((line, language.trim().to_owned(), Vec::new()));
        } else if trimmed.is_empty() {
        } else if let Some(title) = trimmed
            .strip_prefix('#')
            .map(|rest| rest.trim_start_matches('#'))
            .filter(|rest| rest.starts_with(' ') && trimmed.len() - rest.len() <= 6)
        {
            let level = trimmed.len() - title.len();
            let text = title.trim().trim_end_matches('#').trim_end().to_owned();
            found.push((line, Block::Heading { level, text }));
        } else if trimmed.len() >= 3
            && ['-', '*', '_']
                .iter()
                .any(|mark| trimmed.chars().all(|ch| ch == *mark))
        {
            found.push((line, Block::Rule));
        } else if let Some(quoted) = trimmed.strip_prefix('>') {
            match found.last_mut() {
                Some((_, Block::Quote(text))) if open == Open::Quote => {
                    text.push(' ');
                    text.push_str(quoted.trim());
                }
                _ => found.push((line, Block::Quote(quoted.trim().to_owned()))),
            }
            next = Open::Quote;
        } else if trimmed.starts_with('|') {
            let row = cells(trimmed);
            // The line of dashes under the header only says there is one.
            let ruler = row
                .iter()
                .all(|cell| !cell.is_empty() && cell.chars().all(|ch| "-: ".contains(ch)));
            match found.last_mut() {
                Some((_, Block::Table(rows))) if open == Open::Table => {
                    if !ruler {
                        rows.push(row);
                    }
                }
                _ => found.push((line, Block::Table(vec![row]))),
            }
            next = Open::Table;
        } else if let Some((number, done, text)) = item(trimmed) {
            let lead = content.len() - content.trim_start().len();
            let tabs = content[..lead].matches('\t').count();
            found.push((
                line,
                Block::Item {
                    depth: (lead + tabs) / 2,
                    number,
                    done,
                    text: text.to_owned(),
                },
            ));
            next = Open::Item;
        } else {
            // Plain text goes on with the paragraph or the list item above it.
            match found.last_mut() {
                Some((_, Block::Paragraph(text))) if open == Open::Paragraph => {
                    text.push(' ');
                    text.push_str(trimmed);
                    next = Open::Paragraph;
                }
                Some((_, Block::Item { text, .. })) if open == Open::Item => {
                    text.push(' ');
                    text.push_str(trimmed);
                    next = Open::Item;
                }
                _ => {
                    found.push((line, Block::Paragraph(trimmed.to_owned())));
                    next = Open::Paragraph;
                }
            }
        }
        open = next;
    }
    // A fence left open runs to the end of the text.
    if let Some((start, language, lines)) = fence {
        let text = lines.join("\n");
        found.push((start, Block::Code { language, text }));
    }
    found
}

/// The styled stretches of one line of text: `**bold**`, `*italic*`, `` `code` ``,
/// `~~struck~~`, `[link](url)`.
pub fn inline(text: &str) -> Vec<Span> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans: Vec<Span> = Vec::new();
    let mut style = Span::default();
    let mut buffer = String::new();
    let find = |from: usize, wanted: char| {
        chars[from.min(chars.len())..]
            .iter()
            .position(|ch| *ch == wanted)
            .map(|offset| from + offset)
    };
    let flush = |spans: &mut Vec<Span>, buffer: &mut String, style: &Span| {
        if !buffer.is_empty() {
            spans.push(Span {
                text: std::mem::take(buffer),
                ..style.clone()
            });
        }
    };
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        let next = chars.get(index + 1).copied();
        let before = index.checked_sub(1).map(|at| chars[at]);
        match ch {
            '\\' if next.is_some_and(|ch| ch.is_ascii_punctuation()) => {
                buffer.extend(next);
                index += 2;
            }
            '`' => match find(index + 1, '`') {
                Some(close) => {
                    flush(&mut spans, &mut buffer, &style);
                    spans.push(Span {
                        text: chars[index + 1..close].iter().collect(),
                        code: true,
                        ..style.clone()
                    });
                    index = close + 1;
                }
                None => {
                    buffer.push(ch);
                    index += 1;
                }
            },
            '*' | '_' if next == Some(ch) => {
                flush(&mut spans, &mut buffer, &style);
                style.bold = !style.bold;
                index += 2;
            }
            '~' if next == Some('~') => {
                flush(&mut spans, &mut buffer, &style);
                style.strike = !style.strike;
                index += 2;
            }
            '*' | '_' => {
                // An underscore inside a word, or a star before a space, is just itself.
                let inside = ch == '_'
                    && before.is_some_and(char::is_alphanumeric)
                    && next.is_some_and(char::is_alphanumeric);
                let opens = !style.italic && next.is_none_or(char::is_whitespace);
                if inside || opens {
                    buffer.push(ch);
                } else {
                    flush(&mut spans, &mut buffer, &style);
                    style.italic = !style.italic;
                }
                index += 1;
            }
            '!' if next == Some('[') => index += 1,
            '[' => {
                let close = find(index + 1, ']').filter(|close| chars.get(close + 1) == Some(&'('));
                match close.and_then(|close| Some((close, find(close + 2, ')')?))) {
                    Some((close, end)) => {
                        flush(&mut spans, &mut buffer, &style);
                        spans.push(Span {
                            text: chars[index + 1..close].iter().collect(),
                            link: true,
                            ..style.clone()
                        });
                        index = end + 1;
                    }
                    None => {
                        buffer.push(ch);
                        index += 1;
                    }
                }
            }
            _ => {
                buffer.push(ch);
                index += 1;
            }
        }
    }
    flush(&mut spans, &mut buffer, &style);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_found_with_their_line() {
        let text = "# Title\n\nsome *text*\ngoes on\n\n- one\n  more\n  - [x] done\n2. two\n\n\
                    ```rs\nfn a() {}\n\n# not a title\n```\n> said\n> twice\n---\n\
                    | a | b |\n|---|:-:|\n| 1 | 2 |\n####### seven";
        let paragraph = |text: &str| Block::Paragraph(text.to_owned());
        let item = |depth, number: Option<&str>, done, text: &str| Block::Item {
            depth,
            number: number.map(str::to_owned),
            done,
            text: text.to_owned(),
        };
        let row = |cells: [&str; 2]| cells.map(str::to_owned).to_vec();
        assert_eq!(
            blocks(text),
            [
                (
                    0,
                    Block::Heading {
                        level: 1,
                        text: "Title".to_owned()
                    }
                ),
                (2, paragraph("some *text* goes on")),
                (5, item(0, None, None, "one more")),
                (7, item(1, None, Some(true), "done")),
                (8, item(0, Some("2"), None, "two")),
                (
                    10,
                    Block::Code {
                        language: "rs".to_owned(),
                        text: "fn a() {}\n\n# not a title".to_owned()
                    }
                ),
                (15, Block::Quote("said twice".to_owned())),
                (17, Block::Rule),
                (18, Block::Table(vec![row(["a", "b"]), row(["1", "2"])])),
                (21, paragraph("####### seven")),
            ]
        );
    }

    #[test]
    fn styles_are_told_apart_in_a_line() {
        let styled = |text: &str| -> Vec<(String, String)> {
            inline(text)
                .into_iter()
                .map(|span| {
                    let flags = [
                        (span.bold, 'b'),
                        (span.italic, 'i'),
                        (span.code, 'c'),
                        (span.strike, 's'),
                        (span.link, 'l'),
                    ];
                    let style = flags.iter().filter(|(on, _)| *on).map(|(_, ch)| ch);
                    (span.text, style.collect())
                })
                .collect()
        };
        let pair = |text: &str, style: &str| (text.to_owned(), style.to_owned());
        assert_eq!(
            styled("a **b *c*** `d*e` ~~f~~ [g](http://h) snake_case 2 * 3 \\*"),
            [
                pair("a ", ""),
                pair("b ", "b"),
                pair("c", "bi"),
                pair(" ", ""),
                pair("d*e", "c"),
                pair(" ", ""),
                pair("f", "s"),
                pair(" ", ""),
                pair("g", "l"),
                pair(" snake_case 2 * 3 *", ""),
            ]
        );
        assert_eq!(
            styled("![logo](a.png) [not a link]"),
            [pair("logo", "l"), pair(" [not a link]", "")]
        );
    }
}
