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
pub struct Index {
    pub key: u64,
    /// The byte each line starts at, then one past the end of the text: line `n` is the
    /// bytes `starts[n]..starts[n + 1] - 1`.
    pub starts: Vec<usize>,
    /// The char each line starts at, then one past the end of the text.
    chars: Vec<usize>,
    spans: Vec<Span>,
    /// Chars of the longest line.
    widest: usize,
    /// Columns of white space leading each line; `None` for a blank line.
    pub indents: Vec<Option<usize>>,
}

impl Index {
    fn new(key: u64, text: &str, language: &Language) -> Self {
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
        Self {
            key,
            starts,
            chars,
            spans: syntax::highlight(text, language),
            widest,
            indents: crate::editor::indents_of(text),
        }
    }

    /// The char `line` starts at.
    pub fn start_of(&self, line: usize) -> usize {
        self.chars[line.min(self.lines() - 1)]
    }

    /// The lines hidden when the block that `header` opens is folded: those under it that
    /// are indented deeper, the blank ones between them too.
    pub fn block(&self, header: usize) -> Option<Range<usize>> {
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
    fn line_of(&self, index: usize) -> usize {
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
        let first = self.spans.partition_point(|span| span.end <= from);
        for span in &self.spans[first..] {
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
}

impl Cache {
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
    /// The caret covers its char instead of standing before it: keys are commands.
    pub block_caret: bool,
    /// Lines too long for the view go on below instead of running off it.
    pub wrap: bool,
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

fn key_of(text: &str, look: &Look<'_>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    theme::current().name.hash(&mut hasher);
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
fn replace(text: &mut String, caret: Caret, written: &str) -> Caret {
    let chars = caret.sorted();
    let bytes = byte(text, chars.start)..byte(text, chars.end);
    text.replace_range(bytes, written);
    Caret::at(chars.start + written.chars().count())
}

/// What the keyboard did to the text and the caret this frame. `page`: rows in view;
/// `wrap`: the columns a row holds when lines are cut to fit.
fn typed(ui: &Ui, text: &mut String, mut caret: Caret, page: usize, wrap: Option<usize>) -> Caret {
    let events = ui.input(|input| input.events.clone());
    for event in events {
        let len = text.chars().count();
        match event {
            Event::Text(written) | Event::Paste(written) => {
                let written = written.replace("\r\n", "\n");
                caret = replace(text, caret, &written);
            }
            Event::Ime(egui::ImeEvent::Commit(written)) => {
                caret = replace(text, caret, &written);
            }
            // What is being composed shows selected: what comes next takes its place.
            Event::Ime(egui::ImeEvent::Preedit { text: composed, .. }) => {
                let start = caret.sorted().start;
                let head = replace(text, caret, &composed).head;
                caret = Caret {
                    head,
                    anchor: start,
                };
            }
            Event::Copy | Event::Cut => {
                let chars = caret.sorted();
                if chars.is_empty() {
                    continue;
                }
                let bytes = byte(text, chars.start)..byte(text, chars.end);
                ui.ctx().copy_text(text[bytes].to_owned());
                if event == Event::Cut {
                    caret = replace(text, caret, "");
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
                        caret = replace(text, caret, "");
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    caret
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
    let len = text.chars().count();
    let stored = state.cursor.char_range();
    let mut caret = stored.map(|range| Caret::of(range, len));
    let focused = ui.memory(|memory| memory.has_focus(id));
    // Given the keyboard before any caret was put: it starts at the end, as in egui's field.
    if focused && caret.is_none() {
        caret = Some(Caret::at(len));
    }
    let before = (caret, text.len());
    let mut changed = false;
    if focused && let Some(old) = caret {
        // The arrows and Tab are the field's, not a way to another widget.
        let filter = EventFilter {
            tab: true,
            horizontal_arrows: true,
            vertical_arrows: true,
            escape: false,
        };
        ui.memory_mut(|memory| memory.set_focus_lock_filter(id, filter));
        let was = text.clone();
        let (undo, redo) = ui.input_mut(|input| {
            let shifted = Modifiers::COMMAND | Modifiers::SHIFT;
            let redo =
                input.consume_key(shifted, Key::Z) || input.consume_key(Modifiers::COMMAND, Key::Y);
            (input.consume_key(Modifiers::COMMAND, Key::Z), redo)
        });
        let mut new = typed(ui, text, old, page, wrap);
        if undo || redo {
            let mut undoer = state.undoer();
            let current = (new.range(), text.clone());
            let step = if undo {
                undoer.undo(&current)
            } else {
                undoer.redo(&current)
            };
            if let Some((range, restored)) = step.cloned() {
                *text = restored;
                new = Caret::of(range, text.chars().count());
            }
            state.set_undoer(undoer);
        }
        changed = *text != was;
        caret = Some(new);
    }

    // What is known of the text, made again when it changed.
    let key = key_of(text, look);
    let previous = cache.index.clone();
    let index = match cache.index.as_ref().filter(|index| index.key == key) {
        Some(index) => Arc::clone(index),
        None => {
            let index = Arc::new(Index::new(key, text, look.language));
            cache.index = Some(Arc::clone(&index));
            index
        }
    };

    // The folds follow the text: lines typed above them push them down; a text changed
    // from elsewhere gives no way to tell where they went.
    if let Some(old) = previous.filter(|old| old.key != index.key) {
        let grown = index.lines() as isize - old.lines() as isize;
        match before.0 {
            Some(caret) if changed => {
                let edited = old.line_of(caret.head);
                let below = folded.iter_mut().filter(|header| **header > edited);
                below.for_each(|header| *header = header.saturating_add_signed(grown));
            }
            _ if grown != 0 => folded.clear(),
            _ => {}
        }
    }
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
        (index.key, &*folded, wrap).hash(&mut hasher);
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
    let by_line = look.hints.chunk_by(|a, b| a.line == b.line);
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
        let hints = hints_of(look.hints, line).iter().filter_map(|hint| {
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
    if focused && let Some(range) = cursor {
        let mut undoer = state.undoer();
        let time = ui.input(|input| input.time);
        undoer.feed_state(time, &(range, text.clone()));
        state.set_undoer(undoer);
    }
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
}
