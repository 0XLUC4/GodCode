//! Classic monochrome palette (black on white, white on black) and egui style setup.

use egui::{Color32, Context, CornerRadius, Shadow, Stroke, Style, Theme, Ui, Visuals, vec2};

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub surface: Color32,
    pub hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    /// Emphasis color: strokes, primary buttons, current page.
    pub accent: Color32,
    /// Accent usable as text/link color on this background.
    pub accent_text: Color32,
    /// Fill of selected / pressed controls.
    pub selected: Color32,
    /// Viewer canvas behind pages.
    pub canvas: Color32,
    pub success: Color32,
    pub error: Color32,
}

const fn grey(level: u8) -> Color32 {
    Color32::from_rgb(level, level, level)
}

impl Palette {
    pub const DARK: Self = Self {
        bg: grey(0x0E),
        panel: grey(0x14),
        surface: grey(0x1C),
        hover: grey(0x26),
        border: grey(0x2E),
        text: grey(0xF2),
        text_dim: grey(0x9A),
        accent: grey(0xF2),
        accent_text: grey(0xF2),
        selected: grey(0x33),
        canvas: grey(0x0A),
        success: grey(0xF2),
        error: grey(0xF2),
    };

    /// Default: white UI, light grey canvas so white pages stand out.
    pub const LIGHT: Self = Self {
        bg: Color32::WHITE,
        panel: Color32::WHITE,
        surface: grey(0xF5),
        hover: grey(0xEF),
        border: grey(0xE4),
        text: grey(0x11),
        text_dim: grey(0x66),
        accent: grey(0x11),
        accent_text: grey(0x11),
        selected: grey(0xE6),
        canvas: grey(0xF2),
        success: grey(0x11),
        error: grey(0x11),
    };

    pub fn of(ui: &Ui) -> Self {
        if ui.visuals().dark_mode {
            Self::DARK
        } else {
            Self::LIGHT
        }
    }

    pub fn of_ctx(ctx: &Context) -> Self {
        if ctx.global_style().visuals.dark_mode {
            Self::DARK
        } else {
            Self::LIGHT
        }
    }
}

/// Install both styles once; switching later is just [`set_dark`].
pub fn install(ctx: &Context, dark: bool) {
    ctx.style_mut_of(Theme::Dark, |style| {
        configure(style, Visuals::dark(), &Palette::DARK)
    });
    ctx.style_mut_of(Theme::Light, |style| {
        configure(style, Visuals::light(), &Palette::LIGHT)
    });
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
    set_dark(ctx, dark);
}

pub fn set_dark(ctx: &Context, dark: bool) {
    ctx.set_theme(if dark { Theme::Dark } else { Theme::Light });
}

fn configure(style: &mut Style, mut visuals: Visuals, p: &Palette) {
    let radius = CornerRadius::same(7);
    let shadow_alpha = if visuals.dark_mode { 110 } else { 28 };
    visuals.panel_fill = p.panel;
    visuals.window_fill = p.panel;
    visuals.window_stroke = Stroke::new(1.0, p.border);
    visuals.window_corner_radius = CornerRadius::same(12);
    visuals.menu_corner_radius = CornerRadius::same(9);
    visuals.window_shadow = Shadow {
        offset: [0, 10],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(shadow_alpha),
    };
    visuals.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(shadow_alpha / 4 * 3),
    };
    visuals.extreme_bg_color = p.bg;
    visuals.faint_bg_color = p.surface;
    visuals.code_bg_color = p.surface;
    visuals.override_text_color = Some(p.text);
    visuals.hyperlink_color = p.accent_text;
    visuals.error_fg_color = p.error;
    visuals.warn_fg_color = p.text;
    visuals.selection.bg_fill = p.selected;
    visuals.selection.stroke = Stroke::new(1.0, p.accent_text);

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = p.panel;
    widgets.noninteractive.weak_bg_fill = p.panel;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text_dim);
    widgets.noninteractive.corner_radius = radius;

    widgets.inactive.bg_fill = p.surface;
    widgets.inactive.weak_bg_fill = p.surface;
    widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
    widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    widgets.inactive.corner_radius = radius;

    widgets.hovered.bg_fill = p.hover;
    widgets.hovered.weak_bg_fill = p.hover;
    widgets.hovered.bg_stroke = Stroke::new(1.0, p.border);
    widgets.hovered.fg_stroke = Stroke::new(1.5, p.text);
    widgets.hovered.corner_radius = radius;

    widgets.active.bg_fill = p.selected;
    widgets.active.weak_bg_fill = p.selected;
    widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    widgets.active.fg_stroke = Stroke::new(1.5, p.text);
    widgets.active.corner_radius = radius;

    widgets.open.bg_fill = p.hover;
    widgets.open.weak_bg_fill = p.hover;
    widgets.open.bg_stroke = Stroke::new(1.0, p.border);
    widgets.open.fg_stroke = Stroke::new(1.0, p.text);
    widgets.open.corner_radius = radius;

    style.visuals = visuals;
    style.spacing.item_spacing = vec2(8.0, 6.0);
    style.spacing.button_padding = vec2(10.0, 5.0);
    style.spacing.menu_margin = egui::Margin::same(8);
    style.spacing.window_margin = egui::Margin::same(16);
    style.spacing.interact_size.y = 26.0;
}

/// Colour of a syntax token. The interface stays monochrome; only the code is coloured.
pub fn token_color(token: godcode_core::syntax::Token, dark: bool) -> Color32 {
    use godcode_core::syntax::Token;
    let rgb = |hex: u32| Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
    match (token, dark) {
        (Token::Plain, true) => Palette::DARK.text,
        (Token::Plain, false) => Palette::LIGHT.text,
        (Token::Keyword, true) => rgb(0x56_9C_D6),
        (Token::Keyword, false) => rgb(0x00_00_FF),
        (Token::Type, true) => rgb(0x4E_C9_B0),
        (Token::Type, false) => rgb(0x26_7F_99),
        (Token::Function, true) => rgb(0xDC_DC_AA),
        (Token::Function, false) => rgb(0x79_5E_26),
        (Token::String, true) => rgb(0xCE_91_78),
        (Token::String, false) => rgb(0xA3_15_15),
        (Token::Number, true) => rgb(0xB5_CE_A8),
        (Token::Number, false) => rgb(0x09_86_58),
        (Token::Comment, true) => rgb(0x6A_99_55),
        (Token::Comment, false) => rgb(0x00_80_00),
    }
}
