//! Transient notifications, bottom-right.

use crate::theme::Palette;
use crate::widgets::icon;
use egui::{Align2, Context, CornerRadius, Frame, Margin, RichText, Stroke, vec2};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Success,
    Error,
}

struct Toast {
    level: Level,
    text: String,
    expires_at: f64,
}

#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
    /// Set on first show; toasts pushed earlier get their timer then.
    now: f64,
}

impl Toasts {
    pub fn push(&mut self, level: Level, text: impl Into<String>) {
        let ttl = if level == Level::Error { 8.0 } else { 4.0 };
        self.items.push(Toast {
            level,
            text: text.into(),
            expires_at: self.now + ttl,
        });
        if self.items.len() > 5 {
            self.items.remove(0);
        }
    }

    pub fn success(&mut self, text: impl Into<String>) {
        self.push(Level::Success, text);
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.push(Level::Error, text);
    }

    pub fn show(&mut self, ctx: &Context) {
        self.now = ctx.input(|input| input.time);
        let now = self.now;
        self.items.retain(|toast| toast.expires_at > now);
        if self.items.is_empty() {
            return;
        }
        let palette = Palette::of_ctx(ctx);
        let mut dismissed = None;
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -40.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for (index, toast) in self.items.iter().enumerate() {
                    let (accent, icon) = match toast.level {
                        Level::Success => (palette.success, icon::CHECK_CIRCLE),
                        Level::Error => (palette.error, icon::WARNING_CIRCLE),
                    };
                    let response = Frame::new()
                        .fill(palette.surface)
                        .stroke(Stroke::new(1.0, accent))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::symmetric(14, 10))
                        .shadow(ctx.global_style().visuals.popup_shadow)
                        .show(ui, |ui| {
                            ui.set_max_width(380.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(icon).color(accent).size(16.0));
                                ui.label(&toast.text);
                            });
                        })
                        .response
                        .interact(egui::Sense::click())
                        .on_hover_text("Click to dismiss");
                    if response.clicked() {
                        dismissed = Some(index);
                    }
                    ui.add_space(6.0);
                }
            });
        if let Some(index) = dismissed {
            self.items.remove(index);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}
