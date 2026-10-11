//! GodCode editing logic: syntax colours, text edits, search and fuzzy matching.
//!
//! Text in memory always uses `\n` line ends; positions handed to the editor are char indices.

pub mod check;
pub mod complete;
pub mod coverage;
pub mod docker;
pub mod dotenv;
pub mod edit;
pub mod emmet;
pub mod format;
pub mod fuzzy;
pub mod git;
pub mod http;
pub mod markdown;
pub mod merge;
pub mod modal;
pub mod outline;
pub mod search;
pub mod sqlite;
pub mod syntax;

/// Line end a file uses on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn name(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::CrLf => "CRLF",
        }
    }
}

/// How a file indents its lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indent {
    Tabs,
    Spaces(usize),
}

impl Indent {
    /// Guess from the first indented lines; four spaces when nothing is indented.
    pub fn detect(text: &str) -> Self {
        let mut narrowest: Option<usize> = None;
        for line in text.lines().take(2000) {
            if line.starts_with('\t') {
                return Self::Tabs;
            }
            let spaces = line.len() - line.trim_start_matches(' ').len();
            // One space is the continuation of a block comment, not an indent.
            if spaces >= 2 && spaces < line.len() {
                narrowest = Some(narrowest.map_or(spaces, |known| known.min(spaces)));
            }
        }
        Self::Spaces(narrowest.map_or(4, |width| width.min(8)))
    }

    /// The text one indent level adds.
    pub fn unit(self) -> String {
        match self {
            Self::Tabs => "\t".to_owned(),
            Self::Spaces(width) => " ".repeat(width),
        }
    }

    pub fn name(self) -> String {
        match self {
            Self::Tabs => "Tabs".to_owned(),
            Self::Spaces(width) => format!("Spaces: {width}"),
        }
    }
}

/// A text file as the editor holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    /// Content with `\n` line ends and no byte order mark.
    pub text: String,
    pub ending: LineEnding,
    /// The file started with a UTF-8 byte order mark.
    pub bom: bool,
}

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// Read the bytes of a UTF-8 text file; `None` for anything else (binary, other encodings).
pub fn decode(bytes: &[u8]) -> Option<Decoded> {
    let bom = bytes.starts_with(BOM);
    let body = if bom { &bytes[BOM.len()..] } else { bytes };
    if body.contains(&0) {
        return None;
    }
    let text = std::str::from_utf8(body).ok()?;
    let ending = if text.contains("\r\n") {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    };
    Some(Decoded {
        // A carriage return alone ends a line too (old Mac files): without this the
        // whole file would show as one line.
        text: text.replace("\r\n", "\n").replace('\r', "\n"),
        ending,
        bom,
    })
}

/// The bytes to write to disk for `text`, the reverse of [`decode`].
pub fn encode(text: &str, ending: LineEnding, bom: bool) -> Vec<u8> {
    let mut bytes = if bom { BOM.to_vec() } else { Vec::new() };
    match ending {
        LineEnding::Lf => bytes.extend_from_slice(text.as_bytes()),
        LineEnding::CrLf => bytes.extend_from_slice(text.replace('\n', "\r\n").as_bytes()),
    }
    bytes
}

/// Byte offset of the char at `index` (the end of the text when past it).
pub fn byte_index(text: &str, index: usize) -> usize {
    // A char starts at every byte that does not continue one: whole blocks of them are
    // counted at once, which is many times faster than going char by char.
    const BLOCK: usize = 4096;
    let starts_char = |byte: &u8| (*byte as i8) >= -0x40;
    let bytes = text.as_bytes();
    let (mut at, mut left) = (0, index);
    while let Some(block) = bytes.get(at..at + BLOCK) {
        let starts = block.iter().filter(|byte| starts_char(byte)).count();
        if starts > left {
            break;
        }
        left -= starts;
        at += BLOCK;
    }
    // From here the `left`-th char that starts is the one.
    let rest = bytes[at..].iter().enumerate();
    let mut starting = rest.filter(|(_, byte)| starts_char(byte));
    starting
        .nth(left)
        .map_or(text.len(), |(found, _)| at + found)
}

/// Line and column of the char at `index`, both counted from 1.
pub fn line_col(text: &str, index: usize) -> (usize, usize) {
    let before = &text[..byte_index(text, index)];
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(0, |tail| tail.chars().count())
        + 1;
    (line, column)
}

/// Char index where line `line` (counted from 1) starts; the last line when there are fewer.
pub fn line_start(text: &str, line: usize) -> usize {
    let mut start = 0;
    let mut index = 0;
    let mut current = 1;
    for ch in text.chars() {
        if current == line {
            break;
        }
        index += 1;
        if ch == '\n' {
            current += 1;
            start = index;
        }
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding_and_encoding_round_trip() {
        let bytes = b"\xEF\xBB\xBFone\r\ntwo\r\n";
        let decoded = decode(bytes).expect("text");
        assert_eq!(decoded.text, "one\ntwo\n");
        assert_eq!(decoded.ending, LineEnding::CrLf);
        assert!(decoded.bom);
        assert_eq!(encode(&decoded.text, decoded.ending, decoded.bom), bytes);
        assert_eq!(decode(b"one\rtwo").expect("text").text, "one\ntwo");
    }

    #[test]
    fn binary_files_are_refused() {
        assert_eq!(decode(b"PK\x03\x04\x00\x00"), None);
        assert_eq!(decode(b"\xFF\xFE\x41"), None);
    }

    #[test]
    fn indent_is_guessed_from_the_text() {
        assert_eq!(Indent::detect("a\n\tb"), Indent::Tabs);
        assert_eq!(Indent::detect("a {\n  b\n    c\n}"), Indent::Spaces(2));
        assert_eq!(Indent::detect("/*\n * note\n */"), Indent::Spaces(4));
    }

    #[test]
    fn positions_convert_between_chars_and_lines() {
        let text = "é1\nsecond\nthird";
        assert_eq!(line_col(text, 0), (1, 1));
        assert_eq!(line_col(text, 5), (2, 3));
        assert_eq!(line_start(text, 2), 3);
        assert_eq!(line_start(text, 3), 10);
        assert_eq!(line_start(text, 99), 10);
        assert_eq!(byte_index(text, 1), 2);
        // Long enough to be counted by blocks, with chars of every width across them.
        let long = "aé😀\n€z".repeat(3000);
        let slow = |index: usize| {
            long.char_indices()
                .nth(index)
                .map_or(long.len(), |(at, _)| at)
        };
        let chars = long.chars().count();
        for index in [0, 1, 4095, 4096, 4097, 9000, chars - 1, chars, chars + 5] {
            assert_eq!(byte_index(&long, index), slow(index), "char {index}");
        }
        for index in (0..chars).step_by(617) {
            assert_eq!(byte_index(&long, index), slow(index), "char {index}");
        }
    }
}
