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

    /// The colours of the scheme in use.
    pub fn of(_ui: &Ui) -> Self {
        current().palette
    }

    pub fn of_ctx(_ctx: &Context) -> Self {
        current().palette
    }
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// A palette from its background, panel, surface, hover, border, text, dim text, selection
/// and accent.
const fn palette(colors: [u32; 9]) -> Palette {
    Palette {
        bg: rgb(colors[0]),
        panel: rgb(colors[1]),
        surface: rgb(colors[2]),
        hover: rgb(colors[3]),
        border: rgb(colors[4]),
        text: rgb(colors[5]),
        text_dim: rgb(colors[6]),
        selected: rgb(colors[7]),
        accent: rgb(colors[8]),
        accent_text: rgb(colors[8]),
        canvas: rgb(colors[1]),
        success: rgb(colors[5]),
        error: rgb(colors[5]),
    }
}

/// A colour scheme: the interface and the code.
pub struct Scheme {
    pub name: &'static str,
    pub dark: bool,
    pub palette: Palette,
    /// Keywords, types, functions, strings, numbers, comments.
    tokens: [u32; 6],
}

pub static SCHEMES: [Scheme; 6] = [
    Scheme {
        name: "GodCode Dark",
        dark: true,
        palette: Palette::DARK,
        tokens: [
            0x56_9C_D6, 0x4E_C9_B0, 0xDC_DC_AA, 0xCE_91_78, 0xB5_CE_A8, 0x6A_99_55,
        ],
    },
    Scheme {
        name: "GodCode Light",
        dark: false,
        palette: Palette::LIGHT,
        tokens: [
            0x00_00_FF, 0x26_7F_99, 0x79_5E_26, 0xA3_15_15, 0x09_86_58, 0x00_80_00,
        ],
    },
    Scheme {
        name: "Darcula",
        dark: true,
        palette: palette([
            0x2B_2B_2B, 0x31_33_35, 0x3C_3F_41, 0x4B_4F_52, 0x51_51_51, 0xA9_B7_C6, 0x80_88_90,
            0x21_42_83, 0x4A_88_C7,
        ]),
        tokens: [
            0xCC7832, 0x98_76_AA, 0xFF_C6_6D, 0x6A_87_59, 0x68_97_BB, 0x80_80_80,
        ],
    },
    Scheme {
        name: "Monokai",
        dark: true,
        palette: palette([
            0x27_28_22, 0x2D_2E_27, 0x34_35_2D, 0x3E3D32, 0x49_48_3E, 0xF8_F8_F2, 0x90_90_8A,
            0x49_48_3E, 0xA6_E2_2E,
        ]),
        tokens: [
            0xF9_26_72, 0x66_D9_EF, 0xA6_E2_2E, 0xE6_DB_74, 0xAE_81_FF, 0x75_71_5E,
        ],
    },
    Scheme {
        name: "Solarized Light",
        dark: false,
        palette: palette([
            0xFD_F6_E3, 0xFD_F6_E3, 0xEE_E8_D5, 0xE4_DD_C8, 0xD9_D2_BD, 0x58_6E_75, 0x93_A1_A1,
            0xD6_CF_BA, 0x26_8B_D2,
        ]),
        tokens: [
            0x85_99_00, 0xB5_89_00, 0x26_8B_D2, 0x2A_A1_98, 0xD3_36_82, 0x93_A1_A1,
        ],
    },
    Scheme {
        name: "High Contrast",
        dark: true,
        palette: palette([
            0x00_00_00, 0x00_00_00, 0x0C_0C_0C, 0x1F_1F_1F, 0xFF_FF_FF, 0xFF_FF_FF, 0xD0_D0_D0,
            0x26_4F_78, 0xFF_FF_00,
        ]),
        tokens: [
            0xFF_FF_00, 0x00_FF_FF, 0xFF_A6_57, 0x7C_FC_00, 0xFF_8A_D8, 0x9E_E0_9E,
        ],
    },
];

/// The scheme in use, by its place in [`SCHEMES`].
static CURRENT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn current() -> &'static Scheme {
    let index = CURRENT.load(std::sync::atomic::Ordering::Relaxed);
    &SCHEMES[index.min(SCHEMES.len() - 1)]
}

/// Set up the interface once, in scheme `index`.
pub fn install(ctx: &Context, index: usize) {
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
    apply(ctx, index);
}

/// Switch to scheme `index`.
pub fn apply(ctx: &Context, index: usize) {
    CURRENT.store(index, std::sync::atomic::Ordering::Relaxed);
    let scheme = current();
    let (theme, visuals) = if scheme.dark {
        (Theme::Dark, Visuals::dark())
    } else {
        (Theme::Light, Visuals::light())
    };
    ctx.style_mut_of(theme, |style| configure(style, visuals, &scheme.palette));
    ctx.set_theme(theme);
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

/// Colour of a syntax token, in the scheme in use.
pub fn token_color(token: godcode_core::syntax::Token, _dark: bool) -> Color32 {
    use godcode_core::syntax::Token;
    let scheme = current();
    let index = match token {
        Token::Plain => return scheme.palette.text,
        Token::Keyword => 0,
        Token::Type => 1,
        Token::Function => 2,
        Token::String => 3,
        Token::Number => 4,
        Token::Comment => 5,
    };
    rgb(scheme.tokens[index])
}
