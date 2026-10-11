//! GodCode: desktop entry point.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod branding;
mod editor;
mod explorer;
mod field;
mod live;
mod platform;
mod preview;
mod servers;
mod terminal;
mod theme;
mod toasts;
mod tools;
mod webview;
mod widgets;

pub const APP_NAME: &str = "GodCode";

fn main() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_app_id("godcode")
        .with_inner_size([1280.0, 860.0])
        .with_min_inner_size([760.0, 480.0])
        .with_drag_and_drop(true);
    if let Some(icon) = branding::icon_data() {
        viewport = viewport.with_icon(icon);
    }
    let paths = std::env::args_os().skip(1).map(Into::into).collect();
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| Ok(Box::new(app::GodApp::new(cc, paths)))),
    )
}
