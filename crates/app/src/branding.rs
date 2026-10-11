//! Embedded logo assets.

use egui::{ColorImage, Context, TextureHandle, TextureOptions};

const LOGO_PNG: &[u8] = include_bytes!("../../../assets/logo.png");
#[cfg(not(target_arch = "wasm32"))]
const FAVICON_PNG: &[u8] = include_bytes!("../../../assets/favicon.png");

fn decode(png: &[u8]) -> Option<image::RgbaImage> {
    image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()
        .map(|img| img.to_rgba8())
}

#[cfg(not(target_arch = "wasm32"))]
/// Window icon (favicon, falling back to the full logo).
pub fn icon_data() -> Option<egui::IconData> {
    let img = decode(FAVICON_PNG).or_else(|| decode(LOGO_PNG))?;
    Some(egui::IconData {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}

/// Logo as a GPU texture, for the welcome screen, top bar and about dialog.
pub fn logo_texture(ctx: &Context) -> Option<TextureHandle> {
    let img = decode(LOGO_PNG)?;
    let size = [img.width() as usize, img.height() as usize];
    let color = ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture("godcode-logo", color, TextureOptions::LINEAR))
}
