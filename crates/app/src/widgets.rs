//! Small shared widgets: fixed-size icon buttons, file icons.

use egui::{
    Align2, Button, Color32, Context, CornerRadius, FontFamily, FontId, Painter, Pos2, Rect,
    Response, RichText, Ui, Vec2, vec2,
};

use crate::tools::FileIcon;

/// Phosphor icon glyphs (regular weight), installed as a fallback of the proportional font.
pub use egui_phosphor::regular as icon;

/// Font family of the solid Phosphor glyphs: the code points of [`icon`], filled shapes.
const FILLED: &str = "phosphor-fill";
/// The font of the logos of languages and tools: devicon, under the MIT licence
/// (`assets/devicon-LICENSE.txt`).
const LOGOS: &str = "devicon";
const LOGOS_FONT: &[u8] = include_bytes!("../../../assets/devicon.ttf");

/// Side of a standard icon-only button.
pub const ICON_BUTTON: Vec2 = vec2(28.0, 28.0);
/// Side of a compact icon-only button (tab close, steppers).
pub const SMALL_ICON_BUTTON: Vec2 = vec2(22.0, 22.0);

/// Whether `bytes` are those of a font file: TrueType, OpenType or a collection of them.
pub fn is_font(bytes: &[u8]) -> bool {
    let starts: [&[u8]; 4] = [&[0, 1, 0, 0], b"OTTO", b"true", b"ttcf"];
    starts.iter().any(|start| bytes.starts_with(start))
}

/// Install the fonts. `code`: the bytes of a font file the code is drawn with, instead of
/// the built-in one. The icon fonts: outlines as a fallback of the text font, solid
/// shapes as a family of their own.
pub fn install_fonts(ctx: &Context, code: Option<Vec<u8>>) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(bytes) = code {
        let name = "code";
        fonts
            .font_data
            .insert(name.into(), egui::FontData::from_owned(bytes).into());
        // First of its family: the built-in font still draws what it lacks.
        let family = fonts.families.entry(FontFamily::Monospace).or_default();
        family.insert(0, name.into());
    }
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    fonts.font_data.insert(
        FILLED.into(),
        egui_phosphor::Variant::Fill.font_data().into(),
    );
    fonts
        .families
        .insert(FontFamily::Name(FILLED.into()), vec![FILLED.into()]);
    fonts
        .font_data
        .insert(LOGOS.into(), egui::FontData::from_static(LOGOS_FONT).into());
    fonts
        .families
        .insert(FontFamily::Name(LOGOS.into()), vec![LOGOS.into()]);
    ctx.set_fonts(fonts);
}

/// The font that draws the glyphs of [`icon`] as solid shapes.
pub fn filled(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FILLED.into()))
}

/// Frameless-until-hovered square button showing one glyph. Use with `ui.add_sized` / `ui.put`.
pub fn icon_widget(glyph: &str, size: f32) -> Button<'static> {
    Button::new(RichText::new(glyph.to_owned()).size(size))
        .frame_when_inactive(false)
        .stroke(egui::Stroke::NONE)
}

/// Standard 28×28 icon button with a tooltip.
pub fn icon_button(ui: &mut Ui, glyph: &str, tooltip: &str) -> Response {
    ui.add_sized(ICON_BUTTON, icon_widget(glyph, 16.0))
        .on_hover_text(tooltip)
}

/// Compact 22×22 icon button with a tooltip.
pub fn small_icon_button(ui: &mut Ui, glyph: &str, tooltip: &str) -> Response {
    ui.add_sized(SMALL_ICON_BUTTON, icon_widget(glyph, 13.0))
        .on_hover_text(tooltip)
}

/// 28×28 icon toggle; `on` shows a light grey square and the solid glyph.
pub fn toggle_button(ui: &mut Ui, glyph: &str, tooltip: &str, on: bool) -> Response {
    let text = RichText::new(glyph.to_owned());
    let text = if on {
        text.font(filled(16.0))
    } else {
        text.size(16.0)
    };
    ui.add_sized(
        ICON_BUTTON,
        Button::selectable(on, text)
            .frame_when_inactive(on)
            .stroke(egui::Stroke::NONE),
    )
    .on_hover_text(tooltip)
}

/// Icon followed by a label, for menus and text buttons.
pub fn with_icon(glyph: &str, label: &str) -> String {
    format!("{glyph}  {label}")
}

/// Room kept left of a label for the icon painted over its row.
pub const ICON_ROOM: f32 = 24.0;

/// `label` as the text of a button, moved right to leave room for an icon painted over
/// the row afterwards.
pub fn after_icon(ui: &Ui, label: &str) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let color = ui.visuals().text_color();
    let mut job = egui::text::LayoutJob::simple_singleline(label.to_owned(), font, color);
    job.sections[0].leading_space = ICON_ROOM;
    job
}

/// Draw the icon of a file around `center`: the logo of its language in its colour, a
/// letter badge for a language without one, or a solid glyph.
pub fn paint_file_icon(painter: &Painter, center: Pos2, icon: &FileIcon) {
    // The logo of the language, when it has one.
    if let Some(logo) = icon.logo {
        let font = FontId::new(15.0, FontFamily::Name(LOGOS.into()));
        painter.text(center, Align2::CENTER_CENTER, logo, font, icon.color);
        return;
    }
    let Some(label) = icon.badge else {
        painter.text(
            center,
            Align2::CENTER_CENTER,
            icon.glyph,
            filled(16.0),
            icon.color,
        );
        return;
    };
    let rect = Rect::from_center_size(center, vec2(17.0, 15.0));
    painter.rect_filled(rect, CornerRadius::same(3), icon.color);
    let [red, green, blue, _] = icon.color.to_array();
    let light = u32::from(red) * 299 + u32::from(green) * 587 + u32::from(blue) * 114 > 150_000;
    let ink = if light {
        Color32::from_rgb(0x14, 0x16, 0x1A)
    } else {
        Color32::WHITE
    };
    let size = if label.len() > 1 { 9.0 } else { 10.5 };
    painter.text(
        center,
        Align2::CENTER_CENTER,
        label,
        FontId::monospace(size),
        ink,
    );
}

/// Padding of text fields: as tall as a button, so the shared corner radius reads as a
/// rounded rectangle and not as a pill.
pub const FIELD_MARGIN: egui::Margin = egui::Margin::symmetric(8, 4);
