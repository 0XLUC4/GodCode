//! The code editor: a text field with syntax colours, line numbers, completion, problem
//! marks and code-aware keys.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange, LayoutJob, LayoutSection, TextFormat, TextWrapping};
use egui::{
    Align, Align2, Area, Color32, Context, CornerRadius, Event, FontId, Frame, Galley, Id, Key,
    Margin, Modifiers, Order, Pos2, Rect, ScrollArea, Sense, Shape, Stroke, StrokeKind, TextBuffer,
    TextEdit, Ui, UiBuilder, pos2, vec2,
};
use godcode_core::complete::{self, Completion, Kind};
use godcode_core::git::{Hunk, LineChange};
use godcode_core::search::Match;
use godcode_core::syntax::{self, Language};
use godcode_core::{check, edit};

use crate::theme::{self, Palette};

const MARGIN: Margin = Margin {
    left: 6,
    right: 24,
    top: 4,
    bottom: 4,
};
/// Space around the digits of the line numbers.
const GUTTER_PAD: f32 = 14.0;
/// Completions shown at once; the list slides to keep the chosen one in view.
const POPUP_ROWS: usize = 8;
const POPUP_ROW: egui::Vec2 = vec2(300.0, 22.0);
pub const ERROR_COLOR: Color32 = Color32::from_rgb(0xF1, 0x4C, 0x4C);
pub const WARNING_COLOR: Color32 = Color32::from_rgb(0xCC, 0xA7, 0x00);
/// Columns a tab takes when lines are measured for the indent guides.
const TAB_COLUMNS: usize = 4;
const ADDED_COLOR: Color32 = Color32::from_rgb(0x58, 0x8C, 0x2C);
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
    layout: Option<Layout>,
    completion: Option<Popup>,
    /// The bracket pair at a caret, for the text of a layout key.
    pair: Option<PairAt>,
}

impl View {
    /// The completion list is open.
    pub fn completing(&self) -> bool {
        self.completion.is_some()
    }

    /// A view that takes the keyboard as soon as it shows.
    pub fn focused() -> Self {
        Self {
            focus: true,
            ..Self::default()
        }
    }
}

/// The laid out text, reused while nothing it depends on changes.
struct Layout {
    key: u64,
    galley: Arc<Galley>,
    /// Columns of white space leading each line; `None` for a blank line.
    indents: Vec<Option<usize>>,
}

fn indents_of(text: &str) -> Vec<Option<usize>> {
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
            let caret = edit::newline(text, selection, config.indent);
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

/// Step the editor's own history; `None` when there is nothing to undo / redo.
pub fn history(ctx: &Context, id: Id, text: &mut String, undo: bool) -> Option<Range<usize>> {
    let mut state = TextEdit::load_state(ctx, id)?;
    let range = state.cursor.char_range()?;
    let mut undoer = state.undoer();
    let current = (range, text.clone());
    let step = if undo {
        undoer.undo(&current)
    } else {
        undoer.redo(&current)
    }
    .cloned();
    state.set_undoer(undoer);
    let (range, restored) = step?;
    *text = restored;
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

/// Keys the plain text field would handle without knowing it holds code.
fn keys(ui: &Ui, id: Id, text: &mut String, config: &Config<'_>, view: &mut View) -> bool {
    if !ui.memory(|memory| memory.has_focus(id)) {
        view.completion = None;
        return false;
    }
    let pressed = |modifiers, key| ui.input_mut(|input| input.consume_key(modifiers, key));
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
    let Some(caret) = caret else {
        view.completion = None;
        return;
    };
    let (word, erased) = ui.input(|input| {
        let is_word = |text: &str| text.chars().all(|ch| ch.is_alphanumeric() || ch == '_');
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
        let items = complete::complete(text, caret, config.language);
        let typed_chars = caret - complete::word_start(text, caret);
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

fn layout_key(text: &str, config: &Config<'_>, dark: bool, width: f32) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    width.to_bits().hash(&mut hasher);
    config.language.name.hash(&mut hasher);
    config.font_size.to_bits().hash(&mut hasher);
    dark.hash(&mut hasher);
    hasher.finish()
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
    let mut changed = keys(ui, id, text, config, view);
    let selected_before = selection_of(ui.ctx(), id);

    let lines = text.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let (digit, row_height) =
        ui.fonts_mut(|fonts| (fonts.glyph_width(&font, '0'), fonts.row_height(&font)));
    let gutter_width = lines.to_string().len().max(3) as f32 * digit + 2.0 * GUTTER_PAD;
    let full = ui.available_rect_before_wrap();
    let gutter = full.with_max_x(full.min.x + gutter_width);
    let body = full.with_min_x(gutter.max.x);
    ui.painter().rect_filled(full, 0.0, palette.bg);

    let unit_columns = config
        .indent
        .bytes()
        .map(|byte| if byte == b'\t' { TAB_COLUMNS } else { 1 })
        .sum::<usize>()
        .max(1);
    let mut child = ui.new_child(UiBuilder::new().max_rect(body));
    let scroll = if config.wrap {
        ScrollArea::vertical()
    } else {
        ScrollArea::both()
    };
    let scrolled = scroll
        .id_salt(id)
        .auto_shrink(false)
        .show(&mut child, |ui| {
            // Reserved now so the highlights end up under the text.
            let under = ui.painter().add(Shape::Noop);
            let output = {
                let layout = &mut view.layout;
                let mut layouter = |ui: &Ui, buffer: &dyn TextBuffer, wrap_width: f32| {
                    let width = if config.wrap {
                        wrap_width
                    } else {
                        f32::INFINITY
                    };
                    let key = layout_key(buffer.as_str(), config, dark, width);
                    if let Some(layout) = layout.as_ref().filter(|layout| layout.key == key) {
                        return Arc::clone(&layout.galley);
                    }
                    let job = layout_job(buffer.as_str(), config, dark, width);
                    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                    *layout = Some(Layout {
                        key,
                        galley: Arc::clone(&galley),
                        indents: indents_of(buffer.as_str()),
                    });
                    galley
                };
                TextEdit::multiline(text)
                    .id(id)
                    .frame(Frame::NONE)
                    .margin(MARGIN)
                    .desired_width(body.width())
                    .min_size(vec2(
                        0.0,
                        body.height() - f32::from(MARGIN.top + MARGIN.bottom),
                    ))
                    .lock_focus(true)
                    .layouter(&mut layouter)
                    .show(ui)
            };
            if std::mem::take(&mut view.focus) {
                output.response.response.request_focus();
            }
            output.response.response.context_menu(|ui| menu(ui));
            let mut selection = output.cursor_range.map(sorted);
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
            let jump = (output.response.response.clicked()
                && ui.input(|input| input.modifiers.command))
            .then(|| output.cursor_range.map(|range| range.primary.index.0))
            .flatten();
            let origin = output.galley_pos.to_vec2();
            let galley = &output.galley;
            let clip = ui.clip_rect();
            let caret = output
                .cursor_range
                .map(|range| galley.pos_from_cursor(range.primary));
            let caret_index = selection
                .as_ref()
                .filter(|selection| selection.is_empty())
                .map(|selection| selection.start);
            // Where on screen the chars `start..start + len` are, when their line shows.
            let place = |line: usize, start: usize, len: usize| {
                // One row per line unless they wrap: lines out of view are skipped unmeasured.
                let top = origin.y + line as f32 * row_height;
                if !config.wrap && (top > clip.max.y || top + row_height < clip.min.y) {
                    return None;
                }
                let from = galley.pos_from_cursor(CCursor::new(start));
                let to = galley.pos_from_cursor(CCursor::new(start + len));
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
            // A thin line down every indent level, to follow blocks by eye.
            if let Some(layout) = view.layout.as_ref().filter(|_| !config.wrap) {
                let first = ((clip.min.y - origin.y) / row_height).floor().max(0.0) as usize;
                let shown = (clip.height() / row_height).ceil() as usize + 1;
                let stroke = Stroke::new(1.0, palette.border);
                // A blank line continues the block it is in.
                let mut levels = 0;
                for (line, indent) in layout.indents.iter().enumerate().skip(first).take(shown) {
                    if let Some(columns) = indent {
                        levels = columns / unit_columns;
                    }
                    let top = origin.y + line as f32 * row_height;
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
                    let pair = check::matching_bracket(text, config.language, caret);
                    (layout.key, caret, pair)
                });
                view.pair = Some((layout.key, caret, pair));
                for index in pair.into_iter().flat_map(|(a, b)| [a, b]) {
                    let cell = galley.pos_from_cursor(CCursor::new(index));
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
            ui.painter().set(under, Shape::Vec(shapes));

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
                let target = galley
                    .pos_from_cursor(CCursor::new(index))
                    .translate(origin);
                ui.scroll_to_rect(target.expand2(vec2(60.0, 0.0)), Some(Align::Center));
            }
            let typed = output.response.response.changed();
            changed |= typed;
            let caret_screen = caret.map(|caret| caret.translate(origin).left_bottom());
            (
                output.galley,
                output.galley_pos,
                caret,
                selection,
                caret_index,
                caret_screen,
                typed,
                jump,
            )
        });
    ui.allocate_rect(full, Sense::hover());
    let (galley, origin, caret, selection, caret_index, caret_screen, typed, jump) = scrolled.inner;

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
        changed = true;
    }

    let painter = ui.painter_at(gutter);
    painter.rect_filled(gutter, 0.0, palette.bg);
    let number_font = FontId::monospace(config.font_size * 0.92);
    // The row that starts the line the caret is on: a wrapped line takes several rows.
    let caret_row = caret.and_then(|caret| {
        let row = galley
            .rows
            .iter()
            .rposition(|row| row.pos.y <= caret.min.y + 0.5)?;
        let wrapped = galley.rows[..row]
            .iter()
            .rev()
            .take_while(|above| !above.ends_with_newline)
            .count();
        Some(row - wrapped)
    });
    let mut index = 0;
    let mut starts_line = true;
    for (row_index, row) in galley.rows.iter().enumerate() {
        let top = origin.y + row.pos.y;
        if top > gutter.max.y {
            break;
        }
        let numbered = std::mem::replace(&mut starts_line, row.ends_with_newline);
        if !numbered {
            continue;
        }
        index += 1;
        if top + row_height < gutter.min.y {
            continue;
        }
        // Lines are counted from 0 below, as the marks are.
        let index = index - 1;
        // An empty line has no height of its own: the font's row height places the number.
        let middle = top + row_height / 2.0;
        let current = caret_row == Some(row_index);
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
    Output {
        changed,
        selection,
        jump,
    }
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
            output.expect("the editor ran")
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
    fn a_long_line_wraps_only_when_asked() {
        let text = format!("{}\nend", "word ".repeat(400));
        let rows = |wrap: bool| {
            let harness = Harness::of("page.html", &text, wrap);
            let layout = harness.view.layout.as_ref().expect("laid out");
            layout.galley.rows.len()
        };
        assert_eq!(rows(false), 2, "one row per line");
        assert!(rows(true) > 4, "the long line takes several rows");
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
    fn undo_restores_the_text_through_the_menu() {
        let mut harness = Harness::new("");
        harness.type_text("first");
        harness.time += 5.0;
        harness.frame(Vec::new());
        harness.type_text(" second");
        harness.time += 5.0;
        harness.frame(Vec::new());
        let selection = history(&harness.ctx, editor_id(), &mut harness.text, true);
        assert_eq!(harness.text, "first");
        assert_eq!(selection, Some(5..5));
        history(&harness.ctx, editor_id(), &mut harness.text, false);
        assert_eq!(harness.text, "first second");
    }
}
