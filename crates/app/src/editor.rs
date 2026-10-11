//! The code editor: a text field with syntax colours, line numbers, completion, problem
//! marks and code-aware keys.

use std::ops::Range;

use egui::text::{CCursor, CCursorRange, LayoutJob, LayoutSection, TextFormat, TextWrapping};
use egui::{
    Align, Align2, Area, Color32, Context, CornerRadius, Event, FontId, Frame, Id, Key, Margin,
    Modifiers, Order, Pos2, Rect, ScrollArea, Sense, Shape, Stroke, StrokeKind, TextEdit, Ui,
    UiBuilder, pos2, vec2,
};
use godcode_core::complete::{self, Completion, Expanded, Kind, Request, Source, Suggestion};
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
const POPUP_ROW: egui::Vec2 = vec2(400.0, 22.0);
/// What the frame of the list adds around its rows, on each side.
const POPUP_EDGE: f32 = 5.0;
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
    /// The texts of the other open files: their words are offered too.
    pub others: &'a [&'a str],
    /// The files around the one edited: their paths from its folder, with `/`.
    pub files: &'a [String],
    /// The lines (from 0) the debugger stops on.
    pub breakpoints: &'a [usize],
    /// Those of them it stops on only under a condition.
    pub conditional: &'a [usize],
    /// The line the debugged program is stopped at.
    pub stopped: Option<usize>,
    /// The lines (from 0, sorted) the tests are known to reach, and how often they ran.
    pub coverage: &'a [(usize, u64)],
    /// A number that changes whenever the text may have changed since the editor last
    /// showed it.
    pub revision: u64,
    /// What the language server writes in the lines.
    pub hints: &'a [field::Hint],
    /// What the language server says folds (see [`field::Look::folds`]).
    pub folds: Option<(u64, &'a [field::Fold])>,
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
    /// The snippet whose tab stops Tab is going through.
    walk: Option<Walk>,
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
        if items.is_empty() {
            return;
        }
        // The snippets on offer there stay, first: the server knows names, not structures.
        let shown = self.completion.take().filter(|popup| popup.caret == caret);
        let kept = shown.into_iter().flat_map(|popup| popup.items);
        let mut all: Vec<Suggestion> = kept.filter(|item| item.kind == Kind::Snippet).collect();
        // Which of them go with what is typed is told once the text is at hand (`sift`).
        all.extend(items.into_iter().map(|item| Suggestion {
            label: item.label,
            body: item.body,
            kind: item.kind,
            source: Source::Word,
            matched: Vec::new(),
            replace: caret..caret,
        }));
        self.completion = Some(Popup {
            items: all,
            selected: 0,
            caret,
            navigated: false,
            typed: None,
        });
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
            (lead < line.len()).then(|| columns_of(&line[..lead]))
        })
        .collect()
}

/// The completion list open at the caret.
struct Popup {
    /// What is offered. Of the kind `Symbol`: what a language server answered, whose
    /// source and place in the text are not told.
    items: Vec<Suggestion>,
    selected: usize,
    /// Where the caret was when the list was made; it closes when the caret leaves.
    caret: usize,
    /// The arrows were used: Enter then picks instead of breaking the line.
    navigated: bool,
    /// The start of a word that the list completes, once it was read from the text.
    typed: Option<String>,
}

impl Popup {
    /// What a language server answered is read against what is typed in `text`: those
    /// its letters are found in stay, the best first, after the snippets.
    fn sift(&mut self, text: &str) {
        let word = complete::word_start(text, self.caret)..self.caret;
        let typed = chars_of(text, &word);
        let (mut kept, mut served) = (Vec::new(), Vec::new());
        for mut item in std::mem::take(&mut self.items) {
            if item.kind != Kind::Symbol {
                kept.push(item);
            } else if let Some(found) = complete::fuzzy(typed, &item.label) {
                item.matched = found.positions;
                served.push((found.score, item));
            }
        }
        served.sort_by_key(|(score, item)| (std::cmp::Reverse(*score), item.label.chars().count()));
        kept.extend(served.into_iter().map(|(_, item)| item));
        self.items = kept;
    }
}

/// A snippet just written, whose tab stops Tab goes through one after the other.
struct Walk {
    /// The snippet, its stops where they now are in the text.
    snippet: Expanded,
    /// The stop being filled, counted in `snippet.stops`.
    step: usize,
    /// The text as the stops were last placed in it: what changed since is found against
    /// it, which no count of chars would tell from a change made elsewhere.
    seen: String,
}

impl Walk {
    /// The chars of the stop being filled.
    fn place(&self) -> Option<Range<usize>> {
        let stop = self.snippet.stops.get(self.step)?;
        stop.ranges.first().cloned()
    }

    /// Take in what changed in `text` since it was last seen: typed in the stop being
    /// filled, it is written wherever else the snippet has that stop. Returns whether
    /// the text was written in; `None` when something changed outside the stop: the
    /// snippet is left.
    fn keep_up(&mut self, text: &mut String) -> Option<bool> {
        if self.seen == *text {
            return Some(false);
        }
        let stop = self.place()?;
        let old = self.seen.as_str();
        let from = godcode_core::byte_index(old, stop.start);
        let to = from + godcode_core::byte_index(&old[from..], stop.len());
        // What the two start with in common, no further than the stop goes: a char typed
        // at its end may be the very char that follows it.
        let same = old.bytes().zip(text.bytes());
        let mut same = same.take_while(|(was, now)| was == now).count().min(to);
        while !text.is_char_boundary(same) {
            same -= 1;
        }
        if same < from {
            return None;
        }
        let (was, now) = (&old[same..], &text[same..]);
        let rest = was.bytes().rev().zip(now.bytes().rev());
        let mut rest = rest.take_while(|(was, now)| was == now).count();
        while !now.is_char_boundary(now.len() - rest) {
            rest -= 1;
        }
        let at = stop.start + old[from..same].chars().count();
        let removed = was[..was.len() - rest].chars().count();
        let added = now[..now.len() - rest].chars().count();
        if !self.snippet.follow(text, self.step, at, removed, added) {
            return None;
        }
        let twins = self.snippet.stops[self.step].ranges.len() > 1;
        self.seen.clone_from(text);
        Some(twins)
    }
}

/// The chars `chars` of `text`.
fn chars_of<'a>(text: &'a str, chars: &Range<usize>) -> &'a str {
    let start = godcode_core::byte_index(text, chars.start);
    &text[start..start + godcode_core::byte_index(&text[start..], chars.len())]
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

/// Bytes of the lines an edit at the bytes `selected` can read or change: the lines the
/// selection touches, the one above and the one below.
fn lines_around(text: &str, selected: &Range<usize>) -> Range<usize> {
    let line_of = |before: &str| before.rfind('\n').map_or(0, |found| found + 1);
    let end_of = |from: usize| {
        text[from..]
            .find('\n')
            .map_or(text.len(), |found| from + found)
    };
    let start = line_of(&text[..line_of(&text[..selected.start]).saturating_sub(1)]);
    let end = end_of(selected.end);
    start..end_of((end + 1).min(text.len()))
}

/// Apply `op` at `selection`. Returns the selection to show and whether the text changed;
/// `None` when `op` has nothing to do there.
fn apply(
    text: &mut String,
    selection: Range<usize>,
    config: &Config<'_>,
    op: Op,
) -> Option<(Range<usize>, bool)> {
    let comment = config.language.line_comment();
    if matches!(op, Op::Comment) && comment.is_none() {
        return None;
    }
    // The edit is made on a copy of the few lines it can read or change: nothing of a big
    // text is read but what leads to the caret, and what changed is told without a copy
    // of the whole.
    let start = godcode_core::byte_index(text, selection.start);
    let selected = start..start + godcode_core::byte_index(&text[start..], selection.len());
    let lines = lines_around(text, &selected);
    let first = selection.start - text[lines.start..selected.start].chars().count();
    let selection = selection.start - first..selection.end - first;
    let mut part = text[lines.clone()].to_owned();
    let part = &mut part;
    let shown = match op {
        Op::Newline => {
            let caret = edit::newline(part, selection, config.indent, config.language);
            caret..caret
        }
        Op::Indent => edit::indent(part, selection, config.indent),
        Op::Outdent => edit::outdent(part, selection, config.indent),
        Op::Comment => edit::toggle_comment(part, selection, comment?),
        Op::Duplicate => edit::duplicate_lines(part, selection),
        Op::DeleteLine => edit::delete_lines(part, selection),
        Op::Join => edit::join_lines(part, selection),
        Op::ToggleCase => edit::toggle_case(part, selection),
        Op::MoveUp | Op::MoveDown => edit::move_lines(part, selection, matches!(op, Op::MoveUp))?,
    };
    let changed = *part != text[lines.clone()];
    if changed {
        text.replace_range(lines, part);
    }
    Some((shown.start + first..shown.end + first, changed))
}

/// Apply `op` at the selection of the editor `id`. Returns whether the text changed.
pub fn run(ctx: &Context, id: Id, text: &mut String, config: &Config<'_>, op: Op) -> bool {
    let Some(selection) = selection_of(ctx, id) else {
        return false;
    };
    let Some((selection, changed)) = apply(text, selection, config, op) else {
        return false;
    };
    set_selection(ctx, id, &selection);
    changed
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

/// The char that closes what `opener` opens: a bracket, or a quote, which closes itself.
fn closer_of(opener: char) -> Option<char> {
    Some(match opener {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '"' | '\'' | '`' => opener,
        _ => return None,
    })
}

/// Whether `quote` opens a string in `language`, as its colours tell. Not `'` in Rust,
/// where it starts a lifetime as often as a char.
fn opens_string(quote: char, language: &Language) -> bool {
    if quote == '\'' && language.name == "Rust" {
        return false;
    }
    let probe = format!("{quote}x{quote}");
    let spans = syntax::highlight(&probe, language);
    matches!(spans.first(), Some(span) if span.token == syntax::Token::String)
}

/// What typing `'` or `` ` ``, or in markup the `>` that ends a tag, does at `caret`: it
/// steps over the same char when that one is next, as [`paired`] does for the brackets
/// and `"`, whoever wrote it; and a quote that opens a string comes with the one that
/// closes it. Returns the caret after it and whether the text changed; `None` when the
/// char is to be typed as any other.
fn quoted(
    text: &mut String,
    caret: usize,
    typed: char,
    config: &Config<'_>,
) -> Option<(usize, bool)> {
    let tag = typed == '>' && config.language.is_markup();
    if !tag && !matches!(typed, '\'' | '`') {
        return None;
    }
    let at = godcode_core::byte_index(text, caret);
    let after = text[at..].chars().next();
    if after == Some(typed) {
        return Some((caret + 1, false));
    }
    if tag || !opens_string(typed, config.language) {
        return None;
    }
    // After a word, a quote or a backslash it closes, escapes or is an apostrophe.
    let before = text[..at].chars().next_back();
    let opens = after.is_none_or(|ch| ch.is_whitespace() || ")]},;".contains(ch))
        && !before.is_some_and(|ch| ch.is_alphanumeric() || "_\"'`\\".contains(ch));
    if !opens {
        return None;
    }
    text.insert_str(at, &format!("{typed}{typed}"));
    Some((caret + 1, true))
}

/// A bracket or a quote typed with text selected goes around it instead of over it.
/// Returns the selection, which is still what it was; `None` for any other char.
fn surrounded(text: &mut String, selection: &Range<usize>, typed: char) -> Option<Range<usize>> {
    let closer = closer_of(typed)?;
    let start = godcode_core::byte_index(text, selection.start);
    let end = start + godcode_core::byte_index(&text[start..], selection.len());
    text.insert(end, closer);
    text.insert(start, typed);
    Some(selection.start + 1..selection.end + 1)
}

/// Typing a bracket or a quote adds its partner, or puts the selection between the two;
/// typing the partner steps over it.
fn pair(ui: &Ui, id: Id, text: &mut String, config: &Config<'_>) -> bool {
    if is_prose(config.language) {
        return false;
    }
    let Some(selection) = selection_of(ui.ctx(), id) else {
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
    let done = if selection.is_empty() {
        let caret = selection.start;
        quoted(text, caret, typed, config)
            .or_else(|| paired(text, caret, typed, config))
            .map(|(caret, changed)| (caret..caret, changed))
    } else {
        surrounded(text, &selection, typed).map(|shown| (shown, true))
    };
    let Some((shown, changed)) = done else {
        return false;
    };
    // The char is dealt with: the text field is not to type it too.
    let taken = typed.to_string();
    ui.input_mut(|input| {
        input
            .events
            .retain(|event| !matches!(event, Event::Text(text) if *text == taken));
    });
    set_selection(ui.ctx(), id, &shown);
    changed
}

/// The columns the white space `space` takes.
fn columns_of(space: &str) -> usize {
    let columns = |byte| if byte == b'\t' { TAB_COLUMNS } else { 1 };
    space.bytes().map(columns).sum()
}

/// What Backspace does at `caret` when it takes more than a char: between a bracket or a
/// quote and the one that closes it, both go; in the white space that leads a line, the
/// caret goes back to the tab stop before it. Returns the caret after it; `None` when one
/// char is to go, as anywhere.
fn erased(text: &mut String, caret: usize, config: &Config<'_>) -> Option<usize> {
    let at = godcode_core::byte_index(text, caret);
    let before = text[..at].chars().next_back()?;
    let after = text[at..].chars().next();
    let tag = before == '<' && config.language.is_markup();
    let closer = if tag { Some('>') } else { closer_of(before) };
    if closer.is_some() && closer == after && !is_prose(config.language) {
        // Brackets and quotes take one byte each.
        text.replace_range(at - 1..at + 1, "");
        return Some(caret - 1);
    }
    let line = text[..at].rfind('\n').map_or(0, |found| found + 1);
    let lead = &text[line..at];
    if !lead.chars().all(|ch| ch == ' ' || ch == '\t') {
        return None;
    }
    let width = columns_of(config.indent).max(1);
    let past = match columns_of(lead) % width {
        0 => width,
        past => past,
    };
    let spaces = lead.len() - lead.trim_end_matches(' ').len();
    let gone = past.min(spaces);
    if gone < 2 {
        return None;
    }
    text.replace_range(at - gone..at, "");
    Some(caret - gone)
}

/// What Tab writes with the caret in the white space that leads a line, between two tab
/// stops: the spaces it takes to reach the next one. Returns the caret after it; `None`
/// when a whole level is to be written, as anywhere.
fn tab_stop(text: &mut String, caret: usize, config: &Config<'_>) -> Option<usize> {
    if config.indent.contains('\t') {
        return None;
    }
    let at = godcode_core::byte_index(text, caret);
    let line = text[..at].rfind('\n').map_or(0, |found| found + 1);
    let lead = &text[line..at];
    if !lead.chars().all(|ch| ch == ' ' || ch == '\t') {
        return None;
    }
    let width = config.indent.len().max(1);
    let past = columns_of(lead) % width;
    if past == 0 {
        return None;
    }
    text.insert_str(at, &" ".repeat(width - past));
    Some(caret + width - past)
}

/// The markers of the comments that document code in `language`.
fn doc_markers(language: &Language) -> &'static [&'static str] {
    match (language.name, language.line_comment()) {
        ("R", _) => &["#'"],
        ("Nim" | "GDScript", _) => &["##"],
        ("Lua", _) => &["---"],
        (_, Some("//")) => &["///", "//!"],
        _ => &[],
    }
}

/// The marker the comment `code` starts with, when the line after it is to start with it
/// too: the one of a doc comment, always; the one of a plain comment only when `rest`,
/// what follows the caret, has text that would not be a comment without it.
fn comment_marker(code: &str, rest: &str, language: &Language) -> Option<&'static str> {
    let plain = language.line_comment()?;
    // A row of the last char of a marker is a rule drawn across the file, not a comment.
    let documents = |marker: &&&str| {
        let text = code.strip_prefix(**marker);
        text.is_some_and(|text| !text.starts_with(&marker[marker.len() - 1..]))
    };
    let doc = doc_markers(language).iter().find(documents).copied();
    doc.or_else(|| (code.starts_with(plain) && !rest.trim().is_empty()).then_some(plain))
}

/// Whether the line after `above` goes on a `/* … */` comment whose lines start with a
/// star: read up from it, no further than where that comment opens.
fn in_block_comment(above: &str) -> bool {
    for line in above.rsplit('\n').skip(1) {
        let line = line.trim_start();
        if line.contains("*/") {
            return false;
        }
        if line.starts_with("/*") {
            return true;
        }
        if !line.starts_with('*') {
            return false;
        }
    }
    false
}

/// What Enter does at `caret` when it is not only a line broken as code is: a line of
/// white space alone is left empty, and a comment goes on on the next line (a doc comment,
/// a plain one cut in two, a `/* … */` one with a star). Returns the caret after it;
/// `None` when the line is to be broken as anywhere.
fn continued(text: &mut String, caret: usize, config: &Config<'_>) -> Option<usize> {
    let at = godcode_core::byte_index(text, caret);
    let line = text[..at].rfind('\n').map_or(0, |found| found + 1);
    let end = text[at..].find('\n').map_or(text.len(), |found| at + found);
    let (before, rest) = (&text[line..at], &text[at..end]);
    let blank = |part: &str| part.chars().all(|ch| ch == ' ' || ch == '\t');
    if end > line && blank(before) && blank(rest) {
        // The white space goes down with the caret: the line left has none.
        text.insert(line, '\n');
        return Some(caret + 1);
    }
    let code = before.trim_start_matches([' ', '\t']);
    let lead = &before[..before.len() - code.len()];
    // The white space after the caret is not taken along: the marker brings its own.
    let spaces = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let closes = rest[spaces..].starts_with("*/");
    let open = !code.contains("*/");
    let (written, below) = if open && code.starts_with("/*") && !is_prose(config.language) {
        // Between the two ends of a comment, what closes it goes on its own line.
        let below = if closes {
            format!("\n{lead} ")
        } else {
            String::new()
        };
        (format!("\n{lead} * "), below)
    } else if open && code.starts_with('*') && in_block_comment(&text[..line]) {
        let star = if closes { "" } else { "* " };
        (format!("\n{lead}{star}"), String::new())
    } else {
        let marker = comment_marker(code, rest, config.language)?;
        (format!("\n{lead}{marker} "), String::new())
    };
    let caret = caret + written.chars().count();
    text.replace_range(at..at + spaces, &format!("{written}{below}"));
    Some(caret)
}

/// Enter at the selection of the editor `id`: see [`continued`], or else [`edit::newline`].
/// Returns whether the text changed.
fn break_line(ctx: &Context, id: Id, text: &mut String, config: &Config<'_>) -> bool {
    let caret = selection_of(ctx, id).filter(Range::is_empty);
    let Some(caret) = caret.and_then(|caret| continued(text, caret.start, config)) else {
        return run(ctx, id, text, config, Op::Newline);
    };
    set_selection(ctx, id, &(caret..caret));
    true
}

/// Open a line under the line of the caret, or `above` it, without breaking that line:
/// the new one is indented as Enter at the end of the line before it would. Returns
/// whether the text changed.
fn open_line(ctx: &Context, id: Id, text: &mut String, config: &Config<'_>, above: bool) -> bool {
    let Some(selection) = selection_of(ctx, id) else {
        return false;
    };
    let caret = if above {
        selection.start
    } else {
        selection.end
    };
    let at = godcode_core::byte_index(text, caret);
    let line = text[..at].rfind('\n').map_or(0, |found| found + 1);
    let end = text[at..].find('\n').map_or(text.len(), |found| at + found);
    let column = text[line..at].chars().count();
    let from = match (above, line) {
        (false, _) => caret + text[at..end].chars().count(),
        // Above the first line there is no line to break: one is written, indented alike.
        (true, 0) => {
            let lead = text.len() - text.trim_start_matches([' ', '\t']).len();
            text.insert_str(0, &format!("{}\n", &text[..lead]));
            set_selection(ctx, id, &(lead..lead));
            return true;
        }
        (true, _) => caret - column - 1,
    };
    set_selection(ctx, id, &(from..from));
    break_line(ctx, id, text, config)
}

/// Select the word the caret is in or by, when nothing is selected.
fn select_word(ctx: &Context, id: Id, text: &str) {
    let Some(caret) = selection_of(ctx, id).filter(Range::is_empty) else {
        return;
    };
    let word = field::word_at(text, caret.start);
    if !word.is_empty() {
        set_selection(ctx, id, &word);
    }
}

/// `key` was pressed alone, with no modifier held.
fn struck(ui: &Ui, key: Key) -> bool {
    let alone = |event: &Event| {
        matches!(event, Event::Key { key: pressed, pressed: true, modifiers, .. }
            if *pressed == key && modifiers.is_none())
    };
    ui.input(|input| input.events.iter().any(alone))
}

/// What typing `typed` at `caret` does when the char is one that code is helped with: a
/// bracket, a quote, the end of a tag, the last letter of a word that ends a block.
/// Returns the caret after it and whether the text changed; `None` when the char is to be
/// typed as any other. Only the lines around the caret are read, but to find the bracket
/// or the tag that the char closes.
fn paired(
    text: &mut String,
    caret: usize,
    typed: char,
    config: &Config<'_>,
) -> Option<(usize, bool)> {
    let markup = config.language.is_markup();
    let closers = config.language.block_closers();
    // Any other char is typed as it is whatever stands around it: the text is not read.
    let helped = "()[]{}\"".contains(typed)
        || (markup && matches!(typed, '>' | '/'))
        || (typed.is_alphabetic() && !closers.is_empty());
    if !helped {
        return None;
    }
    let at = godcode_core::byte_index(text, caret);
    if typed == '>' && markup {
        let closing = complete::closing_tag(&text[..at], &text[at..])?;
        text.insert_str(at, &format!(">{closing}"));
        return Some((caret + 1, true));
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
    if typed == '/' && markup {
        let before = text[..at].strip_suffix('<')?;
        let (name, opened) = complete::unclosed_tag(before)?;
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
        return Some((caret + name.chars().count() + 2, true));
    }
    let before = text[..at].chars().next_back();
    let after = text[at..].chars().next();
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
    let line = text[..at].rfind('\n').map_or(0, |found| found + 1);
    // A word that ends a block (`end`, `fi`), typed first on its line, goes back one level.
    if typed.is_alphabetic() && !closers.is_empty() {
        let written = text[line..at].trim_start();
        let lead = at - written.len();
        let word = format!("{written}{typed}");
        let goes_on = after.is_some_and(|ch| ch.is_alphanumeric() || ch == '_');
        let unit = config.indent;
        if closers.contains(&word.as_str()) && !goes_on && text[line..lead].ends_with(unit) {
            text.insert(at, typed);
            text.replace_range(lead - unit.len()..lead, "");
            return Some((caret + 1 - unit.chars().count(), true));
        }
    }
    // A closing bracket typed first on its line goes under the line of the one it closes.
    if matches!(typed, ')' | ']' | '}') && !steps_over && at > line && blank(&text[line..at]) {
        text.insert(at, typed);
        let pair = check::matching_bracket(text, config.language, caret);
        let Some(opener) = pair.map(|(one, other)| one.min(other)) else {
            // It closes nothing: typed as it is.
            text.remove(at);
            return None;
        };
        let indent = indent_at(text, godcode_core::byte_index(text, opener));
        let after = caret - text[line..at].chars().count() + indent.chars().count() + 1;
        text.replace_range(line..at, &indent);
        return Some((after, true));
    }
    if !steps_over && !opens {
        return None;
    }
    if !steps_over {
        text.insert_str(at, &format!("{typed}{closer}"));
    }
    Some((caret + 1, !steps_over))
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
    // Nor while the stops of a snippet are gone through, which Tab is then for.
    let short = tab && (markup || styles) && view.walk.is_none();
    if let (true, false, Some(caret)) = (short, chosen, caret) {
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
        } else if pressed(Modifiers::NONE, Key::PageDown) {
            popup.selected = (popup.selected + POPUP_ROWS).min(popup.items.len() - 1);
            popup.navigated = true;
        } else if pressed(Modifiers::NONE, Key::PageUp) {
            popup.selected = popup.selected.saturating_sub(POPUP_ROWS);
            popup.navigated = true;
        } else {
            // Enter picks as Tab does, unless the one in front is the very word typed
            // and writes nothing more, or nothing is typed of it yet (the list came
            // with a dot, which may as well end a sentence), or it merely has the
            // letters typed somewhere: the line is then broken.
            let chosen = popup.items.get(popup.selected);
            let more = chosen.is_some_and(|item| {
                let typed = chars_of(text, &replaced(text, popup.caret, item));
                !typed.is_empty() && item.body != typed && !scattered(item)
            });
            let enter = (popup.navigated || more) && pressed(Modifiers::NONE, Key::Enter);
            if enter || pressed(Modifiers::NONE, Key::Tab) {
                accept(ui.ctx(), id, text, config, view);
                return true;
            }
        }
    }
    // A snippet was written: Tab goes to its next stop, Shift+Tab back to the one before,
    // Escape leaves it. Shifted first: a plain pattern also matches the shifted key.
    if view.walk.is_some() {
        if pressed(Modifiers::NONE, Key::Escape) {
            view.walk = None;
            return false;
        }
        let back = pressed(Modifiers::SHIFT, Key::Tab);
        if back || pressed(Modifiers::NONE, Key::Tab) {
            step_walk(ui.ctx(), id, view, back);
            return false;
        }
    }
    if pair(ui, id, text, config) {
        return true;
    }
    // Not while the keys are commands (Vim): these write.
    if !config.block_caret {
        // Shifted first, as below.
        let above = pressed(Modifiers::COMMAND | Modifiers::SHIFT, Key::Enter);
        if above || pressed(Modifiers::COMMAND, Key::Enter) {
            view.completion = None;
            return open_line(ui.ctx(), id, text, config, above);
        }
        if pressed(Modifiers::ALT, Key::J) {
            select_word(ui.ctx(), id, text);
            return false;
        }
    }
    // Backspace and Tab, where they do more than erase a char or write a level.
    let caret = selection_of(ui.ctx(), id).filter(Range::is_empty);
    if let Some(caret) = caret.map(|caret| caret.start) {
        let moved = if struck(ui, Key::Backspace) {
            erased(text, caret, config).map(|caret| (Key::Backspace, caret))
        } else if struck(ui, Key::Tab) {
            tab_stop(text, caret, config).map(|caret| (Key::Tab, caret))
        } else {
            None
        };
        if let Some((key, caret)) = moved {
            pressed(Modifiers::NONE, key);
            set_selection(ui.ctx(), id, &(caret..caret));
            view.completion = None;
            return true;
        }
    }
    // Shifted first: a plain pattern also matches the shifted key.
    let op = if pressed(Modifiers::SHIFT, Key::Tab) {
        Op::Outdent
    } else if pressed(Modifiers::NONE, Key::Tab) {
        Op::Indent
    } else if pressed(Modifiers::NONE, Key::Enter) {
        view.completion = None;
        return break_line(ui.ctx(), id, text, config);
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
    let Some(item) = popup.items.get(popup.selected) else {
        return;
    };
    let over = replaced(text, popup.caret, item);
    let written = complete::write_stops(text, over, &item.body, config.indent);
    // What its first stop holds is selected: typing writes over it.
    set_selection(ctx, id, &written.first());
    view.focus = true;
    view.accepted = Some(item.completion());
    // A folder of a path: what it holds comes next.
    view.complete_now = item.source == Source::Path && item.label.ends_with('/');
    // With stops, Tab now goes through them; with none, a snippet being gone through
    // goes on: what was picked was written in one of its stops.
    if !written.stops.is_empty() {
        view.walk = Some(Walk {
            snippet: written,
            step: 0,
            seen: text.clone(),
        });
    }
}

/// The chars of `text` that `item`, offered at `caret`, is written over: what was typed
/// of it.
fn replaced(text: &str, caret: usize, item: &Suggestion) -> Range<usize> {
    match item.kind {
        Kind::Symbol => complete::word_start(text, caret)..caret,
        _ => item.replace.clone(),
    }
}

/// Go to the stop after the one being filled of the snippet gone through, or `back` to
/// the one before: what it holds is selected. After the last one the caret goes where the
/// snippet ends, and the snippet is left.
fn step_walk(ctx: &Context, id: Id, view: &mut View, back: bool) {
    let Some(walk) = &mut view.walk else {
        return;
    };
    walk.step = match back {
        true => walk.step.saturating_sub(1),
        false => walk.step + 1,
    };
    let place = walk.place().unwrap_or_else(|| walk.snippet.end.clone());
    if walk.step >= walk.snippet.stops.len() {
        view.walk = None;
    }
    set_selection(ctx, id, &place);
    view.completion = None;
    view.reveal = true;
}

/// After the editor showed: what was typed in the stop being filled of the snippet gone
/// through is written in its twins, and the snippet is left when the text changed
/// elsewhere or the caret is no longer in that stop. Returns whether the text was
/// written in.
fn follow_walk(ctx: &Context, id: Id, text: &mut String, view: &mut View) -> bool {
    let Some(walk) = &mut view.walk else {
        return false;
    };
    let written = walk.keep_up(text);
    let inside = walk.place().zip(selection_of(ctx, id));
    let inside = inside
        .is_some_and(|(stop, selected)| stop.start <= selected.start && selected.end <= stop.end);
    if written.is_none() || !inside {
        view.walk = None;
    }
    written == Some(true)
}

/// Open, refresh or close the completion list after this frame's typing. `changed`: the
/// text is not what it was; `wrote`: because something was typed.
fn refresh_completion(
    ui: &Ui,
    text: &str,
    config: &Config<'_>,
    view: &mut View,
    caret: Option<usize>,
    (changed, wrote): (bool, bool),
) {
    let manual = std::mem::take(&mut view.complete_now);
    // A big text is not searched for its words at every key.
    let Some(caret) = caret.filter(|_| text.len() <= LARGE_BYTES) else {
        view.completion = None;
        return;
    };
    let erased = ui.input(|input| input.key_pressed(Key::Backspace));
    let was_open = view.completion.is_some();
    // With several carets, what is picked would be written at one of them only.
    let typed = view.extra.is_empty() && (wrote || (erased && changed && was_open));
    if !(manual || typed) {
        let left = |popup: &Popup| popup.caret != caret;
        if view.completion.as_ref().is_some_and(left) {
            view.completion = None;
        }
        return;
    }
    let files: Vec<&str> = config.files.iter().map(String::as_str).collect();
    let mut items = complete::suggest(&Request {
        text,
        caret,
        language: config.language,
        snippets: config.snippets,
        others: config.others,
        files: &files,
    });
    let before = &text[..godcode_core::byte_index(text, caret)];
    // What comes with nothing typed: the files of a path, the names that follow a dot.
    let led = items.first().is_some_and(|item| match item.source {
        Source::Path => true,
        Source::Member => asks_members(before, config.language),
        _ => false,
    });
    // Two letters are found somewhere in most names: those are not worth a list.
    if !manual {
        items.retain(|item| item.replace.len() > 2 || !scattered(item));
    }
    let typed = typed_word(text, caret, config.language).chars().count();
    let wanted = manual || led || typed >= 2 || (was_open && typed > 0);
    view.completion = (wanted && !items.is_empty()).then_some(Popup {
        items,
        selected: 0,
        caret,
        navigated: false,
        typed: None,
    });
}

/// Whether the `.` or the `::` that ends `before`, the text up to the caret, or comes
/// before the word that ends it, asks for the names that follow one: not the dot of a
/// number, nor the one that ends a sentence, in prose or in a comment.
fn asks_members(before: &str, language: &Language) -> bool {
    let line = &before[before.rfind('\n').map_or(0, |found| found + 1)..];
    let commented = language
        .line_comment()
        .is_some_and(|marker| line.contains(marker));
    let named = line.trim_end_matches(|ch: char| ch.is_alphanumeric() || ch == '_');
    let named = named.trim_end_matches(['.', ':']);
    let after_name = named.ends_with(|ch: char| !ch.is_ascii_digit() && !ch.is_whitespace());
    after_name && !commented && !is_prose(language) && !language.is_markup()
}

/// Where a completion comes from, in a word.
fn source_word(item: &Suggestion) -> &'static str {
    match (item.kind, item.source) {
        (Kind::Symbol, _) => "symbol",
        (_, Source::Snippet) => "snippet",
        (_, Source::Keyword) => "keyword",
        (_, Source::Builtin) => "built-in",
        (_, Source::Member) => "member",
        (_, Source::Path) => "path",
        (_, Source::Word) => "word",
        (_, Source::Other) => "other file",
    }
}

/// Whether the letters typed are merely somewhere in the name of `item`: not one after
/// the other, nor where its words start. Never so for a file of a path or a name that
/// follows a dot: those are few, and asked for.
fn scattered(item: &Suggestion) -> bool {
    if matches!(item.source, Source::Path | Source::Member) {
        return false;
    }
    let label: Vec<char> = item.label.chars().collect();
    // A word starts after what is no letter, and at a capital after a small letter.
    let starts_word = |at: usize| {
        let before = at.checked_sub(1).and_then(|before| label.get(before));
        match (before, label.get(at)) {
            (None, _) => true,
            (Some(before), Some(ch)) => {
                (!before.is_alphanumeric() && ch.is_alphanumeric())
                    || (before.is_lowercase() && ch.is_uppercase())
            }
            (Some(_), None) => false,
        }
    };
    let mut last = None;
    !item.matched.iter().all(|at| {
        let follows = last.replace(*at).is_some_and(|last| last + 1 == *at);
        follows || starts_word(*at)
    })
}

/// What a snippet writes, on one line: its text, the tab stops left out of it.
fn preview(body: &str) -> String {
    let written = complete::expand(body, "\t", "").text;
    written.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The word being typed that ends at `caret`: what the completions there complete.
fn typed_word(text: &str, caret: usize, language: &Language) -> String {
    // In markup and in style sheets a name goes on over its hyphens.
    let hyphens = language.is_markup() || language.name == "CSS";
    let named = |ch: &char| ch.is_alphanumeric() || *ch == '_' || (hyphens && *ch == '-');
    let before = &text[..godcode_core::byte_index(text, caret)];
    let word: Vec<char> = before.chars().rev().take_while(named).collect();
    word.into_iter().rev().collect()
}

/// Where a list of `size` goes for the `caret`, a rectangle on screen: under it, or above
/// it when it would run off the bottom of `screen`; and never off its sides.
fn popup_pos(caret: Rect, size: egui::Vec2, screen: Rect) -> Pos2 {
    let (under, above) = (caret.max.y + 2.0, caret.min.y - 2.0 - size.y);
    let y = if under + size.y > screen.max.y && above >= screen.min.y {
        above
    } else {
        under
    };
    pos2(caret.min.x.min(screen.max.x - size.x).max(screen.min.x), y)
}

/// Draw the completion list by the `caret`, a rectangle on screen. Returns the item
/// clicked.
fn show_completion(
    ui: &Ui,
    id: Id,
    popup: &Popup,
    caret: Rect,
    palette: &Palette,
) -> Option<usize> {
    let mut clicked = None;
    let first = popup.selected.saturating_sub(POPUP_ROWS - 1);
    let rows = popup.items.len().min(POPUP_ROWS) as f32;
    let size = vec2(POPUP_ROW.x, POPUP_ROW.y * rows) + egui::Vec2::splat(2.0 * POPUP_EDGE);
    let font = FontId::monospace(13.0);
    Area::new(id.with("completion"))
        .order(Order::Foreground)
        .fixed_pos(popup_pos(caret, size, ui.ctx().content_rect()))
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
                        let kind = painter.text(
                            pos2(rect.max.x - 8.0, rect.center().y),
                            Align2::RIGHT_CENTER,
                            source_word(item),
                            FontId::proportional(11.0),
                            palette.text_dim,
                        );
                        // The letters typed stand out in the name they are found in.
                        let mut job = LayoutJob::default();
                        for (at, ch) in item.label.chars().enumerate() {
                            let ink = if item.matched.contains(&at) {
                                palette.accent_text
                            } else {
                                palette.text
                            };
                            let format = TextFormat::simple(font.clone(), ink);
                            job.append(ch.encode_utf8(&mut [0; 4]), 0.0, format);
                        }
                        let label = ui.fonts_mut(|fonts| fonts.layout_job(job));
                        let left = pos2(rect.min.x + 8.0, rect.center().y);
                        let after = left.x + label.size().x + 12.0;
                        let top = left - vec2(0.0, label.size().y / 2.0);
                        painter.galley(top, label, palette.text);
                        // What a snippet writes, as far as there is room before its kind.
                        let written = match item.kind {
                            Kind::Snippet => preview(&item.body),
                            _ => String::new(),
                        };
                        let room = rect.with_min_x(after).with_max_x(kind.min.x - 8.0);
                        if written != item.label && room.width() > 0.0 {
                            painter.with_clip_rect(room).text(
                                pos2(after, rect.center().y),
                                Align2::LEFT_CENTER,
                                written,
                                FontId::monospace(11.5),
                                palette.text_dim,
                            );
                        }
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
    // The text changed since the editor last showed it: the snippet gone through may not
    // be where it was.
    if view.walk.as_ref().is_some_and(|walk| walk.seen != *text) {
        view.walk = None;
    }
    // Looked for before the keys take what they deal with: a quote, a bracket.
    let wrote = ui.input(|input| {
        let mut events = input.events.iter();
        events.any(|event| matches!(event, Event::Text(typed) if !typed.is_empty()))
    });
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

    let unit_columns = columns_of(config.indent).max(1);
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
                folds: config.folds,
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
            // A colour written in a page or a style sheet is underlined with itself.
            let coloured = config.language.is_markup()
                || matches!(config.language.name, "CSS" | "JavaScript" | "TypeScript");
            if coloured && text.len() <= LARGE_BYTES {
                let (above, below) = (clip.min.y - origin.y - row_height, clip.max.y - origin.y);
                for (line, _) in geometry.between(above, below) {
                    let starts = &geometry.index().starts;
                    let written = text.get(starts[line]..starts[line + 1] - 1).unwrap_or("");
                    // Most lines name none: they are not read char by char.
                    if !written.contains('#') && !written.contains("rgb") {
                        continue;
                    }
                    let first = geometry.index().start_of(line);
                    for (chars, [red, green, blue, opacity]) in syntax::colours(written) {
                        let from = geometry.pos(first + chars.start).translate(origin);
                        let to = geometry.pos(first + chars.end).translate(origin);
                        // Cut by the end of a row, it is told on its first row.
                        if to.min.y > from.min.y {
                            continue;
                        }
                        let bar = Rect::from_min_max(
                            pos2(from.min.x, from.max.y - 3.0),
                            pos2(to.min.x, from.max.y),
                        );
                        let ink = Color32::from_rgba_unmultiplied(red, green, blue, opacity);
                        ui.painter().rect_filled(bar, 1.0, ink);
                    }
                }
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
                jump,
                pointed,
                focused,
            )
        });
    ui.allocate_rect(full, Sense::hover());
    let (geometry, origin, caret, selection, caret_index, caret_screen, jump, pointed, focused) =
        scrolled.inner;

    refresh_completion(
        ui,
        text,
        config,
        view,
        caret_index,
        (changed, changed && wrote),
    );
    // A list that was not read against the text yet: one a language server answered.
    if let Some(popup) = view
        .completion
        .as_mut()
        .filter(|popup| popup.typed.is_none())
    {
        popup.typed = Some(typed_word(text, popup.caret, config.language));
        popup.sift(text);
    }
    let empty = |popup: &Popup| popup.items.is_empty();
    if view.completion.as_ref().is_some_and(empty) {
        view.completion = None;
    }
    let clicked = match (&view.completion, caret_screen) {
        (Some(popup), Some(foot)) => {
            let caret = Rect::from_min_max(foot - vec2(0.0, row_height), foot);
            show_completion(ui, id, popup, caret, &palette)
        }
        _ => None,
    };
    let mut written = false;
    if let Some(index) = clicked {
        if let Some(popup) = &mut view.completion {
            popup.selected = index;
        }
        accept(ui.ctx(), id, text, config, view);
        written = true;
    }
    written |= follow_walk(ui.ctx(), id, text, view);
    if written {
        // Written after the field showed: it has not seen it, and shows it next.
        view.index.forget();
        changed = true;
        ui.ctx().request_repaint();
    }
    // Escape closes the list, leaves one caret or the snippet gone through: it does not
    // take the keyboard away then.
    let held = view.completion.is_some()
        || !view.extra.is_empty()
        || view.block.is_some()
        || view.walk.is_some();
    if focused && held {
        ui.memory_mut(|memory| memory.set_focus_lock_filter(id, field::own_keys(true)));
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
        // A line the tests ran is green at the edge, one they never reached is red.
        if let Ok(known) = config
            .coverage
            .binary_search_by_key(&index, |(line, _)| *line)
        {
            let bar = Rect::from_min_max(
                pos2(gutter.min.x, top),
                pos2(gutter.min.x + 2.0, top + row_height),
            );
            let color = match config.coverage[known].1 {
                0 => ERROR_COLOR,
                _ => ADDED_COLOR,
            };
            painter.rect_filled(bar, 0.0, color);
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
        /// What the completions are made of besides the text: the snippets of the user,
        /// the texts of the other open files, the files around this one.
        snippets: Vec<(String, String)>,
        others: Vec<String>,
        files: Vec<String>,
        /// Bumped when the text changed, as the app does; and the size it had then.
        revision: u64,
        bytes: usize,
    }

    fn editor_id() -> Id {
        Id::new("editor")
    }

    /// What the tests show a file with.
    fn config_of<'a>(
        file: &'static str,
        wrap: bool,
        hints: &'a [field::Hint],
        revision: u64,
    ) -> Config<'a> {
        Config {
            language: Language::of(std::path::Path::new(file)),
            font_size: 14.0,
            indent: "    ",
            marks: &[],
            current_mark: None,
            squiggles: &[],
            changes: &[],
            occurrences: &[],
            wrap,
            minimap: true,
            sticky: true,
            note: None,
            snippets: &[],
            others: &[],
            files: &[],
            breakpoints: &[],
            conditional: &[],
            stopped: None,
            hints,
            folds: None,
            coverage: &[],
            block_caret: false,
            revision,
        }
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
                snippets: Vec::new(),
                others: Vec::new(),
                files: Vec::new(),
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
            let others: Vec<&str> = self.others.iter().map(String::as_str).collect();
            let config = Config {
                snippets: &self.snippets,
                others: &others,
                files: &self.files,
                ..config_of(self.file, self.wrap, &self.hints, self.revision)
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
    fn a_word_typed_whole_then_tab_writes_the_structure_it_names() {
        // In Rust, the file of `Harness::new`.
        let mut harness = Harness::new("");
        harness.frame(Vec::new());
        for letter in ["m", "a", "i", "n"] {
            harness.type_text(letter);
        }
        assert_eq!(harness.text, "main");
        let out = harness.key(Key::Tab, Modifiers::NONE);
        assert!(harness.text.starts_with("fn main() {"), "{}", harness.text);
        assert!(harness.text.ends_with('}'), "{}", harness.text);
        // The caret waits inside the function, on its own indented line.
        let caret = out.selection.map(|selection| selection.start);
        assert_eq!(
            caret,
            Some(
                "fn main() {
    "
                .chars()
                .count()
            )
        );
    }

    #[test]
    fn cut_without_a_selection_takes_the_line_of_the_caret() {
        let mut harness = Harness::new("one\ntwo\nthree");
        set_selection(&harness.ctx, editor_id(), &(5..5));
        harness.frame(Vec::new());
        let out = harness.frame(vec![Event::Cut]);
        assert_eq!(harness.text, "one\nthree");
        // The caret waits where the line was.
        assert_eq!(out.selection, Some(4..4));
        // Copying changes nothing, selection or not.
        harness.frame(vec![Event::Copy]);
        assert_eq!(harness.text, "one\nthree");
        // The last line has no end of its own: the one before it goes with it.
        set_selection(&harness.ctx, editor_id(), &(7..7));
        harness.frame(Vec::new());
        harness.frame(vec![Event::Cut]);
        assert_eq!(harness.text, "one");
        // With a selection, what is selected alone is cut.
        set_selection(&harness.ctx, editor_id(), &(0..2));
        harness.frame(Vec::new());
        harness.frame(vec![Event::Cut]);
        assert_eq!(harness.text, "e");
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

        // Enter picks too, as in other editors.
        harness.type_text(" ");
        for ch in ["c", "o", "u"] {
            harness.type_text(ch);
        }
        harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "let counter = 0;\ncounter counter");
        assert!(harness.view.completion.is_none());
        // With nothing on offer it breaks the line, as ever.
        harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "let counter = 0;\ncounter counter\n");
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

    /// What each editing key costs in a big file: through the whole editor, a frame per
    /// key, then the edits alone. `cargo test --release -p godcode the_editing_keys --
    /// --ignored --nocapture`.
    #[test]
    #[ignore = "a measure, not a check"]
    fn the_editing_keys_are_measured_in_big_files() {
        use std::time::{Duration, Instant};
        const ROUNDS: usize = 7;
        // The best of a few: the machine is seldom still.
        let best = |times: Vec<Duration>| times.into_iter().min().unwrap_or_default();
        let code = "fn name(value: usize) -> usize {\n    let total = value + 1;\n    \
                    if total > 3 {\n        return total;\n    }\n    total\n}\n\n";
        let prose = "a line of plain text, as long as the lines of a log often are, or so\n";
        let cases = [
            ("2 MB code", "big.rs", code, 2),
            ("40 MB code", "big.rs", code, 40),
            ("40 MB plain", "big.txt", prose, 40),
        ];
        for (name, file, line, megabytes) in cases {
            let text = line.repeat(megabytes * 1024 * 1024 / line.len());
            // The texts are ASCII: a byte is a char. The middle is the end of a line.
            let half = &text[..text.len() / 2];
            let middle = half
                .rfind(";\n")
                .map_or_else(|| half.rfind('\n').unwrap_or(0), |at| at + 1);
            let mut harness = Harness::of(file, &text, false);
            for (place, caret) in [("middle", middle), ("end", text.len())] {
                // A frame with `events`, the last `selected` chars before the caret selected.
                let mut frame = |selected: usize, events: &dyn Fn() -> Vec<Event>| {
                    let times = (0..ROUNDS).map(|_| {
                        harness.text.clone_from(&text);
                        harness.revision += 1;
                        harness.frame(Vec::new());
                        set_selection(&harness.ctx, editor_id(), &(caret - selected..caret));
                        harness.frame(Vec::new());
                        let start = Instant::now();
                        harness.frame(events());
                        let took = start.elapsed();
                        harness.frame(Vec::new());
                        took
                    });
                    best(times.collect())
                };
                let typed = |typed: &'static str| move || vec![Event::Text(typed.to_owned())];
                let key = |key: Key, modifiers: Modifiers| {
                    move || {
                        vec![Event::Key {
                            key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers,
                        }]
                    }
                };
                let (plain, shift) = (Modifiers::NONE, Modifiers::SHIFT);
                println!("{name}, caret at the {place}: a frame with");
                println!("  nothing      {:?}", frame(0, &Vec::new));
                println!("  a letter     {:?}", frame(0, &typed("x")));
                println!("  a bracket    {:?}", frame(0, &typed("(")));
                println!("  a quote      {:?}", frame(0, &typed("\"")));
                println!("  Enter        {:?}", frame(0, &key(Key::Enter, plain)));
                println!("  Tab          {:?}", frame(0, &key(Key::Tab, plain)));
                println!("  Shift+Tab    {:?}", frame(0, &key(Key::Tab, shift)));
                println!("  Backspace    {:?}", frame(0, &key(Key::Backspace, plain)));
                println!("  over 5 chars {:?}", frame(5, &typed("x")));

                // What the app does for Ctrl+/ and its like: the edit, outside a frame.
                let config = config_of(file, false, &[], 0);
                let mut ran = |op: Op| {
                    let times = (0..ROUNDS).map(|_| {
                        harness.text.clone_from(&text);
                        set_selection(&harness.ctx, editor_id(), &(caret..caret));
                        let start = Instant::now();
                        run(&harness.ctx, editor_id(), &mut harness.text, &config, op);
                        start.elapsed()
                    });
                    best(times.collect())
                };
                println!("{name}, caret at the {place}: `run` alone with");
                println!("  Enter        {:?}", ran(Op::Newline));
                println!("  Tab          {:?}", ran(Op::Indent));
                println!("  Shift+Tab    {:?}", ran(Op::Outdent));
                println!("  Ctrl+/       {:?}", ran(Op::Comment));
                println!("  duplicate    {:?}", ran(Op::Duplicate));
                println!("  delete line  {:?}", ran(Op::DeleteLine));
                println!("  move up      {:?}", ran(Op::MoveUp));
                println!("  join         {:?}", ran(Op::Join));

                // The edits themselves, and what they are made of.
                type Edit<'a> = &'a dyn Fn(&mut String);
                let mut work = text.clone();
                let mut alone = |edit: Edit| {
                    let times = (0..ROUNDS).map(|_| {
                        work.clone_from(&text);
                        let start = Instant::now();
                        edit(&mut work);
                        start.elapsed()
                    });
                    best(times.collect())
                };
                let language = config.language;
                let at = caret..caret;
                let parts: [(&str, Edit); 15] = [
                    ("a letter", &|text| {
                        paired(text, caret, 'x', &config);
                    }),
                    ("a bracket", &|text| {
                        paired(text, caret, '(', &config);
                    }),
                    ("a quote", &|text| {
                        paired(text, caret, '"', &config);
                    }),
                    ("byte_index", &|text| {
                        std::hint::black_box(godcode_core::byte_index(text, caret));
                    }),
                    ("a copy", &|text| {
                        std::hint::black_box(text.clone());
                    }),
                    ("chars().nth", &|text| {
                        std::hint::black_box(text.chars().nth(caret.saturating_sub(1)));
                    }),
                    ("an insert", &|text| text.insert(caret, 'x')),
                    ("newline", &|text| {
                        edit::newline(text, at.clone(), "    ", language);
                    }),
                    ("indent", &|text| {
                        edit::indent(text, at.clone(), "    ");
                    }),
                    ("outdent", &|text| {
                        edit::outdent(text, at.clone(), "    ");
                    }),
                    ("comment", &|text| {
                        edit::toggle_comment(text, at.clone(), "//");
                    }),
                    ("duplicate", &|text| {
                        edit::duplicate_lines(text, at.clone());
                    }),
                    ("delete line", &|text| {
                        edit::delete_lines(text, at.clone());
                    }),
                    ("move up", &|text| {
                        edit::move_lines(text, at.clone(), true);
                    }),
                    ("join", &|text| {
                        edit::join_lines(text, at.clone());
                    }),
                ];
                println!("{name}, caret at the {place}: alone,");
                for (part, edit) in parts {
                    println!("  {part:<12} {:?}", alone(edit));
                }
            }
        }
    }

    /// `apply` as it was: the edit made in the whole text, a copy of which tells whether
    /// it changed.
    fn old_apply(
        text: &mut String,
        selection: Range<usize>,
        config: &Config<'_>,
        op: Op,
    ) -> Option<(Range<usize>, bool)> {
        let before = text.clone();
        let selection = match op {
            Op::Newline => {
                let caret = edit::newline(text, selection, config.indent, config.language);
                caret..caret
            }
            Op::Indent => edit::indent(text, selection, config.indent),
            Op::Outdent => edit::outdent(text, selection, config.indent),
            Op::Comment => edit::toggle_comment(text, selection, config.language.line_comment()?),
            Op::Duplicate => edit::duplicate_lines(text, selection),
            Op::DeleteLine => edit::delete_lines(text, selection),
            Op::Join => edit::join_lines(text, selection),
            Op::ToggleCase => edit::toggle_case(text, selection),
            Op::MoveUp | Op::MoveDown => {
                edit::move_lines(text, selection, matches!(op, Op::MoveUp))?
            }
        };
        Some((selection, *text != before))
    }

    /// `paired` as it was: the text read from its start to find the chars at the caret.
    fn old_paired(
        text: &mut String,
        caret: usize,
        typed: char,
        config: &Config<'_>,
    ) -> Option<(usize, bool)> {
        if typed == '>' && config.language.is_markup() {
            let at = godcode_core::byte_index(text, caret);
            let closing = complete::closing_tag(&text[..at], &text[at..])?;
            text.insert_str(at, &format!(">{closing}"));
            return Some((caret + 1, true));
        }
        let blank = |lead: &str| lead.chars().all(|ch| ch == ' ' || ch == '\t');
        let indent_at = |text: &str, byte: usize| {
            let line = text[..byte].rfind('\n').map_or(0, |found| found + 1);
            let lead = text[line..]
                .chars()
                .take_while(|ch| *ch == ' ' || *ch == '\t');
            lead.collect::<String>()
        };
        if typed == '/' && config.language.is_markup() {
            let at = godcode_core::byte_index(text, caret);
            let before = text[..at].strip_suffix('<')?;
            let (name, opened) = complete::unclosed_tag(before)?;
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
            return Some((caret, true));
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
            && (typed != '"'
                || !before.is_some_and(|ch| ch.is_alphanumeric() || "\"\\".contains(ch)));
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
                text.insert(at, typed);
                text.replace_range(lead - unit.len()..lead, "");
                return Some((caret + 1 - unit.chars().count(), true));
            }
        }
        if matches!(typed, ')' | ']' | '}') && !steps_over {
            let at = godcode_core::byte_index(text, caret);
            let line = text[..at].rfind('\n').map_or(0, |found| found + 1);
            if at > line && blank(&text[line..at]) {
                text.insert(at, typed);
                let pair = check::matching_bracket(text, config.language, caret);
                let Some(opener) = pair.map(|(one, other)| one.min(other)) else {
                    text.remove(at);
                    return None;
                };
                let indent = indent_at(text, godcode_core::byte_index(text, opener));
                let after = caret - text[line..at].chars().count() + indent.chars().count() + 1;
                text.replace_range(line..at, &indent);
                return Some((after, true));
            }
        }
        if !steps_over && !opens {
            return None;
        }
        if !steps_over {
            let at = godcode_core::byte_index(text, caret);
            text.insert_str(at, &format!("{typed}{closer}"));
        }
        Some((caret + 1, !steps_over))
    }

    /// Numbers that look random and are the same at every run.
    struct Dice(u64);

    impl Dice {
        fn below(&mut self, limit: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % limit as u64) as usize
        }

        /// A text made of what the keys look at: brackets, quotes, tags, words that end
        /// blocks, indents, empty lines, line ends of both kinds, chars of every width.
        fn text(&mut self) -> String {
            const PIECES: [&str; 44] = [
                "\n",
                "\n",
                "\n",
                "\r\n",
                "\t",
                "    ",
                "  ",
                " ",
                "a",
                "B",
                "é",
                "😀",
                "€",
                "word",
                "let x = 1;",
                "{",
                "}",
                "(",
                ")",
                "[",
                "]",
                ":",
                "// ",
                " // why",
                "# ",
                "<div>",
                "</div>",
                "<br>",
                "<p class=\"a\">",
                "<",
                "</",
                "if x then",
                "en",
                "els",
                "f",
                "do",
                "\"",
                "\\",
                ";",
                "_",
                ",",
                "\n    ",
                "\n        ",
                "<ul><li>",
            ];
            let most = if self.below(7) == 0 { 4 } else { 30 };
            let pieces = self.below(most);
            (0..pieces)
                .map(|_| PIECES[self.below(PIECES.len())])
                .collect()
        }

        /// A place among `chars` chars: often the start or the end.
        fn place(&mut self, chars: usize) -> usize {
            match self.below(6) {
                0 => 0,
                1 | 2 => chars,
                _ => self.below(chars + 1),
            }
        }
    }

    const FILES: [&str; 7] = ["a.rs", "a.html", "a.lua", "a.rb", "a.sh", "a.css", "a.txt"];
    const UNITS: [&str; 3] = ["    ", "  ", "\t"];

    #[test]
    fn an_edit_made_on_the_lines_around_the_selection_is_the_one_made_on_the_text() {
        let ops = [
            Op::Newline,
            Op::Indent,
            Op::Outdent,
            Op::Comment,
            Op::Duplicate,
            Op::DeleteLine,
            Op::MoveUp,
            Op::MoveDown,
            Op::Join,
            Op::ToggleCase,
        ];
        let mut dice = Dice(0x2545_F491_4F6C_DD1D);
        for round in 0..20_000 {
            let text = dice.text();
            let chars = text.chars().count();
            let start = dice.place(chars);
            let end = match dice.below(3) {
                0 => start,
                1 => (start + dice.below(6)).min(chars),
                _ => start + dice.below(chars - start + 1),
            };
            let config = Config {
                indent: UNITS[round % 3],
                ..config_of(FILES[round % 7], false, &[], 0)
            };
            let op = ops[dice.below(ops.len())];
            let (mut now, mut then) = (text.clone(), text.clone());
            let new = apply(&mut now, start..end, &config, op);
            let old = old_apply(&mut then, start..end, &config, op);
            let told = format!("op {} at {start}..{end} in {text:?}", op as usize);
            assert_eq!(new, old, "{told}");
            assert_eq!(now, then, "{told}");
        }
    }

    #[test]
    fn a_char_typed_is_helped_as_it_was_when_the_text_was_read_from_its_start() {
        let typed: Vec<char> = "()[]{}\"'>/dieéx <,;".chars().collect();
        let mut dice = Dice(0x9E37_79B9_7F4A_7C15);
        let mut helped = 0;
        for round in 0..40_000 {
            let text = dice.text();
            let caret = dice.place(text.chars().count());
            let config = Config {
                indent: UNITS[round % 3],
                ..config_of(FILES[round % 6], false, &[], 0)
            };
            let typed = typed[dice.below(typed.len())];
            let (mut now, mut then) = (text.clone(), text.clone());
            let new = paired(&mut now, caret, typed, &config);
            let old = old_paired(&mut then, caret, typed, &config);
            assert_eq!(new, old, "{typed:?} at {caret} in {text:?}");
            assert_eq!(now, then, "{typed:?} at {caret} in {text:?}");
            helped += usize::from(new.is_some());
        }
        assert!(helped > 4000, "only {helped} chars were helped");
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

    /// The editor showing `text` as the file `file`, with `selection` selected.
    fn editing(file: &'static str, text: &str, selection: Range<usize>) -> Harness {
        let mut harness = Harness::of(file, text, false);
        set_selection(&harness.ctx, editor_id(), &selection);
        harness.frame(Vec::new());
        harness
    }

    /// Ctrl+Z, long enough after what was done for it to be a change of its own.
    fn undo(harness: &mut Harness) {
        harness.time += 5.0;
        harness.frame(Vec::new());
        harness.key(Key::Z, Modifiers::COMMAND);
    }

    #[test]
    fn a_closer_typed_where_it_already_is_is_stepped_over() {
        let mut harness = editing("a.js", "", 0..0);
        // Each opener comes with its closer, and the closer typed next goes over it.
        let pairs = [
            ("(", ")"),
            ("[", "]"),
            ("{", "}"),
            ("\"", "\""),
            ("'", "'"),
            ("`", "`"),
        ];
        for (opener, closer) in pairs {
            harness.type_text(opener);
            harness.type_text(closer);
        }
        assert_eq!(harness.text, "()[]{}\"\"''``");
        assert_eq!(harness.frame(Vec::new()).selection, Some(12..12));
        // One that was there before, typed by hand, is stepped over just the same.
        let mut harness = editing("a.js", "f(x)", 3..3);
        let out = harness.type_text(")");
        assert_eq!((harness.text.as_str(), out.selection), ("f(x)", Some(4..4)));
        // An apostrophe in a word opens nothing.
        let mut harness = editing("a.js", "// don", 6..6);
        harness.type_text("'");
        assert_eq!(harness.text, "// don'");
        // In Rust a `'` may start a lifetime: it comes alone, and is stepped over still.
        let mut harness = editing("main.rs", "&", 1..1);
        harness.type_text("'");
        assert_eq!(harness.text, "&'");
        let mut harness = editing("main.rs", "'a'", 2..2);
        let out = harness.type_text("'");
        assert_eq!((harness.text.as_str(), out.selection), ("'a'", Some(3..3)));
        // The end of a tag that is already there.
        let mut harness = editing("index.html", "<a href=\"x\">", 11..11);
        let out = harness.type_text(">");
        assert_eq!(harness.text, "<a href=\"x\">");
        assert_eq!(out.selection, Some(12..12));
    }

    #[test]
    fn backspace_between_an_empty_pair_takes_both() {
        for pair in ["()", "[]", "{}", "\"\"", "''", "``"] {
            let mut harness = editing("a.js", &format!("a{pair}b"), 2..2);
            let out = harness.key(Key::Backspace, Modifiers::NONE);
            assert_eq!((harness.text.as_str(), out.selection), ("ab", Some(1..1)));
            // In one step to take back.
            undo(&mut harness);
            assert_eq!(harness.text, format!("a{pair}b"));
        }
        // A pair with something in it loses its opener alone.
        let mut harness = editing("a.js", "(x)", 1..1);
        harness.key(Key::Backspace, Modifiers::NONE);
        assert_eq!(harness.text, "x)");
        // An empty tag is a pair in markup, not in code.
        let mut harness = editing("index.html", "a<>", 2..2);
        harness.key(Key::Backspace, Modifiers::NONE);
        assert_eq!(harness.text, "a");
        let mut harness = editing("main.rs", "a<>", 2..2);
        harness.key(Key::Backspace, Modifiers::NONE);
        assert_eq!(harness.text, "a>");
    }

    #[test]
    fn a_bracket_or_a_quote_typed_over_a_selection_goes_around_it() {
        let mut harness = editing("a.js", "let a = one;", 8..11);
        let out = harness.type_text("(");
        assert_eq!(harness.text, "let a = (one);");
        assert_eq!(out.selection, Some(9..12), "what was selected still is");
        harness.time += 5.0;
        let out = harness.type_text("\"");
        assert_eq!(harness.text, "let a = (\"one\");");
        assert_eq!(out.selection, Some(10..13));
        // Each is one step to take back.
        undo(&mut harness);
        assert_eq!(harness.text, "let a = (one);");
        // Any other char is written over the selection, as ever.
        let mut harness = editing("a.js", "let a = one;", 8..11);
        harness.type_text(")");
        assert_eq!(harness.text, "let a = );");
    }

    #[test]
    fn backspace_and_tab_in_the_leading_white_space_go_by_tab_stops() {
        let mut harness = editing("main.rs", "      x", 6..6);
        // From between two stops to the stop before, then a whole level.
        let out = harness.key(Key::Backspace, Modifiers::NONE);
        assert_eq!(
            (harness.text.as_str(), out.selection),
            ("    x", Some(4..4))
        );
        harness.time += 5.0;
        let out = harness.key(Key::Backspace, Modifiers::NONE);
        assert_eq!((harness.text.as_str(), out.selection), ("x", Some(0..0)));
        undo(&mut harness);
        assert_eq!(harness.text, "    x", "a level comes back in one step");
        // After text, a space is a char like the others.
        let mut harness = editing("main.rs", "a    ", 5..5);
        harness.key(Key::Backspace, Modifiers::NONE);
        assert_eq!(harness.text, "a   ");
        // Tab goes to the next stop: three spaces from the second column, four from a stop.
        let mut harness = editing("main.rs", "  x", 1..1);
        let out = harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(
            (harness.text.as_str(), out.selection),
            ("     x", Some(4..4))
        );
        let out = harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "         x");
        assert_eq!(out.selection, Some(8..8));
    }

    #[test]
    fn home_goes_to_the_first_letter_of_the_line_then_to_its_start() {
        let mut harness = editing("main.rs", "one\n    let a;", 13..13);
        let home = |harness: &mut Harness, modifiers| harness.key(Key::Home, modifiers).selection;
        assert_eq!(home(&mut harness, Modifiers::NONE), Some(8..8));
        assert_eq!(home(&mut harness, Modifiers::NONE), Some(4..4));
        assert_eq!(home(&mut harness, Modifiers::NONE), Some(8..8));
        // With Shift, what is passed over is selected.
        harness.key(Key::End, Modifiers::NONE);
        assert_eq!(home(&mut harness, Modifiers::SHIFT), Some(8..14));
        assert_eq!(home(&mut harness, Modifiers::SHIFT), Some(4..14));
    }

    #[test]
    fn a_line_is_opened_under_or_above_the_caret_without_breaking_its_line() {
        // Ctrl+Enter in the middle of a line: the line stays whole.
        let mut harness = editing("main.rs", "fn a() {\n    one();\n}", 15..15);
        let out = harness.key(Key::Enter, Modifiers::COMMAND);
        assert_eq!(harness.text, "fn a() {\n    one();\n    \n}");
        assert_eq!(out.selection, Some(24..24));
        undo(&mut harness);
        assert_eq!(harness.text, "fn a() {\n    one();\n}", "in one step");
        // Under a line that opens a block, the new line is one level in.
        let mut harness = editing("main.rs", "fn a() {\n}", 3..3);
        let out = harness.key(Key::Enter, Modifiers::COMMAND);
        assert_eq!(harness.text, "fn a() {\n    \n}");
        assert_eq!(out.selection, Some(13..13));
        // Ctrl+Shift+Enter: above, indented as Enter after the line before would.
        let above = Modifiers::COMMAND | Modifiers::SHIFT;
        let mut harness = editing("main.rs", "fn a() {\n    two();\n}", 15..15);
        let out = harness.key(Key::Enter, above);
        assert_eq!(harness.text, "fn a() {\n    \n    two();\n}");
        assert_eq!(out.selection, Some(13..13));
        // Above the first line, indented as it is.
        let mut harness = editing("main.rs", "    x", 5..5);
        let out = harness.key(Key::Enter, above);
        assert_eq!(
            (harness.text.as_str(), out.selection),
            ("    \n    x", Some(4..4))
        );
        // Not while the keys are commands (Vim): the same key, given straight to `keys`.
        let ctrl_enter = |commands: bool| {
            let mut harness = editing("main.rs", "one", 1..1);
            let config = Config {
                block_caret: commands,
                ..config_of("main.rs", false, &[], 0)
            };
            let _ = harness.ctx.clone().run_ui(RawInput::default(), |ui| {
                ui.memory_mut(|memory| memory.request_focus(editor_id()));
                let event = Event::Key {
                    key: Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::COMMAND,
                };
                ui.input_mut(|input| input.events.push(event));
                keys(
                    ui,
                    editor_id(),
                    &mut harness.text,
                    &config,
                    &mut harness.view,
                );
            });
            harness.text
        };
        assert_eq!(ctrl_enter(true), "one");
        assert_eq!(ctrl_enter(false), "one\n");
    }

    #[test]
    fn alt_j_selects_the_word_at_the_caret() {
        let mut harness = editing("main.rs", "let alpha = beta;", 6..6);
        let out = harness.key(Key::J, Modifiers::ALT);
        assert_eq!(
            (harness.text.as_str(), out.selection),
            ("let alpha = beta;", Some(4..9))
        );
        // With a selection it has nothing to do yet: the other carets hold no selection.
        let out = harness.key(Key::J, Modifiers::ALT);
        assert_eq!(out.selection, Some(4..9));
        assert!(harness.view.extra.is_empty());
        // By no word, nothing is selected.
        let mut harness = editing("main.rs", "a  b", 2..2);
        assert_eq!(harness.key(Key::J, Modifiers::ALT).selection, Some(2..2));
    }

    #[test]
    fn a_block_pasted_takes_the_indentation_of_the_line_it_is_pasted_on() {
        let page = "fn a() {\n    if b {\n        \n    }\n}";
        // From elsewhere, copied from its first letter: its last line tells how deep it was.
        let mut harness = editing("main.rs", page, 28..28);
        harness.frame(vec![Event::Paste("if x {\r\n    y();\r\n}".to_owned())]);
        let out = harness.frame(Vec::new());
        let block = "        if x {\n            y();\n        }";
        assert_eq!(
            harness.text,
            format!("fn a() {{\n    if b {{\n{block}\n    }}\n}}")
        );
        assert_eq!(
            out.selection,
            Some(61..61),
            "the caret ends after the block"
        );
        // Taken back in one step, and alone: what was typed just before stays.
        let mut harness = editing("main.rs", page, 28..28);
        harness.type_text("z");
        harness.frame(vec![Event::Paste("if x {\n    y();\n}".to_owned())]);
        harness.key(Key::Z, Modifiers::COMMAND);
        assert_eq!(harness.text, "fn a() {\n    if b {\n        z\n    }\n}");
        // Copied here from the middle of a line: where it comes from is known.
        let text = "    one(\n        two,\n    three\nfn b() {\n        \n}";
        let mut harness = editing("main.rs", text, 4..21);
        harness.frame(vec![Event::Copy]);
        set_selection(&harness.ctx, editor_id(), &(49..49));
        harness.frame(Vec::new());
        harness.frame(vec![Event::Paste("one(\n        two,".to_owned())]);
        assert!(
            harness
                .text
                .ends_with("fn b() {\n        one(\n            two,\n}"),
            "{}",
            harness.text
        );
        // A text from elsewhere that tells nothing of where it was is written as it is.
        let mut harness = editing("a.yaml", "    ", 4..4);
        harness.frame(vec![Event::Paste("- a: 1\n  b: 2".to_owned())]);
        assert_eq!(harness.text, "    - a: 1\n  b: 2");
    }

    #[test]
    fn a_whole_line_pasted_goes_above_the_line_of_the_caret() {
        // Copied with nothing selected: the line of the caret, whole.
        let mut harness = editing("main.rs", "one\ntwo\nthree", 5..5);
        harness.frame(vec![Event::Copy]);
        set_selection(&harness.ctx, editor_id(), &(10..10));
        harness.frame(Vec::new());
        harness.frame(vec![Event::Paste("two\n".to_owned())]);
        let out = harness.frame(Vec::new());
        assert_eq!(harness.text, "one\ntwo\ntwo\nthree");
        assert_eq!(out.selection, Some(14..14), "the caret stays in its word");
        undo(&mut harness);
        assert_eq!(harness.text, "one\ntwo\nthree", "in one step");
        // From elsewhere, a text that ends with a line end is taken for whole lines.
        harness.frame(vec![Event::Paste("zero\n".to_owned())]);
        assert_eq!(harness.text, "one\ntwo\nzero\nthree");
        // Selected and copied here, the same text goes where the caret is.
        let mut harness = editing("main.rs", "one\ntwo\nthree", 4..8);
        harness.frame(vec![Event::Copy]);
        set_selection(&harness.ctx, editor_id(), &(1..1));
        harness.frame(Vec::new());
        harness.frame(vec![Event::Paste("two\n".to_owned())]);
        assert_eq!(harness.text, "otwo\nne\ntwo\nthree");
        // Over a selection, a whole line takes its place.
        let mut harness = editing("main.rs", "one\ntwo", 4..7);
        harness.frame(vec![Event::Paste("six\n".to_owned())]);
        assert_eq!(harness.text, "one\nsix\n");
    }

    #[test]
    fn enter_goes_on_with_the_comment_it_is_in() {
        // A doc comment goes on, from its end too.
        let mut harness = editing("main.rs", "    /// one", 11..11);
        let out = harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "    /// one\n    /// ");
        assert_eq!(out.selection, Some(20..20));
        let mut harness = editing("main.rs", "//! one", 7..7);
        harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "//! one\n//! ");
        let mut harness = editing("a.lua", "--- one", 7..7);
        harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "--- one\n--- ");
        // A plain one ends at its end, and goes on when it is cut in two.
        let mut harness = editing("main.rs", "// one two", 10..10);
        harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "// one two\n");
        let mut harness = editing("main.rs", "// one two", 6..6);
        let out = harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(
            (harness.text.as_str(), out.selection),
            ("// one\n// two", Some(10..10))
        );
        undo(&mut harness);
        assert_eq!(harness.text, "// one two", "in one step");
        // A rule drawn with the marker is no doc comment.
        let mut harness = editing("main.rs", "//////", 6..6);
        harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "//////\n");
        // Between the two ends of a block comment, then on each of its lines.
        let mut harness = editing("a.js", "/** */", 3..3);
        let out = harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(
            (harness.text.as_str(), out.selection),
            ("/**\n * \n */", Some(7..7))
        );
        harness.type_text("x");
        harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "/**\n * x\n * \n */");
        // A star that multiplies is not one.
        let mut harness = editing("a.js", "a\n* b", 5..5);
        harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "a\n* b\n");
    }

    #[test]
    fn enter_leaves_no_white_space_on_a_line_it_leaves_empty() {
        let mut harness = editing("main.rs", "fn a() {\n    one();\n}", 19..19);
        harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "fn a() {\n    one();\n    \n}");
        // Left without a word written, the line keeps nothing; the next one is indented.
        harness.time += 5.0;
        let out = harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "fn a() {\n    one();\n\n    \n}");
        assert_eq!(out.selection, Some(25..25));
        undo(&mut harness);
        assert_eq!(harness.text, "fn a() {\n    one();\n    \n}");
        // Opening a line under it leaves it empty too.
        harness.key(Key::Enter, Modifiers::COMMAND);
        assert_eq!(harness.text, "fn a() {\n    one();\n\n    \n}");
    }

    /// A file where `abc` starts more names than the list shows at once, `abc` typed
    /// at its end.
    fn completing() -> Harness {
        let names: Vec<String> = (0..12).map(|number| format!("abcde{number:02}")).collect();
        let text = format!("{}\n", names.join(" "));
        let end = text.chars().count();
        let mut harness = editing("main.rs", &text, end..end);
        for letter in ["a", "b", "c"] {
            harness.type_text(letter);
        }
        harness
    }

    #[test]
    fn the_completion_list_is_walked_by_rows_and_by_pages() {
        let mut harness = completing();
        let chosen = |harness: &Harness| harness.view.completion.as_ref().map(|list| list.selected);
        let count = harness
            .view
            .completion
            .as_ref()
            .map_or(0, |list| list.items.len());
        assert!(count > POPUP_ROWS, "{count} completions");
        // Up from the first one is the last one, down from it the first again.
        harness.key(Key::ArrowUp, Modifiers::NONE);
        assert_eq!(chosen(&harness), Some(count - 1));
        harness.key(Key::ArrowDown, Modifiers::NONE);
        assert_eq!(chosen(&harness), Some(0));
        // A page is what shows at once; the ends stop it.
        harness.key(Key::PageDown, Modifiers::NONE);
        assert_eq!(chosen(&harness), Some(POPUP_ROWS));
        harness.key(Key::PageDown, Modifiers::NONE);
        assert_eq!(chosen(&harness), Some(count - 1));
        harness.key(Key::PageUp, Modifiers::NONE);
        assert_eq!(chosen(&harness), Some(count - 1 - POPUP_ROWS));
        harness.key(Key::PageUp, Modifiers::NONE);
        assert_eq!(chosen(&harness), Some(0));
        // The caret did not move: what was typed is what the list completes.
        let list = harness.view.completion.as_ref().expect("the list is open");
        assert_eq!(list.typed.as_deref(), Some("abc"));
    }

    #[test]
    fn escape_closes_the_list_and_leaves_the_keyboard_to_the_editor() {
        let mut harness = completing();
        let out = harness.key(Key::Escape, Modifiers::NONE);
        assert!(harness.view.completion.is_none());
        assert!(out.focused, "the editor still has the keyboard");
        harness.type_text("!");
        assert!(harness.text.ends_with("abc!"), "{}", harness.text);
        // With several carets too: one is left, in the editor.
        harness.view.extra = vec![0];
        harness.frame(Vec::new());
        let out = harness.key(Key::Escape, Modifiers::NONE);
        assert!(harness.view.extra.is_empty() && out.focused);
        // With neither, Escape gives the keyboard up, as before.
        assert!(!harness.key(Key::Escape, Modifiers::NONE).focused);
    }

    #[test]
    fn the_completion_list_stays_in_the_window() {
        let list = |harness: &Harness| {
            let id = editor_id().with("completion");
            harness.ctx.memory(|memory| memory.area_rect(id))
        };
        // At the top of the window it hangs under the caret.
        let mut harness = completing();
        let caret = harness.frame(Vec::new()).caret_pos.expect("a caret");
        let rect = list(&harness).expect("the list shows");
        assert!(rect.min.y >= caret.y, "{rect:?} under {caret:?}");
        // At the bottom there is no room under: it stands on the line of the caret.
        let lines = "\n".repeat(34);
        let mut harness = editing("main.rs", &format!("abcde01 abcde02{lines}"), 49..49);
        for letter in ["a", "b", "c"] {
            harness.type_text(letter);
        }
        let caret = harness.frame(Vec::new()).caret_pos.expect("a caret");
        let rect = list(&harness).expect("the list shows");
        assert!(
            caret.y > 560.0 && rect.max.y < caret.y,
            "{rect:?} over {caret:?}"
        );
        assert!(rect.min.y >= 0.0, "{rect:?}");
        // A place worked out alone: off the right edge it is pushed back in.
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 600.0));
        let caret = Rect::from_min_max(pos2(880.0, 100.0), pos2(880.0, 117.0));
        assert_eq!(
            popup_pos(caret, vec2(410.0, 98.0), screen),
            pos2(490.0, 119.0)
        );
    }

    /// The names on offer, in order.
    fn offered(harness: &Harness) -> Vec<&str> {
        let items = harness.view.completion.iter().flat_map(|list| &list.items);
        items.map(|item| item.label.as_str()).collect()
    }

    fn type_letters(harness: &mut Harness, letters: &str) -> Output {
        let typed = letters.chars().map(|letter| letter.to_string());
        let shown = typed.map(|letter| harness.type_text(&letter));
        shown.last().expect("letters to type")
    }

    /// `text` with the caret at its end, and a snippet of the user that has the same
    /// stop twice, written there: `left + left = sum;`.
    fn walking(text: &str) -> Harness {
        let end = text.chars().count();
        let mut harness = editing("main.rs", text, end..end);
        let body = "${1:left} + ${1:left} = ${2:sum};$0";
        harness.snippets = vec![("pair".to_owned(), body.to_owned())];
        type_letters(&mut harness, "pair");
        assert_eq!(offered(&harness).first(), Some(&"pair"));
        harness.key(Key::Tab, Modifiers::NONE);
        assert!(
            harness.text.ends_with("left + left = sum;"),
            "{}",
            harness.text
        );
        harness
    }

    #[test]
    fn letters_typed_find_a_name_by_where_its_words_start() {
        let mut harness = editing("main.rs", "fn get_text_bounds() {}\n", 24..24);
        type_letters(&mut harness, "gtb");
        let list = harness.view.completion.as_ref().expect("the list is open");
        assert_eq!(list.items[0].label, "get_text_bounds");
        assert_eq!(list.items[0].matched, [0, 4, 9]);
        harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "fn get_text_bounds() {}\nget_text_bounds");
    }

    #[test]
    fn two_letters_merely_found_in_a_name_do_not_open_the_list() {
        let mut harness = editing("main.rs", "let length = 1;\n", 16..16);
        type_letters(&mut harness, "gt");
        assert!(harness.view.completion.is_none(), "{:?}", offered(&harness));
        // A third one makes it worth offering.
        harness.type_text("h");
        assert_eq!(offered(&harness), ["length"]);
        // Found that way it is Tab's to pick: Enter breaks the line, as it was meant to.
        harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "let length = 1;\ngth\n");
        harness.key(Key::Backspace, Modifiers::NONE);
        harness.key(Key::Backspace, Modifiers::NONE);
        harness.type_text("h");
        harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "let length = 1;\nlength");
        // Two that start it, or its words, are enough.
        let mut harness = editing("main.rs", "let length = 1;\n", 16..16);
        type_letters(&mut harness, "le");
        assert!(
            offered(&harness).contains(&"length"),
            "{:?}",
            offered(&harness)
        );
    }

    #[test]
    fn a_word_of_another_open_file_is_offered_after_those_of_this_one() {
        let mut harness = editing("main.rs", "let counter_near = 0;\n", 22..22);
        harness.others = vec!["let counter_far = 1;".to_owned()];
        type_letters(&mut harness, "cou");
        assert_eq!(offered(&harness)[..2], ["counter_near", "counter_far"]);
        let list = harness.view.completion.as_ref().expect("the list is open");
        let words: Vec<&str> = list.items.iter().map(source_word).collect();
        assert_eq!(words[..2], ["word", "other file"]);
        harness.key(Key::ArrowDown, Modifiers::NONE);
        harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "let counter_near = 0;\ncounter_far");
    }

    #[test]
    fn a_dot_opens_the_list_of_the_names_that_follow_one() {
        let text = "point.width = 1;\npoint";
        let mut harness = editing("main.rs", text, 22..22);
        harness.type_text(".");
        assert_eq!(offered(&harness), ["width"]);
        let list = harness.view.completion.as_ref().expect("the list is open");
        assert_eq!(source_word(&list.items[0]), "member");
        harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "point.width = 1;\npoint.width");
        // With nothing typed of it, Enter breaks the line: the dot may end a sentence.
        let mut harness = editing("main.rs", text, 22..22);
        harness.type_text(".");
        assert!(harness.view.completion.is_some());
        harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "point.width = 1;\npoint.\n");
        // The dot of a number asks for nothing, nor does one in a comment or in prose.
        for (file, text) in [
            ("main.rs", "point.width = 1;\nlet x = 3"),
            ("main.rs", "point.width = 1;\n// the point"),
            ("notes.md", "point.width = 1;\nthe point"),
        ] {
            let end = text.chars().count();
            let mut harness = editing(file, text, end..end);
            harness.type_text(".");
            assert!(harness.view.completion.is_none(), "{text}");
        }
    }

    #[test]
    fn a_path_in_a_string_is_completed_one_step_at_a_time() {
        let mut harness = Harness::of("main.js", "", false);
        harness.files = ["lib/math.js", "lib/text.js", "app.js"]
            .map(str::to_owned)
            .to_vec();
        harness.type_text("import x from ");
        // The quote that opens the string is enough: what is in the folder shows.
        harness.type_text("\"");
        assert_eq!(harness.text, "import x from \"\"");
        assert_eq!(offered(&harness), ["lib/", "app.js"]);
        let list = harness.view.completion.as_ref().expect("the list is open");
        assert_eq!(source_word(&list.items[0]), "path");
        // A folder picked, what it holds comes next.
        harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "import x from \"lib/\"");
        assert_eq!(offered(&harness), ["math.js", "text.js"]);
        harness.type_text("t");
        assert_eq!(offered(&harness).first(), Some(&"text.js"));
        let out = harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "import x from \"lib/text.js\"");
        assert_eq!(out.selection, Some(26..26));
        assert!(harness.view.completion.is_none());
    }

    #[test]
    fn tab_goes_through_the_stops_of_a_snippet_and_what_is_typed_in_one_is_written_in_its_twins() {
        let mut harness = walking("");
        // What the first stop holds is selected: typing writes over it, here and there.
        assert_eq!(harness.frame(Vec::new()).selection, Some(0..4));
        let out = harness.type_text("x");
        assert_eq!(harness.text, "x + x = sum;");
        assert_eq!(out.selection, Some(1..1));
        harness.type_text("y");
        assert_eq!(harness.text, "xy + xy = sum;");
        harness.key(Key::Backspace, Modifiers::NONE);
        assert_eq!(harness.text, "x + x = sum;");
        harness.type_text("y");
        assert!(harness.view.completion.is_none(), "{:?}", offered(&harness));
        // Tab selects the next stop, Shift+Tab the one before, as it now is.
        let out = harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(
            (harness.text.as_str(), out.selection),
            ("xy + xy = sum;", Some(10..13))
        );
        let out = harness.key(Key::Tab, Modifiers::SHIFT);
        assert_eq!(out.selection, Some(0..2));
        let out = harness.key(Key::Tab, Modifiers::SHIFT);
        assert_eq!(out.selection, Some(0..2), "there is none before the first");
        harness.key(Key::Tab, Modifiers::NONE);
        harness.type_text("z");
        assert_eq!(harness.text, "xy + xy = z;");
        // After the last stop, Tab goes where the snippet ends; then it indents again.
        let out = harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(
            (harness.text.as_str(), out.selection),
            ("xy + xy = z;", Some(12..12))
        );
        assert!(harness.view.walk.is_none());
        harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "xy + xy = z;    ");
    }

    #[test]
    fn a_snippet_is_left_with_escape_or_when_the_caret_leaves_its_stop() {
        // Escape: the keyboard stays, and Tab indents as anywhere.
        let mut harness = walking("");
        let out = harness.key(Key::Escape, Modifiers::NONE);
        assert!(harness.view.walk.is_none() && out.focused);
        assert_eq!(out.selection, Some(0..4));
        harness.key(Key::Home, Modifiers::NONE);
        harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "    left + left = sum;");
        // An arrow inside the stop keeps it, one that takes the caret out leaves it.
        let mut harness = walking("");
        harness.key(Key::ArrowRight, Modifiers::NONE);
        assert!(harness.view.walk.is_some());
        harness.key(Key::End, Modifiers::NONE);
        assert!(harness.view.walk.is_none());
        harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "left + left = sum;    ");
        // A click elsewhere.
        let mut harness = walking("");
        let pos = pos2(600.0, 10.0);
        let button = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        harness.frame(vec![Event::PointerMoved(pos)]);
        harness.frame(vec![button(true)]);
        harness.frame(vec![button(false)]);
        assert_eq!(harness.frame(Vec::new()).selection, Some(18..18));
        assert!(harness.view.walk.is_none());
        // What is typed outside the stop: its twins are left as they are.
        let mut harness = walking("");
        set_selection(&harness.ctx, editor_id(), &(18..18));
        harness.type_text("!");
        assert_eq!(harness.text, "left + left = sum;!");
        assert!(harness.view.walk.is_none());
        // The text changed from elsewhere.
        let mut harness = walking("");
        harness.text.insert_str(0, "// note\n");
        harness.frame(Vec::new());
        assert!(harness.view.walk.is_none());
        assert_eq!(harness.text, "// note\nleft + left = sum;");
    }

    #[test]
    fn what_is_typed_in_a_stop_and_in_its_twins_is_taken_back_together() {
        let mut harness = walking("");
        harness.time += 5.0;
        harness.type_text("x");
        assert_eq!(harness.text, "x + x = sum;");
        undo(&mut harness);
        assert_eq!(harness.text, "left + left = sum;");
        // The stops are not where they were: the snippet is left.
        assert!(harness.view.walk.is_none());
        harness.key(Key::Y, Modifiers::COMMAND);
        assert_eq!(harness.text, "x + x = sum;");
    }

    #[test]
    fn a_completion_picked_in_a_stop_is_written_in_its_twins_too() {
        let mut harness = walking("let counter = 1;\n");
        type_letters(&mut harness, "cou");
        assert_eq!(offered(&harness).first(), Some(&"counter"));
        harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "let counter = 1;\ncounter + counter = sum;");
        // The snippet goes on: Tab is for its next stop again.
        let out = harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(out.selection, Some(37..40));
    }

    #[test]
    fn a_snippet_is_gone_through_after_line_ends_and_chars_of_any_width() {
        let above = "// héllo ✓ wörld\r\nlet añb = \"日本\";\r\n";
        let mut harness = walking(above);
        let start = above.chars().count();
        assert_eq!(harness.frame(Vec::new()).selection, Some(start..start + 4));
        harness.type_text("é");
        assert_eq!(harness.text, format!("{above}é + é = sum;"));
        let out = harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(out.selection, Some(start + 8..start + 11));
        harness.type_text("日");
        let out = harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, format!("{above}é + é = 日;"));
        assert_eq!(out.selection, Some(start + 10..start + 10));
        // A snippet that comes with the language, its lines indented as the line it is on.
        let mut harness = editing("main.rs", "// ✓\r\n    ", 10..10);
        type_letters(&mut harness, "fn");
        let out = harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "// ✓\r\n    fn name() {\n        \n    }");
        assert_eq!(out.selection, Some(13..17));
        harness.type_text("a");
        harness.key(Key::Tab, Modifiers::NONE);
        let out = harness.key(Key::Tab, Modifiers::NONE);
        assert_eq!(harness.text, "// ✓\r\n    fn a() {\n        \n    }");
        assert_eq!(out.selection, Some(27..27));
    }

    #[test]
    fn what_a_language_server_offers_is_found_by_its_letters_after_the_snippets() {
        let mut harness = editing("main.rs", "", 0..0);
        type_letters(&mut harness, "ma");
        let snippets = harness.view.completion.as_ref().map_or(0, |list| {
            let snippets = list.items.iter().filter(|item| item.kind == Kind::Snippet);
            snippets.count()
        });
        assert!(snippets > 0, "{:?}", offered(&harness));
        let served = ["other_thing", "my_map", "make_all"].map(|label| Completion {
            label: label.to_owned(),
            body: label.to_owned(),
            kind: Kind::Symbol,
        });
        harness.view.offer(2, served.to_vec());
        harness.frame(Vec::new());
        let list = harness.view.completion.as_ref().expect("the list is open");
        let kinds = list.items.iter().map(|item| item.kind);
        let first: Vec<Kind> = kinds.take(snippets).collect();
        assert_eq!(first, vec![Kind::Snippet; snippets]);
        // The one that starts with what is typed, then the one that has its letters.
        assert_eq!(offered(&harness)[snippets..], ["make_all", "my_map"]);
        assert_eq!(list.items[snippets].matched, [0, 1]);
        assert_eq!(source_word(&list.items[snippets]), "symbol");
        // Picked, it is told to the app, which may have an import to write for it.
        for _ in 0..=snippets {
            harness.key(Key::ArrowDown, Modifiers::NONE);
        }
        harness.key(Key::Enter, Modifiers::NONE);
        assert_eq!(harness.text, "my_map");
        let accepted = harness.view.accepted.take().expect("one was picked");
        assert_eq!(
            (accepted.label.as_str(), accepted.kind),
            ("my_map", Kind::Symbol)
        );
    }

    #[test]
    fn a_completion_shows_what_was_typed_of_it_and_what_it_writes() {
        // The start of a name, whatever the case; or its letters, in order.
        let text = "let Counter = 0;\nlet set_counter = 1;\n";
        let shown = |typed: &str| {
            let mut harness = editing("main.rs", text, 38..38);
            for letter in typed.chars() {
                harness.type_text(&letter.to_string());
            }
            let items = harness.view.completion.map(|list| list.items);
            let items = items.unwrap_or_default().into_iter();
            let shown = items.map(|item| (item.label, item.matched, item.source));
            shown.collect::<Vec<_>>()
        };
        let word =
            |label: &str, matched: &[usize]| (label.to_owned(), matched.to_vec(), Source::Word);
        assert_eq!(
            shown("cou")[..2],
            [word("Counter", &[0, 1, 2]), word("set_counter", &[4, 5, 6])]
        );
        assert_eq!(shown("sc"), [word("set_counter", &[0, 4])]);
        assert!(shown("cox").is_empty());
        // Where each comes from is told in a word.
        let mut harness = editing("main.rs", text, 38..38);
        harness.type_text("f");
        harness.type_text("n");
        let list = harness.view.completion.as_ref().expect("the list is open");
        let words: Vec<&str> = list.items.iter().map(source_word).collect();
        assert_eq!(words[..2], ["snippet", "built-in"], "{:?}", list.items);
        // What a snippet writes is shown without its tab stops.
        assert_eq!(preview("fn $0() {\n\t\n}"), "fn () { }");
        assert_eq!(preview("fn ${1:name}($2) {\n\t$0\n}"), "fn name() { }");
        // A name goes on over its hyphens in a style sheet, not in code.
        let css = Language::of(std::path::Path::new("a.css"));
        let rust = Language::of(std::path::Path::new("a.rs"));
        assert_eq!(typed_word("a { font-si", 11, css), "font-si");
        assert_eq!(typed_word("a { font-si", 11, rust), "si");
    }
}
