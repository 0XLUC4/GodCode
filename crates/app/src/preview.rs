//! The rendered view of a Markdown file, shown beside its text.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, CornerRadius, FontId, Frame, Margin, Rect, Stroke, Ui, pos2, vec2};
use godcode_core::markdown::{self, Block, Placed, Span};
use godcode_core::syntax::{self, Language};

use crate::theme::{self, Palette};
use crate::widgets::icon;

const LINK: Color32 = Color32::from_rgb(0x4F, 0x9C, 0xE8);
/// Sizes of the six heading levels, at 100 % zoom.
const HEADINGS: [f32; 6] = [28.0, 23.0, 19.0, 16.5, 15.0, 14.0];
const BODY: f32 = 14.5;
/// Width of one level of list nesting, and of the column of bullets.
const NEST: f32 = 20.0;

/// Colours and size of the text being drawn.
#[derive(Clone, Copy)]
struct Ink {
    size: f32,
    text: Color32,
    /// Bold text: the fonts have one weight, so it is told by its colour.
    strong: Color32,
    code_fill: Color32,
}

fn job(spans: &[Span], ink: Ink) -> LayoutJob {
    let mut job = LayoutJob::default();
    for span in spans {
        let font = if span.code {
            FontId::monospace(ink.size * 0.93)
        } else {
            FontId::proportional(ink.size)
        };
        let color = if span.bold { ink.strong } else { ink.text };
        let mut format = TextFormat::simple(font, color);
        format.italics = span.italic;
        if span.code {
            format.background = ink.code_fill;
        }
        if span.strike {
            format.strikethrough = Stroke::new(1.0, color);
        }
        if span.link {
            format.color = LINK;
            format.underline = Stroke::new(1.0, LINK);
        }
        job.append(&span.text, 0.0, format);
    }
    job
}

/// `text` coloured as the editor colours `language`.
fn code(text: &str, language: &Language, size: f32, dark: bool) -> LayoutJob {
    let font = FontId::monospace(size);
    let mut job = LayoutJob::default();
    let mut start = 0;
    for span in syntax::highlight(text, language) {
        let color = theme::token_color(span.token, dark);
        let format = TextFormat::simple(font.clone(), color);
        job.append(&text[start..span.end], 0.0, format);
        start = span.end;
    }
    job
}

fn rule(ui: &mut Ui, color: Color32) {
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1.0, color));
    ui.add_space(1.0);
}

/// Draw `blocks` down `ui`. Returns where each one starts: its line, and how far below the
/// top of the page it is.
pub fn show(ui: &mut Ui, blocks: &[Placed], zoom: f32) -> Vec<(f32, f32)> {
    let palette = Palette::of(ui);
    let dark = ui.visuals().dark_mode;
    let grey = |level: u8| Color32::from_rgb(level, level, level);
    let ink = Ink {
        size: BODY * zoom,
        text: grey(if dark { 0xCE } else { 0x2C }),
        strong: palette.text,
        code_fill: palette.surface,
    };
    let top = ui.cursor().top();
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    let mut starts = Vec::with_capacity(blocks.len());
    for (line, block) in blocks {
        starts.push((*line as f32, ui.cursor().top() - top));
        match block {
            Block::Heading { level, text } => {
                let size = HEADINGS[(*level).clamp(1, 6) - 1] * zoom;
                let title = Ink {
                    size,
                    text: palette.text,
                    ..ink
                };
                ui.add_space(if starts.len() > 1 { size * 0.6 } else { 0.0 });
                ui.label(job(&markdown::inline(text), title));
                ui.add_space(size * 0.25);
                if *level <= 2 {
                    rule(ui, palette.border);
                }
                ui.add_space(size * 0.35);
            }
            Block::Paragraph(text) => {
                ui.label(job(&markdown::inline(text), ink));
                ui.add_space(ink.size * 0.7);
            }
            Block::Item {
                depth,
                number,
                done,
                text,
            } => {
                ui.horizontal_top(|ui| {
                    ui.add_space(*depth as f32 * NEST);
                    let mark = match (done, number) {
                        (Some(true), _) => icon::CHECK_SQUARE.to_owned(),
                        (Some(false), _) => icon::SQUARE.to_owned(),
                        (None, Some(number)) => format!("{number}."),
                        (None, None) => "•".to_owned(),
                    };
                    let mark = egui::RichText::new(mark)
                        .size(ink.size)
                        .color(palette.text_dim);
                    ui.add_sized(
                        vec2(NEST + 4.0, ink.size),
                        egui::Label::new(mark).selectable(false),
                    );
                    ui.label(job(&markdown::inline(text), ink));
                });
                ui.add_space(ink.size * 0.3);
            }
            Block::Code { language, text } => {
                Frame::new()
                    .fill(palette.surface)
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(code(
                            text,
                            Language::fenced(language),
                            ink.size * 0.93,
                            dark,
                        ));
                    });
                ui.add_space(ink.size * 0.7);
            }
            Block::Quote(text) => {
                let quoted = Frame::new()
                    .inner_margin(Margin {
                        left: 14,
                        ..Margin::ZERO
                    })
                    .show(ui, |ui| {
                        let dim = Ink {
                            text: palette.text_dim,
                            ..ink
                        };
                        ui.label(job(&markdown::inline(text), dim));
                    });
                let rect = quoted.response.rect;
                let bar = Rect::from_min_max(rect.min, pos2(rect.min.x + 3.0, rect.max.y));
                ui.painter().rect_filled(bar, 1.0, palette.border);
                ui.add_space(ink.size * 0.7);
            }
            Block::Rule => {
                ui.add_space(ink.size * 0.3);
                rule(ui, palette.border);
                ui.add_space(ink.size * 0.8);
            }
            Block::Table(rows) => {
                egui::Grid::new(("markdown-table", *line))
                    .striped(true)
                    .spacing(vec2(20.0, 7.0))
                    .show(ui, |ui| {
                        for (index, row) in rows.iter().enumerate() {
                            let ink = Ink {
                                text: if index == 0 { ink.strong } else { ink.text },
                                ..ink
                            };
                            for cell in row {
                                ui.label(job(&markdown::inline(cell), ink));
                            }
                            ui.end_row();
                        }
                    });
                ui.add_space(ink.size * 0.7);
            }
        }
    }
    starts
}

/// Where `at` falls among `points`, given as (from, to) pairs growing together: between
/// two of them it is carried over in proportion.
pub fn interpolate(points: impl Iterator<Item = (f32, f32)>, at: f32) -> f32 {
    let mut previous: Option<(f32, f32)> = None;
    for (from, to) in points {
        if at <= from {
            return match previous {
                Some((start, value)) if from > start => {
                    value + (at - start) / (from - start) * (to - value)
                }
                _ => to,
            };
        }
        previous = Some((from, to));
    }
    previous.map_or(0.0, |(_, to)| to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_carry_over_between_the_two_views() {
        let starts = [(0.0, 0.0), (10.0, 300.0), (20.0, 400.0)];
        let y_of = |line: f32| interpolate(starts.into_iter(), line);
        let line_of = |y: f32| interpolate(starts.into_iter().map(|(line, y)| (y, line)), y);
        assert_eq!(y_of(5.0), 150.0);
        assert_eq!(y_of(15.0), 350.0);
        assert_eq!(y_of(99.0), 400.0);
        assert_eq!(y_of(f32::INFINITY), 400.0);
        assert_eq!(line_of(350.0), 15.0);
        assert_eq!(line_of(y_of(7.5)), 7.5);
        assert_eq!(interpolate(std::iter::empty(), 3.0), 0.0);
    }
}
