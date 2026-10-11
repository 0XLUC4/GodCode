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
    fn job(&self, text: &str, line: usize, font: &FontId, dark: bool, plain: Color32) -> LayoutJob {
        let (from, to) = (self.starts[line], self.starts[line + 1] - 1);
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
    job.sections.sort_by_key(|section| section.byte_range.start.0);
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

/// A line in view, laid out.
struct Row {
    line: usize,
    galley: Arc<Galley>,
    /// The hints written in it: the char each one is before, and the chars it takes.
    hints: Vec<(usize, usize)>,
}

/// Where the chars of the text are, measured for the lines in view only.
pub struct Lines {
    index: Arc<Index>,
    /// The lines that show, in order, when blocks are folded.
    rows: Option<Vec<usize>>,
    /// The lines whose block is folded.
    folded: Vec<usize>,
    row_height: f32,
    /// Width of a digit: a column, for the lines that are not measured.
    digit: f32,
    shown: Vec<Row>,
}

impl Lines {
    fn row(&self, line: usize) -> Option<&Row> {
        self.shown.iter().find(|row| row.line == line)
    }

    /// Where the char `index` is, from the top left of the text: a rectangle without
    /// width, as tall as its row.
    pub fn pos(&self, index: usize) -> Rect {
        let line = self.index.line_of(index);
        let column = (index - self.index.chars[line]).min(self.index.length(line));
        let x = match self.row(line) {
            Some(row) => {
                let shown = CCursor::new(pushed(&row.hints, column));
                row.galley.pos_from_cursor(shown).min.x
            }
            None => column as f32 * self.digit,
        };
        let top = self.row_of(line) as f32 * self.row_height;
        Rect::from_min_size(pos2(x, top), vec2(0.0, self.row_height))
    }

    /// The row `line` shows on: for a line folded away, the row of the line it is under.
    fn row_of(&self, line: usize) -> usize {
        match &self.rows {
            None => line,
            Some(rows) => rows
                .partition_point(|shown| *shown <= line)
                .saturating_sub(1),
        }
    }

    /// The line that shows on `row`.
    fn line_on(&self, row: usize) -> usize {
        match &self.rows {
            None => row.min(self.index.lines() - 1),
            Some(rows) => rows[row.min(rows.len() - 1)],
        }
    }

    /// The height `line` starts at, or `None` when it is folded away.
    pub fn top(&self, line: usize) -> Option<f32> {
        let row = self.row_of(line);
        (self.line_on(row) == line).then_some(row as f32 * self.row_height)
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
        let column = match self.row(line) {
            Some(row) => {
                let shown = row.galley.cursor_from_pos(vec2(within.x, 0.0)).index.0;
                pulled(&row.hints, shown)
            }
            None => (within.x / self.digit).round().max(0.0) as usize,
        };
        self.index.chars[line] + column.min(self.index.length(line))
    }

    /// The line at the height `y`, counted from the top of the text.
    pub fn line_at(&self, y: f32) -> usize {
        self.line_on((y / self.row_height).floor().max(0.0) as usize)
    }

    /// The lines between two heights, each with the height it starts at.
    pub fn between(&self, top: f32, bottom: f32) -> Vec<(usize, f32)> {
        let rows = self.rows.as_ref().map_or(self.index.lines(), Vec::len);
        let row = |y: f32| ((y / self.row_height).floor().max(0.0) as usize).min(rows - 1);
        (row(top)..=row(bottom))
            .map(|row| (self.line_on(row), row as f32 * self.row_height))
            .collect()
    }

    pub fn index(&self) -> &Arc<Index> {
        &self.index
    }
}

/// The lines that show, in order, once the blocks of `folded` are closed; `None` when
/// nothing is folded.
fn rows(index: &Index, folded: &[usize]) -> Option<Vec<usize>> {
    if folded.is_empty() {
        return None;
    }
    let mut hidden = vec![false; index.lines()];
    for block in folded.iter().filter_map(|header| index.block(*header)) {
        hidden[block].fill(true);
    }
    Some((0..index.lines()).filter(|line| !hidden[*line]).collect())
}

/// The row `line` shows on when the blocks of `folded` are closed, keeping its fraction.
pub fn row_of(index: &Index, folded: &[usize], line: f32) -> f32 {
    match rows(index, folded) {
        None => line,
        Some(rows) => {
            let above = rows.partition_point(|shown| *shown <= line as usize);
            above.saturating_sub(1) as f32 + line.fract()
        }
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

/// One line up or down, keeping the column when the line is long enough.
fn vertical(text: &str, index: usize, down: bool) -> usize {
    let (start, end) = line_bounds(text, index);
    let column = index - start;
    if down {
        if byte(text, end) == text.len() {
            return end;
        }
        let (next, next_end) = line_bounds(text, end + 1);
        (next + column).min(next_end)
    } else {
        if start == 0 {
            return 0;
        }
        let (previous, previous_end) = line_bounds(text, start - 1);
        (previous + column).min(previous_end)
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

/// What the keyboard did to the text and the caret this frame. `page`: rows in view.
fn typed(ui: &Ui, text: &mut String, mut caret: Caret, page: usize) -> Caret {
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
                    Key::ArrowUp => Some(vertical(text, caret.head, false)),
                    Key::ArrowDown => Some(vertical(text, caret.head, true)),
                    Key::PageUp | Key::PageDown => {
                        let down = key == Key::PageDown;
                        let step = |at, _| vertical(text, at, down);
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
    cache: &mut Option<Arc<Index>>,
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
        let mut new = typed(ui, text, old, page);
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
    let previous = cache.clone();
    let index = match cache.as_ref().filter(|index| index.key == key) {
        Some(index) => Arc::clone(index),
        None => {
            let index = Arc::new(Index::new(key, text, look.language));
            *cache = Some(Arc::clone(&index));
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
    let visible = rows(&index, folded);

    let margin = look.margin;
    // The hints make their lines longer: by no more than the longest of them all.
    let by_line = look.hints.chunk_by(|a, b| a.line == b.line);
    let chars = |line: &[Hint]| line.iter().map(|hint| hint.label.chars().count()).sum();
    let hinted: usize = by_line.map(chars).max().unwrap_or(0);
    let columns = (index.widest + hinted) as f32;
    let width = columns * digit + f32::from(margin.left + margin.right) + digit;
    let rows = visible.as_ref().map_or(index.lines(), Vec::len);
    let height = rows as f32 * row_height + f32::from(margin.top + margin.bottom);
    let size = vec2(width, height).max(look.min_size);
    let (_, rect) = ui.allocate_space(size);
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let origin = rect.min + vec2(f32::from(margin.left), f32::from(margin.top));

    // Only the lines in view are laid out.
    let plain = ui.visuals().text_color();
    let first = (((clip.min.y - origin.y) / row_height).floor().max(0.0) as usize).min(rows - 1);
    let last = (((clip.max.y - origin.y) / row_height).ceil().max(0.0) as usize).min(rows - 1);
    let mut lines = Lines {
        index,
        rows: visible,
        folded: folded.clone(),
        row_height,
        digit,
        shown: Vec::new(),
    };
    let hint_format = TextFormat {
        font_id: FontId::new(look.font.size * 0.9, look.font.family.clone()),
        color: ui.visuals().weak_text_color(),
        background: ui.visuals().faint_bg_color,
        valign: egui::Align::Center,
        ..Default::default()
    };
    lines.shown = (first..=last)
        .map(|row| {
            let line = lines.line_on(row);
            let mut job = lines.index.job(text, line, look.font, look.dark, plain);
            let hints = annotate(&mut job, hints_of(look.hints, line), &hint_format);
            Row {
                line,
                galley: ui.fonts_mut(|fonts| fonts.layout_job(job)),
                hints,
            }
        })
        .collect();

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
    for Row { line, galley, .. } in &lines.shown {
        let top = origin + vec2(0.0, lines.row_of(*line) as f32 * row_height);
        if let Some(chars) = &selected {
            let (start, end) = (lines.index.chars[*line], lines.index.chars[*line + 1] - 1);
            if chars.start <= end && chars.end >= start {
                let from = lines.pos(chars.start.max(start)).min.x;
                // A selection that goes on past the line takes its line end too.
                let past = if chars.end > end { digit * 0.5 } else { 0.0 };
                let to = lines.pos(chars.end.min(end)).min.x + past;
                let band = Rect::from_min_max(pos2(from, 0.0), pos2(to, row_height));
                painter.rect_filled(band.translate(top.to_vec2()), 0.0, selection);
            }
        }
        // What a folded block hides is told after the line that opens it.
        if lines.folded.contains(line) {
            let hidden = lines.index.block(*line).map_or(0, |block| block.len());
            let note = format!("  ... {hidden} lines");
            let at = top + vec2(galley.size().x, 0.0);
            let dim = ui.visuals().weak_text_color();
            painter.text(at, egui::Align2::LEFT_TOP, note, look.font.clone(), dim);
        }
        painter.galley(top, Arc::clone(galley), plain);
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
    }

    // The caret stays in view when the keyboard moved it.
    if focused
        && (caret, text.len()) != before
        && !primary
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
        assert_eq!(vertical(text, 6, true), 15);
        assert_eq!(vertical(text, 6, false), 0);
        assert_eq!(vertical(text, 20, false), 15);
        assert_eq!(vertical(text, 20, true), 31);
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
        assert_eq!(rows(&index, &[]), None);
        assert_eq!(rows(&index, &[0]), Some(vec![0, 6, 7, 8, 9]));
        assert_eq!(rows(&index, &[3]), Some(vec![0, 1, 2, 3, 5, 6, 7, 8, 9]));
        // A line under a fold is on the row of the line that hides it.
        assert_eq!(
            (row_of(&index, &[0], 8.5), row_of(&index, &[0], 4.0)),
            (3.5, 0.0)
        );
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
        assert_eq!((hints_of(&hints, 1).len(), hints_of(&hints, 2).len()), (0, 1));
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
