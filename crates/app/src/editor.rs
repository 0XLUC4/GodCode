//! The code editor: a text field with syntax colours, line numbers, completion, problem
//! marks and code-aware keys.

use std::ops::Range;

use egui::text::{CCursor, CCursorRange, LayoutJob, LayoutSection, TextFormat, TextWrapping};
use egui::{
    Align, Align2, Area, Color32, Context, CornerRadius, Event, FontId, Frame, Id, Key, Margin,
    Modifiers, Order, Pos2, Rect, ScrollArea, Sense, Shape, Stroke, StrokeKind, TextEdit, Ui,
    UiBuilder, pos2, vec2,
};
use godcode_core::complete::{self, Completion, Kind};
use godcode_core::git::{Hunk, LineChange};
use godcode_core::search::Match;
use godcode_core::syntax::{self, Language};
use godcode_core::{check, edit};

use crate::field;
use crate::theme::{self, Palette};

const MARGIN: Margin = Margin {
    left: 6,
    right: 24,
    top: 4,
    bottom: 4,
};
/// Over this size a text is too big to be read through at every key: it gets no colours,
/// no completion from its own words, no bracket matching.
pub const LARGE_BYTES: usize = 2 * 1024 * 1024;
/// Space around the digits of the line numbers.
const GUTTER_PAD: f32 = 14.0;
/// Completions shown at once; the list slides to keep the chosen one in view.
const POPUP_ROWS: usize = 8;
const POPUP_ROW: egui::Vec2 = vec2(300.0, 22.0);
pub const ERROR_COLOR: Color32 = Color32::from_rgb(0xF1, 0x4C, 0x4C);
pub const WARNING_COLOR: Color32 = Color32::from_rgb(0xCC, 0xA7, 0x00);
/// Columns a tab takes when lines are measured for the indent guides.
const TAB_COLUMNS: usize = 4;
const MINIMAP_WIDTH: f32 = 84.0;
/// Height of a line and width of a column in the minimap, at most.
const MINIMAP_ROW: f32 = 3.0;
const MINIMAP_COLUMN: f32 = 0.75;
/// Lines kept in view by sticky scroll, and how far above the view they are looked for.
const STICKY_LINES: usize = 4;
const STICKY_REACH: usize = 3000;
const ADDED_COLOR: Color32 = Color32::from_rgb(0x58, 0x8C, 0x2C);
/// Behind the line the debugged program is stopped at.
const STOPPED_COLOR: Color32 = Color32::from_rgba_premultiplied(0x4A, 0x3C, 0x08, 0x70);
const MODIFIED_COLOR: Color32 = Color32::from_rgb(0x1B, 0x81, 0xA8);

/// A problem underlined in the text.
pub struct Squiggle {
    /// Char index where the underline starts, and how many chars it covers.
    pub start: usize,
    pub len: usize,
    /// Line counted from 0.
    pub line: usize,
    pub error: bool,
    pub message: String,
}

/// What the editor needs to draw one file.
pub struct Config<'a> {
    pub language: &'static Language,
    pub font_size: f32,
    /// The text one indent level adds.
    pub indent: &'a str,
    /// Occurrences of the searched text, in order.
    pub marks: &'a [Match],
    /// The occurrence the search is on.
    pub current_mark: Option<usize>,
    pub squiggles: &'a [Squiggle],
    /// Lines changed since the last commit.
    pub changes: &'a [Hunk],
    /// The other places of the word at the caret.
    pub occurrences: &'a [Match],
    /// Long lines go on below instead of running off the view.
    pub wrap: bool,
    /// Show the whole file as a strip on the right.
    pub minimap: bool,
    /// Keep the lines that open the blocks in view above the code.
    pub sticky: bool,
    /// A note written after the line of the caret: who last changed it.
    pub note: Option<&'a str>,
    /// The snippets of the user for this language: what is typed, what it becomes.
    pub snippets: &'a [(String, String)],
    /// The lines (from 0) the debugger stops on.
    pub breakpoints: &'a [usize],
    /// Those of them it stops on only under a condition.
    pub conditional: &'a [usize],
    /// The line the debugged program is stopped at.
    pub stopped: Option<usize>,
    /// A number that changes whenever the text may have changed since the editor last
    /// showed it.
    pub revision: u64,
    /// What the language server writes in the lines.
    pub hints: &'a [field::Hint],
    /// The caret covers its char: keys are commands (Vim).
    pub block_caret: bool,
}

/// Layout key and caret a bracket pair was looked up for, and the pair found.
type PairAt = (u64, usize, Option<(usize, usize)>);

/// Per-file view state kept between frames.
#[derive(Default)]
pub struct View {
    /// Bring this char into view on the next frame.
    pub scroll_to: Option<usize>,
    /// Give the keyboard to the editor on the next frame.
    pub focus: bool,
    /// Offer completions on the next frame, whatever was typed.
    pub complete_now: bool,
    /// Put this line (a fraction is a part of it) at the top of the view on the next frame.
    pub scroll_to_line: Option<f32>,
    /// The other carets: what is typed at the caret is typed at each of them too.
    pub extra: Vec<usize>,
    /// The lines whose block is folded.
    pub folded: Vec<usize>,
    /// Fold or unfold the block the caret is in, on the next frame.
    pub fold_at_caret: bool,
    /// The completion just picked, for the app to take: it may come with an import.
    pub accepted: Option<Completion>,
    /// A rectangle of text being selected: the same columns on every line it crosses.
    pub block: Option<Block>,
    /// Move the far corner of that rectangle by lines and columns, on the next frame.
    pub block_step: Option<(isize, isize)>,
    /// Scroll no more than it takes to see the caret, on the next frame.
    pub reveal: bool,
    /// Something was typed in markup: the tag that goes with the one edited, if one was,
    /// is to take its name on the next frame.
    retag: bool,
    layout: Option<Layout>,
    /// What the text field worked out of the text.
    index: field::Cache,
    completion: Option<Popup>,
    /// The bracket pair at a caret, for the text of a layout key.
    pair: Option<PairAt>,
}

impl View {
    /// The completion list is open.
    pub fn completing(&self) -> bool {
        self.completion.is_some()
    }

    /// Show `items` as the completions at `caret`: what a language server answered.
    pub fn offer(&mut self, caret: usize, items: Vec<Completion>) {
        if !items.is_empty() {
            self.completion = Some(Popup {
                items,
                selected: 0,
                caret,
                navigated: false,
            });
        }
    }

    /// A view that takes the keyboard as soon as it shows.
    pub fn focused() -> Self {
        Self {
            focus: true,
            ..Self::default()
        }
    }
}

/// What the guides, the pinned lines and the strip read of the text: what the field worked
/// out of it, held from the end of a frame to the start of the next.
type Layout = std::sync::Arc<field::Index>;

pub(crate) fn indents_of(text: &str) -> Vec<Option<usize>> {
    text.split('\n')
        .map(|line| {
            let lead = line.len() - line.trim_start_matches([' ', '\t']).len();
            let columns = line[..lead]
                .bytes()
                .map(|byte| if byte == b'\t' { TAB_COLUMNS } else { 1 })
                .sum();
            (lead < line.len()).then_some(columns)
        })
        .collect()
}

/// The completion list open at the caret.
struct Popup {
    items: Vec<Completion>,
    selected: usize,
    /// Where the caret was when the list was made; it closes when the caret leaves.
    caret: usize,
    /// The arrows were used: Enter then picks instead of breaking the line.
    navigated: bool,
}

pub struct Output {
    pub changed: bool,
    /// Sorted char range of the selection; empty for a plain caret.
    pub selection: Option<Range<usize>>,
    /// The char clicked with Ctrl held: go to what is named there.
    pub jump: Option<usize>,
    /// The line at the top of the view, with the fraction of it scrolled away; infinite
    /// once the end of the text shows.
    pub top_line: f32,
    /// The char under the pointer when it is over the text, and where the pointer is.
    pub pointed: Option<(usize, Pos2)>,
    /// Where the caret is on screen: the foot of it.
    pub caret_pos: Option<Pos2>,
    /// The editor has the keyboard.
    pub focused: bool,
    /// A line whose number was clicked: its breakpoint is to be toggled.
    pub gutter: Option<usize>,
    /// Line and column, from 1, of the start and of the end of the selection.
    pub places: Option<((usize, usize), (usize, usize))>,
    /// How many lines the text has.
    pub lines: usize,
}

/// A code-aware edit at the selection.
#[derive(Clone, Copy)]
pub enum Op {
    Newline,
    Indent,
    Outdent,
    Comment,
    Duplicate,
    DeleteLine,
    MoveUp,
    MoveDown,
    Join,
    ToggleCase,
}

fn sorted(range: CCursorRange) -> Range<usize> {
    let (a, b) = (range.primary.index.0, range.secondary.index.0);
    a.min(b)..a.max(b)
}

fn selection_of(ctx: &Context, id: Id) -> Option<Range<usize>> {
    TextEdit::load_state(ctx, id)
        .and_then(|state| state.cursor.char_range())
        .map(sorted)
}

/// Put the caret / selection of the editor `id` at `range`.
pub fn set_selection(ctx: &Context, id: Id, range: &Range<usize>) {
    let mut state = TextEdit::load_state(ctx, id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange::two(
        CCursor::new(range.start),
        CCursor::new(range.end),
    )));
    state.store(ctx, id);
}

/// Apply `op` at the selection of the editor `id`. Returns whether the text changed.
pub fn run(ctx: &Context, id: Id, text: &mut String, config: &Config<'_>, op: Op) -> bool {
    let Some(selection) = selection_of(ctx, id) else {
        return false;
    };
    let before = text.clone();
    let selection = match op {
        Op::Newline => {
            let caret = edit::newline(text, selection, config.indent, config.language);
            caret..caret
        }
        Op::Indent => edit::indent(text, selection, config.indent),
        Op::Outdent => edit::outdent(text, selection, config.indent),
        Op::Comment => match config.language.line_comment() {
            Some(prefix) => edit::toggle_comment(text, selection, prefix),
            None => return false,
        },
        Op::Duplicate => edit::duplicate_lines(text, selection),
        Op::DeleteLine => edit::delete_lines(text, selection),
        Op::Join => edit::join_lines(text, selection),
        Op::ToggleCase => edit::toggle_case(text, selection),
        Op::MoveUp | Op::MoveDown => {
            match edit::move_lines(text, selection, matches!(op, Op::MoveUp)) {
                Some(selection) => selection,
                None => return false,
            }
        }
    };
    set_selection(ctx, id, &selection);
    *text != before
}

/// Step the history of the editor `id`, whose view is `view`; `None` when there is nothing
/// to undo / redo.
pub fn history(
    ctx: &Context,
    id: Id,
    text: &mut String,
    view: &mut View,
    undo: bool,
) -> Option<Range<usize>> {
    let mut state = TextEdit::load_state(ctx, id)?;
    let time = ctx.input(|input| input.time);
    let history = view.index.history();
    // What changed since the editor last showed is a change like the others.
    history.see(text, state.cursor.char_range(), time, Vec::new(), true);
    let range = if undo {
        history.undo(text)
    } else {
        history.redo(text)
    }?;
    state.cursor.set_char_range(Some(range));
    state.store(ctx, id);
    Some(sorted(range))
}

fn is_prose(language: &Language) -> bool {
    matches!(language.name, "Plain Text" | "Markdown")
}

/// Typing a bracket or a quote adds its partner; typing the partner steps over it.
fn pair(ui: &Ui, id: Id, text: &mut String, config: &Config<'_>) -> bool {
    if is_prose(config.language) {
        return false;
    }
    let Some(caret) = selection_of(ui.ctx(), id)
        .filter(Range::is_empty)
        .map(|selection| selection.start)
    else {
        return false;
    };
    let typed = ui.input(|input| {
        input.events.iter().find_map(|event| match event {
            Event::Text(typed) if typed.chars().count() == 1 => typed.chars().next(),
            _ => None,
        })
    });
    let Some(typed) = typed else {
        return false;
    };
    let take = |ui: &Ui, typed: char| {
        let taken = typed.to_string();
        ui.input_mut(|input| {
            input
                .events
                .retain(|event| !matches!(event, Event::Text(text) if *text == taken));
        });
    };
    if typed == '>' && config.language.is_markup() {
        let at = godcode_core::byte_index(text, caret);
        let Some(closing) = complete::closing_tag(&text[..at], &text[at..]) else {
            return false;
        };
        take(ui, typed);
        text.insert_str(at, &format!(">{closing}"));
        set_selection(ui.ctx(), id, &(caret + 1..caret + 1));
        return true;
    }
    let blank = |lead: &str| lead.chars().all(|ch| ch == ' ' || ch == '\t');
    let indent_at = |text: &str, byte: usize| {
        let line = text[..byte].rfind('\n').map_or(0, |found| found + 1);
        let lead = text[line..]
            .chars()
            .take_while(|ch| *ch == ' ' || *ch == '\t');
        lead.collect::<String>()
    };
    // `</` closes the element still open: its name is written, and a closing tag that
    // starts its line goes under the tag it closes.
    if typed == '/' && config.language.is_markup() {
        let at = godcode_core::byte_index(text, caret);
        let Some(before) = text[..at].strip_suffix('<') else {
            return false;
        };
        let Some((name, opened)) = complete::unclosed_tag(before) else {
            return false;
        };
        take(ui, typed);
        let line = before.rfind('\n').map_or(0, |found| found + 1);
        let (mut at, mut caret) = (at, caret);
        if blank(&before[line..]) {
            let indent = indent_at(text, opened);
            let lead = line..before.len();
            caret = caret - text[lead.clone()].chars().count() + indent.chars().count();
            at = line + indent.len() + 1;
            text.replace_range(lead, &indent);
        }
        text.insert_str(at, &format!("/{name}>"));
        caret += name.chars().count() + 2;
        set_selection(ui.ctx(), id, &(caret..caret));
        return true;
    }
    let mut around = text.chars().skip(caret.saturating_sub(1));
    let before = if caret == 0 { None } else { around.next() };
    let after = around.next();
    let closer = match typed {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '"' => '"',
        _ => '\0',
    };
    let steps_over = matches!(typed, ')' | ']' | '}' | '"') && after == Some(typed);
    let opens = closer != '\0'
        && after.is_none_or(|ch| ch.is_whitespace() || ")]},;".contains(ch))
        // A quote after a word or a backslash closes or escapes, it does not open.
        && (typed != '"' || !before.is_some_and(|ch| ch.is_alphanumeric() || "\"\\".contains(ch)));
    // A word that ends a block (`end`, `fi`), typed first on its line, goes back one level.
    let closers = config.language.block_closers();
    if typed.is_alphabetic() && !closers.is_empty() {
        let at = godcode_core::byte_index(text, caret);
        let line = text[..at].rfind('\n').map_or(0, |found| found + 1);
        let written = text[line..at].trim_start();
        let lead = at - written.len();
        let word = format!("{written}{typed}");
        let goes_on = after.is_some_and(|ch| ch.is_alphanumeric() || ch == '_');
        let unit = config.indent;
        if closers.contains(&word.as_str()) && !goes_on && text[line..lead].ends_with(unit) {
            take(ui, typed);
            text.insert(at, typed);
            text.replace_range(lead - unit.len()..lead, "");
            let after = caret + 1 - unit.chars().count();
            set_selection(ui.ctx(), id, &(after..after));
            return true;
        }
    }
    // A closing bracket typed first on its line goes under the line of the one it closes.
    if matches!(typed, ')' | ']' | '}') && !steps_over {
        let at = godcode_core::byte_index(text, caret);
        let line = text[..at].rfind('\n').map_or(0, |found| found + 1);
        if at > line && blank(&text[line..at]) {
            text.insert(at, typed);
            let pair = check::matching_bracket(text, config.language, caret);
            let Some(opener) = pair.map(|(one, other)| one.min(other)) else {
                // It closes nothing: typed as it is.
                text.remove(at);
                return false;
            };
            take(ui, typed);
            let indent = indent_at(text, godcode_core::byte_index(text, opener));
            let after = caret - text[line..at].chars().count() + indent.chars().count() + 1;
            text.replace_range(line..at, &indent);
            set_selection(ui.ctx(), id, &(after..after));
            return true;
        }
    }
    if !steps_over && !opens {
        return false;
    }
    take(ui, typed);
    if !steps_over {
        let at = godcode_core::byte_index(text, caret);
        text.insert_str(at, &format!("{typed}{closer}"));
    }
    set_selection(ui.ctx(), id, &(caret + 1..caret + 1));
    !steps_over
}

/// A rectangle of text: two corners, each a line and a column. A column may be past the end
/// of its line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    /// The corner that stays.
    anchor: (usize, usize),
    /// The corner that moves.
    head: (usize, usize),
    /// Where the caret of the text field was put for it, once it is: the rectangle is given
    /// up when the caret is found elsewhere.
    caret: Option<usize>,
}

/// The chars a block covers on each of its lines, from the first line down.
fn block_ranges(text: &str, block: &Block) -> Vec<Range<usize>> {
    let (first, last) = (
        block.anchor.0.min(block.head.0),
        block.anchor.0.max(block.head.0),
    );
    let (left, right) = (
        block.anchor.1.min(block.head.1),
        block.anchor.1.max(block.head.1),
    );
    let mut start = 0;
    let mut ranges = Vec::new();
    for (line, content) in text.split('\n').enumerate().take(last + 1) {
        let len = content.chars().count();
        if line >= first {
            ranges.push(start + left.min(len)..start + right.min(len));
        }
        start += len + 1;
    }
    ranges
}

/// Write `written` over each of `ranges`, which come in order and apart. Returns the caret
/// after each.
fn write_over(text: &mut String, ranges: &[Range<usize>], written: &str) -> Vec<usize> {
    let width = written.chars().count();
    let mut grown = 0;
    let mut shrunk = 0;
    let carets = ranges.iter().map(|range| {
        grown += width;
        let caret = range.start + grown - shrunk;
        shrunk += range.len();
        caret
    });
    let carets = carets.collect();
    // From the last one to the first, so that the places still to change stay put.
    for range in ranges.iter().rev() {
        let bytes =
            godcode_core::byte_index(text, range.start)..godcode_core::byte_index(text, range.end);
        text.replace_range(bytes, written);
    }
    carets
}

/// With a rectangle selected, what is typed is written over each of its lines; then each
/// line keeps a caret. Returns whether the text changed.
fn type_in_block(ui: &Ui, id: Id, text: &mut String, view: &mut View) -> bool {
    enum Act {
        Write(String),
        Erase,
        Copy,
        Cut,
        /// Back to carets, one on each line.
        Carets,
        Leave,
    }
    let caret = selection_of(ui.ctx(), id)
        .filter(Range::is_empty)
        .map(|selection| selection.start);
    if let (Some((lines, columns)), Some(caret)) = (view.block_step.take(), caret) {
        // Without a rectangle yet, it starts at the caret.
        let mut block = view.block.unwrap_or_else(|| {
            let before = &text[..godcode_core::byte_index(text, caret)];
            let line = before.rsplit('\n').next().unwrap_or("");
            let at = (before.matches('\n').count(), line.chars().count());
            Block {
                anchor: at,
                head: at,
                caret: None,
            }
        });
        let last = text.matches('\n').count();
        block.head.0 = block.head.0.saturating_add_signed(lines).min(last);
        block.head.1 = block.head.1.saturating_add_signed(columns);
        block.caret = None;
        view.block = Some(block);
        return false;
    }
    let Some(block) = view.block else {
        return false;
    };
    if block.caret.is_some() && block.caret != caret {
        view.block = None;
        return false;
    }
    let ranges = block_ranges(text, &block);
    let empty = ranges.iter().all(Range::is_empty);
    let act = ui.input_mut(|input| {
        if input.consume_key(Modifiers::NONE, Key::Escape) {
            return Some(Act::Leave);
        }
        // Nothing selected: erasing is the business of the carets, one on each line.
        let erases = input.key_pressed(Key::Backspace) || input.key_pressed(Key::Delete);
        if erases && !empty {
            input.consume_key(Modifiers::NONE, Key::Backspace);
            input.consume_key(Modifiers::NONE, Key::Delete);
            return Some(Act::Erase);
        }
        let act = input.events.iter().find_map(|event| match event {
            Event::Text(typed) => Some(Act::Write(typed.clone())),
            Event::Paste(pasted) if !pasted.contains('\n') => Some(Act::Write(pasted.clone())),
            Event::Copy => Some(Act::Copy),
            Event::Cut => Some(Act::Cut),
            _ => None,
        });
        if act.is_some() {
            let taken = |event: &Event| {
                matches!(
                    event,
                    Event::Text(_) | Event::Paste(_) | Event::Copy | Event::Cut
                )
            };
            input.events.retain(|event| !taken(event));
        } else if erases {
            return Some(Act::Carets);
        }
        act
    });
    let Some(act) = act else {
        return false;
    };
    let written = match &act {
        Act::Leave => {
            view.block = None;
            return false;
        }
        Act::Carets => "",
        Act::Copy | Act::Cut => {
            let chars = |range: &Range<usize>| {
                godcode_core::byte_index(text, range.start)
                    ..godcode_core::byte_index(text, range.end)
            };
            let lines: Vec<&str> = ranges.iter().map(|range| &text[chars(range)]).collect();
            ui.ctx().copy_text(lines.join("\n"));
            if matches!(act, Act::Copy) {
                return false;
            }
            ""
        }
        Act::Erase => "",
        Act::Write(written) => written,
    };
    let before = text.len();
    let carets = match &act {
        Act::Carets => ranges.iter().map(|range| range.start).collect(),
        _ => write_over(text, &ranges, written),
    };
    // The line of the corner that moved keeps the caret of the text field.
    let main = if block.head.0 < block.anchor.0 {
        carets.first()
    } else {
        carets.last()
    };
    view.block = None;
    if let Some(main) = main.copied() {
        view.extra = carets.into_iter().filter(|caret| *caret != main).collect();
        set_selection(ui.ctx(), id, &(main..main));
    }
    text.len() != before || !written.is_empty()
}

/// With several carets, what is typed or erased happens at each of them. Returns whether
/// the text changed.
fn type_at_carets(ui: &Ui, id: Id, text: &mut String, view: &mut View) -> bool {
    enum Act {
        Insert(String),
        Back,
        Delete,
        Move(isize),
        Leave,
    }
    let primary = selection_of(ui.ctx(), id)
        .filter(Range::is_empty)
        .map(|selection| selection.start);
    let Some(primary) = primary else {
        view.extra.clear();
        return false;
    };
    // Most of the time there is one caret: nothing to read the text for.
    if view.extra.is_empty() {
        return false;
    }
    let len = text.chars().count();
    view.extra
        .retain(|caret| *caret <= len && *caret != primary);
    if view.extra.is_empty() {
        return false;
    }
    let act = ui.input_mut(|input| {
        let keys = [
            (Key::Escape, 0),
            (Key::Backspace, 1),
            (Key::Delete, 2),
            (Key::ArrowLeft, 3),
            (Key::ArrowRight, 4),
        ];
        for (key, act) in keys {
            if input.consume_key(Modifiers::NONE, key) {
                return Some(match act {
                    0 => Act::Leave,
                    1 => Act::Back,
                    2 => Act::Delete,
                    3 => Act::Move(-1),
                    _ => Act::Move(1),
                });
            }
        }
        // Any other move is the move of one caret: the others go.
        let moves = |event: &Event| {
            matches!(event, Event::Key { key, pressed: true, .. }
                if matches!(key, Key::ArrowUp | Key::ArrowDown | Key::Home | Key::End
                    | Key::PageUp | Key::PageDown | Key::Enter | Key::Tab))
        };
        if input.events.iter().any(moves) {
            return Some(Act::Leave);
        }
        let typed = input.events.iter().find_map(|event| match event {
            Event::Text(typed) => Some(typed.clone()),
            Event::Paste(pasted) if !pasted.contains('\n') => Some(pasted.clone()),
            _ => None,
        })?;
        input
            .events
            .retain(|event| !matches!(event, Event::Text(_) | Event::Paste(_)));
        Some(Act::Insert(typed))
    });
    let Some(act) = act else {
        return false;
    };
    // Every caret in order, the main one told apart.
    let mut carets: Vec<(usize, bool)> = view.extra.iter().map(|caret| (*caret, false)).collect();
    carets.push((primary, true));
    carets.sort_unstable();
    let before = text.len();
    match act {
        Act::Leave => {
            view.extra.clear();
            return false;
        }
        Act::Move(step) => {
            for caret in &mut carets {
                caret.0 = caret.0.saturating_add_signed(step).min(len);
            }
        }
        // From the last caret to the first, so that the places still to change stay put.
        Act::Insert(typed) => {
            let width = typed.chars().count();
            for (rank, caret) in carets.iter_mut().enumerate().rev() {
                text.insert_str(godcode_core::byte_index(text, caret.0), &typed);
                caret.0 += (rank + 1) * width;
            }
        }
        Act::Back => {
            for caret in carets.iter().rev().filter(|caret| caret.0 > 0) {
                text.remove(godcode_core::byte_index(text, caret.0 - 1));
            }
            let mut removed = 0;
            for caret in &mut carets {
                removed += usize::from(caret.0 > 0);
                caret.0 -= removed.min(caret.0);
            }
        }
        Act::Delete => {
            for caret in carets.iter().rev().filter(|caret| caret.0 < len) {
                text.remove(godcode_core::byte_index(text, caret.0));
            }
            let mut removed = 0;
            for caret in &mut carets {
                let erased = caret.0 < len;
                caret.0 -= removed;
                removed += usize::from(erased);
            }
        }
    }
    view.extra = carets
        .iter()
        .filter(|caret| !caret.1)
        .map(|caret| caret.0)
        .collect();
    if let Some((main, _)) = carets.iter().find(|caret| caret.1) {
        set_selection(ui.ctx(), id, &(*main..*main));
    }
    text.len() != before
}

/// The name of a tag was typed in: the tag that goes with it takes the same name, so that
/// the two are renamed as one. Returns whether the text changed.
fn retag(ui: &Ui, id: Id, text: &mut String, view: &mut View) -> bool {
    if !std::mem::take(&mut view.retag) {
        return false;
    }
    let Some(caret) = selection_of(ui.ctx(), id).filter(Range::is_empty) else {
        return false;
    };
    let at = godcode_core::byte_index(text, caret.start);
    let Some((twin, name)) = complete::twin_tag(text, at) else {
        return false;
    };
    // A twin before the caret moves it along with what changes there.
    let before = twin.end <= at;
    let (was, now) = (text[twin.clone()].chars().count(), name.chars().count());
    text.replace_range(twin, &name);
    if before {
        let moved = caret.start + now - was;
        set_selection(ui.ctx(), id, &(moved..moved));
    }
    true
}

/// Keys the plain text field would handle without knowing it holds code.
fn keys(ui: &Ui, id: Id, text: &mut String, config: &Config<'_>, view: &mut View) -> bool {
    if !ui.memory(|memory| memory.has_focus(id)) {
        view.completion = None;
        // Escape takes the keyboard away before it gets here: it also means one caret.
        if ui.input(|input| input.key_pressed(Key::Escape)) {
            view.extra.clear();
        }
        return false;
    }
    if type_in_block(ui, id, text, view) || type_at_carets(ui, id, text, view) {
        return true;
    }
    let pressed = |modifiers, key| ui.input_mut(|input| input.consume_key(modifiers, key));
    // Tab after an abbreviation writes what it stands for: elements in markup, a
    // declaration in a block of a style sheet. Unless a completion was chosen with the
    // arrows, which Tab is then for.
    let chosen = view
        .completion
        .as_ref()
        .is_some_and(|popup| popup.navigated);
    let tab = ui.input(|input| input.key_pressed(Key::Tab) && input.modifiers.is_none());
    let caret = selection_of(ui.ctx(), id).filter(Range::is_empty);
    let (markup, styles) = (config.language.is_markup(), config.language.name == "CSS");
    if let (true, false, Some(caret)) = (tab && (markup || styles), chosen, caret) {
        let caret = caret.start;
        let at = godcode_core::byte_index(text, caret);
        let line = text[..at].rfind('\n').map_or(0, |found| found + 1);
        // The style sheet the line is in: the file itself, or a `style` element of a page.
        let sheet = match styles {
            true => Some(&text[..line]),
            false => complete::style_sheet(&text[..line]),
        };
        // Alone on its line, in a block: outside one, a short word is a selector.
        let declared = sheet.and_then(|sheet| {
            let alone = text[line..at].trim_start();
            let in_block = sheet.matches('{').count() > sheet.matches('}').count();
            let body = godcode_core::emmet::expand_css(alone).filter(|_| in_block)?;
            Some((alone, body))
        });
        let written = declared.or_else(|| {
            let abbreviation = godcode_core::emmet::abbreviation(&text[line..at]);
            let abbreviation = abbreviation.filter(|_| markup && sheet.is_none())?;
            let tags = config.language.keywords();
            Some((
                abbreviation,
                godcode_core::emmet::expand(abbreviation, tags)?,
            ))
        });
        if let Some((abbreviation, body)) = written {
            let start = caret - abbreviation.chars().count();
            pressed(Modifiers::NONE, Key::Tab);
            let after = complete::write_snippet(text, start..caret, &body, config.indent);
            set_selection(ui.ctx(), id, &(after..after));
            view.completion = None;
            return true;
        }
    }
    if let Some(popup) = &mut view.completion {
        if pressed(Modifiers::NONE, Key::Escape) {
            view.completion = None;
        } else if pressed(Modifiers::NONE, Key::ArrowDown) {
            popup.selected = (popup.selected + 1) % popup.items.len();
            popup.navigated = true;
        } else if pressed(Modifiers::NONE, Key::ArrowUp) {
            popup.selected = (popup.selected + popup.items.len() - 1) % popup.items.len();
            popup.navigated = true;
        } else if pressed(Modifiers::NONE, Key::Tab)
            || (popup.navigated && pressed(Modifiers::NONE, Key::Enter))
        {
            accept(ui.ctx(), id, text, config, view);
            return true;
        }
    }
    if pair(ui, id, text, config) {
        return true;
    }
    // Shifted first: a plain pattern also matches the shifted key.
    let op = if pressed(Modifiers::SHIFT, Key::Tab) {
        Op::Outdent
    } else if pressed(Modifiers::NONE, Key::Tab) {
        Op::Indent
    } else if pressed(Modifiers::NONE, Key::Enter) {
        Op::Newline
    } else {
        return false;
    };
    view.completion = None;
    run(ui.ctx(), id, text, config, op)
}

/// Write the chosen completion in place of the word being typed.
fn accept(ctx: &Context, id: Id, text: &mut String, config: &Config<'_>, view: &mut View) {
    let Some(popup) = view.completion.take() else {
        return;
    };
    if let Some(item) = popup.items.get(popup.selected) {
        let caret = complete::apply(text, popup.caret, item, config.indent);
        set_selection(ctx, id, &(caret..caret));
        view.focus = true;
        view.accepted = Some(item.clone());
    }
}

/// Open, refresh or close the completion list after this frame's typing.
fn refresh_completion(
    ui: &Ui,
    text: &str,
    config: &Config<'_>,
    view: &mut View,
    caret: Option<usize>,
    typed: bool,
) {
    let manual = std::mem::take(&mut view.complete_now);
    // A big text is not searched for its words at every key.
    let Some(caret) = caret.filter(|_| text.len() <= LARGE_BYTES) else {
        view.completion = None;
        return;
    };
    // In markup and in style sheets a name goes on over its hyphens.
    let hyphens = config.language.is_markup() || config.language.name == "CSS";
    let named = move |ch: char| ch.is_alphanumeric() || ch == '_' || (hyphens && ch == '-');
    let (word, erased) = ui.input(|input| {
        let is_word = |text: &str| text.chars().all(named);
        (
            input
                .events
                .iter()
                .any(|event| matches!(event, Event::Text(text) if is_word(text))),
            input.key_pressed(Key::Backspace),
        )
    });
    let was_open = view.completion.is_some();
    if manual || (typed && (word || (erased && was_open))) {
        let start = complete::word_start(text, caret);
        let typed: String = text.chars().skip(start).take(caret - start).collect();
        let typed = typed.to_lowercase();
        let fits = |label: &str| !typed.is_empty() && label.to_lowercase().starts_with(&typed);
        let own = config.snippets.iter().filter(|(label, _)| fits(label));
        let own = own.map(|(label, body)| Completion {
            label: label.clone(),
            body: body.clone(),
            kind: Kind::Snippet,
        });
        let known = complete::complete(text, caret, config.language);
        let items: Vec<Completion> = own.chain(known).collect();
        let before = &text[..godcode_core::byte_index(text, caret)];
        let typed_chars = before.chars().rev().take_while(|ch| named(*ch)).count();
        let wanted = manual || was_open || typed_chars >= 2;
        view.completion = (wanted && !items.is_empty()).then_some(Popup {
            items,
            selected: 0,
            caret,
            navigated: false,
        });
    } else if view
        .completion
        .as_ref()
        .is_some_and(|popup| popup.caret != caret)
    {
        view.completion = None;
    }
}

/// Draw the completion list under the caret. Returns the item clicked.
fn show_completion(ui: &Ui, id: Id, popup: &Popup, at: Pos2, palette: &Palette) -> Option<usize> {
    let mut clicked = None;
    let first = popup.selected.saturating_sub(POPUP_ROWS - 1);
    Area::new(id.with("completion"))
        .order(Order::Foreground)
        .fixed_pos(at)
        .show(ui.ctx(), |ui| {
            Frame::popup(ui.style())
                .inner_margin(Margin::same(4))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let shown = popup.items.iter().enumerate().skip(first).take(POPUP_ROWS);
                    for (index, item) in shown {
                        let (rect, response) = ui.allocate_exact_size(POPUP_ROW, Sense::click());
                        let painter = ui.painter();
                        if index == popup.selected {
                            painter.rect_filled(rect, CornerRadius::same(4), palette.selected);
                        } else if response.hovered() {
                            painter.rect_filled(rect, CornerRadius::same(4), palette.hover);
                        }
                        painter.text(
                            pos2(rect.min.x + 8.0, rect.center().y),
                            Align2::LEFT_CENTER,
                            &item.label,
                            FontId::monospace(13.0),
                            palette.text,
                        );
                        let kind = match item.kind {
                            Kind::Snippet => "snippet",
                            Kind::Keyword => "keyword",
                            Kind::Word => "word",
                            Kind::Symbol => "symbol",
                        };
                        painter.text(
                            pos2(rect.max.x - 8.0, rect.center().y),
                            Align2::RIGHT_CENTER,
                            kind,
                            FontId::proportional(11.0),
                            palette.text_dim,
                        );
                        if response.clicked() {
                            clicked = Some(index);
                        }
                    }
                });
        });
    clicked
}

/// The text as coloured sections, rows being cut at `width` (never, when infinite).
fn layout_job(text: &str, config: &Config<'_>, dark: bool, width: f32) -> LayoutJob {
    let font = FontId::monospace(config.font_size);
    let mut start = 0;
    let sections = syntax::highlight(text, config.language)
        .into_iter()
        .map(|span| {
            let byte_range = egui::text::ByteIndex(start)..egui::text::ByteIndex(span.end);
            start = span.end;
            LayoutSection {
                leading_space: 0.0,
                byte_range,
                format: TextFormat::simple(font.clone(), theme::token_color(span.token, dark)),
            }
        })
        .collect();
    LayoutJob {
        text: text.to_owned(),
        sections,
        wrap: TextWrapping {
            max_width: width,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A zigzag under `rect`, the way editors underline mistakes.
fn zigzag(rect: Rect, color: Color32) -> Shape {
    let y = rect.max.y - 1.0;
    let steps = (rect.width() / 2.0).ceil().max(2.0) as usize;
    let points = (0..=steps)
        .map(|step| {
            let lift = if step % 2 == 0 { 0.0 } else { -2.0 };
            pos2(rect.min.x + step as f32 * 2.0, y + lift)
        })
        .collect();
    Shape::line(points, Stroke::new(1.0, color))
}

fn problem_color(error: bool) -> Color32 {
    if error { ERROR_COLOR } else { WARNING_COLOR }
}

/// Show the editor over the rest of `ui`; `menu` fills its right-click menu.
pub fn show(
    ui: &mut Ui,
    id: Id,
    text: &mut String,
    config: &Config<'_>,
    view: &mut View,
    menu: &mut dyn FnMut(&mut Ui),
) -> Output {
    let palette = Palette::of(ui);
    let dark = ui.visuals().dark_mode;
    let font = FontId::monospace(config.font_size);
    let mut changed = retag(ui, id, text, view) | keys(ui, id, text, config, view);
    let selected_before = selection_of(ui.ctx(), id);

    // Counted when the text was last read through: it only tells how wide the numbers are.
    // Given back before the field shows: alone to hold it, the field can follow what is
    // typed in it instead of making it again.
    let lines = match view.layout.take() {
        Some(layout) => layout.starts.len() - 1,
        None => text.bytes().filter(|byte| *byte == b'\n').count() + 1,
    };
    let (digit, row_height) =
        ui.fonts_mut(|fonts| (fonts.glyph_width(&font, '0'), fonts.row_height(&font)));
    let gutter_width = lines.to_string().len().max(3) as f32 * digit + 2.0 * GUTTER_PAD;
    let full = ui.available_rect_before_wrap();
    let gutter = full.with_max_x(full.min.x + gutter_width);
    let map_width = if config.minimap { MINIMAP_WIDTH } else { 0.0 };
    let map = full.with_min_x(full.max.x - map_width);
    let body = full.with_min_x(gutter.max.x).with_max_x(map.min.x);
    ui.painter().rect_filled(full, 0.0, palette.bg);

    let unit_columns = config
        .indent
        .bytes()
        .map(|byte| if byte == b'\t' { TAB_COLUMNS } else { 1 })
        .sum::<usize>()
        .max(1);
    let mut child = ui.new_child(UiBuilder::new().max_rect(body));
    let mut scroll = if config.wrap {
        ScrollArea::vertical()
    } else {
        ScrollArea::both()
    };
    if let Some(line) = view.scroll_to_line.take() {
        // A block folded away takes no row, a line cut to fit takes several.
        let row = view.index.row_of(line);
        scroll = scroll.vertical_scroll_offset(row * row_height);
    }
    let scrolled = scroll
        .id_salt(id)
        .auto_shrink(false)
        .show(&mut child, |ui| {
            // Reserved now so the highlights end up under the text.
            let under = ui.painter().add(Shape::Noop);
            let look = field::Look {
                language: config.language,
                font: &font,
                dark,
                margin: MARGIN,
                min_size: body.size(),
                hints: config.hints,
                block_caret: config.block_caret,
                wrap: config.wrap,
                // What the keys just did to the text is not in the revision yet.
                revision: config.revision.wrapping_add(u64::from(changed)),
            };
            let (index, folded) = (&mut view.index, &mut view.folded);
            let shown = field::show(ui, id, text, &look, index, folded);
            // What the guides, the pinned lines and the strip read of the text.
            view.layout = Some(std::sync::Arc::clone(shown.lines.index()));
            let (response, cursor_range, galley_pos, typed) =
                (shown.response, shown.cursor, shown.origin, shown.changed);
            // Where the chars of the text are, counted from its top left.
            let geometry = shown.lines;
            if std::mem::take(&mut view.focus) {
                response.request_focus();
            }
            response.context_menu(|ui| menu(ui));
            let mut selection = cursor_range.map(sorted);
            // The text field moves the caret on any click: a right click inside the
            // selection keeps it, for the menu to act on.
            let right_click =
                ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Secondary));
            if right_click
                && let (Some(old), Some(new)) = (&selected_before, &selection)
                && old.contains(&new.start)
            {
                set_selection(ui.ctx(), id, old);
                selection = selected_before.clone();
            }
            // Alt+click leaves a caret where the caret was; a plain click is one caret again.
            if response.clicked() {
                match &selected_before {
                    Some(old)
                        if ui.input(|input| input.modifiers.alt && !input.modifiers.shift) =>
                    {
                        view.extra.push(old.start)
                    }
                    _ => view.extra.clear(),
                }
            }
            // Alt+Shift and a drag select a rectangle, from where the button went down.
            let columnar = ui.input(|input| input.modifiers.alt && input.modifiers.shift);
            let pressed_at = ui.input(|input| input.pointer.press_origin());
            if columnar
                && response.dragged_by(egui::PointerButton::Primary)
                && let (Some(from), Some(to)) = (pressed_at, response.interact_pointer_pos())
            {
                let cell = |pos: Pos2| {
                    let within = pos - galley_pos;
                    let column = (within.x / digit).round().max(0.0) as usize;
                    (geometry.line_at(within.y.max(0.0)), column)
                };
                view.block = Some(Block {
                    anchor: cell(from),
                    head: cell(to),
                    caret: None,
                });
            }
            // The caret of the text field waits at the corner that moves.
            let mut block_shown = Vec::new();
            if let Some(block) = &mut view.block {
                block_shown = block_ranges(text, block);
                let at = if block.head.0 < block.anchor.0 {
                    block_shown.first()
                } else {
                    block_shown.last()
                };
                if let (None, Some(at)) = (block.caret, at) {
                    let caret = if block.head.1 < block.anchor.1 {
                        at.start
                    } else {
                        at.end
                    };
                    set_selection(ui.ctx(), id, &(caret..caret));
                    block.caret = Some(caret);
                    selection = Some(caret..caret);
                    view.extra.clear();
                }
            }
            let jump = (response.clicked() && ui.input(|input| input.modifiers.command))
                .then(|| cursor_range.map(|range| range.primary.index.0))
                .flatten();
            let pointed = response.hover_pos().and_then(|pos| {
                let within = pos - galley_pos;
                let index = geometry.at(within);
                let cell = geometry.pos(index);
                // Past the end of a line, the pointer is on nothing.
                (within.x <= cell.max.x + digit).then_some((index, pos))
            });
            let origin = galley_pos.to_vec2();
            let clip = ui.clip_rect();
            let caret = cursor_range.map(|range| geometry.pos(range.primary.index.0));
            let caret_index = selection
                .as_ref()
                .filter(|selection| selection.is_empty())
                .map(|selection| selection.start);
            // Where on screen the chars `start..start + len` are, when their line shows.
            let place = |line: usize, start: usize, len: usize| {
                // One row per line unless they wrap: lines out of view are skipped unmeasured,
                // and so are the ones folded away.
                let top = origin.y + geometry.top(line)?;
                if !config.wrap && (top > clip.max.y || top + row_height < clip.min.y) {
                    return None;
                }
                let from = geometry.pos(start);
                let to = geometry.pos(start + len);
                // Cut by the end of a row, only its part on the first row is marked.
                let end = if to.min.y > from.min.y {
                    pos2(from.min.x + len as f32 * digit, from.max.y)
                } else {
                    to.max
                };
                let rect = Rect::from_min_max(from.min, end).translate(origin);
                clip.intersects(rect).then_some(rect)
            };

            let mut shapes = Vec::new();
            if let Some(caret) = caret
                && caret_index.is_some()
            {
                let line = Rect::from_x_y_ranges(clip.x_range(), caret.translate(origin).y_range());
                shapes.push(Shape::rect_filled(line, 0.0, palette.surface));
            }
            // Where the debugged program waits.
            let stopped = config.stopped.filter(|_| !config.wrap);
            if let Some(top) = stopped.and_then(|line| geometry.top(line)) {
                let top = origin.y + top;
                let row = Rect::from_x_y_ranges(clip.x_range(), top..=top + row_height);
                shapes.push(Shape::rect_filled(row, 0.0, STOPPED_COLOR));
            }
            // A thin line down every indent level, to follow blocks by eye.
            if let Some(layout) = view.layout.as_ref().filter(|_| !config.wrap) {
                let stroke = Stroke::new(1.0, palette.border);
                // A blank line continues the block it is in.
                let mut levels = 0;
                let (above, below) = (clip.min.y - origin.y - row_height, clip.max.y - origin.y);
                for (line, top) in geometry.between(above, below) {
                    if let Some(columns) = layout.indents.get(line).copied().flatten() {
                        levels = columns / unit_columns;
                    }
                    let top = origin.y + top;
                    for level in 0..levels {
                        let x = origin.x + (level * unit_columns) as f32 * digit + 0.5;
                        shapes.push(Shape::line_segment(
                            [pos2(x, top), pos2(x, top + row_height)],
                            stroke,
                        ));
                    }
                }
            }
            let radius = CornerRadius::same(2);
            for found in config.occurrences {
                if let Some(rect) = place(found.line, found.start, found.len) {
                    shapes.push(Shape::rect_filled(rect, radius, palette.hover));
                }
            }
            for (index, mark) in config.marks.iter().enumerate() {
                let Some(rect) = place(mark.line, mark.start, mark.len) else {
                    continue;
                };
                shapes.push(Shape::rect_filled(rect, radius, palette.selected));
                if config.current_mark == Some(index) {
                    let stroke = Stroke::new(1.0, palette.text);
                    shapes.push(Shape::rect_stroke(
                        rect,
                        radius,
                        stroke,
                        StrokeKind::Outside,
                    ));
                }
            }
            // The bracket at the caret and its partner.
            if let (Some(caret), Some(layout)) = (caret_index, &view.layout) {
                let known = view
                    .pair
                    .filter(|(key, at, _)| *key == layout.key && *at == caret);
                let (_, _, pair) = known.unwrap_or_else(|| {
                    let small = text.len() <= LARGE_BYTES;
                    let pair = small
                        .then(|| check::matching_bracket(text, config.language, caret))
                        .flatten();
                    (layout.key, caret, pair)
                });
                view.pair = Some((layout.key, caret, pair));
                for index in pair.into_iter().flat_map(|(a, b)| [a, b]) {
                    let cell = geometry.pos(index);
                    let rect = Rect::from_min_size(cell.min, vec2(digit, cell.height()));
                    let stroke = Stroke::new(1.0, palette.text_dim);
                    shapes.push(Shape::rect_stroke(
                        rect.translate(origin),
                        radius,
                        stroke,
                        StrokeKind::Inside,
                    ));
                }
            }
            // The rectangle: a band on each line, a bar where the line has nothing in it.
            for range in &block_shown {
                let from = geometry.pos(range.start).translate(origin);
                let to = geometry.pos(range.end).translate(origin);
                let right = to.min.x.max(from.min.x + 1.5);
                let band = Rect::from_min_max(from.min, pos2(right, from.max.y));
                let ink = if range.is_empty() {
                    palette.text
                } else {
                    ui.visuals().selection.bg_fill
                };
                shapes.push(Shape::rect_filled(band, 0.0, ink));
            }
            ui.painter().set(under, Shape::Vec(shapes));
            if let (Some(note), Some(range)) = (config.note, cursor_range) {
                let index = range.primary.index.0;
                let rest = text.chars().skip(index).take_while(|ch| *ch != '\n');
                let end = geometry.pos(index + rest.count()).translate(origin);
                ui.painter().text(
                    pos2(end.max.x + 5.0 * digit, end.center().y),
                    Align2::LEFT_CENTER,
                    note,
                    FontId::monospace(config.font_size * 0.9),
                    palette.text_dim.gamma_multiply(0.75),
                );
            }
            for caret in &view.extra {
                let cell = geometry.pos(*caret).translate(origin);
                let bar = Rect::from_min_size(cell.min, vec2(1.5, cell.height()));
                ui.painter().rect_filled(bar, 0.0, palette.text);
            }
            // The keyboard is here as far as the app goes, whether or not the window has it.
            let focused = ui.memory(|memory| memory.has_focus(response.id));

            for (index, squiggle) in config.squiggles.iter().enumerate() {
                let Some(rect) = place(squiggle.line, squiggle.start, squiggle.len.max(1)) else {
                    continue;
                };
                let rect = rect.with_max_x(rect.max.x.max(rect.min.x + digit));
                ui.painter()
                    .add(zigzag(rect, problem_color(squiggle.error)));
                ui.interact(rect, id.with(("problem", index)), Sense::hover())
                    .on_hover_text(&squiggle.message);
            }

            if let Some(index) = view.scroll_to.take() {
                let target = geometry.pos(index).translate(origin);
                ui.scroll_to_rect(target.expand2(vec2(60.0, 0.0)), Some(Align::Center));
            }
            if let (true, Some(caret)) = (std::mem::take(&mut view.reveal), caret) {
                let around = caret
                    .translate(origin)
                    .expand2(vec2(4.0 * digit, row_height));
                ui.scroll_to_rect(around, None);
            }
            changed |= typed;
            view.retag = typed && config.language.is_markup();
            let caret_screen = caret.map(|caret| caret.translate(origin).left_bottom());
            (
                geometry,
                galley_pos,
                caret,
                selection,
                caret_index,
                caret_screen,
                typed,
                jump,
                pointed,
                focused,
            )
        });
    ui.allocate_rect(full, Sense::hover());
    let (
        geometry,
        origin,
        caret,
        selection,
        caret_index,
        caret_screen,
        typed,
        jump,
        pointed,
        focused,
    ) = scrolled.inner;

    refresh_completion(ui, text, config, view, caret_index, typed);
    let clicked = match (&view.completion, caret_screen) {
        (Some(popup), Some(at)) => show_completion(ui, id, popup, at + vec2(0.0, 2.0), &palette),
        _ => None,
    };
    if let Some(index) = clicked {
        if let Some(popup) = &mut view.completion {
            popup.selected = index;
        }
        accept(ui.ctx(), id, text, config, view);
        // Written after the field showed: it has not seen it.
        view.index.forget();
        changed = true;
    }

    let painter = ui.painter_at(gutter);
    painter.rect_filled(gutter, 0.0, palette.bg);
    let number_font = FontId::monospace(config.font_size * 0.92);
    // The line the caret is on: wrapped, a line takes several rows.
    let caret_line = caret.map(|caret| geometry.line_at(caret.min.y));
    let mut gutter_clicked = None;
    let mut fold_clicked = None;
    let over_numbers = ui.rect_contains_pointer(gutter);
    // Lines are counted from 0, as the marks are.
    let in_view = geometry.between(
        gutter.min.y - origin.y - row_height,
        gutter.max.y - origin.y,
    );
    for (index, top) in in_view {
        let top = origin.y + top;
        // An empty line has no height of its own: the font's row height places the number.
        let middle = top + row_height / 2.0;
        let current = caret_line == Some(index);
        // A click on the number of a line sets or removes its breakpoint.
        let number = Rect::from_min_max(
            pos2(gutter.min.x, top),
            pos2(gutter.max.x, top + row_height),
        );
        let clicked = ui
            .interact(number, id.with(("gutter", index)), Sense::click())
            .clicked();
        if clicked {
            gutter_clicked = Some(index);
        }
        // A block can be folded from the line that opens it: an arrow by its number, shown
        // while the pointer is over the numbers, and always when the block is closed.
        if let Some(closed) = geometry.foldable(index) {
            let zone = number.with_min_x(gutter.max.x - GUTTER_PAD);
            let arrow = ui
                .interact(zone, id.with(("fold", index)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if arrow.clicked() {
                fold_clicked = Some(index);
            }
            if closed || over_numbers {
                let glyph = if closed {
                    egui_phosphor::fill::CARET_RIGHT
                } else {
                    egui_phosphor::fill::CARET_DOWN
                };
                let ink = if arrow.hovered() {
                    palette.text
                } else {
                    palette.text_dim
                };
                painter.text(
                    zone.center(),
                    Align2::CENTER_CENTER,
                    glyph,
                    FontId::proportional(10.0),
                    ink,
                );
            }
        }
        if config.breakpoints.contains(&index) {
            let centre = pos2(gutter.min.x + 7.0, middle);
            painter.circle_filled(centre, 4.5, ERROR_COLOR);
            // One with a condition is hollow: it does not always stop.
            if config.conditional.contains(&index) {
                painter.circle_filled(centre, 2.0, palette.bg);
            }
        }
        painter.text(
            pos2(gutter.max.x - GUTTER_PAD, middle),
            Align2::RIGHT_CENTER,
            (index + 1).to_string(),
            number_font.clone(),
            if current {
                palette.text
            } else {
                palette.text_dim
            },
        );
        // A dot marks the lines with a problem, errors winning over warnings.
        let change = config
            .changes
            .iter()
            .find(|(first, count, _)| (*first..first + count).contains(&index));
        if let Some((_, _, kind)) = change {
            let bar = Rect::from_min_max(
                pos2(gutter.max.x - 5.0, top),
                pos2(gutter.max.x - 2.0, top + row_height),
            );
            let (bar, color) = match kind {
                LineChange::Added => (bar, ADDED_COLOR),
                LineChange::Modified => (bar, MODIFIED_COLOR),
                // What was removed sat under this line: only its lower edge is marked.
                LineChange::Deleted => (bar.with_min_y(bar.max.y - 3.0), ERROR_COLOR),
            };
            painter.rect_filled(bar, 0.0, color);
        }
        let worst = config
            .squiggles
            .iter()
            .filter(|squiggle| squiggle.line == index)
            .map(|squiggle| squiggle.error)
            .max();
        if let Some(error) = worst {
            painter.circle_filled(pos2(gutter.min.x + 7.0, middle), 3.0, problem_color(error));
        }
    }
    // Fold or unfold: the block of the arrow clicked, or the one the caret is in.
    {
        let lines = &geometry;
        let asked = std::mem::take(&mut view.fold_at_caret);
        let at_caret = caret_line.filter(|_| asked);
        let header =
            fold_clicked.or_else(|| at_caret.and_then(|line| lines.index().header_of(line)));
        if let Some(header) = header {
            if let Some(open) = view.folded.iter().position(|folded| *folded == header) {
                view.folded.remove(open);
            } else {
                view.folded.push(header);
                // A caret inside the block would open it again: it waits on the first line.
                let inside = lines.index().block(header);
                if caret_line.is_some_and(|line| inside.is_some_and(|block| block.contains(&line)))
                {
                    let start = lines.index().start_of(header);
                    set_selection(ui.ctx(), id, &(start..start));
                }
            }
            ui.ctx().request_repaint();
        }
    }
    let scrolled_rows = scrolled.state.offset.y / row_height;
    let at_end = scrolled.state.offset.y > 0.0
        && scrolled.state.offset.y + scrolled.inner_rect.height() >= scrolled.content_size.y - 1.0;
    let first_line = geometry.line_of_row(scrolled_rows);
    let top_line = if at_end { f32::INFINITY } else { first_line };

    let mut go_to = None;
    if let Some(layout) = &view.layout {
        if config.sticky && !config.wrap && !is_prose(config.language) {
            // The lines that open the blocks `top` is in, outermost first: going up, each
            // line less indented than the last one kept opens a block.
            let opening = |top: usize| {
                let top = top.min(layout.indents.len());
                // The first line with text tells how deep the view starts.
                let mut limit = layout.indents[top..]
                    .iter()
                    .find_map(|indent| *indent)
                    .unwrap_or(0);
                let mut headers = Vec::new();
                let mut line = top;
                while line > 0
                    && limit > 0
                    && headers.len() < STICKY_LINES
                    && top - line < STICKY_REACH
                {
                    line -= 1;
                    if let Some(indent) = layout.indents[line].filter(|indent| *indent < limit) {
                        headers.push(line);
                        limit = indent;
                    }
                }
                headers.reverse();
                headers
            };
            // The pinned lines hide as many rows: what counts is the first line under them.
            let top = first_line.floor() as usize;
            let hidden = opening(top).len();
            let headers = opening(top + hidden);
            let size = vec2(map.min.x - full.min.x, headers.len() as f32 * row_height);
            let strip = Rect::from_min_size(full.min, size);
            let painter = ui.painter_at(strip);
            if !headers.is_empty() {
                painter.rect_filled(strip, 0.0, palette.bg);
                painter.hline(
                    strip.x_range(),
                    strip.max.y - 0.5,
                    Stroke::new(1.0, palette.border),
                );
            }
            for (row, line) in headers.iter().enumerate() {
                let y = strip.min.y + row as f32 * row_height;
                painter.text(
                    pos2(gutter.max.x - GUTTER_PAD, y + row_height / 2.0),
                    Align2::RIGHT_CENTER,
                    (line + 1).to_string(),
                    number_font.clone(),
                    palette.text_dim,
                );
                // The text may have changed since it was laid out: no line is better
                // than a wrong one.
                let bytes = layout.starts[*line]..layout.starts[line + 1] - 1;
                let content = text.get(bytes).unwrap_or("");
                let job = layout_job(content, config, dark, f32::INFINITY);
                let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                ui.painter_at(strip.with_min_x(gutter.max.x)).galley(
                    pos2(origin.x, y),
                    galley,
                    palette.text,
                );
                let rect = Rect::from_min_size(pos2(strip.min.x, y), vec2(size.x, row_height));
                let clicked = ui
                    .interact(rect, id.with(("sticky", row)), Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked();
                if clicked {
                    // Under the lines that stay pinned above it.
                    go_to = Some((*line as f32 - row as f32).max(0.0));
                }
            }
        }
        if config.minimap {
            let shown = scrolled.inner_rect.height() / row_height;
            go_to = show_minimap(ui, id, map, layout, first_line, shown).or(go_to);
        }
    }
    if go_to.is_some() {
        view.scroll_to_line = go_to;
        ui.ctx().request_repaint();
    }
    // Found from where the lines are known to start, not by reading the text up to them.
    let place = |index: usize| {
        let line = geometry.index().line_of(index);
        (line + 1, index - geometry.index().start_of(line) + 1)
    };
    let places = selection
        .as_ref()
        .map(|selection| (place(selection.start), place(selection.end)));
    Output {
        places,
        lines: geometry.index().lines(),
        changed,
        selection,
        jump,
        top_line,
        pointed,
        caret_pos: caret_screen,
        focused,
        gutter: gutter_clicked,
    }
}

/// The whole file as a strip: a bar per line, as long as its text, and the part in view.
/// Returns the line to put at the top of the view when the strip is clicked or dragged.
fn show_minimap(
    ui: &Ui,
    id: Id,
    rect: Rect,
    layout: &Layout,
    first: f32,
    shown: f32,
) -> Option<f32> {
    let palette = Palette::of(ui);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, palette.bg);
    painter.vline(
        rect.min.x + 0.5,
        rect.y_range(),
        Stroke::new(1.0, palette.border),
    );
    let lines = layout.indents.len();
    // A long file is squeezed into the height of the strip.
    let step = (rect.height() / lines as f32).min(MINIMAP_ROW);
    let view = Rect::from_min_size(
        pos2(rect.min.x + 1.0, rect.min.y + first * step),
        vec2(rect.width() - 1.0, (shown * step).max(8.0)),
    );
    painter.rect_filled(view, 0.0, palette.hover);
    let ink = palette.text_dim.gamma_multiply(0.55);
    // Squeezed under a pixel a line, only some lines are drawn.
    let every = (1.0 / step).ceil().max(1.0) as usize;
    for line in (0..lines).step_by(every) {
        let Some(indent) = layout.indents[line] else {
            continue;
        };
        let bytes = layout.starts[line + 1] - 1 - layout.starts[line];
        let left = rect.min.x + 6.0 + indent as f32 * MINIMAP_COLUMN;
        let width = bytes.saturating_sub(indent) as f32 * MINIMAP_COLUMN;
        let width = width.min(rect.max.x - 4.0 - left).max(1.0);
        let bar = Rect::from_min_size(
            pos2(left, rect.min.y + line as f32 * step),
            vec2(width, (step - 1.0).max(1.0)),
        );
        painter.rect_filled(bar, 0.0, ink);
    }
    let response = ui.interact(rect, id.with("minimap"), Sense::click_and_drag());
    let pointer = response
        .interact_pointer_pos()
        .filter(|_| response.clicked() || response.dragged())?;
    // The place pointed at goes to the middle of the view.
    Some(((pointer.y - rect.min.y) / step - shown / 2.0).clamp(0.0, lines as f32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{PointerButton, RawInput};

    /// The editor run frame by frame with scripted input.
    struct Harness {
        ctx: Context,
        text: String,
        view: View,
        time: f64,
        file: &'static str,
        wrap: bool,
        hints: Vec<field::Hint>,
        /// Bumped when the text changed, as the app does; and the size it had then.
        revision: u64,
        bytes: usize,
    }

    fn editor_id() -> Id {
        Id::new("editor")
    }

    impl Harness {
        fn new(text: &str) -> Self {
            Self::of("main.rs", text, false)
        }

        fn of(file: &'static str, text: &str, wrap: bool) -> Self {
            let mut harness = Self {
                ctx: Context::default(),
                text: text.to_owned(),
                view: View::focused(),
                time: 0.0,
                file,
                wrap,
                hints: Vec::new(),
                revision: 0,
                bytes: text.len(),
            };
            harness.frame(Vec::new());
            harness.frame(Vec::new());
            harness
        }

        fn frame(&mut self, events: Vec<Event>) -> Output {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 600.0))),
                events,
                time: Some(self.time),
                focused: true,
                ..Default::default()
            };
            self.time += 0.1;
            // A test that wrote in the text itself is seen by its size.
            if std::mem::replace(&mut self.bytes, self.text.len()) != self.text.len() {
                self.revision += 1;
            }
            let config = Config {
                language: Language::of(std::path::Path::new(self.file)),
                font_size: 14.0,
                indent: "    ",
                marks: &[],
                current_mark: None,
                squiggles: &[],
                changes: &[],
                occurrences: &[],
                wrap: self.wrap,
                minimap: true,
                sticky: true,
                note: None,
                snippets: &[],
                breakpoints: &[],
                conditional: &[],
                stopped: None,
                hints: &self.hints,
                block_caret: false,
                revision: self.revision,
            };
            let mut output = None;
            let _ = self.ctx.run_ui(input, |ui| {
                output = Some(show(
                    ui,
                    editor_id(),
                    &mut self.text,
                    &config,
                    &mut self.view,
                    &mut |_| {},
                ));
            });
            let output = output.expect("the editor ran");
            if output.changed {
                self.revision += 1;
                self.bytes = self.text.len();
            }
            output
        }

        fn key(&mut self, key: Key, modifiers: Modifiers) -> Output {
            let event = Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            };
            self.frame(vec![event]);
            self.frame(Vec::new())
        }

        fn type_text(&mut self, text: &str) -> Output {
            self.frame(vec![Event::Text(text.to_owned())]);
            self.frame(Vec::new())
        }
    }

    #[test]
    fn typing_enter_and_tab_write_indented_code() {
        let mut harness = Harness::new("");
        harness.type_text("fn main");
        for ch in ["(", ")", " ", "{"] {
            harness.type_text(ch);
        }
        assert_eq!(harness.text, "fn main() {}", "brackets come in pairs");
        harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "fn main() {\n    \n}");
        let output = harness.type_text("go();");
        assert_eq!(harness.text, "fn main() {\n    go();\n}");
        assert_eq!(output.selection, Some(21..21));
        assert!(!output.changed, "nothing was typed in the last frame");

        harness.key(Key::Tab, Modifiers::SHIFT);
        assert_eq!(harness.text, "fn main() {\ngo();\n}");
        harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "fn main() {\ngo();    \n}");
    }

    #[test]
    fn several_carets_type_and_erase_together() {
        let mut harness = Harness::new("ab\nab\nab");
        set_selection(&harness.ctx, editor_id(), &(1..1));
        harness.view.extra = vec![4, 7];
        harness.frame(Vec::new());
        harness.type_text("X");
        assert_eq!(harness.text, "aXb\naXb\naXb");
        assert_eq!(harness.view.extra, [6, 10]);
        harness.key(Key::Backspace, Modifiers::NONE);
        assert_eq!(harness.text, "ab\nab\nab");
        harness.key(Key::Delete, Modifiers::NONE);
        assert_eq!(harness.text, "a\na\na");
        assert_eq!(harness.view.extra, [3, 5]);
        harness.key(Key::Escape, Modifiers::NONE);
        assert!(harness.view.extra.is_empty());
    }

    #[test]
    fn typing_a_tag_closes_it() {
        let mut harness = Harness::of("index.html", "", false);
        harness.type_text("<div");
        let output = harness.type_text(">");
        assert_eq!(harness.text, "<div></div>");
        assert_eq!(
            output.selection,
            Some(5..5),
            "the caret sits between the tags"
        );
        harness.type_text("<br");
        harness.type_text(">");
        assert_eq!(harness.text, "<div><br></div>");
    }

    #[test]
    fn tab_writes_what_an_abbreviation_stands_for() {
        let mut harness = Harness::of("index.html", "<body>\n    \n</body>", false);
        set_selection(&harness.ctx, editor_id(), &(11..11));
        harness.frame(Vec::new());
        harness.type_text("ul>li.item*2");
        let out = harness.key(Key::Tab, Modifiers::NONE);
        let list = "    <ul>\n        <li class=\"item\"></li>\n        <li class=\"item\"></li>\n    </ul>";
        assert_eq!(harness.text, format!("<body>\n{list}\n</body>"));
        // The caret waits in the first item.
        assert_eq!(out.selection, Some(41..41));
        // A word that names no element is a word: Tab indents, as anywhere else.
        let mut harness = Harness::of("index.html", "hello", false);
        set_selection(&harness.ctx, editor_id(), &(5..5));
        harness.frame(Vec::new());
        harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "hello    ");
        // In code, nothing of the kind.
        let mut harness = Harness::new("div");
        set_selection(&harness.ctx, editor_id(), &(3..3));
        harness.frame(Vec::new());
        harness.key(Key::Tab, Modifiers::NONE);
        assert!(!harness.text.contains('<'), "{}", harness.text);
    }

    #[test]
    fn the_two_tags_of_an_element_are_renamed_as_one() {
        let mut harness = Harness::of("index.html", "<div>\n  <p>a</p>\n</div>", false);
        // The name of the opening tag is erased and typed again: the closing one follows.
        set_selection(&harness.ctx, editor_id(), &(4..4));
        harness.frame(Vec::new());
        for _ in 0..3 {
            harness.key(Key::Backspace, Modifiers::NONE);
        }
        assert_eq!(harness.text, "<>\n  <p>a</p>\n</>");
        for letter in ["n", "a", "v"] {
            harness.type_text(letter);
        }
        assert_eq!(harness.text, "<nav>\n  <p>a</p>\n</nav>");
        // From the closing tag too, the caret staying where it types.
        let end = harness.text.chars().count() - 1;
        set_selection(&harness.ctx, editor_id(), &(end..end));
        harness.frame(Vec::new());
        let out = harness.type_text("x");
        assert_eq!(harness.text, "<navx>\n  <p>a</p>\n</navx>");
        assert_eq!(out.selection, Some(end + 2..end + 2));
    }

    #[test]
    fn style_sheets_and_worded_blocks_are_helped_too() {
        // An abbreviation alone on a line of a block, then Tab.
        let mut harness = Harness::of("style.css", "a {\n    \n}", false);
        set_selection(&harness.ctx, editor_id(), &(8..8));
        harness.frame(Vec::new());
        harness.type_text("m10-20");
        harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "a {\n    margin: 10px 20px;\n}");
        // In the style element of a page too; and there, elements are not written.
        let page = "<style>\nbody {\n    \n}\n</style>";
        let mut harness = Harness::of("index.html", page, false);
        set_selection(&harness.ctx, editor_id(), &(19..19));
        harness.frame(Vec::new());
        harness.type_text("df");
        harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(
            harness.text,
            "<style>\nbody {\n    display: flex;\n}\n</style>"
        );
        // Outside a block the same letters are a selector: Tab indents.
        let mut harness = Harness::of("style.css", "p", false);
        set_selection(&harness.ctx, editor_id(), &(1..1));
        harness.frame(Vec::new());
        harness.key(Key::Tab, Modifiers::NONE);
        assert!(!harness.text.contains("padding"), "{}", harness.text);
        // A hyphenated property is offered, and Tab writes the rest of it.
        let mut harness = Harness::of("style.css", "a {\n    \n}", false);
        set_selection(&harness.ctx, editor_id(), &(8..8));
        harness.frame(Vec::new());
        for letter in ["f", "o", "n", "t", "-", "s", "i"] {
            harness.type_text(letter);
        }
        assert!(harness.view.completion.is_some(), "the list is open");
        harness.key(Key::ArrowDown, Modifiers::NONE);
        harness.key(Key::ArrowUp, Modifiers::NONE);
        harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "a {\n    font-size: ;\n}");
        // Where blocks end with a word: Enter indents after `then`, `end` comes back.
        let mut harness = Harness::of("a.lua", "if x then", false);
        set_selection(&harness.ctx, editor_id(), &(9..9));
        harness.frame(Vec::new());
        harness.key(Key::Enter, Modifiers::NONE);
        harness.type_text("y()");
        harness.key(Key::Enter, Modifiers::NONE);
        for letter in ["e", "n", "d"] {
            harness.type_text(letter);
        }
        assert_eq!(harness.text, "if x then\n    y()\nend");
    }

    #[test]
    fn code_is_laid_out_as_it_is_typed() {
        // Enter between the two tags of an element.
        let mut harness = Harness::of("index.html", "<body></body>", false);
        set_selection(&harness.ctx, editor_id(), &(6..6));
        harness.frame(Vec::new());
        let out = harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "<body>\n    \n</body>");
        assert_eq!(out.selection, Some(11..11));
        // An element whose closing tag is already there is not closed twice.
        let mut harness = Harness::of("index.html", "\n</body>", false);
        set_selection(&harness.ctx, editor_id(), &(0..0));
        harness.frame(Vec::new());
        harness.type_text("<body");
        harness.type_text(">");
        assert_eq!(harness.text, "<body>\n</body>");
        // `</` writes the name of the element still open, under its opening tag.
        let mut harness = Harness::of("index.html", "<div>\n    <p>x</p>\n    ", false);
        let end = harness.text.chars().count();
        set_selection(&harness.ctx, editor_id(), &(end..end));
        harness.frame(Vec::new());
        harness.type_text("<");
        let out = harness.type_text("/");
        assert_eq!(harness.text, "<div>\n    <p>x</p>\n</div>");
        assert_eq!(out.selection, Some(25..25));
        // A closing bracket typed first on its line goes under what it closes.
        let mut harness = Harness::new("fn a() {\n    if x {\n        y();\n        ");
        let end = harness.text.chars().count();
        set_selection(&harness.ctx, editor_id(), &(end..end));
        harness.frame(Vec::new());
        harness.type_text("}");
        assert_eq!(harness.text, "fn a() {\n    if x {\n        y();\n    }");
        harness.key(Key::Enter, Modifiers::NONE);
        harness.type_text("}");
        assert_eq!(harness.text, "fn a() {\n    if x {\n        y();\n    }\n}");
    }

    #[test]
    fn a_long_line_wraps_only_when_asked() {
        let text = format!("{}\nend", "word ".repeat(400));
        // The row the second line shows on: the first line takes one row, or many.
        let row = |wrap: bool| Harness::of("page.html", &text, wrap).view.index.row_of(1.0);
        assert_eq!(row(false), 1.0);
        assert!(row(true) > 4.0, "the long line takes several rows");
        // Down goes to the next row of the line, not past the line.
        let mut harness = Harness::of("page.html", &text, true);
        set_selection(&harness.ctx, editor_id(), &(0..0));
        harness.frame(Vec::new());
        let down = harness.key(Key::ArrowDown, Modifiers::NONE);
        let caret = down.selection.expect("a caret").start;
        assert!(caret > 0 && caret < 2000, "{caret}");
        // What is typed there is typed in the long line.
        harness.type_text("X");
        assert_eq!(harness.text.find('X'), Some(caret));
    }

    #[test]
    fn a_completion_is_offered_and_accepted_with_tab() {
        let mut harness = Harness::new("let counter = 0;\n");
        set_selection(&harness.ctx, editor_id(), &(17..17));
        harness.frame(Vec::new());
        for ch in ["c", "o", "u"] {
            harness.type_text(ch);
        }
        assert!(harness.view.completion.is_some(), "the list opened");
        harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "let counter = 0;\ncounter");
        assert!(harness.view.completion.is_none());

        // Without the arrows, Enter breaks the line instead of picking.
        harness.type_text(" ");
        for ch in ["c", "o", "u"] {
            harness.type_text(ch);
        }
        harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "let counter = 0;\ncounter cou\n");
    }

    #[test]
    fn a_click_puts_the_caret_in_the_text() {
        let mut harness = Harness::new("one\ntwo\nthree");
        let pos = pos2(600.0, 26.0);
        let button = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        harness.frame(vec![Event::PointerMoved(pos)]);
        harness.frame(vec![button(true)]);
        harness.frame(vec![button(false)]);
        // Right of the second line: the caret lands at its end.
        assert_eq!(harness.frame(Vec::new()).selection, Some(7..7));
    }

    #[test]
    fn the_field_of_lines_in_view_moves_selects_and_erases() {
        let mut harness = Harness::new("one two\nthree\n\nfour");
        set_selection(&harness.ctx, editor_id(), &(0..0));
        harness.frame(Vec::new());
        let shift = Modifiers::SHIFT;
        // Down keeps the column, the end of a shorter line stops it.
        harness.key(Key::End, Modifiers::NONE);
        assert_eq!(
            harness.key(Key::ArrowDown, Modifiers::NONE).selection,
            Some(13..13)
        );
        assert_eq!(
            harness.key(Key::ArrowDown, Modifiers::NONE).selection,
            Some(14..14)
        );
        assert_eq!(
            harness.key(Key::ArrowUp, Modifiers::NONE).selection,
            Some(8..8)
        );
        // Word by word, selecting.
        let word = Modifiers::COMMAND | Modifiers::SHIFT;
        assert_eq!(harness.key(Key::ArrowRight, word).selection, Some(8..13));
        harness.type_text("3");
        assert_eq!(harness.text, "one two\n3\n\nfour");
        harness.key(Key::Backspace, Modifiers::NONE);
        harness.key(Key::Backspace, Modifiers::NONE);
        assert_eq!(harness.text, "one two\n\nfour");
        assert_eq!(harness.key(Key::ArrowLeft, shift).selection, Some(6..7));
        harness.key(Key::Delete, Modifiers::NONE);
        assert_eq!(harness.text, "one tw\n\nfour");
        // Everything, then written over.
        harness.key(Key::A, Modifiers::COMMAND);
        let out = harness.type_text("x");
        assert_eq!((harness.text.as_str(), out.selection), ("x", Some(1..1)));
    }

    #[test]
    fn a_folded_block_takes_no_row_and_opens_for_the_caret() {
        let mut harness = Harness::new("fn a() {\n    one();\n    two();\n}\nfn b() {}\n");
        set_selection(&harness.ctx, editor_id(), &(0..0));
        harness.view.fold_at_caret = true;
        harness.frame(Vec::new());
        assert_eq!(harness.view.folded, [0]);
        // The second row is now the brace that closes the block: a click there lands on it.
        let row_height = 17.0;
        let pos = pos2(600.0, 4.0 + 1.5 * row_height);
        let button = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        harness.frame(vec![Event::PointerMoved(pos)]);
        harness.frame(vec![button(true)]);
        harness.frame(vec![button(false)]);
        let caret = harness.frame(Vec::new()).selection.expect("a caret").start;
        assert_eq!(
            harness.text[..caret].matches('\n').count(),
            3,
            "caret at {caret}"
        );
        assert_eq!(harness.view.folded, [0]);
        // Going up enters the block: it opens rather than hide the caret.
        harness.key(Key::ArrowUp, Modifiers::NONE);
        assert!(harness.view.folded.is_empty());
        // Typed lines above a fold push it down with its block.
        harness.text.push_str("    z();\n");
        set_selection(&harness.ctx, editor_id(), &(0..0));
        harness.frame(Vec::new());
        harness.view.folded = vec![4];
        harness.frame(Vec::new());
        harness.type_text("// top\n");
        assert_eq!(harness.view.folded, [5]);
    }

    #[test]
    fn a_rectangle_is_selected_and_written_over_line_by_line() {
        let mut harness = Harness::new("abcd\nef\nghij\n");
        set_selection(&harness.ctx, editor_id(), &(1..1));
        harness.frame(Vec::new());
        // Two lines down, two columns right: the short line gives what it has.
        for step in [(1, 0), (1, 0), (0, 2)] {
            harness.view.block_step = Some(step);
            harness.frame(Vec::new());
        }
        let block = harness.view.block.expect("a rectangle");
        assert_eq!(block_ranges(&harness.text, &block), [1..3, 6..7, 9..11]);
        assert_eq!(harness.frame(Vec::new()).selection, Some(11..11));
        harness.type_text("X");
        assert_eq!(harness.text, "aXd\neX\ngXj\n");
        // Each line keeps a caret: what is typed next goes on all of them.
        assert_eq!(harness.view.extra, [2, 6]);
        harness.type_text("Y");
        assert_eq!(harness.text, "aXYd\neXY\ngXYj\n");

        // A column past the end of a line is its end.
        let wide = Block {
            anchor: (0, 9),
            head: (1, 2),
            caret: None,
        };
        assert_eq!(block_ranges("abc\nd\n", &wide), [2..3, 5..5]);
        let mut text = "abc\nd\n".to_owned();
        assert_eq!(write_over(&mut text, &[2..3, 5..5], "--"), [4, 8]);
        assert_eq!(text, "ab--\nd--\n");
    }

    /// What a key costs in a big file, through the whole editor. `cargo test --release -p
    /// godcode typing_in_a_big -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measure, not a check"]
    fn typing_in_a_big_file_is_measured() {
        let line = "a line of plain text, as long as the lines of a log often are, or so\n";
        for megabytes in [2, 10, 50] {
            let text = line.repeat(megabytes * 1024 * 1024 / line.len());
            let mut harness = Harness::of("big.txt", &text, false);
            set_selection(&harness.ctx, editor_id(), &(0..0));
            harness.frame(Vec::new());
            let start = std::time::Instant::now();
            harness.frame(vec![Event::Text("x".to_owned())]);
            let key = start.elapsed();
            let start = std::time::Instant::now();
            harness.frame(Vec::new());
            let after = start.elapsed();
            harness.time += 5.0;
            harness.frame(Vec::new());
            let start = std::time::Instant::now();
            harness.frame(Vec::new());
            let rest = start.elapsed();
            // At the end of the text too, where finding the caret by reading would be slowest.
            let end = harness.text.chars().count();
            set_selection(&harness.ctx, editor_id(), &(end..end));
            harness.frame(Vec::new());
            let start = std::time::Instant::now();
            harness.frame(vec![Event::Text("y".to_owned())]);
            let last = start.elapsed();
            println!(
                "{megabytes} MB  a key: {key:?}, the frame after: {after:?}, at rest: {rest:?}, a key at the end: {last:?}"
            );
            assert!(harness.text.starts_with("xa line") && harness.text.ends_with('y'));
        }
    }

    #[test]
    fn a_text_changed_from_outside_is_read_again() {
        let mut harness = Harness::new("one\ntwo\n");
        harness.type_text("X");
        // Same size, same revision: nothing tells the field. The app always bumps it.
        harness.text = "fn a() {\n    b();\n}\n".to_owned();
        harness.frame(Vec::new());
        let layout = harness.view.layout.as_ref().expect("read");
        assert_eq!(layout.starts.len() - 1, 4, "the new text was read through");
        // Undo comes back through the history, which was fed while the text changed.
        let mut harness = Harness::new("");
        harness.type_text("first");
        harness.time += 5.0;
        harness.frame(Vec::new());
        harness.type_text(" second");
        harness.key(Key::Z, Modifiers::COMMAND);
        assert_eq!(harness.text, "first");
    }

    #[test]
    fn the_editor_says_where_the_selection_is() {
        let mut harness = Harness::new("one\ntwo é\nthree");
        set_selection(&harness.ctx, editor_id(), &(5..9));
        let out = harness.frame(Vec::new());
        assert_eq!(out.places, Some(((2, 2), (2, 6))));
        harness.key(Key::End, Modifiers::COMMAND);
        assert_eq!(harness.frame(Vec::new()).places, Some(((3, 6), (3, 6))));
    }

    #[test]
    fn a_plain_text_is_followed_key_by_key() {
        // Typed, erased, pasted over lines, undone: the lines are where reading says.
        let mut harness = Harness::of("notes.txt", "one\ntwo\nthree\n", false);
        set_selection(&harness.ctx, editor_id(), &(4..4));
        harness.frame(Vec::new());
        harness.type_text("2");
        harness.key(Key::End, Modifiers::NONE);
        harness.frame(vec![Event::Paste(" and\nmore\nlines".to_owned())]);
        harness.frame(Vec::new());
        harness.key(Key::Backspace, Modifiers::NONE);
        harness.key(Key::ArrowUp, Modifiers::NONE);
        harness.key(Key::Backspace, Modifiers::COMMAND);
        let out = harness.type_text("!");
        assert_eq!(harness.text, "one\n2two and\n!\nline\nthree\n");
        assert_eq!(out.selection, Some(14..14));
        let starts = |text: &str| {
            let mut starts = vec![0];
            starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
            starts.push(text.len() + 1);
            starts
        };
        let layout = harness.view.layout.as_ref().expect("read");
        assert_eq!(layout.starts, starts(&harness.text));
        // A text of code is read again whole, as before.
        let mut code = Harness::new("fn a() {}\n");
        code.type_text("// ");
        let layout = code.view.layout.as_ref().expect("read");
        assert_eq!(layout.starts, starts(&code.text));
    }

    #[test]
    fn a_hint_pushes_the_text_along_without_being_in_it() {
        let mut harness = Harness::new(&"a".repeat(60));
        let pos = pos2(400.0, 10.0);
        let click = |harness: &mut Harness| {
            let button = |pressed| Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            harness.frame(vec![Event::PointerMoved(pos)]);
            harness.frame(vec![button(true)]);
            harness.frame(vec![button(false)]);
            harness.frame(Vec::new()).selection.expect("a caret").start
        };
        let plain = click(&mut harness);
        harness.hints = vec![field::Hint {
            line: 0,
            column: 2,
            label: ": number".to_owned(),
        }];
        // The same place is now a char further left by the width of the hint, which is
        // written a little smaller than the text.
        let hinted = click(&mut harness);
        assert!((7..=8).contains(&(plain - hinted)), "{plain} {hinted}");
        // What is typed where the hint is goes in the text; the hint does not.
        set_selection(&harness.ctx, editor_id(), &(2..2));
        harness.frame(Vec::new());
        let out = harness.type_text("X");
        assert_eq!(harness.text, format!("aaX{}", "a".repeat(58)));
        assert_eq!(out.selection, Some(3..3));
    }

    #[test]
    fn the_field_of_lines_in_view_measures_only_what_shows() {
        let text: String = (0..5000).map(|line| format!("line {line}\n")).collect();
        let mut harness = Harness::new(&text);
        let out = harness.frame(Vec::new());
        assert!(out.top_line < 1.0);
        // A click lands on the line under it, far from the lines never measured.
        let pos = pos2(600.0, 300.0);
        let button = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        harness.frame(vec![Event::PointerMoved(pos)]);
        harness.frame(vec![button(true)]);
        harness.frame(vec![button(false)]);
        let caret = harness.frame(Vec::new()).selection.expect("a caret").start;
        let line = harness.text[..caret].matches('\n').count();
        assert!((10..40).contains(&line), "line {line}");
        // The end of the text is reached and shown.
        harness.key(Key::End, Modifiers::COMMAND);
        // Scrolling there takes a few frames.
        for _ in 0..30 {
            harness.frame(Vec::new());
        }
        let out = harness.frame(Vec::new());
        assert_eq!(out.selection, Some(text.len()..text.len()));
        assert!(out.top_line > 4000.0, "{}", out.top_line);
    }

    #[test]
    fn undo_restores_the_text_through_the_menu() {
        let mut harness = Harness::new("");
        harness.type_text("first");
        harness.time += 5.0;
        harness.frame(Vec::new());
        harness.type_text(" second");
        harness.time += 5.0;
        harness.frame(Vec::new());
        let selection = history(
            &harness.ctx,
            editor_id(),
            &mut harness.text,
            &mut harness.view,
            true,
        );
        assert_eq!(harness.text, "first");
        assert_eq!(selection, Some(5..5));
        history(
            &harness.ctx,
            editor_id(),
            &mut harness.text,
            &mut harness.view,
            false,
        );
        assert_eq!(harness.text, "first second");
    }
}
