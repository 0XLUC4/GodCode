use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use egui::text::{LayoutJob, TextFormat};
use egui::{
    Button, Color32, Context, FontFamily, FontId, Id, Modal, Response, RichText, ScrollArea, Ui,
    vec2,
};
use egui_phosphor::regular as icon;

const REPO: &str = "0XLUC4/GodCode";
const ASSET_NAME: &str = "GodCode-windows-x64.exe";
const BOLD_FAMILY: &str = "updates-bold";
const DOWNLOAD_BLUE: Color32 = Color32::from_rgb(0x25, 0x63, 0xEB);
const CAN_INSTALL: bool = cfg!(windows);

#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    pub name: String,
    pub date: String,
    pub notes: String,
    pub url: String,
    pub asset: Option<String>,
}

impl Release {
    fn is_newer(&self) -> bool {
        is_newer(&self.tag, env!("CARGO_PKG_VERSION"))
    }
}

#[derive(Clone)]
enum Check {
    Idle,
    Loading,
    Ready(Vec<Release>),
    Failed(String),
}

#[derive(Clone)]
enum Install {
    Idle,
    Downloading,
    Failed(String),
}

struct Inner {
    check: Check,
    install: Install,
    open: bool,
    restart: Option<PathBuf>,
    close_now: bool,
}

#[derive(Clone)]
pub struct Updates(Arc<Mutex<Inner>>);

impl Default for Updates {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Inner {
            check: Check::Idle,
            install: Install::Idle,
            open: false,
            restart: None,
            close_now: false,
        })))
    }
}

pub fn add_bold_font(fonts: &mut egui::FontDefinitions) {
    let mut family = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    if let Some(bytes) = bold_font() {
        fonts.font_data.insert(
            BOLD_FAMILY.to_owned(),
            egui::FontData::from_owned(bytes).into(),
        );
        family.insert(0, BOLD_FAMILY.to_owned());
    }
    fonts
        .families
        .insert(FontFamily::Name(BOLD_FAMILY.into()), family);
}

fn bold_font() -> Option<Vec<u8>> {
    let windows =
        std::env::var_os("WINDIR").map(|dir| PathBuf::from(dir).join("Fonts").join("arialbd.ttf"));
    let unix = [
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        "/Library/Fonts/Arial Bold.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
        "/usr/share/fonts/liberation-sans/LiberationSans-Bold.ttf",
        "/usr/share/fonts/TTF/LiberationSans-Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    ];
    windows
        .into_iter()
        .chain(unix.iter().map(PathBuf::from))
        .find_map(|path| std::fs::read(path).ok())
}

impl Updates {
    fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> Option<T> {
        self.0.lock().ok().map(|mut inner| f(&mut inner))
    }

    pub fn start(&self, ctx: &Context) {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::fs::remove_file(old_program(&exe));
        }
        self.check(ctx);
    }

    fn check(&self, ctx: &Context) {
        let busy = self
            .with(|inner| {
                let busy = matches!(inner.check, Check::Loading);
                if !busy {
                    inner.check = Check::Loading;
                }
                busy
            })
            .unwrap_or(true);
        if busy {
            return;
        }
        let shared = self.clone();
        let ctx = ctx.clone();
        self.background("updates", move || {
            let check = match fetch() {
                Ok(releases) => Check::Ready(releases),
                Err(err) => Check::Failed(err),
            };
            shared.with(|inner| inner.check = check);
            ctx.request_repaint();
        });
    }

    fn background(&self, name: &str, work: impl FnOnce() + Send + 'static) {
        if std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(work)
            .is_err()
        {
            self.with(|inner| inner.check = Check::Failed("Cannot start in the background".into()));
        }
    }

    fn newest(&self) -> Option<Release> {
        self.with(|inner| match &inner.check {
            Check::Ready(releases) => releases.iter().find(|release| release.is_newer()).cloned(),
            _ => None,
        })
        .flatten()
    }

    pub fn open(&self, ctx: &Context) {
        let ready = self
            .with(|inner| {
                inner.open = true;
                matches!(inner.check, Check::Ready(_))
            })
            .unwrap_or(false);
        if !ready {
            self.check(ctx);
        }
    }

    pub fn button(&self, ui: &mut Ui) -> Response {
        let newest = self.newest();
        let tip = newest.as_ref().map_or_else(
            || "Updates".to_owned(),
            |release| format!("Update available: {}", release.tag),
        );
        let response = ui
            .add_sized(
                vec2(22.0, 22.0),
                Button::new(RichText::new(icon::ARROW_CIRCLE_UP).size(14.0))
                    .frame_when_inactive(false),
            )
            .on_hover_text(tip);
        if newest.is_some() {
            let dot = response.rect.right_top() + vec2(-4.0, 4.0);
            ui.painter().circle_filled(dot, 3.5, DOWNLOAD_BLUE);
        }
        if response.clicked() {
            self.open(ui.ctx());
        }
        response
    }

    pub fn relaunch(&self) {
        if let Some(Some(program)) = self.with(|inner| inner.restart.take()) {
            let _ = std::process::Command::new(program).spawn();
        }
    }

    pub fn show(&self, ctx: &Context) {
        if self.with(|inner| std::mem::take(&mut inner.close_now)) == Some(true) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.with(|inner| inner.open) != Some(true) {
            return;
        }
        let mut close = false;
        let response = Modal::new(Id::new("updates")).show(ctx, |ui| {
            ui.set_width(460.0);
            self.body(ui);
            ui.add_space(12.0);
            close = ui.button("Close").clicked();
        });
        if close || response.should_close() {
            self.with(|inner| inner.open = false);
        }
    }

    fn body(&self, ui: &mut Ui) {
        let dim = ui.visuals().weak_text_color();
        let error = ui.visuals().error_fg_color;
        ui.label(
            RichText::new(format!("{}  Updates", icon::ARROW_CIRCLE_UP))
                .size(18.0)
                .strong(),
        );
        ui.add_space(6.0);
        let current = env!("CARGO_PKG_VERSION");
        let Some(check) = self.with(|inner| inner.check.clone()) else {
            return;
        };
        match check {
            Check::Idle | Check::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Looking for new versions…");
                });
            }
            Check::Failed(err) => {
                ui.label(RichText::new(err).color(error));
                if ui
                    .button(format!("{}  Try again", icon::ARROW_CLOCKWISE))
                    .clicked()
                {
                    self.check(ui.ctx());
                }
            }
            Check::Ready(releases) => {
                let newest = releases.iter().find(|release| release.is_newer());
                let status = match newest {
                    Some(release) => {
                        format!("{} is out. You have version {current}.", release.tag)
                    }
                    None => format!("You have the latest version ({current})."),
                };
                ui.label(RichText::new(status).strong());
                ui.add_space(6.0);
                if releases.is_empty() {
                    ui.label(RichText::new("No release published yet.").color(dim));
                }
                ScrollArea::vertical()
                    .max_height(380.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for release in &releases {
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&release.name).size(16.0).strong());
                                if release.is_newer() {
                                    ui.label(RichText::new("New").color(DOWNLOAD_BLUE).strong());
                                }
                            });
                            ui.label(
                                RichText::new(format!("{} · {}", release.tag, release.date))
                                    .small()
                                    .color(dim),
                            );
                            if !release.notes.is_empty() {
                                markdown(ui, &release.notes);
                            }
                            let latest = newest.is_some_and(|newest| newest.tag == release.tag);
                            match (&release.asset, latest && CAN_INSTALL) {
                                (Some(asset), true) => self.install_row(ui, asset),
                                _ if !release.url.is_empty() => {
                                    ui.hyperlink_to(
                                        format!("{}  View on GitHub", icon::ARROW_SQUARE_OUT),
                                        &release.url,
                                    );
                                }
                                _ => {}
                            }
                        }
                    });
            }
        }
        ui.add_space(6.0);
        ui.hyperlink_to(
            "All releases on GitHub",
            format!("https://github.com/{REPO}/releases"),
        );
    }

    fn install_row(&self, ui: &mut Ui, asset: &str) {
        ui.add_space(4.0);
        let Some(state) = self.with(|inner| inner.install.clone()) else {
            return;
        };
        if matches!(state, Install::Downloading) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!(
                    "Downloading the update… {} restarts when it is done.",
                    crate::APP_NAME
                ));
            });
            return;
        }
        let text = RichText::new(format!("{}  Download and install", icon::DOWNLOAD_SIMPLE))
            .color(Color32::WHITE)
            .strong();
        let button = Button::new(text)
            .fill(DOWNLOAD_BLUE)
            .min_size(vec2(0.0, 30.0));
        if ui
            .add(button)
            .on_hover_text(format!(
                "Replace {} with this version and restart",
                crate::APP_NAME
            ))
            .clicked()
        {
            self.install(ui.ctx(), asset.to_owned());
        }
        if let Install::Failed(err) = state {
            ui.label(RichText::new(err).color(ui.visuals().error_fg_color));
        }
    }

    fn install(&self, ctx: &Context, url: String) {
        self.with(|inner| inner.install = Install::Downloading);
        let shared = self.clone();
        let ctx = ctx.clone();
        self.background("install", move || {
            let result = replace_program(&url);
            shared.with(|inner| match result {
                Ok(program) => {
                    inner.install = Install::Idle;
                    inner.open = false;
                    inner.restart = Some(program);
                    inner.close_now = true;
                }
                Err(err) => inner.install = Install::Failed(err),
            });
            ctx.request_repaint();
        });
    }
}

fn markdown(ui: &mut Ui, text: &str) {
    let color = ui.visuals().text_color();
    let size = 14.0;
    let line_job = |line: &str, size: f32, all_bold: bool, width: f32| {
        let mut job = LayoutJob::default();
        job.wrap.max_width = width;
        for (index, part) in line.split("**").enumerate() {
            let family = if all_bold || index % 2 == 1 {
                FontFamily::Name(BOLD_FAMILY.into())
            } else {
                FontFamily::Proportional
            };
            job.append(
                &part.replace('`', ""),
                0.0,
                TextFormat {
                    font_id: FontId::new(size, family),
                    color,
                    ..Default::default()
                },
            );
        }
        job
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            ui.add_space(4.0);
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix('#') {
            let heading = heading.trim_start_matches('#').trim();
            ui.label(line_job(heading, size + 1.0, true, ui.available_width()));
            continue;
        }
        let bullet = ["- ", "* ", "+ "]
            .iter()
            .find_map(|marker| trimmed.strip_prefix(marker));
        match bullet {
            Some(item) => {
                ui.horizontal_top(|ui| {
                    ui.label(RichText::new("•").size(size).color(color));
                    let width = ui.available_width();
                    ui.label(line_job(item, size, false, width));
                });
            }
            None => {
                ui.label(line_job(trimmed, size, false, ui.available_width()));
            }
        }
    }
}

fn user_agent() -> String {
    format!("{}/{}", crate::APP_NAME, env!("CARGO_PKG_VERSION"))
}

fn fetch() -> Result<Vec<Release>, String> {
    let body = ureq::get(format!("https://api.github.com/repos/{REPO}/releases"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", user_agent())
        .call()
        .map_err(|err| format!("Cannot reach GitHub: {err}"))?
        .body_mut()
        .read_to_string()
        .map_err(|err| format!("Cannot read the releases: {err}"))?;
    parse(&body)
}

fn old_program(exe: &Path) -> PathBuf {
    exe.with_extension("old.exe")
}

fn replace_program(url: &str) -> Result<PathBuf, String> {
    let name = crate::APP_NAME;
    let exe = std::env::current_exe().map_err(|err| format!("Cannot find {name}: {err}"))?;
    let new = exe.with_extension("new.exe");
    let old = old_program(&exe);
    let mut response = ureq::get(url)
        .header("User-Agent", user_agent())
        .call()
        .map_err(|err| format!("Cannot download the update: {err}"))?;
    let mut file =
        std::fs::File::create(&new).map_err(|err| format!("Cannot write the update: {err}"))?;
    let written = std::io::copy(&mut response.body_mut().as_reader(), &mut file)
        .map_err(|err| format!("Download interrupted: {err}"));
    drop(file);
    match written {
        Ok(bytes) if bytes > 0 => {}
        Ok(_) => {
            let _ = std::fs::remove_file(&new);
            return Err("The download is empty".to_owned());
        }
        Err(err) => {
            let _ = std::fs::remove_file(&new);
            return Err(err);
        }
    }
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|err| {
        let _ = std::fs::remove_file(&new);
        format!("Cannot replace {name}: {err}")
    })?;
    if let Err(err) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe);
        let _ = std::fs::remove_file(&new);
        return Err(format!("Cannot replace {name}: {err}"));
    }
    Ok(exe)
}

fn parse(body: &str) -> Result<Vec<Release>, String> {
    let list: Vec<serde_json::Value> = serde_json::from_str(body)
        .map_err(|err| format!("Unexpected answer from GitHub: {err}"))?;
    let text = |value: &serde_json::Value, key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let mut releases: Vec<Release> = list
        .iter()
        .filter(|value| {
            !value
                .get("draft")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
        .map(|value| {
            let tag = text(value, "tag_name");
            let name = text(value, "name");
            let assets = value
                .get("assets")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            let asset = assets
                .iter()
                .find(|asset| text(asset, "name") == ASSET_NAME)
                .or_else(|| {
                    assets
                        .iter()
                        .find(|asset| text(asset, "name").to_ascii_lowercase().ends_with(".exe"))
                })
                .map(|asset| text(asset, "browser_download_url"))
                .filter(|url| !url.is_empty());
            Release {
                asset,
                name: if name.is_empty() { tag.clone() } else { name },
                date: text(value, "published_at").chars().take(10).collect(),
                notes: text(value, "body").replace("\r\n", "\n"),
                url: text(value, "html_url"),
                tag,
            }
        })
        .collect();
    releases.sort_by(|a, b| version(&b.tag).cmp(&version(&a.tag)));
    Ok(releases)
}

fn version(tag: &str) -> Vec<u64> {
    tag.trim_start_matches(|ch: char| !ch.is_ascii_digit())
        .split(|ch: char| !ch.is_ascii_digit())
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

fn is_newer(tag: &str, current: &str) -> bool {
    let tag = version(tag);
    !tag.is_empty() && tag > version(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v1.1.0", "1.0.0"));
        assert!(is_newer("1.0.10", "1.0.9"));
        assert!(!is_newer("v1.0.0", "1.0.0"));
        assert!(!is_newer("nightly", "1.0.0"));
    }

    #[test]
    fn parses_releases_newest_first_with_their_program() {
        let body = format!(
            r#"[
            {{"tag_name": "v1.0.0", "name": "", "published_at": "2026-09-01T10:00:00Z",
             "body": "First", "html_url": "https://example.com/1", "draft": false}},
            {{"tag_name": "v1.2.0", "name": "Big one", "published_at": "2026-10-01T10:00:00Z",
             "body": "Second\r\nline", "html_url": "https://example.com/2", "draft": false,
             "assets": [{{"name": "notes.txt", "browser_download_url": "https://example.com/n"}},
                        {{"name": "{ASSET_NAME}", "browser_download_url": "https://example.com/exe"}}]}},
            {{"tag_name": "v9.0.0", "name": "Draft", "draft": true}}
        ]"#
        );
        let releases = parse(&body).unwrap();
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].name, "Big one");
        assert_eq!(releases[0].date, "2026-10-01");
        assert_eq!(releases[0].notes, "Second\nline");
        assert_eq!(
            releases[0].asset.as_deref(),
            Some("https://example.com/exe")
        );
        assert_eq!(releases[1].asset, None);
    }
}
