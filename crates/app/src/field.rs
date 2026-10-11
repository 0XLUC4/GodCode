//! A text field that lays out only the lines in view.
//!
//! It keeps its caret and its history where egui's own text field keeps them (the state
//! stored under its id), so everything that reads or sets the selection of an editor works
//! with it unchanged. Lines do not wrap: every line is one row, which is what lets a line
//! be found from a height without measuring the ones above it.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use egui::text::{ByteIndex, CCursor, CCursorRange, LayoutJob, LayoutSection, TextFormat};
use egui::{
    Color32, CursorIcon, Event, EventFilter, FontId, Galley, Id, Key, Margin, Modifiers, Pos2,
    Rect, Response, Sense, TextEdit, Ui, Vec2, pos2, vec2,
};
use godcode_core::syntax::{self, Language, Span};

use crate::theme;

/// What is known of a text once it is read through: where its lines start, how it is
/// coloured. Made again only when the text or its colours change.
#[derive(Clone)]
pub struct Index {
    pub key: u64,
    /// The byte each line starts at, then one past the end of the text: line `n` is the
    /// bytes `starts[n]..starts[n + 1] - 1`.
    pub starts: Vec<usize>,
    /// The char each line starts at, then one past the end of the text.
    chars: Vec<usize>,
    /// The colours of a text that has some, kept so that a change is coloured without
    /// the whole text being read again; and the one span of a text that has none.
    coloured: Option<syntax::Coloured>,
    flat: Vec<Span>,
    /// The language the colours are of.
    language: &'static str,
    /// Chars of the longest line.
    widest: usize,
    /// Columns of white space leading each line; `None` for a blank line.
    pub indents: Vec<Option<usize>>,
    /// What the brackets of each line do, kept so that `closes` is made again after a
    /// change without the text being read again.
    brackets: Vec<Brackets>,
    /// For a line that leaves a bracket open, the line that closes it.
    closes: Vec<Option<usize>>,
    /// What a language server said folds, and the number its answer came with: it is
    /// believed instead of the brackets and the indentation.
    given: Option<(u64, Vec<Fold>)>,
}

/// A block as a language server tells it: the line that opens it, the last line it hides.
pub type Fold = (usize, usize);

/// What the brackets of a line do: how many it closes of those the lines above left open,
/// then how many it leaves open itself.
type Brackets = (usize, usize);

/// What the brackets of each line of `region` do; `region` is whole lines of the text the
/// `spans` are of, and starts at its byte `from`. Brackets in strings and in comments do
/// not count; prose has none that open a block.
fn brackets_of(region: &str, from: usize, spans: &[Span], language: &Language) -> Vec<Brackets> {
    if matches!(language.name, "Plain Text" | "Markdown") {
        return region.split('\n').map(|_| (0, 0)).collect();
    }
    let mut lines = Vec::new();
    let (mut closed, mut left_open) = (0, 0);
    let mut span = spans.partition_point(|span| span.end <= from);
    for (at, byte) in region.bytes().enumerate() {
        let opens = matches!(byte, b'(' | b'[' | b'{');
        if byte == b'\n' {
            lines.push((closed, left_open));
            (closed, left_open) = (0, 0);
        } else if opens || matches!(byte, b')' | b']' | b'}') {
            while spans.get(span).is_some_and(|span| span.end <= from + at) {
                span += 1;
            }
            let token = spans.get(span).map(|span| span.token);
            if matches!(token, Some(syntax::Token::String | syntax::Token::Comment)) {
                continue;
            }
            if opens {
                left_open += 1;
            } else if left_open > 0 {
                left_open -= 1;
            } else {
                closed += 1;
            }
        }
    }
    lines.push((closed, left_open));
    lines
}

/// For each line that leaves a bracket open, the line that closes it; when not all of
/// those it leaves open get closed, the line that closed one last.
fn closers(brackets: &[Brackets]) -> Vec<Option<usize>> {
    let mut closes = vec![None; brackets.len()];
    // The lines with brackets still open, and how many each has.
    let mut open: Vec<(usize, usize)> = Vec::new();
    for (line, (closed, left_open)) in brackets.iter().enumerate() {
        let mut closed = *closed;
        while closed > 0
            && let Some((opened, count)) = open.pop()
        {
            // Of several left open on a line, the first one closes last: it is kept.
            closes[opened] = Some(line);
            if count > closed {
                open.push((opened, count - closed));
            }
            closed = closed.saturating_sub(count);
        }
        if *left_open > 0 {
            open.push((line, *left_open));
        }
    }
    closes
}

/// A change of a text: the bytes that were replaced, and the bytes they now are.
type Change = (Range<usize>, Range<usize>);

impl Index {
    #[cfg(test)]
    fn new(key: u64, text: &str, language: &Language) -> Self {
        Self::after(key, text, language, None)
    }

    /// What is known of `text`. `before`: what was known of it before one change, and
    /// that change; only the lines the change has coloured again are then read.
    fn after(key: u64, text: &str, language: &Language, before: Option<(&Index, Change)>) -> Self {
        let followed = before.and_then(|(index, change)| index.follow(key, text, language, change));
        followed.unwrap_or_else(|| Self::read(key, text, language))
    }

    /// What is known of `text`, all of it read.
    fn read(key: u64, text: &str, language: &Language) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
        starts.push(text.len() + 1);
        let mut chars = Vec::with_capacity(starts.len());
        let (mut count, mut widest) = (0, 0);
        for line in starts.windows(2) {
            chars.push(count);
            let length = text[line[0]..line[1] - 1].chars().count();
            widest = widest.max(length);
            count += length + 1;
        }
        chars.push(count);
        // A text without colours keeps no more than its one span.
        let plain = std::ptr::eq(language, Language::plain());
        let coloured = (!plain).then(|| syntax::Coloured::new(text, language));
        let coloured = coloured.filter(|coloured| coloured.spans().len() > 1);
        let flat = match &coloured {
            Some(_) => Vec::new(),
            None => syntax::highlight(text, Language::plain()),
        };
        let spans = coloured
            .as_ref()
            .map_or(&flat[..], |coloured| coloured.spans());
        let brackets = brackets_of(text, 0, spans, language);
        Self {
            key,
            closes: closers(&brackets),
            brackets,
            given: None,
            starts,
            chars,
            coloured,
            flat,
            language: language.name,
            widest,
            indents: crate::editor::indents_of(text),
        }
    }

    /// What is known of `text`, worked out from what was known of it before the change
    /// `(old, new)`: the lines coloured again are read, the others are told by what was
    /// known of them. `None` when that cannot be: a text without colours, before or
    /// after; another language; a change that is not one from the text known.
    fn follow(
        &self,
        key: u64,
        text: &str,
        language: &Language,
        (old, new): Change,
    ) -> Option<Self> {
        let lines = self.lines();
        let was = self.starts[lines] - 1;
        let fits = self.language == language.name
            && old.start == new.start
            && old.start <= old.end
            && old.end <= was
            && new.start <= new.end
            && new.end <= text.len()
            && was - old.end == text.len() - new.end;
        if !fits {
            return None;
        }
        // Where the colouring starts again: the line of the byte before the change.
        let above = self.starts[..lines].partition_point(|start| *start < new.start);
        let first = above.saturating_sub(1);
        let restart = self.starts[first];
        if restart > 0 && text.as_bytes().get(restart - 1) != Some(&b'\n') {
            return None;
        }
        let mut coloured = self.coloured.clone()?;
        coloured.edit(text, language, old.clone(), new.clone());
        // Where it stopped: a line from which the colours are those known.
        let stop = restart + coloured.last_read();
        if coloured.spans().len() < 2 || stop < new.end || stop > text.len() {
            return None;
        }
        // The lines read again, as they are now, and the line known that follows them.
        let (region, past) = if stop < text.len() {
            let region = text.get(restart..stop)?.strip_suffix('\n')?;
            let known = stop - new.end + old.end;
            (region, self.starts[..lines].binary_search(&known).ok()?)
        } else {
            (text.get(restart..)?, lines)
        };
        if past < first {
            return None;
        }
        let size = lines + 1 + region.len() / 16;
        let (mut starts, mut chars) = (Vec::with_capacity(size), Vec::with_capacity(size));
        starts.extend_from_slice(&self.starts[..first]);
        chars.extend_from_slice(&self.chars[..first]);
        let (mut byte, mut count, mut longest) = (restart, self.chars[first], 0);
        for content in region.split('\n') {
            starts.push(byte);
            chars.push(count);
            let length = content.chars().count();
            longest = longest.max(length);
            byte += content.len() + 1;
            count += length + 1;
        }
        // What follows moves along.
        let (from, chars_from) = (self.starts[past], self.chars[past]);
        starts.extend(self.starts[past..].iter().map(|start| start - from + byte));
        chars.extend(
            self.chars[past..]
                .iter()
                .map(|start| start - chars_from + count),
        );
        // The longest line is looked for again only when it was one of those read again
        // and none of them is as long now; the lengths tell it, not the text.
        let had = (first..past).map(|line| self.length(line)).max();
        let widest = if longest >= self.widest {
            longest
        } else if had.is_none_or(|had| had < self.widest) {
            self.widest
        } else {
            let lengths = chars.windows(2).map(|line| line[1] - line[0] - 1);
            lengths.max().unwrap_or(0)
        };
        let mut indents = Vec::with_capacity(size);
        indents.extend_from_slice(&self.indents[..first]);
        indents.extend(crate::editor::indents_of(region));
        indents.extend_from_slice(&self.indents[past..]);
        let mut brackets = Vec::with_capacity(size);
        brackets.extend_from_slice(&self.brackets[..first]);
        brackets.extend(brackets_of(region, restart, coloured.spans(), language));
        brackets.extend_from_slice(&self.brackets[past..]);
        Some(Self {
            key,
            closes: closers(&brackets),
            brackets,
            given: None,
            starts,
            chars,
            coloured: Some(coloured),
            flat: Vec::new(),
            language: language.name,
            widest,
            indents,
        })
    }

    /// All that is known of the text is the same, the key and what a server said left out.
    #[cfg(test)]
    fn same_as(&self, other: &Index) -> bool {
        self.starts == other.starts
            && self.chars == other.chars
            && self.coloured.is_some() == other.coloured.is_some()
            && self.spans() == other.spans()
            && self.flat == other.flat
            && self.language == other.language
            && self.widest == other.widest
            && self.indents == other.indents
            && self.brackets == other.brackets
            && self.closes == other.closes
    }

    /// Chars of the text.
    fn total(&self) -> usize {
        self.chars[self.lines()] - 1
    }

    /// It is all one colour: only where its lines are is known of it, and that can follow
    /// an edit without the text being read through again.
    fn plain(&self) -> bool {
        self.coloured.is_none()
    }

    /// How the text is coloured: each span up to its end.
    fn spans(&self) -> &[Span] {
        match &self.coloured {
            Some(coloured) => coloured.spans(),
            None => &self.flat,
        }
    }

    /// The byte of the char `index`, found by reading its line alone.
    fn byte_of(&self, text: &str, index: usize) -> usize {
        let line = self.line_of(index);
        let (from, to) = (self.starts[line], self.starts[line + 1] - 1);
        match text.get(from..to) {
            Some(content) => from + byte(content, index - self.chars[line]),
            None => byte(text, index),
        }
    }

    /// Follow an edit of a plain text: `removed` bytes at the byte `at` gave way to
    /// `written` ones, and `text` is the result. Only the lines touched are read.
    fn splice(&mut self, text: &str, at: usize, removed: usize, written: usize) {
        let lines = self.lines();
        let line_at = |byte: usize| {
            let after = self.starts[..lines].partition_point(|start| *start <= byte);
            after.saturating_sub(1)
        };
        let (first, last) = (line_at(at), line_at(at + removed));
        let grown = written as isize - removed as isize;
        // The lines touched, as they are now.
        let from = self.starts[first];
        let to = (self.starts[last + 1] - 1).saturating_add_signed(grown);
        let region = text.get(from..to).unwrap_or("");
        let before = self.chars[last + 1] - self.chars[first];
        let (mut starts, mut chars) = (Vec::new(), Vec::new());
        let (mut byte, mut count) = (from, self.chars[first]);
        for content in region.split('\n') {
            starts.push(byte);
            chars.push(count);
            let length = content.chars().count();
            self.widest = self.widest.max(length);
            byte += content.len() + 1;
            count += length + 1;
        }
        let moved = (count - self.chars[first]) as isize - before as isize;
        let added = starts.len();
        self.starts.splice(first..=last, starts);
        self.chars.splice(first..=last, chars);
        self.indents
            .splice(first..=last, crate::editor::indents_of(region));
        self.brackets
            .splice(first..=last, std::iter::repeat_n((0, 0), added));
        self.closes
            .splice(first..=last, std::iter::repeat_n(None, added));
        // What follows moves along.
        for start in &mut self.starts[first + added..] {
            *start = start.saturating_add_signed(grown);
        }
        for start in &mut self.chars[first + added..] {
            *start = start.saturating_add_signed(moved);
        }
        self.flat = vec![Span {
            end: text.len(),
            token: syntax::Token::Plain,
        }];
    }

    /// The char `line` starts at.
    pub fn start_of(&self, line: usize) -> usize {
        self.chars[line.min(self.lines() - 1)]
    }

    /// The lines hidden when the block that `header` opens is folded: up to the line that
    /// closes the bracket it leaves open, or else those under it that are indented deeper,
    /// the blank ones between them too.
    pub fn block(&self, header: usize) -> Option<Range<usize>> {
        if let Some((_, given)) = &self.given {
            // Of several that start on a line, the longest comes last.
            let after = given.partition_point(|(first, _)| *first <= header);
            let (first, last) = *given[..after].last()?;
            let end = (last + 1).min(self.lines());
            return (first == header && end > header + 1).then(|| header + 1..end);
        }
        if let Some(close) = self.closes.get(header).copied().flatten() {
            return (close > header + 1).then(|| header + 1..close);
        }
        let depth = (*self.indents.get(header)?)?;
        let mut last = header;
        for (line, indent) in self.indents.iter().enumerate().skip(header + 1) {
            match indent {
                Some(indent) if *indent <= depth => break,
                Some(_) => last = line,
                None => {}
            }
        }
        (last > header).then(|| header + 1..last + 1)
    }

    /// The line that opens the innermost block `line` is in, or `line` when it opens one.
    pub fn header_of(&self, line: usize) -> Option<usize> {
        if self.block(line).is_some() {
            return Some(line);
        }
        let inside = |header: &usize| {
            self.block(*header)
                .is_some_and(|block| block.contains(&line))
        };
        (0..line).rev().find(inside)
    }

    pub fn lines(&self) -> usize {
        self.starts.len() - 1
    }

    /// The line the char `index` is on.
    pub fn line_of(&self, index: usize) -> usize {
        let after = self.chars.partition_point(|start| *start <= index);
        after.saturating_sub(1).min(self.lines() - 1)
    }

    /// Chars of `line`, its line end left out.
    fn length(&self, line: usize) -> usize {
        self.chars[line + 1] - self.chars[line] - 1
    }

    /// `line` as coloured sections.
    #[cfg(test)]
    fn job(&self, text: &str, line: usize, font: &FontId, dark: bool, plain: Color32) -> LayoutJob {
        let bytes = self.starts[line]..self.starts[line + 1] - 1;
        self.job_of(text, bytes, font, dark, plain)
    }

    /// The bytes `bytes` of the text, all of one line, as coloured sections.
    fn job_of(
        &self,
        text: &str,
        bytes: Range<usize>,
        font: &FontId,
        dark: bool,
        plain: Color32,
    ) -> LayoutJob {
        let (from, to) = (bytes.start, bytes.end);
        let content = text.get(from..to).unwrap_or("");
        let section = |range: Range<usize>, color: Color32| LayoutSection {
            leading_space: 0.0,
            byte_range: ByteIndex(range.start - from)..ByteIndex(range.end - from),
            format: TextFormat::simple(font.clone(), color),
        };
        let mut sections = Vec::new();
        let mut at = from;
        let spans = self.spans();
        let first = spans.partition_point(|span| span.end <= from);
        for span in &spans[first..] {
            if at >= to {
                break;
            }
            let end = span.end.min(to);
            if end > at {
                sections.push(section(at..end, theme::token_color(span.token, dark)));
                at = end;
            }
        }
        if at < to {
            sections.push(section(at..to, plain));
        }
        LayoutJob {
            text: content.to_owned(),
            sections,
            ..Default::default()
        }
    }
}

/// A note written in a line without being part of the text: what a language server says
/// of the type of a name, of the argument a value is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    /// Counted from 0.
    pub line: usize,
    /// The char of the line it is written before.
    pub column: usize,
    pub label: String,
}

/// Move `hints` along with a change made on the line `edited`, which had `width` chars and
/// now has `now`, the caret ending at its char `caret`; the text has `grown` lines more.
/// What cannot be followed is left out: the server says it again.
fn follow(
    hints: &mut Vec<Hint>,
    edited: usize,
    (width, now): (usize, usize),
    caret: usize,
    grown: isize,
) {
    if grown != 0 {
        // The line was cut or joined: where its hints went is not known.
        hints.retain(|hint| hint.line != edited);
        let below = hints.iter_mut().filter(|hint| hint.line > edited);
        below.for_each(|hint| hint.line = hint.line.saturating_add_signed(grown));
        return;
    }
    // Chars came or went before the caret: what stood after them moves as much.
    let moved = now as isize - width as isize;
    let from = caret.saturating_add_signed(-moved);
    let after = |hint: &&mut Hint| hint.line == edited && hint.column >= from;
    let pushed = hints.iter_mut().filter(after);
    pushed.for_each(|hint| hint.column = hint.column.saturating_add_signed(moved));
}

/// The hints of `line`, in a list sorted by line.
fn hints_of(hints: &[Hint], line: usize) -> &[Hint] {
    let from = hints.partition_point(|hint| hint.line < line);
    let to = hints.partition_point(|hint| hint.line <= line);
    &hints[from..to]
}

/// Write `hints` in the line `job`. Returns where each one went: the char of the line it
/// is before, and the chars it takes.
fn annotate(job: &mut LayoutJob, hints: &[Hint], format: &TextFormat) -> Vec<(usize, usize)> {
    let length = job.text.chars().count();
    let mut placed = Vec::with_capacity(hints.len());
    // From the end, so that the places still to write at do not move.
    for hint in hints.iter().rev() {
        let column = hint.column.min(length);
        let (at, added) = (byte(&job.text, column), hint.label.len());
        job.text.insert_str(at, &hint.label);
        // The sections after it move along; the one it lands in is cut in two.
        let mut tail = None;
        for section in &mut job.sections {
            let (start, end) = (section.byte_range.start.0, section.byte_range.end.0);
            if start >= at {
                section.byte_range = ByteIndex(start + added)..ByteIndex(end + added);
            } else if end > at {
                let mut rest = section.clone();
                rest.byte_range = ByteIndex(at + added)..ByteIndex(end + added);
                section.byte_range.end = ByteIndex(at);
                tail = Some(rest);
            }
        }
        job.sections.extend(tail);
        job.sections.push(LayoutSection {
            leading_space: 0.0,
            byte_range: ByteIndex(at)..ByteIndex(at + added),
            format: format.clone(),
        });
        placed.push((column, hint.label.chars().count()));
    }
    job.sections
        .sort_by_key(|section| section.byte_range.start.0);
    placed.reverse();
    placed
}

/// The column the char `column` of a line shows at: the hints written before it push it
/// along. A caret where a hint is stays before it.
fn pushed(hints: &[(usize, usize)], column: usize) -> usize {
    let before = hints.iter().filter(|(at, _)| *at < column);
    column + before.map(|(_, chars)| chars).sum::<usize>()
}

/// The reverse of [`pushed`]: the char of the line that shows at the column `shown`. A
/// hint is no char of the line: a place in it is the char it is written before.
fn pulled(hints: &[(usize, usize)], shown: usize) -> usize {
    let mut pushed = 0;
    for (at, chars) in hints {
        if shown <= at + pushed {
            break;
        }
        if shown < at + pushed + chars {
            return *at;
        }
        pushed += chars;
    }
    shown - pushed
}

/// The columns a char takes in a row: what a row holds is counted in them.
fn columns_of(ch: char) -> usize {
    match ch {
        '\t' => 4,
        // The scripts of East Asia are twice as wide.
        ch if ch as u32 >= 0x2E80 => 2,
        _ => 1,
    }
}

/// The chars at which the rows of `line` start when a row holds `columns`: after a space
/// when there is one to cut at, anywhere otherwise. The first is 0.
fn breaks(line: &str, columns: usize) -> Vec<usize> {
    let mut starts = vec![0];
    // The width of the row so far; the char after its last space, and the width up to it.
    let (mut width, mut space, mut space_width) = (0, None, 0);
    for (at, ch) in line.chars().enumerate() {
        let wide = columns_of(ch);
        if width + wide > columns && width > 0 {
            let (start, carried) = match space {
                Some(after) => (after, width - space_width),
                None => (at, 0),
            };
            starts.push(start);
            (width, space) = (carried, None);
        }
        width += wide;
        if ch == ' ' || ch == '\t' {
            (space, space_width) = (Some(at + 1), width);
        }
    }
    starts
}

/// The row each line starts on, then how many rows there are; `None` when every line takes
/// one row. A line folded away takes none; with `wrap`, the columns a row holds, a long
/// line takes several.
fn row_starts(
    index: &Index,
    text: &str,
    folded: &[usize],
    wrap: Option<usize>,
) -> Option<Vec<usize>> {
    if folded.is_empty() && wrap.is_none() {
        return None;
    }
    let mut hidden = vec![false; index.lines()];
    for block in folded.iter().filter_map(|header| index.block(*header)) {
        hidden[block].fill(true);
    }
    let mut starts = Vec::with_capacity(index.lines() + 1);
    let mut row = 0;
    for (line, hidden) in hidden.iter().enumerate() {
        starts.push(row);
        if *hidden {
            continue;
        }
        let content = text
            .get(index.starts[line]..index.starts[line + 1] - 1)
            .unwrap_or("");
        row += match wrap {
            // Most lines fit: only a long one, or one with tabs, is measured.
            Some(columns) if content.len() > columns || content.contains('\t') => {
                breaks(content, columns).len()
            }
            _ => 1,
        };
    }
    starts.push(row);
    Some(starts)
}

/// The row `line` shows on: for a line folded away, the row of the line it is under.
fn first_row(starts: &[usize], line: usize) -> usize {
    let line = line.min(starts.len() - 2);
    let row = starts[line];
    if starts[line + 1] == row {
        row.saturating_sub(1)
    } else {
        row
    }
}

/// The line that shows on `row`.
fn line_on(starts: &[usize], row: usize) -> usize {
    let lines = &starts[..starts.len() - 1];
    let row = row.min(starts[lines.len()].saturating_sub(1));
    lines
        .partition_point(|start| *start <= row)
        .saturating_sub(1)
}

/// What was worked out of a text the last time it showed.
#[derive(Default)]
pub struct Cache {
    index: Option<Arc<Index>>,
    /// The rows of the lines (see [`row_starts`]), and the key of what they were counted for.
    rows: Option<(u64, Option<Arc<Vec<usize>>>)>,
    /// The text as it last showed.
    seen: Option<Seen>,
    /// The changes made to the text, to take back.
    history: History,
    /// The hints as they were last given, and where the typing since has pushed them.
    hints: (Vec<Hint>, Arc<Vec<Hint>>),
}

/// The text as it last showed: the revision it is expected to come back with, its size,
/// and a number that names it, its hash when it was read through.
#[derive(Clone, Copy)]
struct Seen {
    revision: u64,
    bytes: usize,
    hash: u64,
}

/// Writes in the text for the keyboard, and keeps what is known of the text in step when
/// that can be done without reading it all.
struct Pen<'a> {
    index: Option<&'a mut Index>,
    /// What was written, in order.
    steps: Vec<Written>,
}

impl Pen<'_> {
    /// Chars of the text.
    fn len(&self, text: &str) -> usize {
        match &self.index {
            Some(index) => index.total(),
            None => text.chars().count(),
        }
    }

    /// Write `written` over the selection. Returns the caret after it.
    fn write(&mut self, text: &mut String, caret: Caret, written: &str) -> Caret {
        let chars = caret.sorted();
        // Writing nothing over nothing changes nothing.
        if written.is_empty() && chars.is_empty() {
            return Caret::at(chars.start);
        }
        let bytes = match &self.index {
            Some(index) => index.byte_of(text, chars.start)..index.byte_of(text, chars.end),
            None => byte(text, chars.start)..byte(text, chars.end),
        };
        let removed = text[bytes.clone()].to_owned();
        self.steps.push((bytes.start, removed, written.to_owned()));
        text.replace_range(bytes.clone(), written);
        if let Some(index) = &mut self.index {
            index.splice(text, bytes.start, bytes.len(), written.len());
        }
        Caret::at(chars.start + written.chars().count())
    }
}

/// A change as the keyboard made it: the byte it is at, what was there, what was written.
type Written = (usize, String, String);

/// A change of the text: at the byte `at`, `removed` gave way to `written`.
struct Step {
    at: usize,
    removed: String,
    written: String,
    /// The selection before it and after it.
    before: CCursorRange,
    after: CCursorRange,
    /// Changes made in one go are taken back in one go.
    group: u64,
}

/// Changes less than this apart are made in one go.
const PAUSE_SECONDS: f64 = 1.0;
/// What the history keeps of the text taken out and written takes no more than this: the
/// oldest changes are forgotten first.
const HISTORY_BYTES: usize = 64 * 1024 * 1024;

/// The changes made to a text, to take back and to make again. Only what changed is kept,
/// so a big text costs no more than a small one; and one copy of the text as the history
/// last saw it, against which a change made elsewhere is found.
#[derive(Default)]
pub struct History {
    shadow: Option<String>,
    undos: Vec<Step>,
    redos: Vec<Step>,
    group: u64,
    changed_at: f64,
    /// The selection as last seen: where it was before the next change.
    caret: Option<CCursorRange>,
    /// Bytes of text held by the changes kept.
    bytes: usize,
}

/// How many bytes two texts start with in common.
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    const BLOCK: usize = 4096;
    let mut at = 0;
    // Whole blocks are compared at once.
    while let (Some(ours), Some(theirs)) = (a.get(at..at + BLOCK), b.get(at..at + BLOCK)) {
        if ours != theirs {
            break;
        }
        at += BLOCK;
    }
    let rest = a[at..].iter().zip(&b[at..]);
    at + rest.take_while(|(ours, theirs)| ours == theirs).count()
}

/// How many bytes two texts end with in common.
fn common_suffix(a: &[u8], b: &[u8]) -> usize {
    const BLOCK: usize = 4096;
    let mut len = 0;
    while len + BLOCK <= a.len().min(b.len())
        && a[a.len() - len - BLOCK..a.len() - len] == b[b.len() - len - BLOCK..b.len() - len]
    {
        len += BLOCK;
    }
    let rest = a[..a.len() - len]
        .iter()
        .rev()
        .zip(b[..b.len() - len].iter().rev());
    len + rest.take_while(|(ours, theirs)| ours == theirs).count()
}

impl History {
    /// Take in the text as it is now, at `time`. `own`: what the keyboard wrote since the
    /// last time, when nothing else can have changed the text; with `elsewhere`, the text
    /// may have been changed by anything, and is compared with what it was. Returns the
    /// stretch that changed since the last time, when one stretch did.
    pub fn see(
        &mut self,
        text: &str,
        caret: Option<CCursorRange>,
        time: f64,
        own: Vec<Written>,
        elsewhere: bool,
    ) -> Option<Change> {
        let caret = caret.or(self.caret);
        let Some(mut shadow) = self.shadow.take() else {
            self.shadow = Some(text.to_owned());
            self.caret = caret;
            return None;
        };
        let after = caret.unwrap_or_default();
        // The one stretch that changed, when it is one.
        let mut change = None;
        if !elsewhere {
            let alone = own.len() == 1;
            for (at, removed, written) in own {
                if alone {
                    change = Some((at..at + removed.len(), at..at + written.len()));
                }
                shadow.replace_range(at..at + removed.len(), &written);
                self.push(at, removed, written, after, time);
            }
        } else if shadow != text {
            // One stretch differs: what the two have in common around it stays.
            let mut prefix = common_prefix(shadow.as_bytes(), text.as_bytes());
            while !text.is_char_boundary(prefix) {
                prefix -= 1;
            }
            let (ours, theirs) = (&shadow.as_bytes()[prefix..], &text.as_bytes()[prefix..]);
            let mut suffix = common_suffix(ours, theirs);
            while !text.is_char_boundary(text.len() - suffix) {
                suffix -= 1;
            }
            let removed = shadow[prefix..shadow.len() - suffix].to_owned();
            let written = text[prefix..text.len() - suffix].to_owned();
            change = Some((prefix..shadow.len() - suffix, prefix..text.len() - suffix));
            shadow.replace_range(prefix..shadow.len() - suffix, &written);
            self.push(prefix, removed, written, after, time);
        }
        self.shadow = Some(shadow);
        self.caret = caret;
        change
    }

    fn push(
        &mut self,
        at: usize,
        removed: String,
        written: String,
        after: CCursorRange,
        time: f64,
    ) {
        if time - self.changed_at > PAUSE_SECONDS {
            self.group += 1;
        }
        self.changed_at = time;
        let held = |step: &Step| step.removed.len() + step.written.len();
        // What was taken back cannot be made again once the text went another way.
        self.bytes -= self.redos.drain(..).map(|step| held(&step)).sum::<usize>();
        self.bytes += removed.len() + written.len();
        self.undos.push(Step {
            at,
            removed,
            written,
            before: self.caret.unwrap_or(after),
            after,
            group: self.group,
        });
        while self.bytes > HISTORY_BYTES && self.undos.len() > 1 {
            self.bytes -= held(&self.undos.remove(0));
        }
    }

    /// Take back the last changes made in one go. Returns the selection as it was before
    /// them; `None` when there is nothing to take back.
    pub fn undo(&mut self, text: &mut String) -> Option<CCursorRange> {
        self.step(text, true)
    }

    /// Make again the changes last taken back. Returns the selection after them.
    pub fn redo(&mut self, text: &mut String) -> Option<CCursorRange> {
        self.step(text, false)
    }

    fn step(&mut self, text: &mut String, back: bool) -> Option<CCursorRange> {
        let (from, to) = match back {
            true => (&mut self.undos, &mut self.redos),
            false => (&mut self.redos, &mut self.undos),
        };
        let group = from.last()?.group;
        let mut caret = None;
        while let Some(step) = from.pop_if(|step| step.group == group) {
            let (there, wanted) = match back {
                true => (&step.written, &step.removed),
                false => (&step.removed, &step.written),
            };
            let range = step.at..step.at + there.len();
            if text.get(range.clone()) != Some(there.as_str()) {
                // The text is not what the history thinks: better to forget than to spoil it.
                *self = Self::default();
                return caret;
            }
            text.replace_range(range.clone(), wanted);
            if let Some(shadow) = &mut self.shadow {
                shadow.replace_range(range, wanted);
            }
            caret = Some(if back { step.before } else { step.after });
            to.push(step);
        }
        // What is typed next is a change of its own.
        self.group += 1;
        self.caret = caret;
        caret
    }
}

impl Cache {
    /// The changes made to the text.
    pub fn history(&mut self) -> &mut History {
        &mut self.history
    }

    /// The text may have changed in a way the revision will not tell: read it again.
    pub fn forget(&mut self) {
        self.seen = None;
    }

    /// The row `line` showed on the last time, keeping its fraction.
    pub fn row_of(&self, line: f32) -> f32 {
        match self.rows.as_ref().and_then(|(_, rows)| rows.as_ref()) {
            Some(starts) => first_row(starts, line as usize) as f32 + line.fract(),
            None => line,
        }
    }
}

/// A row in view, laid out: a line, or the part of a line that fits the width.
struct Row {
    line: usize,
    /// Which row of its line it is, and whether it is the last one.
    part: usize,
    last: bool,
    /// The chars of the line it shows.
    chars: Range<usize>,
    galley: Arc<Galley>,
    /// The hints written in it: the char of the row each one is before, and the chars it
    /// takes.
    hints: Vec<(usize, usize)>,
}

impl Row {
    /// Where the char `column` of the line is in the row.
    fn x(&self, column: usize) -> f32 {
        let within = column.clamp(self.chars.start, self.chars.end) - self.chars.start;
        let shown = CCursor::new(pushed(&self.hints, within));
        self.galley.pos_from_cursor(shown).min.x
    }

    /// The char of the line at `x`.
    fn column(&self, x: f32) -> usize {
        let shown = self.galley.cursor_from_pos(vec2(x, 0.0)).index.0;
        let column = self.chars.start + pulled(&self.hints, shown);
        // The end of a row that goes on below is the start of the next one.
        let end = self.chars.end - usize::from(!self.last);
        column.min(end.max(self.chars.start))
    }
}

/// Where the chars of the text are, measured for the lines in view only.
pub struct Lines {
    index: Arc<Index>,
    /// The row each line starts on (see [`row_starts`]); `None` when it is its own number.
    rows: Option<Arc<Vec<usize>>>,
    /// The columns a row holds, when lines are cut to fit.
    wrap: Option<usize>,
    /// The lines whose block is folded.
    folded: Vec<usize>,
    row_height: f32,
    /// Width of a digit: a column, for the lines that are not measured.
    digit: f32,
    shown: Vec<Row>,
}

impl Lines {
    /// The row in view that shows the char `column` of `line`.
    fn row(&self, line: usize, column: usize) -> Option<&Row> {
        let holds = |row: &&Row| {
            row.line == line
                && row.chars.start <= column
                && (column < row.chars.end || (row.last && column == row.chars.end))
        };
        self.shown.iter().find(holds)
    }

    /// How many rows `line` takes.
    fn rows_of(&self, line: usize) -> usize {
        match &self.rows {
            None => 1,
            Some(starts) => starts[line + 1] - starts[line],
        }
    }

    /// Where the char `index` is, from the top left of the text: a rectangle without
    /// width, as tall as its row.
    pub fn pos(&self, index: usize) -> Rect {
        let line = self.index.line_of(index);
        let column = (index - self.index.chars[line]).min(self.index.length(line));
        let (part, x) = match (self.row(line, column), self.wrap) {
            (Some(row), _) => (row.part, row.x(column)),
            // Not laid out: a column is as wide as a digit, a row is cut where it is full.
            (None, Some(columns)) if self.rows_of(line) > 1 => {
                let part = (column / columns).min(self.rows_of(line) - 1);
                (part, (column - part * columns) as f32 * self.digit)
            }
            (None, _) => (0, column as f32 * self.digit),
        };
        let top = (self.row_of(line) + part) as f32 * self.row_height;
        Rect::from_min_size(pos2(x, top), vec2(0.0, self.row_height))
    }

    /// The first row `line` shows on: for a line folded away, the row of the line it is
    /// under.
    fn row_of(&self, line: usize) -> usize {
        match &self.rows {
            None => line,
            Some(starts) => first_row(starts, line),
        }
    }

    /// The line that shows on `row`.
    fn line_on(&self, row: usize) -> usize {
        match &self.rows {
            None => row.min(self.index.lines() - 1),
            Some(starts) => line_on(starts, row),
        }
    }

    /// How many rows there are.
    fn total(&self) -> usize {
        let rows = self.rows.as_ref().map(|starts| starts[starts.len() - 1]);
        rows.unwrap_or(self.index.lines()).max(1)
    }

    /// The height `line` starts at, or `None` when it is folded away.
    pub fn top(&self, line: usize) -> Option<f32> {
        (self.rows_of(line) > 0).then(|| self.row_of(line) as f32 * self.row_height)
    }

    /// The line that row `row` shows, keeping its fraction.
    pub fn line_of_row(&self, row: f32) -> f32 {
        self.line_on(row.max(0.0) as usize) as f32 + row.fract()
    }

    /// Whether `line` opens a block, and whether that block is folded.
    pub fn foldable(&self, line: usize) -> Option<bool> {
        self.index.block(line)?;
        Some(self.folded.contains(&line))
    }

    /// The char at `within`, a place counted from the top left of the text.
    pub fn at(&self, within: Vec2) -> usize {
        let row = (within.y / self.row_height).floor().max(0.0) as usize;
        let line = self.line_on(row);
        let part = row.saturating_sub(self.row_of(line));
        let laid_out = |shown: &&Row| shown.line == line && shown.part == part;
        let column = match (self.shown.iter().find(laid_out), self.wrap) {
            (Some(row), _) => row.column(within.x),
            (None, wrap) => {
                let before = wrap.map_or(0, |columns| part * columns);
                before + (within.x / self.digit).round().max(0.0) as usize
            }
        };
        self.index.chars[line] + column.min(self.index.length(line))
    }

    /// The line at the height `y`, counted from the top of the text.
    pub fn line_at(&self, y: f32) -> usize {
        self.line_on((y / self.row_height).floor().max(0.0) as usize)
    }

    /// The lines that start between two heights, each with the height it starts at.
    pub fn between(&self, top: f32, bottom: f32) -> Vec<(usize, f32)> {
        let row = |y: f32| ((y / self.row_height).floor().max(0.0) as usize).min(self.total() - 1);
        let starting = (row(top)..=row(bottom)).filter_map(|row| {
            let line = self.line_on(row);
            (self.row_of(line) == row).then_some((line, row as f32 * self.row_height))
        });
        starting.collect()
    }

    pub fn index(&self) -> &Arc<Index> {
        &self.index
    }
}

/// How the field looks.
pub struct Look<'a> {
    pub language: &'static Language,
    pub font: &'a FontId,
    pub dark: bool,
    pub margin: Margin,
    /// The field is at least this big, so that a click under the text lands in it.
    pub min_size: Vec2,
    /// What a language server writes in the lines, sorted by line then by column.
    pub hints: &'a [Hint],
    /// What a language server says folds, sorted, and a number that changes with its
    /// answers; `None` when none says: the brackets and the indentation tell.
    pub folds: Option<(u64, &'a [Fold])>,
    /// The caret covers its char instead of standing before it: keys are commands.
    pub block_caret: bool,
    /// Lines too long for the view go on below instead of running off it.
    pub wrap: bool,
    /// A number that changes whenever the text may have changed since the field last
    /// showed it: while it stays, the text is not read through again.
    pub revision: u64,
}

pub struct Shown {
    pub response: Response,
    /// The top left of the text on screen.
    pub origin: Pos2,
    pub cursor: Option<CCursorRange>,
    /// The text was changed by what was typed.
    pub changed: bool,
    pub lines: Lines,
}

fn key_of(text_hash: u64, look: &Look<'_>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text_hash.hash(&mut hasher);
    // A theme of the user read again keeps its name, not its place.
    std::ptr::from_ref(theme::current()).hash(&mut hasher);
    look.language.name.hash(&mut hasher);
    look.font.size.to_bits().hash(&mut hasher);
    look.dark.hash(&mut hasher);
    hasher.finish()
}

/// A caret and where its selection started, in chars.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Caret {
    /// What moves.
    head: usize,
    /// What stays when the selection is extended.
    anchor: usize,
}

impl Caret {
    fn of(range: CCursorRange, len: usize) -> Self {
        Self {
            head: range.primary.index.0.min(len),
            anchor: range.secondary.index.0.min(len),
        }
    }

    fn range(self) -> CCursorRange {
        CCursorRange {
            primary: CCursor::new(self.head),
            secondary: CCursor::new(self.anchor),
            h_pos: None,
        }
    }

    fn sorted(self) -> Range<usize> {
        self.head.min(self.anchor)..self.head.max(self.anchor)
    }

    fn at(index: usize) -> Self {
        Self {
            head: index,
            anchor: index,
        }
    }
}

fn byte(text: &str, index: usize) -> usize {
    godcode_core::byte_index(text, index)
}

/// The chars the line of `index` starts and ends at.
fn line_bounds(text: &str, index: usize) -> (usize, usize) {
    let at = byte(text, index);
    let start = text[..at].rfind('\n').map_or(0, |found| found + 1);
    let end = text[at..].find('\n').map_or(text.len(), |found| at + found);
    let before = text[start..at].chars().count();
    (index - before, index + text[at..end].chars().count())
}

/// One row up or down, keeping the column when the row is long enough. `wrap`: the columns
/// a row holds when lines are cut to fit; without it a row is a line.
fn vertical(text: &str, index: usize, down: bool, wrap: Option<usize>) -> usize {
    let (start, end) = line_bounds(text, index);
    let parts = |start: usize, end: usize| match wrap {
        Some(columns) => breaks(&text[byte(text, start)..byte(text, end)], columns),
        None => vec![0],
    };
    // The chars of the row `part` of a line, the last one left to the row that follows.
    let bounds = |cut: &[usize], part: usize, length: usize| {
        let to = cut.get(part + 1).map_or(length, |next| next - 1);
        (cut[part], to)
    };
    let cut = parts(start, end);
    let part = cut.partition_point(|from| *from <= index - start) - 1;
    let within = index - start - cut[part];
    let place = |line: usize, (from, to): (usize, usize)| line + (from + within).min(to);
    if down {
        if part + 1 < cut.len() {
            return place(start, bounds(&cut, part + 1, end - start));
        }
        if byte(text, end) == text.len() {
            return end;
        }
        let (next, next_end) = line_bounds(text, end + 1);
        place(next, bounds(&parts(next, next_end), 0, next_end - next))
    } else {
        if part > 0 {
            return place(start, bounds(&cut, part - 1, end - start));
        }
        if start == 0 {
            return 0;
        }
        let (previous, previous_end) = line_bounds(text, start - 1);
        let cut = parts(previous, previous_end);
        place(
            previous,
            bounds(&cut, cut.len() - 1, previous_end - previous),
        )
    }
}

/// The next stop of a caret moving word by word.
fn word_stop(text: &str, index: usize, forward: bool) -> usize {
    let kind = |ch: char| {
        if ch.is_alphanumeric() || ch == '_' {
            0
        } else if ch.is_whitespace() {
            1
        } else {
            2
        }
    };
    let at = byte(text, index);
    let mut moved = 0;
    if forward {
        // Over the white space, then over what follows it.
        let mut chars = text[at..].chars().peekable();
        while chars.next_if(|ch| kind(*ch) == 1).is_some() {
            moved += 1;
        }
        let word = chars.peek().map(|ch| kind(*ch));
        while chars.next_if(|ch| Some(kind(*ch)) == word).is_some() {
            moved += 1;
        }
        index + moved
    } else {
        let mut chars = text[..at].chars().rev().peekable();
        while chars.next_if(|ch| kind(*ch) == 1).is_some() {
            moved += 1;
        }
        let word = chars.peek().map(|ch| kind(*ch));
        while chars.next_if(|ch| Some(kind(*ch)) == word).is_some() {
            moved += 1;
        }
        index - moved
    }
}

/// The word around `index`: what a double click selects.
fn word_at(text: &str, index: usize) -> Range<usize> {
    let wordy = |ch: char| ch.is_alphanumeric() || ch == '_';
    let at = byte(text, index);
    let after = text[at..].chars().take_while(|ch| wordy(*ch)).count();
    let before = text[..at].chars().rev().take_while(|ch| wordy(*ch)).count();
    index - before..index + after
}

/// Write `written` over the selection. Returns the caret after it.
#[cfg(test)]
fn replace(text: &mut String, caret: Caret, written: &str) -> Caret {
    let chars = caret.sorted();
    let bytes = byte(text, chars.start)..byte(text, chars.end);
    text.replace_range(bytes, written);
    Caret::at(chars.start + written.chars().count())
}

/// What the keyboard did to the text and the caret this frame. `page`: rows in view;
/// `wrap`: the columns a row holds when lines are cut to fit.
fn typed(
    ui: &Ui,
    text: &mut String,
    mut caret: Caret,
    page: usize,
    wrap: Option<usize>,
    index: Option<&mut Index>,
) -> (Caret, Vec<Written>) {
    let mut pen = Pen {
        index,
        steps: Vec::new(),
    };
    let events = ui.input(|input| input.events.clone());
    for event in events {
        let len = pen.len(text);
        match event {
            Event::Text(written) | Event::Paste(written) => {
                let written = written.replace("\r\n", "\n");
                caret = pen.write(text, caret, &written);
            }
            Event::Ime(egui::ImeEvent::Commit(written)) => {
                caret = pen.write(text, caret, &written);
            }
            // What is being composed shows selected: what comes next takes its place.
            Event::Ime(egui::ImeEvent::Preedit { text: composed, .. }) => {
                let start = caret.sorted().start;
                let head = pen.write(text, caret, &composed).head;
                caret = Caret {
                    head,
                    anchor: start,
                };
            }
            Event::Copy | Event::Cut => {
                let chars = caret.sorted();
                if !chars.is_empty() {
                    let bytes = byte(text, chars.start)..byte(text, chars.end);
                    ui.ctx().copy_text(text[bytes].to_owned());
                    if event == Event::Cut {
                        caret = pen.write(text, caret, "");
                    }
                    continue;
                }
                // Nothing selected: the line of the caret is meant, whole, with its end.
                let at = byte(text, caret.head);
                let start = text[..at].rfind('\n').map_or(0, |found| found + 1);
                let end = text[at..]
                    .find('\n')
                    .map_or(text.len(), |found| at + found + 1);
                let mut line = text[start..end].to_owned();
                if !line.ends_with('\n') {
                    line.push('\n');
                }
                ui.ctx().copy_text(line);
                if event == Event::Cut {
                    // The last line has no end of its own: the one before it goes with it.
                    let last = end == text.len() && !text[start..end].ends_with('\n');
                    let start = start - usize::from(last && start > 0);
                    let first = caret.head - text[start..at].chars().count();
                    let whole = Caret {
                        head: first + text[start..end].chars().count(),
                        anchor: first,
                    };
                    caret = pen.write(text, whole, "");
                }
            }
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                let word = modifiers.command || modifiers.alt;
                let moved = match key {
                    Key::ArrowLeft | Key::ArrowRight if !modifiers.shift && !word => {
                        // A selection collapses to its side instead of moving.
                        let chars = caret.sorted();
                        let right = key == Key::ArrowRight;
                        Some(match (chars.is_empty(), right) {
                            (false, false) => chars.start,
                            (false, true) => chars.end,
                            (true, false) => caret.head.saturating_sub(1),
                            (true, true) => (caret.head + 1).min(len),
                        })
                    }
                    Key::ArrowLeft if word => Some(word_stop(text, caret.head, false)),
                    Key::ArrowRight if word => Some(word_stop(text, caret.head, true)),
                    Key::ArrowLeft => Some(caret.head.saturating_sub(1)),
                    Key::ArrowRight => Some((caret.head + 1).min(len)),
                    Key::ArrowUp => Some(vertical(text, caret.head, false, wrap)),
                    Key::ArrowDown => Some(vertical(text, caret.head, true, wrap)),
                    Key::PageUp | Key::PageDown => {
                        let down = key == Key::PageDown;
                        let step = |at, _| vertical(text, at, down, wrap);
                        Some((0..page.max(1)).fold(caret.head, step))
                    }
                    Key::Home if modifiers.command => Some(0),
                    Key::End if modifiers.command => Some(len),
                    Key::Home => {
                        // To the first letter of the line, then to its very start.
                        let (start, end) = line_bounds(text, caret.head);
                        let line = text.chars().skip(start).take(end - start);
                        let lead = line.take_while(|ch| *ch == ' ' || *ch == '\t').count();
                        Some(if caret.head == start + lead {
                            start
                        } else {
                            start + lead
                        })
                    }
                    Key::End => Some(line_bounds(text, caret.head).1),
                    _ => None,
                };
                if let Some(head) = moved {
                    caret = if modifiers.shift {
                        Caret { head, ..caret }
                    } else {
                        Caret::at(head)
                    };
                    continue;
                }
                match key {
                    Key::A if modifiers.command => {
                        caret = Caret {
                            head: len,
                            anchor: 0,
                        }
                    }
                    Key::Backspace | Key::Delete => {
                        if caret.sorted().is_empty() {
                            let back = key == Key::Backspace;
                            let to = match (back, word) {
                                (true, true) => word_stop(text, caret.head, false),
                                (true, false) => caret.head.saturating_sub(1),
                                (false, true) => word_stop(text, caret.head, true),
                                (false, false) => (caret.head + 1).min(len),
                            };
                            caret.anchor = to;
                        }
                        caret = pen.write(text, caret, "");
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    (caret, pen.steps)
}

/// Show `text` over the rest of `ui`, which is expected to scroll. `cache` holds what was
/// worked out of the text the last time.
pub fn show(
    ui: &mut Ui,
    id: Id,
    text: &mut String,
    look: &Look<'_>,
    cache: &mut Cache,
    folded: &mut Vec<usize>,
) -> Shown {
    let (digit, row_height) = ui.fonts_mut(|fonts| {
        (
            fonts.glyph_width(look.font, '0'),
            fonts.row_height(look.font),
        )
    });
    let clip = ui.clip_rect();
    let page = ((clip.height() / row_height) as usize).saturating_sub(1);
    let margin = look.margin;
    let sides = f32::from(margin.left + margin.right);
    // Cut to fit, a row holds what the view is wide, less a column kept for the caret.
    let wrap = look.wrap.then(|| {
        let columns = ((look.min_size.x - sides) / digit) as usize;
        columns.saturating_sub(1).max(20)
    });

    let mut state = TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
    // The text as it last showed, when nothing can have changed it since.
    let known = cache
        .seen
        .filter(|seen| seen.revision == look.revision && seen.bytes == text.len())
        .zip(cache.index.as_ref());
    let len = match &known {
        Some((_, index)) => index.total(),
        None => text.chars().count(),
    };
    let known = known.map(|(seen, _)| seen);
    let stored = state.cursor.char_range();
    let mut caret = stored.map(|range| Caret::of(range, len));
    let focused = ui.memory(|memory| memory.has_focus(id));
    // Given the keyboard before any caret was put: it starts at the end, as in egui's field.
    if focused && caret.is_none() {
        caret = Some(Caret::at(len));
    }
    let before = (caret, text.len());
    // What the folds are told of the text as it was: its key, its lines, the line of the
    // caret.
    let old = known.zip(cache.index.as_ref()).map(|(_, index)| {
        let line = caret.map(|caret| index.line_of(caret.head));
        let width = line.map_or(0, |line| index.length(line));
        (index.key, index.lines(), line, width)
    });
    let mut changed = false;
    // What is known of the text was kept in step with what was typed.
    let mut followed = false;
    // What the keyboard wrote, and whether it asked to take a change back or make it again.
    let mut own = Vec::new();
    let mut wanted = None;
    if focused && let Some(old) = caret {
        // The arrows and Tab are the field's, not a way to another widget.
        let filter = EventFilter {
            tab: true,
            horizontal_arrows: true,
            vertical_arrows: true,
            escape: false,
        };
        ui.memory_mut(|memory| memory.set_focus_lock_filter(id, filter));
        let (undo, redo) = ui.input_mut(|input| {
            let shifted = Modifiers::COMMAND | Modifiers::SHIFT;
            let redo =
                input.consume_key(shifted, Key::Z) || input.consume_key(Modifiers::COMMAND, Key::Y);
            (input.consume_key(Modifiers::COMMAND, Key::Z), redo)
        });
        // A plain text that is known as it is: the lines touched alone are read again.
        let live = match &mut cache.index {
            Some(index) if known.is_some() && index.plain() && !undo && !redo => {
                Some(Arc::make_mut(index))
            }
            _ => None,
        };
        followed = live.is_some();
        let (new, written) = typed(ui, text, old, page, wrap, live);
        own = written;
        wanted = undo.then_some(true).or(redo.then_some(false));
        changed = !own.is_empty();
        caret = Some(new);
    }
    // The history is told what changed, here or elsewhere; then it is asked to step.
    let time = ui.input(|input| input.time);
    let history = &mut cache.history;
    let mut change = history.see(text, caret.map(Caret::range), time, own, known.is_none());
    let stepped = wanted.and_then(|back| match back {
        true => history.undo(text),
        false => history.redo(text),
    });
    if let Some(range) = stepped {
        // The text went back or forth after it was seen: what changed is not known.
        change = None;
        caret = Some(Caret::of(range, text.chars().count()));
        changed = true;
    }

    // What is known of the text, made again when it changed. Once the field itself changed
    // it, the caller is expected to say so by moving its revision on by one.
    let seen = match known {
        Some(seen) if !changed => seen,
        Some(seen) if followed => {
            // Not read through: a name of its own, made from the one it had.
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (seen.hash, text.len(), caret.map(|caret| caret.head)).hash(&mut hasher);
            Seen {
                revision: look.revision.wrapping_add(1),
                bytes: text.len(),
                hash: hasher.finish(),
            }
        }
        _ => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut hasher);
            Seen {
                revision: look.revision.wrapping_add(u64::from(changed)),
                bytes: text.len(),
                hash: hasher.finish(),
            }
        }
    };
    cache.seen = Some(seen);
    let key = key_of(seen.hash, look);
    if let (true, true, Some(index)) = (followed, changed, &mut cache.index) {
        Arc::make_mut(index).key = key;
    }
    let mut given = cache.index.as_ref().and_then(|index| index.given.clone());
    let mut index = match cache.index.as_ref().filter(|index| index.key == key) {
        Some(index) => Arc::clone(index),
        None => {
            let before = cache.index.as_deref().zip(change);
            Arc::new(Index::after(key, text, look.language, before))
        }
    };

    // The folds follow the text: lines typed above them push them down; a text changed
    // from elsewhere gives no way to tell where they went.
    // The hints given anew are the ones; the others follow the typing like the folds.
    if cache.hints.0 != look.hints {
        cache.hints = (look.hints.to_vec(), Arc::new(look.hints.to_vec()));
    }
    if let Some((_, lines, line, width)) = old.filter(|(key, ..)| *key != index.key) {
        let grown = index.lines() as isize - lines as isize;
        match line {
            Some(edited) if changed => {
                if !cache.hints.1.is_empty() {
                    let line = edited.min(index.lines() - 1);
                    let head = caret.map_or(0, |caret| caret.head);
                    let column = head.saturating_sub(index.start_of(line));
                    let widths = (width, index.length(line));
                    follow(
                        Arc::make_mut(&mut cache.hints.1),
                        edited,
                        widths,
                        column,
                        grown,
                    );
                }
                let below = folded.iter_mut().filter(|header| **header > edited);
                below.for_each(|header| *header = header.saturating_add_signed(grown));
                // What the server said follows too, until it says again.
                for (first, last) in given.iter_mut().flat_map(|(_, list)| list) {
                    if *first > edited {
                        *first = first.saturating_add_signed(grown);
                    }
                    if *last >= edited {
                        *last = last.saturating_add_signed(grown);
                    }
                }
            }
            _ if grown != 0 => {
                folded.clear();
                given = None;
                cache.hints.1 = Arc::default();
            }
            _ => {}
        }
    }
    match look.folds {
        Some((stamp, list)) if given.as_ref().is_none_or(|(had, _)| *had != stamp) => {
            given = Some((stamp, list.to_vec()));
        }
        None => given = None,
        _ => {}
    }
    if index.given != given {
        // Let go of here first, it is not copied to be changed.
        cache.index = None;
        Arc::make_mut(&mut index).given = given;
    }
    cache.index = Some(Arc::clone(&index));
    folded.retain(|header| index.block(*header).is_some());
    // A caret is never left where it cannot be seen: its block opens.
    if let Some(caret) = caret {
        let line = index.line_of(caret.head);
        folded.retain(|header| {
            !index
                .block(*header)
                .is_some_and(|block| block.contains(&line))
        });
    }
    folded.sort_unstable();
    folded.dedup();
    // The rows of the lines, counted again when the text, the folds or the width change.
    let rows_key = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (index.key, &index.given, &*folded, wrap).hash(&mut hasher);
        hasher.finish()
    };
    let rows = match &cache.rows {
        Some((key, rows)) if *key == rows_key => rows.clone(),
        _ => {
            let rows = row_starts(&index, text, folded, wrap).map(Arc::new);
            cache.rows = Some((rows_key, rows.clone()));
            rows
        }
    };

    // The hints make their lines longer: by no more than the longest of them all.
    let placed = Arc::clone(&cache.hints.1);
    let by_line = placed.chunk_by(|a, b| a.line == b.line);
    let chars = |line: &[Hint]| line.iter().map(|hint| hint.label.chars().count()).sum();
    let hinted: usize = by_line.map(chars).max().unwrap_or(0);
    let mut lines = Lines {
        index,
        rows,
        wrap,
        folded: folded.clone(),
        row_height,
        digit,
        shown: Vec::new(),
    };
    let total = lines.total();
    // Cut to fit, the lines are no wider than the view.
    let width = match wrap {
        Some(_) => 0.0,
        None => (lines.index.widest + hinted) as f32 * digit + sides + digit,
    };
    let height = total as f32 * row_height + f32::from(margin.top + margin.bottom);
    let size = vec2(width, height).max(look.min_size);
    let (_, rect) = ui.allocate_space(size);
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let origin = rect.min + vec2(f32::from(margin.left), f32::from(margin.top));

    // Only the rows in view are laid out.
    let plain = ui.visuals().text_color();
    let first = (((clip.min.y - origin.y) / row_height).floor().max(0.0) as usize).min(total - 1);
    let last = (((clip.max.y - origin.y) / row_height).ceil().max(0.0) as usize).min(total - 1);
    let hint_format = TextFormat {
        font_id: FontId::new(look.font.size * 0.9, look.font.family.clone()),
        color: ui.visuals().weak_text_color(),
        background: ui.visuals().faint_bg_color,
        valign: egui::Align::Center,
        ..Default::default()
    };
    // The line being laid out, and the chars its rows start at.
    let mut cut: (usize, Vec<usize>) = (usize::MAX, Vec::new());
    for row in first..=last {
        let line = lines.line_on(row);
        let part = row.saturating_sub(lines.row_of(line));
        let bytes = lines.index.starts[line]..lines.index.starts[line + 1] - 1;
        let content = text.get(bytes.clone()).unwrap_or("");
        if cut.0 != line {
            let starts = wrap.map_or_else(|| vec![0], |columns| breaks(content, columns));
            cut = (line, starts);
        }
        let length = lines.index.length(line);
        let from = cut.1.get(part).copied().unwrap_or(length);
        let to = cut.1.get(part + 1).copied().unwrap_or(length);
        let last_part = part + 1 >= cut.1.len();
        let shown = match wrap {
            Some(_) => bytes.start + byte(content, from)..bytes.start + byte(content, to),
            None => bytes,
        };
        let mut job = lines.index.job_of(text, shown, look.font, look.dark, plain);
        // The hints of the row, placed from its first char.
        let hints = hints_of(&placed, line).iter().filter_map(|hint| {
            let column = hint.column.min(length);
            let here = column >= from && (column < to || last_part);
            here.then(|| Hint {
                line,
                column: column - from,
                label: hint.label.clone(),
            })
        });
        let hints: Vec<Hint> = hints.collect();
        let hints = annotate(&mut job, &hints, &hint_format);
        lines.shown.push(Row {
            line,
            part,
            last: last_part,
            chars: from..to,
            galley: ui.fonts_mut(|fonts| fonts.layout_job(job)),
            hints,
        });
    }

    // The mouse: a press puts the caret, a drag selects, two and three clicks take more.
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Text);
    }
    let pointer = response.interact_pointer_pos().or(response.hover_pos());
    let primary = ui.input(|input| input.pointer.primary_down());
    if let Some(pointer) = pointer.filter(|_| primary) {
        let at = lines.at(pointer - origin);
        let pressed = ui.input(|input| input.pointer.primary_pressed());
        let (shift, alt) = ui.input(|input| (input.modifiers.shift, input.modifiers.alt));
        // With Alt and Shift a rectangle is being selected: that is the editor's to do.
        let columnar = shift && alt;
        if pressed && response.hovered() {
            response.request_focus();
            let anchor = caret.filter(|_| shift).map_or(at, |old| old.anchor);
            if !columnar {
                caret = Some(Caret { head: at, anchor });
            }
        } else if response.dragged()
            && !columnar
            && let Some(old) = caret
        {
            caret = Some(Caret { head: at, ..old });
        }
    }
    if let Some(pointer) = response.interact_pointer_pos() {
        let at = lines.at(pointer - origin);
        if response.triple_clicked() {
            let (start, end) = line_bounds(text, at);
            caret = Some(Caret {
                head: end,
                anchor: start,
            });
        } else if response.double_clicked() {
            let word = word_at(text, at);
            caret = Some(Caret {
                head: word.end,
                anchor: word.start,
            });
        }
    }

    // Selection, text, caret: in that order, each over the one before.
    let painter = ui.painter_at(clip);
    let selection = ui.visuals().selection.bg_fill;
    let selected = caret.map(Caret::sorted).filter(|chars| !chars.is_empty());
    for row in &lines.shown {
        let rows_above = lines.row_of(row.line) + row.part;
        let top = origin + vec2(0.0, rows_above as f32 * row_height);
        if let Some(chars) = &selected {
            let base = lines.index.chars[row.line];
            let (start, end) = (base + row.chars.start, base + row.chars.end);
            if chars.start <= end && chars.end >= start {
                let from = row.x(chars.start.max(start) - base);
                // A selection that goes on past the row takes its end too.
                let past = if chars.end > end { digit * 0.5 } else { 0.0 };
                let to = row.x(chars.end.min(end) - base) + past;
                let band = Rect::from_min_max(pos2(from, 0.0), pos2(to, row_height));
                painter.rect_filled(band.translate(top.to_vec2()), 0.0, selection);
            }
        }
        // What a folded block hides is told after the line that opens it.
        if row.last && lines.folded.contains(&row.line) {
            let hidden = lines.index.block(row.line).map_or(0, |block| block.len());
            let note = format!("  ... {hidden} lines");
            let at = top + vec2(row.galley.size().x, 0.0);
            let dim = ui.visuals().weak_text_color();
            painter.text(at, egui::Align2::LEFT_TOP, note, look.font.clone(), dim);
        }
        painter.galley(top, Arc::clone(&row.galley), plain);
    }
    if focused && let Some(caret) = caret {
        let bar = lines.pos(caret.head).translate(origin.to_vec2());
        let ink = ui.visuals().text_cursor.stroke.color;
        if look.block_caret {
            let cell = Rect::from_min_size(bar.min, vec2(digit, bar.height()));
            painter.rect_filled(cell, 0.0, ink.gamma_multiply(0.45));
        } else {
            let bar = Rect::from_min_size(bar.min, vec2(1.5, bar.height()));
            painter.rect_filled(bar, 0.0, ink);
        }
        // Where the window of an input method is to show what is being composed.
        let to_global = ui
            .ctx()
            .layer_transform_to_global(ui.layer_id())
            .unwrap_or_default();
        ui.output_mut(|output| {
            output.ime = Some(egui::output::IMEOutput {
                rect: to_global * rect,
                cursor_rect: to_global * bar,
                should_interrupt_composition: false,
            });
        });
    }

    // The caret stays in view when the keyboard moved it, or a drag took it out of view.
    if focused
        && (caret, text.len()) != before
        && (!primary || response.dragged())
        && let Some(caret) = caret
    {
        let bar = lines.pos(caret.head).translate(origin.to_vec2());
        ui.scroll_to_rect(bar.expand2(vec2(4.0 * digit, row_height)), None);
    }

    let cursor = caret.map(Caret::range);
    state.cursor.set_char_range(cursor);
    state.store(ui.ctx(), id);
    Shown {
        response,
        origin,
        cursor,
        changed,
        lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caret_moves_by_lines_and_by_words() {
        let text = "let alpha = 1;\n\n    beta_two(é)";
        assert_eq!(line_bounds(text, 6), (0, 14));
        assert_eq!(line_bounds(text, 15), (15, 15));
        // Down a short line the column is given up, and found again further down.
        assert_eq!(vertical(text, 6, true, None), 15);
        assert_eq!(vertical(text, 6, false, None), 0);
        assert_eq!(vertical(text, 20, false, None), 15);
        assert_eq!(vertical(text, 20, true, None), 31);
        assert_eq!(word_stop(text, 0, true), 3);
        assert_eq!(word_stop(text, 3, true), 9);
        assert_eq!(word_stop(text, 9, false), 4);
        // A name with an underscore is one word.
        assert_eq!(word_at(text, 22), 20..28);
        assert_eq!(word_at(text, 15), 15..15);
    }

    #[test]
    fn a_block_folds_under_the_line_that_opens_it() {
        let text = "fn a() {\n    one();\n\n    if x {\n        two();\n    }\n}\n\nfn b() {}\n";
        let language = Language::of(std::path::Path::new("a.rs"));
        let index = Index::new(0, text, language);
        // The blank line inside is hidden with the rest, the one after the block is not.
        assert_eq!(index.block(0), Some(1..6));
        assert_eq!(index.block(3), Some(4..5));
        assert_eq!(
            (index.block(1), index.block(8), index.block(99)),
            (None, None, None)
        );
        assert_eq!(
            (index.header_of(4), index.header_of(1), index.header_of(3)),
            (Some(3), Some(0), Some(3))
        );
        assert_eq!(index.header_of(8), None);
        assert_eq!(row_starts(&index, text, &[], None), None);
        let closed = row_starts(&index, text, &[0], None).expect("rows");
        assert_eq!(closed, [0, 1, 1, 1, 1, 1, 1, 2, 3, 4, 5]);
        assert_eq!((line_on(&closed, 1), line_on(&closed, 4)), (6, 9));
        // Past the last row is still the last line.
        assert_eq!(line_on(&closed, 99), 9);
        let inner = row_starts(&index, text, &[3], None).expect("rows");
        assert_eq!((line_on(&inner, 4), inner[10]), (5, 9));
        // A line under a fold is on the row of the line that hides it.
        assert_eq!((first_row(&closed, 8), first_row(&closed, 4)), (3, 0));
        assert_eq!(index.start_of(1), 9);
    }

    #[test]
    fn hints_are_written_in_a_line_without_being_chars_of_it() {
        let text = "let n = f(1);";
        let language = Language::of(std::path::Path::new("a.rs"));
        let index = Index::new(0, text, language);
        let font = FontId::monospace(14.0);
        let mut job = index.job(text, 0, &font, true, Color32::WHITE);
        let hint = |line, column, label: &str| Hint {
            line,
            column,
            label: label.to_owned(),
        };
        let hints = [
            hint(0, 5, ": é"),
            hint(0, 10, "x: "),
            hint(0, 99, " end"),
            hint(2, 0, "other"),
        ];
        assert_eq!(hints_of(&hints, 0).len(), 3);
        assert_eq!(
            (hints_of(&hints, 1).len(), hints_of(&hints, 2).len()),
            (0, 1)
        );
        let placed = annotate(&mut job, hints_of(&hints, 0), &TextFormat::default());
        assert_eq!(job.text, "let n: é = f(x: 1); end");
        // One past the end of the line is its end.
        assert_eq!(placed, [(5, 3), (10, 3), (13, 4)]);
        // The sections still cover the line, in order, each byte once.
        let mut at = 0;
        for section in &job.sections {
            assert_eq!(section.byte_range.start.0, at);
            at = section.byte_range.end.0;
        }
        assert_eq!(at, job.text.len());
        // A caret where a hint is stays before it; a click in a hint lands there too.
        assert_eq!((pushed(&placed, 5), pushed(&placed, 6)), (5, 9));
        assert_eq!((pushed(&placed, 10), pushed(&placed, 13)), (13, 19));
        for column in 0..=13 {
            assert_eq!(pulled(&placed, pushed(&placed, column)), column);
        }
        assert_eq!((pulled(&placed, 6), pulled(&placed, 7)), (5, 5));
        assert_eq!((pulled(&placed, 8), pulled(&placed, 23)), (5, 13));
    }

    #[test]
    fn a_long_line_is_cut_into_rows_at_its_spaces() {
        assert_eq!(breaks("one two three", 8), [0, 8]);
        assert_eq!(breaks("abcdefghij", 4), [0, 4, 8]);
        assert_eq!((breaks("short", 80), breaks("", 80)), (vec![0], vec![0]));
        // A tab takes four columns.
        assert_eq!(breaks("\ta\tb", 6), [0, 1]);
        let text = "one two three\nx\n";
        let language = Language::of(std::path::Path::new("a.txt"));
        let index = Index::new(0, text, language);
        let starts = row_starts(&index, text, &[], Some(8)).expect("rows");
        assert_eq!(starts, [0, 2, 3, 4]);
        assert_eq!((line_on(&starts, 1), line_on(&starts, 2)), (0, 1));
        // Up and down go row by row, inside a line cut in two as between lines.
        assert_eq!(vertical(text, 2, true, Some(8)), 10);
        assert_eq!(vertical(text, 10, true, Some(8)), 15);
        assert_eq!(vertical(text, 10, false, Some(8)), 2);
        assert_eq!(vertical(text, 14, false, Some(8)), 8);
        // The end of a row that goes on below belongs to the next row.
        assert_eq!(vertical(text, 12, false, Some(8)), 4);
        assert_eq!(vertical(text, 7, true, Some(8)), 13);
    }

    /// What a keystroke costs in a big file. `cargo test --release -p godcode big -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore = "a measure, not a check"]
    fn a_big_file_is_measured() {
        let line = "    let total = compute(alpha, \"text\", 42); // a note about it\n";
        let language = Language::of(std::path::Path::new("a.rs"));
        for megabytes in [2, 10, 50] {
            let text = line.repeat(megabytes * 1024 * 1024 / line.len());
            let timed = |name: &str, work: &mut dyn FnMut()| {
                let start = std::time::Instant::now();
                work();
                println!("{megabytes} MB  {name}: {:?}", start.elapsed());
            };
            timed("hash", &mut || {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                text.hash(&mut hasher);
                std::hint::black_box(hasher.finish());
            });
            timed("clone", &mut || {
                std::hint::black_box(text.clone());
            });
            timed("count chars", &mut || {
                std::hint::black_box(text.chars().count());
            });
            timed("count lines", &mut || {
                std::hint::black_box(text.bytes().filter(|byte| *byte == b'\n').count());
            });
            timed("highlight", &mut || {
                std::hint::black_box(syntax::highlight(&text, language));
            });
            timed("index", &mut || {
                std::hint::black_box(Index::new(0, &text, language).lines());
            });
            let plain = Language::plain();
            timed("index, plain", &mut || {
                std::hint::black_box(Index::new(0, &text, plain).lines());
            });
        }
    }

    #[test]
    fn a_block_is_what_a_bracket_left_open_holds() {
        // Not indented at all: the brackets alone tell the blocks.
        let text = "fn a()\n{\nlet s = \"}\";\nif x { // }\ny();\n}\n}\ncall(\n1,\n2);\n";
        let language = Language::of(std::path::Path::new("a.rs"));
        let index = Index::new(0, text, language);
        // The brace in the string and the one in the comment close nothing.
        assert_eq!(index.block(1), Some(2..6));
        assert_eq!(index.block(3), Some(4..5));
        assert_eq!(index.block(7), Some(8..9));
        assert_eq!((index.block(0), index.block(2)), (None, None));
        // Prose is folded by its indentation alone.
        let prose = Index::new(0, "a (\n  b\nc)\n", Language::plain());
        assert_eq!(prose.block(0), Some(1..2));
    }

    #[test]
    fn hints_follow_what_is_typed_before_and_above_them() {
        let hint = |line, column| Hint {
            line,
            column,
            label: ": u8".to_owned(),
        };
        let places = |hints: &[Hint]| -> Vec<(usize, usize)> {
            hints.iter().map(|hint| (hint.line, hint.column)).collect()
        };
        let given = vec![hint(1, 2), hint(1, 9), hint(4, 3)];
        // A char typed at column 5 of line 1: the hint after it moves, not the one before.
        let mut hints = given.clone();
        follow(&mut hints, 1, (12, 13), 6, 0);
        assert_eq!(places(&hints), [(1, 2), (1, 10), (4, 3)]);
        // Taken back: they are where they were.
        follow(&mut hints, 1, (13, 12), 5, 0);
        assert_eq!(hints, given);
        // A line added on line 1: its hints are left out, those below go down.
        follow(&mut hints, 1, (12, 5), 0, 1);
        assert_eq!(places(&hints), [(5, 3)]);
        // Two lines removed above: it comes back up.
        follow(&mut hints, 0, (3, 3), 0, -2);
        assert_eq!(places(&hints), [(3, 3)]);
    }

    #[test]
    fn colours_worked_out_from_a_change_are_those_of_the_whole_text() {
        let language = Language::of(std::path::Path::new("a.rs"));
        let mut text = "fn a() {\n    let s = \"one\";\n}\n// end\n".to_owned();
        let mut index = Index::new(0, &text, language);
        // Typed in a line, a comment opened over what follows, then all of it replaced.
        let changes = [
            (14..14, "t"),
            (9..9, "/* "),
            (0..text.len() + 4, "let x = 1; // one\n"),
        ];
        for (old, written) in changes {
            text.replace_range(old.clone(), written);
            let change = (old.clone(), old.start..old.start + written.len());
            index = Index::after(1, &text, language, Some((&index, change)));
            assert_eq!(
                index.spans(),
                Index::new(1, &text, language).spans(),
                "{text:?}"
            );
        }
        // A change that is not the one from the text known is not believed.
        let other = Index::after(2, "fn b() {}\n", language, Some((&index, (0..1, 0..2))));
        assert_eq!(
            other.spans(),
            Index::new(2, "fn b() {}\n", language).spans()
        );
        // Nor are the colours of another language.
        let script = Language::of(std::path::Path::new("a.js"));
        let same = Index::after(3, &text, script, Some((&index, (0..0, 0..0))));
        assert_eq!(same.spans(), Index::new(3, &text, script).spans());
    }

    #[test]
    fn what_a_server_says_folds_is_believed_over_the_brackets() {
        let text = "// one
// two
fn a() {
    b(
        1,
    );
}
";
        let language = Language::of(std::path::Path::new("a.rs"));
        let mut index = Index::new(0, text, language);
        assert_eq!((index.block(0), index.block(2)), (None, Some(3..6)));
        // The comment folds, the call does not; of two on a line the longest is kept.
        index.given = Some((1, vec![(0, 1), (2, 3), (2, 5), (6, 40)]));
        assert_eq!(index.block(0), Some(1..2));
        assert_eq!(index.block(2), Some(3..6));
        assert_eq!((index.block(1), index.block(3)), (None, None));
        assert_eq!((index.header_of(1), index.header_of(4)), (Some(0), Some(2)));
        // Lines the text does not have are not hidden.
        assert_eq!(index.block(6), Some(7..8));
    }

    #[test]
    fn what_is_known_of_a_plain_text_follows_an_edit() {
        let plain = Language::plain();
        let mut text = "one\n  two é\n\nfour\n".to_owned();
        let mut index = Index::new(0, &text, plain);
        assert!(index.plain());
        // Each edit: the chars written over, and what is written.
        let edits: [(Range<usize>, &str); 8] = [
            (0..0, "X"),
            (5..5, "\n\t"),
            (2..9, ""),
            (0..3, "a\nb\nc"),
            (4..4, "é😀"),
            (7..9, "\n\n\n"),
            (0..0, ""),
            (3..3, "end"),
        ];
        for (chars, written) in edits {
            let chars = chars.start.min(index.total())..chars.end.min(index.total());
            let bytes = index.byte_of(&text, chars.start)..index.byte_of(&text, chars.end);
            assert_eq!(bytes.start, byte(&text, chars.start));
            text.replace_range(bytes.clone(), written);
            index.splice(&text, bytes.start, bytes.len(), written.len());
            let read = Index::new(0, &text, plain);
            assert_eq!(index.starts, read.starts, "{text:?}");
            assert_eq!(index.chars, read.chars, "{text:?}");
            assert_eq!(index.indents, read.indents, "{text:?}");
            assert_eq!(index.closes.len(), read.closes.len());
            assert!(index.widest >= read.widest);
            assert_eq!(index.total(), text.chars().count());
        }
        // To the very end, and everything gone.
        let end = text.len();
        text.push_str("\nlast");
        index.splice(&text, end, 0, 5);
        assert_eq!(index.starts, Index::new(0, &text, plain).starts);
        text.clear();
        index.splice(&text, 0, end + 5, 0);
        assert_eq!((index.lines(), index.total()), (1, 0));
    }

    #[test]
    fn the_history_keeps_what_changed_and_gives_it_back() {
        let at = |index: usize| CCursorRange::one(CCursor::new(index));
        let mut history = History::default();
        let mut text = "one é two".to_owned();
        history.see(&text, Some(at(0)), 0.0, Vec::new(), true);
        // Typed here: told as it was written.
        text.insert_str(3, "!!");
        history.see(
            &text,
            Some(at(5)),
            0.1,
            vec![(3, String::new(), "!!".to_owned())],
            false,
        );
        // Changed elsewhere, after a pause: found by comparing, cut on whole chars.
        text = text.replace('é', "è");
        history.see(&text, Some(at(7)), 5.0, Vec::new(), true);
        text.push_str(" three");
        history.see(&text, Some(at(18)), 5.2, Vec::new(), true);
        assert_eq!(text, "one!! è two three");
        // The two changes made together go together; then the one before.
        assert_eq!(history.undo(&mut text), Some(at(5)));
        assert_eq!(text, "one!! é two");
        assert_eq!(history.undo(&mut text), Some(at(0)));
        assert_eq!(text, "one é two");
        assert_eq!(history.undo(&mut text), None);
        assert_eq!(history.redo(&mut text), Some(at(5)));
        assert_eq!(history.redo(&mut text), Some(at(18)));
        assert_eq!(text, "one!! è two three");
        assert_eq!(history.redo(&mut text), None);
        // A new change after going back: what was taken back is gone for good.
        history.undo(&mut text);
        text.push('?');
        history.see(&text, Some(at(12)), 9.0, Vec::new(), true);
        assert_eq!(
            (history.redo(&mut text), text.as_str()),
            (None, "one!! é two?")
        );
        // A text the history was not told about is left alone.
        let mut other = "something else".to_owned();
        assert_eq!(history.undo(&mut other), None);
        assert_eq!(other, "something else");

        assert_eq!(common_prefix(b"abcd", b"abXd"), 2);
        assert_eq!(common_suffix(b"abcd", b"Xd"), 1);
        let (long, mut longer) = ("ab".repeat(5000), "ab".repeat(5000));
        longer.insert(7001, '-');
        assert_eq!(common_prefix(long.as_bytes(), longer.as_bytes()), 7001);
        assert_eq!(common_suffix(long.as_bytes(), longer.as_bytes()), 2999);
    }

    #[test]
    fn a_selection_is_written_over() {
        let mut text = "one two three".to_owned();
        let caret = Caret { head: 3, anchor: 8 };
        assert_eq!(replace(&mut text, caret, "é"), Caret::at(4));
        assert_eq!(text, "oneéthree");
    }

    #[test]
    fn lines_are_found_and_coloured_one_by_one() {
        let text = "fn a() {}\n// é note\n";
        let language = Language::of(std::path::Path::new("a.rs"));
        let index = Index::new(0, text, language);
        assert_eq!((index.lines(), index.widest), (3, 9));
        assert_eq!(
            (index.line_of(0), index.line_of(9), index.line_of(10)),
            (0, 0, 1)
        );
        assert_eq!((index.length(1), index.length(2)), (9, 0));
        // Past the end of the text is still its last line.
        assert_eq!(index.line_of(99), 2);
        let font = FontId::monospace(14.0);
        let job = index.job(text, 1, &font, true, Color32::WHITE);
        assert_eq!(job.text, "// é note");
        // The sections cover the line, no more: bytes, the accent counting two.
        assert_eq!(
            job.sections
                .first()
                .map(|section| section.byte_range.start.0),
            Some(0)
        );
        assert_eq!(
            job.sections.last().map(|section| section.byte_range.end.0),
            Some(10)
        );
        assert!(
            index
                .job(text, 2, &font, true, Color32::WHITE)
                .sections
                .is_empty()
        );
    }

    /// The line that closes each line, as reading the whole text tells it: the way it was
    /// worked out before the lines kept what their brackets do.
    fn closes_read_through(text: &str, spans: &[Span], language: &Language) -> Vec<Option<usize>> {
        let mut closes = vec![None; text.split('\n').count()];
        if matches!(language.name, "Plain Text" | "Markdown") {
            return closes;
        }
        let mut open: Vec<usize> = Vec::new();
        let (mut line, mut span) = (0, 0);
        for (at, byte) in text.bytes().enumerate() {
            let opens = matches!(byte, b'(' | b'[' | b'{');
            if byte == b'\n' {
                line += 1;
            } else if opens || matches!(byte, b')' | b']' | b'}') {
                while spans.get(span).is_some_and(|span| span.end <= at) {
                    span += 1;
                }
                let token = spans.get(span).map(|span| span.token);
                if matches!(token, Some(syntax::Token::String | syntax::Token::Comment)) {
                    continue;
                }
                if opens {
                    open.push(line);
                } else if let Some(opened) = open.pop().filter(|opened| *opened < line) {
                    closes[opened] = Some(line);
                }
            }
        }
        closes
    }

    /// Write `written` over the bytes `old` of `text` and tell what is known of it from
    /// what `index` knew: it has to be what reading the whole text tells. Returns it, and
    /// whether it was told without the text being read through.
    fn edited(
        index: &Index,
        text: &mut String,
        language: &Language,
        old: Range<usize>,
        written: &str,
    ) -> (Index, bool) {
        let was = text.clone();
        text.replace_range(old.clone(), written);
        let change = (old.clone(), old.start..old.start + written.len());
        let after = Index::after(1, text, language, Some((index, change.clone())));
        let read = Index::new(1, text, language);
        let told = format!(
            "{}: {written:?} over {old:?} of {was:?} gives {text:?}",
            language.name
        );
        assert!(after.same_as(&read), "{told}");
        assert_eq!(
            read.closes,
            closes_read_through(text, read.spans(), language),
            "{told}"
        );
        let followed = index.follow(1, text, language, change).is_some();
        (after, followed)
    }

    /// The languages the edits are tried in: code, a page, prose, and no colours at all.
    fn languages() -> Vec<&'static Language> {
        ["a.rs", "a.js", "a.py", "a.html", "a.md", "a.txt"]
            .iter()
            .map(|name| Language::of(std::path::Path::new(name)))
            .collect()
    }

    #[test]
    fn what_is_known_of_a_coloured_text_follows_any_edit() {
        // What changes the colours and the blocks of the lines below, and chars of
        // several bytes.
        let pieces = [
            "\n",
            "\n",
            "\n",
            "\r\n",
            "(",
            ")",
            "[",
            "]",
            "{",
            "}",
            "{\n",
            "}\n",
            "\"",
            "'",
            "`",
            "/*",
            "*/",
            "//",
            "#",
            "<!--",
            "-->",
            "<p>",
            "</p>",
            "```",
            "\\",
            "r#\"",
            "\"#",
            "\"\"\"",
            "\t",
            "    ",
            " ",
            "é",
            "😀",
            "fn a",
            "let x = 1;",
            "# ",
            "= ",
        ];
        for (number, language) in languages().into_iter().enumerate() {
            // The same edits each time the test runs.
            let mut seed = 0x9E37_79B9_7F4A_7C15_u64.wrapping_add(number as u64);
            let mut random = |below: usize| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed % below.max(1) as u64) as usize
            };
            let mut text = "fn a() {\n    let s = \"one\"; // (\n}\n".to_owned();
            let mut index = Index::new(0, &text, language);
            let mut followed = 0;
            for _ in 0..3000 {
                // At the start, at the end, or anywhere; on whole chars.
                let mut start = match random(12) {
                    0 => 0,
                    1 => text.len(),
                    _ => random(text.len() + 1),
                };
                while !text.is_char_boundary(start) {
                    start -= 1;
                }
                // A long text loses more than it gets, a short one is only written in.
                let most = [0, 0, 3, 6, 40][random(3) + text.len().min(400) / 200];
                let mut end = (start + random(most + 1)).min(text.len());
                while !text.is_char_boundary(end) {
                    end += 1;
                }
                let written: String = (0..random(4))
                    .map(|_| pieces[random(pieces.len())])
                    .collect();
                let (after, incremental) =
                    edited(&index, &mut text, language, start..end, &written);
                index = after;
                followed += usize::from(incremental);
            }
            // The edits were followed, not read through, wherever there are colours.
            let coloured = !std::ptr::eq(language, Language::plain());
            assert_eq!(followed > 2500, coloured, "{}: {followed}", language.name);
        }
    }

    #[test]
    fn what_is_known_of_a_coloured_text_follows_what_is_typed() {
        for language in languages() {
            let code = matches!(language.name, "Rust" | "JavaScript" | "Python");
            let mut text = "fn a() {\r\n    let s = \"é\";\r\n}\n\n// end".to_owned();
            let mut index = Index::new(0, &text, language);
            let end = text.len();
            // Typed in a line, a line end added and removed, the very start and the very
            // end written in, a line end of two bytes cut in two and taken away.
            let edits: [(Range<usize>, &str); 10] = [
                (16..16, "t"),
                (17..17, "\n"),
                (17..18, ""),
                (0..0, "x"),
                (0..0, "\n"),
                (0..2, ""),
                (end + 1..end + 1, "!"),
                (end + 2..end + 2, "\nlast\n"),
                (9..9, "\n"),
                (8..11, ""),
            ];
            for (old, written) in edits {
                let (after, followed) = edited(&index, &mut text, language, old, written);
                // A page without a tag, prose without a mark have no more colours than
                // a plain text; code always has some here.
                let colours = !index.plain() && !after.plain();
                assert_eq!(followed, colours, "{}: {text:?}", language.name);
                assert!(colours || !code, "{}: {text:?}", language.name);
                index = after;
            }
            assert_eq!(text, "fn a() {    lett s = \"é\";\r\n}\n\n// end!\nlast\n");
        }
    }

    #[test]
    fn the_blocks_follow_a_bracket_or_a_string_opened_above_them() {
        let language = Language::of(std::path::Path::new("a.rs"));
        let mut text =
            "fn a() {\n    b(\n        1,\n    );\n}\nfn c() {\n    d();\n}\n".to_owned();
        let mut index = Index::new(0, &text, language);
        // The line that closes what each of the lines asked about leaves open.
        let closes = |index: &Index, lines: &[usize]| -> Vec<Option<usize>> {
            lines.iter().map(|line| index.closes[*line]).collect()
        };
        assert_eq!(closes(&index, &[0, 1, 5]), [Some(4), Some(3), Some(7)]);
        let mut write = |old: Range<usize>, written: &str, index: &mut Index| {
            let (after, followed) = edited(index, &mut text, language, old, written);
            assert!(followed, "{text:?}");
            *index = after;
        };
        // A bracket opened on a line of its own under the first: it takes the brace that
        // closed the block, which nothing closes any more.
        write(9..9, "(\n", &mut index);
        assert_eq!(
            closes(&index, &[0, 1, 2, 6]),
            [None, Some(5), Some(4), Some(8)]
        );
        // Closed on the line it was opened on: the blocks are what they were, a line down.
        write(10..10, ")", &mut index);
        assert_eq!(
            closes(&index, &[0, 1, 2, 6]),
            [Some(5), None, Some(4), Some(8)]
        );
        // A string opened on the third line swallows the brackets down to the end.
        write(12..12, "let s = \"", &mut index);
        assert!(index.closes.iter().all(Option::is_none));
        // Closed: the brackets below count again.
        write(21..21, "\";", &mut index);
        assert_eq!(closes(&index, &[0, 2, 6]), [Some(5), Some(4), Some(8)]);
        // The brace that closes the first block taken away: the next block keeps its own.
        write(48..49, "", &mut index);
        assert_eq!(closes(&index, &[0, 2, 6]), [None, Some(4), Some(8)]);
        // The line end before it taken away too: the lines below come up by one.
        write(47..48, "", &mut index);
        assert_eq!(closes(&index, &[0, 2, 5]), [None, Some(4), Some(7)]);
        // A comment opened under the first line and never closed: nothing below counts.
        write(9..9, "/* ", &mut index);
        assert!(index.closes.iter().all(Option::is_none));
    }

    #[test]
    fn the_longest_line_is_found_again_once_it_is_shortened() {
        let language = Language::of(std::path::Path::new("a.rs"));
        let mut text = "let a = 1;\nlet longest = \"é\";\nlet b = 22;\n".to_owned();
        let mut index = Index::new(0, &text, language);
        assert_eq!(index.widest, 18);
        // Each edit, and the chars of the longest line after it: shortened, it is still
        // the longest; shortened again, another one is; removed with its line end; then
        // a line grows past them all.
        let edits: [(Range<usize>, &str, usize); 4] = [
            (15..22, "", 11),
            (11..16, "", 11),
            (0..11, "", 11),
            (0..0, "let widest = 333;", 23),
        ];
        for (old, written, widest) in edits {
            let (after, followed) = edited(&index, &mut text, language, old, written);
            assert!(followed, "{text:?}");
            assert_eq!(after.widest, widest, "{text:?}");
            index = after;
        }
    }

    #[test]
    fn a_text_is_read_through_when_what_was_known_cannot_tell_it() {
        let language = Language::of(std::path::Path::new("a.rs"));
        // An empty text has no colours to start from, nor does one emptied.
        let mut text = String::new();
        let index = Index::new(0, &text, language);
        let (index, followed) = edited(&index, &mut text, language, 0..0, "fn a() {\n}\n");
        assert!(!followed);
        let (index, followed) = edited(&index, &mut text, language, 9..9, "    b();\n");
        assert!(followed);
        let (emptied, followed) = edited(&index, &mut text, language, 0..20, "");
        assert!(!followed && emptied.lines() == 1);
        // A change that is not one from the text known: the sizes do not add up, it
        // starts elsewhere, it ends past the text.
        let text = "fn b() {}\n";
        let read = Index::new(2, text, language);
        for change in [(0..1, 0..2), (0..1, 1..2), (0..25, 0..5)] {
            assert!(index.follow(2, text, language, change.clone()).is_none());
            let after = Index::after(2, text, language, Some((&index, change)));
            assert!(after.same_as(&read));
        }
        // Nor are the lines of another language believed, or those of a text that was
        // followed as a plain one.
        let script = Language::of(std::path::Path::new("a.js"));
        let known = Index::new(3, text, language);
        assert!(known.follow(3, text, script, (0..0, 0..0)).is_none());
        assert!(known.follow(3, text, language, (0..0, 0..0)).is_some());
        let plain = Index::new(3, text, Language::plain());
        assert!(
            plain
                .follow(3, text, Language::plain(), (0..0, 0..0))
                .is_none()
        );
    }

    /// What a keystroke costs in a big coloured file, read through and followed.
    /// `cargo test -p godcode --release -- --ignored keystroke --nocapture`.
    #[test]
    #[ignore = "a measure, not a check"]
    fn a_keystroke_in_a_big_coloured_file_is_measured() {
        let language = Language::of(std::path::Path::new("a.rs"));
        let block = "fn name(alpha: u32) -> u32 {\n    let total = compute(alpha, \"text\", 42); // a note about it\n    if total > 3 {\n        return total;\n    }\n    total\n}\n\n";
        let text = block.repeat(25_000);
        let index = Index::new(0, &text, language);
        println!("{} lines, {} bytes", index.lines(), text.len());
        // The best of a few runs.
        let timed = |name: &str, work: &mut dyn FnMut()| {
            let best = (0..7).map(|_| {
                let start = std::time::Instant::now();
                work();
                start.elapsed()
            });
            println!("{name}: {:?}", best.min().unwrap_or_default());
        };
        timed("read through", &mut || {
            std::hint::black_box(Index::new(0, &text, language).lines());
        });
        timed("the colours alone copied", &mut || {
            std::hint::black_box(index.coloured.clone());
        });
        timed("the blocks told again by the brackets", &mut || {
            std::hint::black_box(closers(&index.brackets));
        });
        let middle = index.starts[index.lines() / 2 + 1] + 8;
        for (place, at) in [("the middle", middle), ("the start", 0)] {
            for written in ["x", "\n", "{"] {
                let mut typed = text.clone();
                typed.insert_str(at, written);
                let change = (at..at, at..at + written.len());
                let mut after = None;
                timed(
                    &format!("{written:?} typed at {place}, followed"),
                    &mut || {
                        let before = Some((&index, change.clone()));
                        after = Some(Index::after(1, &typed, language, before));
                    },
                );
                assert!(index.follow(1, &typed, language, change).is_some());
                let read = Index::new(1, &typed, language);
                assert!(after.is_some_and(|after| after.same_as(&read)));
            }
        }
    }
}
