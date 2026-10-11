//! Classic monochrome palette (black on white, white on black) and egui style setup.
//!
//! Beside the built-in [`SCHEMES`], the user writes themes of their own in a JSON file
//! ([`EXAMPLE`] shows its shape): [`parse_user_themes`] reads it, [`set_user_themes`] puts
//! them after the built-in ones, where [`apply`] finds them by their place.

use std::sync::{PoisonError, RwLock};

use egui::{Color32, Context, CornerRadius, Shadow, Stroke, Style, Theme, Ui, Visuals, vec2};
use godcode_core::syntax::Token;
use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, PartialEq)]
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

/// The colours of keywords, types, functions, strings, numbers and comments.
const fn tokens(colors: [u32; 6]) -> [Color32; 6] {
    [
        rgb(colors[0]),
        rgb(colors[1]),
        rgb(colors[2]),
        rgb(colors[3]),
        rgb(colors[4]),
        rgb(colors[5]),
    ]
}

/// A colour scheme: the interface and the code.
pub struct Scheme {
    pub name: &'static str,
    pub dark: bool,
    pub palette: Palette,
    /// Keywords, types, functions, strings, numbers, comments.
    tokens: [Color32; 6],
}

impl Scheme {
    /// Colour of a syntax token in this scheme.
    fn token(&self, token: Token) -> Color32 {
        let index = match token {
            Token::Plain => return self.palette.text,
            Token::Keyword => 0,
            Token::Type => 1,
            Token::Function => 2,
            Token::String => 3,
            Token::Number => 4,
            Token::Comment => 5,
        };
        self.tokens[index]
    }
}

pub static SCHEMES: [Scheme; 6] = [
    Scheme {
        name: "GodCode Dark",
        dark: true,
        palette: Palette::DARK,
        tokens: tokens([
            0x56_9C_D6, 0x4E_C9_B0, 0xDC_DC_AA, 0xCE_91_78, 0xB5_CE_A8, 0x6A_99_55,
        ]),
    },
    Scheme {
        name: "GodCode Light",
        dark: false,
        palette: Palette::LIGHT,
        tokens: tokens([
            0x00_00_FF, 0x26_7F_99, 0x79_5E_26, 0xA3_15_15, 0x09_86_58, 0x00_80_00,
        ]),
    },
    Scheme {
        name: "Darcula",
        dark: true,
        palette: palette([
            0x2B_2B_2B, 0x31_33_35, 0x3C_3F_41, 0x4B_4F_52, 0x51_51_51, 0xA9_B7_C6, 0x80_88_90,
            0x21_42_83, 0x4A_88_C7,
        ]),
        tokens: tokens([
            0xCC7832, 0x98_76_AA, 0xFF_C6_6D, 0x6A_87_59, 0x68_97_BB, 0x80_80_80,
        ]),
    },
    Scheme {
        name: "Monokai",
        dark: true,
        palette: palette([
            0x27_28_22, 0x2D_2E_27, 0x34_35_2D, 0x3E3D32, 0x49_48_3E, 0xF8_F8_F2, 0x90_90_8A,
            0x49_48_3E, 0xA6_E2_2E,
        ]),
        tokens: tokens([
            0xF9_26_72, 0x66_D9_EF, 0xA6_E2_2E, 0xE6_DB_74, 0xAE_81_FF, 0x75_71_5E,
        ]),
    },
    Scheme {
        name: "Solarized Light",
        dark: false,
        palette: palette([
            0xFD_F6_E3, 0xFD_F6_E3, 0xEE_E8_D5, 0xE4_DD_C8, 0xD9_D2_BD, 0x58_6E_75, 0x93_A1_A1,
            0xD6_CF_BA, 0x26_8B_D2,
        ]),
        tokens: tokens([
            0x85_99_00, 0xB5_89_00, 0x26_8B_D2, 0x2A_A1_98, 0xD3_36_82, 0x93_A1_A1,
        ]),
    },
    Scheme {
        name: "High Contrast",
        dark: true,
        palette: palette([
            0x00_00_00, 0x00_00_00, 0x0C_0C_0C, 0x1F_1F_1F, 0xFF_FF_FF, 0xFF_FF_FF, 0xD0_D0_D0,
            0x26_4F_78, 0xFF_FF_00,
        ]),
        tokens: tokens([
            0xFF_FF_00, 0x00_FF_FF, 0xFF_A6_57, 0x7C_FC_00, 0xFF_8A_D8, 0x9E_E0_9E,
        ]),
    },
];

/// The themes of the user, which come after [`SCHEMES`]. A scheme is never freed, so that
/// the one in use outlives the list it came from.
static USER_SCHEMES: RwLock<Vec<&'static Scheme>> = RwLock::new(Vec::new());

/// The scheme in use; before any is chosen, the first of [`SCHEMES`].
static CURRENT: RwLock<Option<&'static Scheme>> = RwLock::new(None);

pub fn current() -> &'static Scheme {
    let current = *CURRENT.read().unwrap_or_else(PoisonError::into_inner);
    current.unwrap_or(&SCHEMES[0])
}

/// The scheme at `index`: the built-in ones, then those of the user.
pub fn scheme(index: usize) -> Option<&'static Scheme> {
    match index.checked_sub(SCHEMES.len()) {
        None => SCHEMES.get(index),
        Some(nth) => {
            let user = USER_SCHEMES.read().unwrap_or_else(PoisonError::into_inner);
            user.get(nth).copied()
        }
    }
}

/// The names of all the schemes, each at its index: the built-in ones, then those of the user.
// Waiting to be wired: the theme picker of `app.rs` still lists `SCHEMES` alone.
#[allow(dead_code)]
pub fn scheme_names() -> Vec<&'static str> {
    let user = USER_SCHEMES.read().unwrap_or_else(PoisonError::into_inner);
    let all = SCHEMES.iter().chain(user.iter().copied());
    all.map(|scheme| scheme.name).collect()
}

/// The index of the scheme of this name. The index of a theme of the user changes when the
/// user's file does, its name does not: the name is what to remember between two runs.
// Waiting to be wired.
#[allow(dead_code)]
pub fn scheme_index(name: &str) -> Option<usize> {
    scheme_names().iter().position(|known| *known == name)
}

/// Set up the interface once, in scheme `index`.
pub fn install(ctx: &Context, index: usize) {
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
    apply(ctx, index);
}

/// Switch to scheme `index`; an index that names no scheme gives the first one.
pub fn apply(ctx: &Context, index: usize) {
    let scheme = scheme(index).unwrap_or(&SCHEMES[0]);
    *CURRENT.write().unwrap_or_else(PoisonError::into_inner) = Some(scheme);
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
pub fn token_color(token: Token, _dark: bool) -> Color32 {
    current().token(token)
}

/// What the file of the user's themes holds the first time. JSON has no comments: a key
/// that starts with `//` is one.
// Waiting to be wired: `app.rs` writes it into the file it opens when there is none.
#[allow(dead_code)]
pub const EXAMPLE: &str = r##"{
  "// 1": "Each theme below is listed with the built-in ones, after them.",
  "// 2": "A key that starts with // is a comment. A colour is #rgb, #rrggbb or #rrggbbaa.",
  "// 3": "Only name is required. dark is true unless said otherwise. A colour left out is that of GodCode Dark, or of GodCode Light when dark is false.",
  "themes": [
    {
      "name": "My Dark Theme",
      "dark": true,
      "palette": {
        "// bg": "behind the code, the tabs and the status bar",
        "bg": "#1e1e2e",
        "// panel": "panels, menus and windows",
        "panel": "#181825",
        "// surface": "buttons and fields at rest",
        "surface": "#313244",
        "// hover": "a button under the pointer",
        "hover": "#45475a",
        "// border": "lines around and between things",
        "border": "#45475a",
        "// text": "text, and the code that is no token",
        "text": "#cdd6f4",
        "// text_dim": "labels and what matters less",
        "text_dim": "#a6adc8",
        "// accent": "emphasis: strokes, main buttons",
        "accent": "#89b4fa",
        "// accent_text": "links, and the edge of the selection",
        "accent_text": "#89b4fa",
        "// selected": "selected text, a pressed button",
        "selected": "#585b70",
        "// canvas": "the empty window, when no file is open",
        "canvas": "#11111b",
        "// success": "a message that says it went well",
        "success": "#a6e3a1",
        "// error": "a message that says it went wrong",
        "error": "#f38ba8"
      },
      "tokens": {
        "keyword": "#cba6f7",
        "type": "#f9e2af",
        "function": "#89b4fa",
        "string": "#a6e3a1",
        "number": "#fab387",
        "comment": "#6c7086"
      }
    },
    {
      "// ": "A light theme that only says what it changes.",
      "name": "My Light Theme",
      "dark": false,
      "palette": {
        "bg": "#fbf7f0",
        "accent": "#b4637a",
        "accent_text": "#b4637a"
      },
      "tokens": {
        "comment": "#9893a5"
      }
    }
  ]
}
"##;

/// A theme of the user, as read from their file.
#[derive(Clone, Debug, PartialEq)]
pub struct UserTheme {
    pub name: String,
    pub dark: bool,
    pub palette: Palette,
    /// Keywords, types, functions, strings, numbers, comments.
    tokens: [Color32; 6],
}

impl UserTheme {
    /// The theme as a scheme that lasts as long as the program, as [`current`] hands out.
    fn leak(self) -> &'static Scheme {
        Box::leak(Box::new(Scheme {
            name: Box::leak(self.name.into_boxed_str()),
            dark: self.dark,
            palette: self.palette,
            tokens: self.tokens,
        }))
    }
}

/// The keys of the `palette` of a theme.
const PALETTE_KEYS: [&str; 13] = [
    "bg",
    "panel",
    "surface",
    "hover",
    "border",
    "text",
    "text_dim",
    "accent",
    "accent_text",
    "selected",
    "canvas",
    "success",
    "error",
];

/// The keys of the `tokens` of a theme, in the order a scheme holds them.
const TOKEN_KEYS: [&str; 6] = ["keyword", "type", "function", "string", "number", "comment"];

/// The colour of a palette that the key `key` sets.
fn palette_color<'a>(palette: &'a mut Palette, key: &str) -> Option<&'a mut Color32> {
    Some(match key {
        "bg" => &mut palette.bg,
        "panel" => &mut palette.panel,
        "surface" => &mut palette.surface,
        "hover" => &mut palette.hover,
        "border" => &mut palette.border,
        "text" => &mut palette.text,
        "text_dim" => &mut palette.text_dim,
        "accent" => &mut palette.accent,
        "accent_text" => &mut palette.accent_text,
        "selected" => &mut palette.selected,
        "canvas" => &mut palette.canvas,
        "success" => &mut palette.success,
        "error" => &mut palette.error,
        _ => return None,
    })
}

/// The colour written as `#rgb`, `#rrggbb` or `#rrggbbaa`.
fn parse_color(text: &str) -> Option<Color32> {
    let digits: Option<Vec<u8>> = text
        .trim()
        .strip_prefix('#')?
        .chars()
        .map(hex_digit)
        .collect();
    let digits = digits?;
    let channels: Vec<u8> = match digits.len() {
        3 => digits.iter().map(|digit| digit * 0x11).collect(),
        6 | 8 => digits
            .chunks(2)
            .map(|pair| pair[0] * 0x10 + pair[1])
            .collect(),
        _ => return None,
    };
    let alpha = channels.get(3).copied().unwrap_or(u8::MAX);
    Some(Color32::from_rgba_unmultiplied(
        channels[0],
        channels[1],
        channels[2],
        alpha,
    ))
}

fn hex_digit(ch: char) -> Option<u8> {
    ch.to_digit(16).map(|digit| digit as u8)
}

/// The keys of an object and their values, without the comments.
fn entries(object: &Map<String, Value>) -> impl Iterator<Item = (&str, &Value)> {
    let said = object.iter().filter(|(key, _)| !key.starts_with("//"));
    said.map(|(key, value)| (key.as_str(), value))
}

fn unknown_key(place: &str, key: &str, expected: &[&str]) -> String {
    let expected = expected.join(", ");
    format!("{place}: unknown key \"{key}\" (the keys are: {expected})")
}

/// The colours an object such as `palette` gives, each handed to `set` with its key, which
/// is one of `keys`.
fn read_colors(
    place: &str,
    group: &str,
    value: &Value,
    keys: &[&str],
    mut set: impl FnMut(&str, Color32),
) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{place}: \"{group}\" must be an object, not {value}"))?;
    for (key, written) in entries(object) {
        let path = format!("{group}.{key}");
        if !keys.contains(&key) {
            return Err(unknown_key(place, &path, keys));
        }
        let color = written.as_str().and_then(parse_color).ok_or_else(|| {
            format!(
                "{place}: {path}: {written} is not a colour (write \"#rgb\", \"#rrggbb\" or \"#rrggbbaa\")"
            )
        })?;
        set(key, color);
    }
    Ok(())
}

/// The theme that the `nth` object of the list describes, `nth` counting from one.
fn user_theme(nth: usize, value: &Value) -> Result<UserTheme, String> {
    let place = format!("theme {nth}");
    let object = value
        .as_object()
        .ok_or_else(|| format!("{place} must be an object, not {value}"))?;
    let name = match object.get("name") {
        Some(Value::String(name)) if !name.trim().is_empty() => name.trim().to_owned(),
        Some(Value::String(_)) | None => return Err(format!("{place} has no \"name\"")),
        Some(other) => return Err(format!("{place}: \"name\" must be a text, not {other}")),
    };
    let place = format!("theme \"{name}\"");
    if SCHEMES.iter().any(|scheme| scheme.name == name) {
        return Err(format!("{place}: a built-in theme already has this name"));
    }
    let dark = match object.get("dark") {
        None => true,
        Some(Value::Bool(dark)) => *dark,
        Some(other) => {
            return Err(format!(
                "{place}: \"dark\" must be true or false, not {other}"
            ));
        }
    };
    let base = &SCHEMES[if dark { 0 } else { 1 }];
    let mut theme = UserTheme {
        name,
        dark,
        palette: base.palette,
        tokens: base.tokens,
    };
    for (key, value) in entries(object) {
        match key {
            "name" | "dark" => {}
            "palette" => read_colors(&place, key, value, &PALETTE_KEYS, |key, color| {
                if let Some(slot) = palette_color(&mut theme.palette, key) {
                    *slot = color;
                }
            })?,
            "tokens" => read_colors(&place, key, value, &TOKEN_KEYS, |key, color| {
                if let Some(index) = TOKEN_KEYS.iter().position(|known| *known == key) {
                    theme.tokens[index] = color;
                }
            })?,
            _ => {
                return Err(unknown_key(
                    &place,
                    key,
                    &["name", "dark", "palette", "tokens"],
                ));
            }
        }
    }
    Ok(theme)
}

/// The themes a JSON text describes, in the shape of [`EXAMPLE`]. A colour a theme leaves
/// out is that of the first built-in scheme, or of the second when the theme is light. Of
/// two themes of one name, the later is kept. The error names the key or the value at fault.
// Waiting to be wired: `app.rs` reads the file of the user at startup and when it is saved.
#[allow(dead_code)]
pub fn parse_user_themes(json: &str) -> Result<Vec<UserTheme>, String> {
    let root: Value =
        serde_json::from_str(json).map_err(|error| format!("this is not JSON: {error}"))?;
    let object = root
        .as_object()
        .ok_or_else(|| format!("the file must hold an object, not {root}"))?;
    let mut themes: Vec<UserTheme> = Vec::new();
    for (key, value) in entries(object) {
        if key != "themes" {
            return Err(unknown_key("the file", key, &["themes"]));
        }
        let list = value
            .as_array()
            .ok_or_else(|| format!("\"themes\" must be a list, not {value}"))?;
        for (index, described) in list.iter().enumerate() {
            let theme = user_theme(index + 1, described)?;
            themes.retain(|earlier| earlier.name != theme.name);
            themes.push(theme);
        }
    }
    Ok(themes)
}

/// Make `themes` the themes of the user, in place of those set before: they are the schemes
/// after the built-in ones, in this order. The scheme in use stays as it is, even when it is
/// one of those replaced, until [`apply`] chooses again.
// Waiting to be wired.
#[allow(dead_code)]
pub fn set_user_themes(themes: Vec<UserTheme>) {
    let schemes = themes.into_iter().map(UserTheme::leak).collect();
    *USER_SCHEMES.write().unwrap_or_else(PoisonError::into_inner) = schemes;
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard};

    use super::*;

    const MARK: Color32 = Color32::from_rgb(0x12, 0x34, 0x56);

    const TOKENS: [(&str, Token); 6] = [
        ("keyword", Token::Keyword),
        ("type", Token::Type),
        ("function", Token::Function),
        ("string", Token::String),
        ("number", Token::Number),
        ("comment", Token::Comment),
    ];

    /// The file holding one theme, `body` being what stands between its braces, written
    /// with `'` for `"`.
    fn file_of(body: &str) -> String {
        format!("{{\"themes\": [{{{}}}]}}", body.replace('\'', "\""))
    }

    fn theme(body: &str) -> UserTheme {
        let mut themes = parse_user_themes(&file_of(body)).expect("the theme is valid");
        assert_eq!(themes.len(), 1);
        themes.remove(0)
    }

    fn refusal(body: &str) -> String {
        parse_user_themes(&file_of(body)).expect_err("the theme is refused")
    }

    /// The colours of a palette, in the order of [`PALETTE_KEYS`].
    fn colors(palette: &Palette) -> [Color32; 13] {
        [
            palette.bg,
            palette.panel,
            palette.surface,
            palette.hover,
            palette.border,
            palette.text,
            palette.text_dim,
            palette.accent,
            palette.accent_text,
            palette.selected,
            palette.canvas,
            palette.success,
            palette.error,
        ]
    }

    /// The scheme in use and the themes of the user belong to the whole program: the tests
    /// that change them run one at a time, and each starts with no theme of the user.
    fn alone() -> MutexGuard<'static, ()> {
        static TURN: Mutex<()> = Mutex::new(());
        let turn = TURN.lock().unwrap_or_else(PoisonError::into_inner);
        set_user_themes(Vec::new());
        turn
    }

    #[test]
    fn the_example_parses_into_the_two_themes_it_describes() {
        let themes = parse_user_themes(EXAMPLE).expect("the example is valid");
        let names: Vec<&str> = themes.iter().map(|theme| theme.name.as_str()).collect();
        assert_eq!(names, ["My Dark Theme", "My Light Theme"]);

        let dark = &themes[0];
        assert!(dark.dark);
        assert_eq!(dark.palette.bg, rgb(0x1E_1E_2E));
        assert_eq!(dark.palette.error, rgb(0xF3_8B_A8));
        assert_eq!(
            dark.tokens,
            tokens([0xCBA6F7, 0xF9E2AF, 0x89B4FA, 0xA6E3A1, 0xFAB387, 0x6C7086])
        );
        // It says every colour: none is left from its base.
        let base = colors(&Palette::DARK);
        assert!(
            colors(&dark.palette)
                .iter()
                .zip(base)
                .all(|(said, base)| *said != base)
        );

        let light = &themes[1];
        assert!(!light.dark);
        assert_eq!(light.palette.bg, rgb(0xFB_F7_F0));
        assert_eq!(light.palette.accent, rgb(0xB4_63_7A));
        assert_eq!(light.palette.text, Palette::LIGHT.text);
        assert_eq!(light.tokens[5], rgb(0x98_93_A5));
        assert_eq!(light.tokens[0], SCHEMES[1].tokens[0]);
    }

    #[test]
    fn the_example_comments_every_key_of_the_palette() {
        for key in PALETTE_KEYS {
            assert!(EXAMPLE.contains(&format!("\"// {key}\": ")), "{key}");
        }
    }

    #[test]
    fn each_key_of_the_palette_sets_its_own_colour_and_no_other() {
        for (index, key) in PALETTE_KEYS.iter().enumerate() {
            let theme = theme(&format!("'name': 'One', 'palette': {{'{key}': '#123456'}}"));
            let mut expected = colors(&Palette::DARK);
            expected[index] = MARK;
            assert_eq!(colors(&theme.palette), expected, "{key}");
            assert_eq!(theme.tokens, SCHEMES[0].tokens, "{key}");
        }
    }

    #[test]
    fn each_key_of_the_tokens_colours_its_own_token_and_no_other() {
        for (key, token) in TOKENS {
            let theme = theme(&format!("'name': 'One', 'tokens': {{'{key}': '#123456'}}"));
            assert_eq!(theme.palette, Palette::DARK, "{key}");
            let scheme = theme.leak();
            for (_, other) in TOKENS {
                let expected = if other == token {
                    MARK
                } else {
                    SCHEMES[0].token(other)
                };
                assert_eq!(scheme.token(other), expected, "{key} on {other:?}");
            }
            assert_eq!(scheme.token(Token::Plain), Palette::DARK.text, "{key}");
        }
    }

    #[test]
    fn the_text_of_the_palette_colours_the_code_that_is_no_token() {
        let scheme = theme("'name': 'One', 'palette': {'text': '#123456'}").leak();
        assert_eq!(scheme.token(Token::Plain), MARK);
    }

    #[test]
    fn a_dark_theme_keeps_what_it_does_not_say_from_godcode_dark() {
        for body in ["'name': 'Mine'", "'name': 'Mine', 'dark': true"] {
            let theme = theme(body);
            assert!(theme.dark);
            assert_eq!(theme.palette, Palette::DARK);
            assert_eq!(theme.tokens, SCHEMES[0].tokens);
        }
        let theme = theme("'name': 'Mine', 'palette': {'bg': '#123456'}, 'tokens': {}");
        assert_eq!(
            theme.palette,
            Palette {
                bg: MARK,
                ..Palette::DARK
            }
        );
        assert_eq!(theme.tokens, SCHEMES[0].tokens);
    }

    #[test]
    fn a_light_theme_keeps_what_it_does_not_say_from_godcode_light() {
        let theme = theme("'name': 'Mine', 'dark': false, 'tokens': {'string': '#123456'}");
        assert!(!theme.dark);
        assert_eq!(theme.palette, Palette::LIGHT);
        let mut expected = SCHEMES[1].tokens;
        expected[3] = MARK;
        assert_eq!(theme.tokens, expected);
    }

    #[test]
    fn a_colour_is_written_with_three_six_or_eight_digits() {
        assert_eq!(parse_color("#123456"), Some(MARK));
        assert_eq!(parse_color("#ABCDEF"), Some(rgb(0xAB_CD_EF)));
        assert_eq!(parse_color("#abcdef"), Some(rgb(0xAB_CD_EF)));
        assert_eq!(parse_color("#1af"), Some(rgb(0x11_AA_FF)));
        assert_eq!(parse_color("#123456ff"), Some(MARK));
        let half = Color32::from_rgba_unmultiplied(0x12, 0x34, 0x56, 0x80);
        assert_eq!(parse_color("#12345680"), Some(half));
        assert_ne!(half, MARK);
    }

    #[test]
    fn what_is_not_a_colour_is_refused_with_its_key_and_what_was_written() {
        for written in [
            "123456",
            "#12",
            "#1234",
            "#12345",
            "#1234567",
            "#123456789",
            "#12345g",
            "red",
            "",
            "#",
        ] {
            assert_eq!(parse_color(written), None, "{written}");
            let error = refusal(&format!(
                "'name': 'Mine', 'palette': {{'hover': '{written}'}}"
            ));
            assert!(error.contains("theme \"Mine\""), "{error}");
            assert!(error.contains("palette.hover"), "{error}");
            assert!(error.contains(&format!("\"{written}\"")), "{error}");
        }
        let error = refusal("'name': 'Mine', 'tokens': {'number': 'blue'}");
        assert!(
            error.contains("tokens.number") && error.contains("\"blue\""),
            "{error}"
        );
        let error = refusal("'name': 'Mine', 'tokens': {'number': 255}");
        assert!(
            error.contains("tokens.number") && error.contains("255"),
            "{error}"
        );
    }

    #[test]
    fn a_key_that_is_not_known_is_refused_by_its_name() {
        let error = refusal("'name': 'Mine', 'palete': {}");
        assert!(error.contains("unknown key \"palete\""), "{error}");
        assert!(error.contains("theme \"Mine\""), "{error}");
        let error = refusal("'name': 'Mine', 'palette': {'background': '#123456'}");
        assert!(
            error.contains("unknown key \"palette.background\""),
            "{error}"
        );
        let error = refusal("'name': 'Mine', 'tokens': {'keywords': '#123456'}");
        assert!(error.contains("unknown key \"tokens.keywords\""), "{error}");
        // A key of the palette is not one of the tokens.
        let error = refusal("'name': 'Mine', 'tokens': {'bg': '#123456'}");
        assert!(error.contains("unknown key \"tokens.bg\""), "{error}");
        let error = parse_user_themes(r#"{"theme": []}"#).expect_err("themes is misspelt");
        assert!(error.contains("unknown key \"theme\""), "{error}");
    }

    #[test]
    fn a_theme_without_a_name_is_refused_by_its_place_in_the_list() {
        let error = parse_user_themes(r#"{"themes": [{"name": "One"}, {"dark": false}]}"#)
            .expect_err("the second theme has no name");
        assert!(
            error.contains("theme 2") && error.contains("\"name\""),
            "{error}"
        );
        let error = refusal("'name': '  '");
        assert!(
            error.contains("theme 1") && error.contains("\"name\""),
            "{error}"
        );
        let error = refusal("'name': 3");
        assert!(
            error.contains("theme 1") && error.contains("\"name\""),
            "{error}"
        );
    }

    #[test]
    fn a_value_of_the_wrong_kind_is_refused_with_its_key() {
        let error = refusal("'name': 'Mine', 'dark': 'yes'");
        assert!(
            error.contains("\"dark\"") && error.contains("\"yes\""),
            "{error}"
        );
        let error = refusal("'name': 'Mine', 'palette': '#123456'");
        assert!(error.contains("\"palette\""), "{error}");
        let error = parse_user_themes(r#"{"themes": {"name": "Mine"}}"#).expect_err("no list");
        assert!(error.contains("\"themes\""), "{error}");
        let error = parse_user_themes(r#"{"themes": ["Mine"]}"#).expect_err("no object");
        assert!(
            error.contains("theme 1") && error.contains("\"Mine\""),
            "{error}"
        );
        let error = parse_user_themes("[]").expect_err("no object");
        assert!(error.contains("[]"), "{error}");
    }

    #[test]
    fn a_text_that_is_not_json_is_refused_with_the_line_at_fault() {
        let error = parse_user_themes("{\n  \"themes\": [\n    {\"name\": \"Mine\",}\n  ]\n}")
            .expect_err("a comma too many");
        assert!(error.contains("not JSON"), "{error}");
        assert!(error.contains("line 3"), "{error}");
        assert!(parse_user_themes("").is_err());
    }

    #[test]
    fn a_file_without_themes_holds_none() {
        assert_eq!(parse_user_themes("{}"), Ok(Vec::new()));
        assert_eq!(parse_user_themes(r#"{"themes": []}"#), Ok(Vec::new()));
    }

    #[test]
    fn a_key_that_starts_with_two_slashes_is_a_comment_wherever_it_stands() {
        let commented = parse_user_themes(
            r##"{"// a": 1, "themes": [{"//": [], "name": "Mine",
                "palette": {"// bg": "the back", "bg": "#123456"},
                "tokens": {"//string": "not a colour"}}]}"##,
        );
        assert_eq!(
            commented,
            Ok(vec![theme("'name': 'Mine', 'palette': {'bg': '#123456'}")])
        );
    }

    #[test]
    fn of_two_themes_of_one_name_the_later_is_kept() {
        let themes = parse_user_themes(
            r##"{"themes": [
                {"name": "Twice", "palette": {"bg": "#111111"}},
                {"name": "Other"},
                {"name": "Twice", "dark": false, "palette": {"panel": "#123456"}}
            ]}"##,
        )
        .expect("the themes are valid");
        let names: Vec<&str> = themes.iter().map(|theme| theme.name.as_str()).collect();
        assert_eq!(names, ["Other", "Twice"]);
        assert_eq!(
            themes[1].palette,
            Palette {
                panel: MARK,
                ..Palette::LIGHT
            }
        );
    }

    #[test]
    fn a_theme_may_not_take_the_name_of_a_built_in_one() {
        let error = refusal("'name': 'Monokai'");
        assert!(
            error.contains("\"Monokai\"") && error.contains("built-in"),
            "{error}"
        );
    }

    #[test]
    fn the_themes_of_the_user_are_listed_after_the_built_in_ones() {
        let _turn = alone();
        let built_in: Vec<&str> = SCHEMES.iter().map(|scheme| scheme.name).collect();
        assert_eq!(scheme_names(), built_in);
        assert!(scheme(SCHEMES.len()).is_none());

        set_user_themes(parse_user_themes(EXAMPLE).expect("the example is valid"));
        let names = scheme_names();
        assert_eq!(names[..SCHEMES.len()], built_in);
        assert_eq!(names[SCHEMES.len()..], ["My Dark Theme", "My Light Theme"]);
        for (index, name) in names.iter().enumerate() {
            assert_eq!(scheme_index(name), Some(index));
            assert_eq!(scheme(index).map(|scheme| scheme.name), Some(*name));
        }
        assert_eq!(scheme_index("No Such Theme"), None);
        assert!(scheme(names.len()).is_none());
        set_user_themes(Vec::new());
    }

    #[test]
    fn a_theme_of_the_user_is_installed_like_a_built_in_one() {
        let _turn = alone();
        let ctx = Context::default();
        set_user_themes(parse_user_themes(EXAMPLE).expect("the example is valid"));

        install(&ctx, SCHEMES.len());
        assert_eq!(current().name, "My Dark Theme");
        assert!(current().dark);
        assert_eq!(Palette::of_ctx(&ctx).bg, rgb(0x1E_1E_2E));
        assert_eq!(token_color(Token::Keyword, true), rgb(0xCB_A6_F7));
        assert_eq!(token_color(Token::Plain, true), rgb(0xCD_D6_F4));
        assert_eq!(ctx.theme(), Theme::Dark);
        assert_eq!(
            ctx.style_of(Theme::Dark).visuals.panel_fill,
            rgb(0x18_18_25)
        );

        install(&ctx, SCHEMES.len() + 1);
        assert_eq!(current().name, "My Light Theme");
        assert!(!current().dark);
        assert_eq!(Palette::of_ctx(&ctx).accent, rgb(0xB4_63_7A));
        assert_eq!(token_color(Token::Comment, false), rgb(0x98_93_A5));
        assert_eq!(
            token_color(Token::Keyword, false),
            SCHEMES[1].token(Token::Keyword)
        );
        assert_eq!(ctx.theme(), Theme::Light);
        assert_eq!(
            ctx.style_of(Theme::Light).visuals.extreme_bg_color,
            rgb(0xFB_F7_F0)
        );

        apply(&ctx, 0);
        set_user_themes(Vec::new());
    }

    #[test]
    fn a_built_in_scheme_is_installed_as_before() {
        let _turn = alone();
        let ctx = Context::default();
        set_user_themes(parse_user_themes(EXAMPLE).expect("the example is valid"));
        for (index, scheme) in SCHEMES.iter().enumerate() {
            install(&ctx, index);
            assert_eq!(current().name, scheme.name);
            assert_eq!(Palette::of_ctx(&ctx), scheme.palette);
            assert_eq!(token_color(Token::String, scheme.dark), scheme.tokens[3]);
            let theme = if scheme.dark {
                Theme::Dark
            } else {
                Theme::Light
            };
            assert_eq!(ctx.theme(), theme);
            assert_eq!(ctx.style_of(theme).visuals.panel_fill, scheme.palette.panel);
        }
        assert_eq!(SCHEMES[3].name, "Monokai");
        assert_eq!(SCHEMES[3].token(Token::Keyword), rgb(0xF9_26_72));

        apply(&ctx, 0);
        set_user_themes(Vec::new());
    }

    #[test]
    fn an_index_that_names_no_scheme_installs_the_first_one() {
        let _turn = alone();
        let ctx = Context::default();
        install(&ctx, 3);
        install(&ctx, SCHEMES.len());
        assert_eq!(current().name, SCHEMES[0].name);
        assert_eq!(ctx.theme(), Theme::Dark);
    }

    #[test]
    fn a_new_list_replaces_the_themes_of_the_user_and_leaves_the_built_in_ones() {
        let _turn = alone();
        let ctx = Context::default();
        let built_in: Vec<&str> = SCHEMES.iter().map(|scheme| scheme.name).collect();
        set_user_themes(parse_user_themes(EXAMPLE).expect("the example is valid"));
        install(&ctx, SCHEMES.len());

        set_user_themes(vec![theme("'name': 'Third', 'palette': {'bg': '#123456'}")]);
        let names = scheme_names();
        assert_eq!(names[..SCHEMES.len()], built_in);
        assert_eq!(names[SCHEMES.len()..], ["Third"]);
        assert_eq!(scheme_index("My Dark Theme"), None);
        assert!(scheme(SCHEMES.len() + 1).is_none());
        for (index, built) in SCHEMES.iter().enumerate() {
            assert!(std::ptr::eq(
                scheme(index).expect("a built-in scheme"),
                built
            ));
        }
        // The theme in use was replaced: it stays in use until another is chosen.
        assert_eq!(current().name, "My Dark Theme");
        assert_eq!(Palette::of_ctx(&ctx).bg, rgb(0x1E_1E_2E));

        apply(&ctx, SCHEMES.len());
        assert_eq!(current().name, "Third");
        assert_eq!(Palette::of_ctx(&ctx).bg, MARK);

        apply(&ctx, 0);
        set_user_themes(Vec::new());
    }
}
