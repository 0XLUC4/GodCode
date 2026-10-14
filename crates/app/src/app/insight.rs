//! What the other editors leave to extensions: secrets stopped before a commit, the ports
//! in use and their programs, a look through the whole project, and a pretend API served
//! from the answers written in a `.http` file.

use std::collections::HashMap;

use godcode_core::mock::{self, MockServer, Route};
use godcode_core::ports::{self, Listener};
use godcode_core::secrets;
use godcode_core::xray::{self, Dependency, Registry};

use super::*;

/// Seconds between two looks at the ports while their window is open.
const PORTS_EVERY: f64 = 3.0;
/// Files bigger than this are counted, not read.
const READ_LIMIT: u64 = 2 * 1024 * 1024;
/// Dependencies asked about online, at most.
const MAX_ASKED: usize = 150;

#[derive(Default)]
pub(super) struct Insight {
    secrets: Option<Secrets>,
    /// The next commit goes on although secrets were found: the user said so.
    allow_commit: bool,
    ports: Option<Ports>,
    xray: Option<Xray>,
    mock: Option<Mock>,
}

/// A line that looks like a secret.
#[derive(Clone)]
struct Leak {
    /// Relative to the folder, with `/`.
    shown: String,
    path: PathBuf,
    /// 1-based; 0 for a whole file.
    line: usize,
    kind: &'static str,
    masked: String,
}

struct Secrets {
    found: Vec<Leak>,
    /// Stopped a commit (and whether it was to push after).
    commit: Option<bool>,
    scanning: Option<Receiver<(usize, Vec<Leak>)>>,
    /// Files looked at by the last scan.
    looked: usize,
}

#[derive(Default)]
struct Ports {
    list: Vec<Listener>,
    error: Option<String>,
    asking: Option<Receiver<Result<Vec<Listener>, String>>>,
    asked_at: f64,
    filter: String,
    /// Windows' own services are listed too.
    system: bool,
    /// Asked once more before a program is ended.
    ending: Option<u32>,
}

struct Report {
    files: usize,
    lines: usize,
    bytes: u64,
    languages: Vec<(String, usize, usize)>,
    biggest: Vec<(String, PathBuf, u64, usize)>,
    notes: Vec<(String, usize)>,
    dependencies: Vec<(String, Dependency)>,
}

/// A dependency as a registry knows it.
type Package = (Registry, String);
/// The latest version of a package, or why it is not known.
type Latest = Result<String, String>;

#[derive(Default)]
struct Xray {
    report: Option<Report>,
    asking: Option<Receiver<Report>>,
    /// Latest version per registry and name, as the registry answered.
    latest: HashMap<Package, Latest>,
    checking: Option<Receiver<(Package, Latest)>>,
}

struct Mock {
    path: PathBuf,
    port: u16,
    server: Option<MockServer>,
    routes: Vec<Route>,
    error: Option<String>,
    read_at: f64,
}

/// Files that can be committed: tracked or new, without what Git ignores. Without Git,
/// every file under the folder.
fn project_files(root: &Path) -> Vec<(String, PathBuf)> {
    let listed = tools::git(root, &["ls-files", "-co", "--exclude-standard", "-z"]);
    match listed {
        Ok(text) if !text.is_empty() => text
            .split('\0')
            .filter(|name| !name.is_empty())
            .map(|name| (name.to_owned(), root.join(name)))
            .filter(|(_, path)| path.is_file())
            .collect(),
        _ => search::files(root, &AtomicBool::new(false))
            .into_iter()
            .map(|path| {
                let shown = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                (shown, path)
            })
            .collect(),
    }
}

fn read_text(path: &Path) -> Option<String> {
    let size = std::fs::metadata(path).ok()?.len();
    if size > READ_LIMIT {
        return None;
    }
    Some(godcode_core::decode(&std::fs::read(path).ok()?)?.text)
}

fn leaks_in(shown: &str, path: &Path, text: &str) -> Vec<Leak> {
    let mut found = Vec::new();
    if secrets::is_secret_file(shown) {
        found.push(Leak {
            shown: shown.to_owned(),
            path: path.to_path_buf(),
            line: 0,
            kind: "A file made to hold secrets",
            masked: String::new(),
        });
    }
    found.extend(secrets::scan(text).into_iter().map(|finding| Leak {
        shown: shown.to_owned(),
        path: path.to_path_buf(),
        line: finding.line,
        kind: finding.kind,
        masked: finding.masked,
    }));
    found
}

/// What the registry says is the newest stable version.
fn latest_version(registry: Registry, name: &str) -> Result<String, String> {
    let url = match registry {
        Registry::Crates => format!("https://crates.io/api/v1/crates/{name}"),
        Registry::Npm => format!(
            "https://registry.npmjs.org/{}/latest",
            name.replace('/', "%2F")
        ),
    };
    let body = ureq::get(&url)
        .header(
            "User-Agent",
            format!(
                "{APP_NAME}/{} (dependency check)",
                env!("CARGO_PKG_VERSION")
            ),
        )
        .call()
        .map_err(|err| err.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|err| err.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&body).map_err(|err| err.to_string())?;
    let version = match registry {
        Registry::Crates => json["crate"]["max_stable_version"]
            .as_str()
            .or_else(|| json["crate"]["max_version"].as_str()),
        Registry::Npm => json["version"].as_str(),
    };
    version
        .map(str::to_owned)
        .ok_or_else(|| "No version given".to_owned())
}

impl GodApp {
    // ------------------------------------------------------------ secrets

    /// Look at the ticked files before they are committed. True when the commit must wait.
    pub(super) fn secrets_stop_commit(&mut self, root: &Path, push: bool) -> bool {
        if std::mem::take(&mut self.insight.allow_commit) {
            return false;
        }
        let ticked: Vec<String> = self
            .repo
            .iter()
            .flat_map(|repo| &repo.changes)
            .filter(|change| change.state != git::State::Deleted)
            .filter(|change| !self.left_out.contains(&change.path))
            .map(|change| change.path.clone())
            .collect();
        let found: Vec<Leak> = ticked
            .iter()
            .flat_map(|shown| {
                let path = root.join(shown);
                leaks_in(shown, &path, &self.text_of(&path))
            })
            .collect();
        if found.is_empty() {
            return false;
        }
        self.insight.secrets = Some(Secrets {
            found,
            commit: Some(push),
            scanning: None,
            looked: ticked.len(),
        });
        true
    }

    pub(super) fn scan_secrets(&mut self, ctx: &Context) {
        let Some(root) = self.folder() else {
            self.toasts.error("Open a folder first");
            return;
        };
        let (sender, receiver) = channel();
        let waking = ctx.clone();
        std::thread::spawn(move || {
            let files = project_files(&root);
            let mut found = Vec::new();
            for (shown, path) in &files {
                if let Some(text) = read_text(path) {
                    found.extend(leaks_in(shown, path, &text));
                }
            }
            let _ = sender.send((files.len(), found));
            waking.request_repaint();
        });
        self.insight.secrets = Some(Secrets {
            found: Vec::new(),
            commit: None,
            scanning: Some(receiver),
            looked: 0,
        });
    }

    fn open_leak(&mut self, ctx: &Context, leak: &Leak) {
        self.record_jump();
        if self.open_file(ctx, &leak.path)
            && leak.line > 0
            && let Some(tab) = self.tabs.get_mut(self.active)
        {
            let start = godcode_core::line_start(&tab.text, leak.line);
            tab.select(ctx, start..start);
        }
    }

    pub(super) fn secrets_window(&mut self, ctx: &Context) {
        let Some(mut shown) = self.insight.secrets.take() else {
            return;
        };
        if let Some(Ok((looked, found))) = shown.scanning.as_ref().map(Receiver::try_recv) {
            shown.scanning = None;
            shown.looked = looked;
            shown.found = found;
        }
        let palette = Palette::of_ctx(ctx);
        let mut open = true;
        let mut jump = None;
        let mut commit_anyway = None;
        egui::Window::new(with_icon(icon::SHIELD_WARNING, "Secrets"))
            .id(Id::new("secrets-window"))
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .default_size(vec2(760.0, 420.0))
            .show(ctx, |ui| {
                if shown.scanning.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Looking through every file that can be committed…");
                    });
                    return;
                }
                if shown.found.is_empty() {
                    let text = format!("No secret found in {} files.", shown.looked);
                    ui.label(RichText::new(with_icon(icon::SHIELD_CHECK, &text)).strong());
                    return;
                }
                let title = match shown.commit {
                    Some(_) => "The commit waits: these lines look like secrets.".to_owned(),
                    None if shown.found.len() == 1 => "1 line looks like a secret.".to_owned(),
                    None => format!("{} lines look like secrets.", shown.found.len()),
                };
                ui.label(RichText::new(title).strong().color(palette.error));
                ui.label(RichText::new(format!(
                    "Move them to a file Git ignores, or write {} on a line that is meant to stay.",
                    secrets::ALLOW_MARKER
                ))
                .color(palette.text_dim));
                ui.add_space(6.0);
                ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    egui::Grid::new("secrets-found")
                        .striped(true)
                        .show(ui, |ui| {
                            for leak in &shown.found {
                                let place = match leak.line {
                                    0 => leak.shown.clone(),
                                    line => format!("{}:{line}", leak.shown),
                                };
                                if ui.link(place).clicked() {
                                    jump = Some(leak.clone());
                                }
                                ui.label(leak.kind);
                                ui.label(
                                    RichText::new(&leak.masked)
                                        .monospace()
                                        .color(palette.text_dim),
                                );
                                ui.end_row();
                            }
                        });
                });
                if let Some(push) = shown.commit {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let label = if push {
                            "Commit and Push anyway"
                        } else {
                            "Commit anyway"
                        };
                        if ui.button(label).clicked() {
                            commit_anyway = Some(push);
                        }
                    });
                }
            });
        if let Some(leak) = jump {
            self.open_leak(ctx, &leak);
        }
        if let Some(push) = commit_anyway {
            self.insight.allow_commit = true;
            self.commit(ctx, push);
            return;
        }
        if open {
            self.insight.secrets = Some(shown);
        }
    }

    // ------------------------------------------------------------ ports

    pub(super) fn open_ports(&mut self, ctx: &Context) {
        self.insight.ports = Some(Ports::default());
        self.ask_ports(ctx);
    }

    fn ask_ports(&mut self, ctx: &Context) {
        let Some(shown) = &mut self.insight.ports else {
            return;
        };
        if shown.asking.is_some() {
            return;
        }
        let (sender, receiver) = channel();
        let waking = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(ports::listening());
            waking.request_repaint();
        });
        shown.asking = Some(receiver);
        shown.asked_at = ctx.input(|input| input.time);
    }

    pub(super) fn ports_window(&mut self, ctx: &Context) {
        let Some(mut shown) = self.insight.ports.take() else {
            return;
        };
        if let Some(Ok(answer)) = shown.asking.as_ref().map(Receiver::try_recv) {
            shown.asking = None;
            match answer {
                Ok(list) => {
                    shown.list = list;
                    shown.error = None;
                }
                Err(err) => shown.error = Some(err),
            }
        }
        let palette = Palette::of_ctx(ctx);
        let now = ctx.input(|input| input.time);
        let mut open = true;
        let mut refresh = now - shown.asked_at > PORTS_EVERY;
        let mut end = None;
        egui::Window::new(with_icon(icon::PLUGS, "Ports in use"))
            .id(Id::new("ports-window"))
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .default_size(vec2(760.0, 480.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut shown.filter)
                            .hint_text("Port or program")
                            .margin(widgets::FIELD_MARGIN)
                            .desired_width(220.0),
                    );
                    ui.checkbox(&mut shown.system, "Windows services");
                    refresh |= ui
                        .button(with_icon(icon::ARROWS_CLOCKWISE, "Refresh"))
                        .clicked();
                    if shown.asking.is_some() {
                        ui.spinner();
                    }
                });
                if let Some(err) = &shown.error {
                    ui.colored_label(palette.error, err);
                }
                let needle = shown.filter.trim().to_lowercase();
                let listed: Vec<&Listener> = shown
                    .list
                    .iter()
                    .filter(|listener| shown.system || !is_system(listener))
                    .filter(|listener| {
                        needle.is_empty()
                            || listener.port.to_string().contains(&needle)
                            || listener.process.to_lowercase().contains(&needle)
                    })
                    .collect();
                ui.label(
                    RichText::new(format!("{} ports listen on this computer", listed.len()))
                        .color(palette.text_dim),
                );
                ui.add_space(4.0);
                ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new("ports-list")
                        .striped(true)
                        .num_columns(5)
                        .spacing(vec2(16.0, 6.0))
                        .show(ui, |ui| {
                            for header in ["Port", "Reached from", "Program", "PID", ""] {
                                ui.label(RichText::new(header).color(palette.text_dim));
                            }
                            ui.end_row();
                            for listener in listed {
                                ui.label(
                                    RichText::new(listener.port.to_string())
                                        .strong()
                                        .monospace(),
                                );
                                let reach = if listener.public() {
                                    "The network"
                                } else {
                                    "This computer"
                                };
                                ui.label(reach).on_hover_text(&listener.address);
                                ui.label(&listener.process);
                                ui.label(
                                    RichText::new(listener.pid.to_string()).color(palette.text_dim),
                                );
                                ui.horizontal(|ui| {
                                    if ui
                                        .small_button(with_icon(icon::ARROW_SQUARE_OUT, "Open"))
                                        .clicked()
                                        && let Err(err) = tools::open_in_browser(&listener.url())
                                    {
                                        shown.error = Some(err);
                                    }
                                    let asked = shown.ending == Some(listener.pid);
                                    let label = if asked { "Sure? End it" } else { "End" };
                                    let button = ui.small_button(with_icon(icon::X_CIRCLE, label));
                                    if button.clicked() {
                                        if asked {
                                            end = Some(listener.pid);
                                        } else {
                                            shown.ending = Some(listener.pid);
                                        }
                                    }
                                });
                                ui.end_row();
                            }
                        });
                });
            });
        if let Some(pid) = end {
            shown.ending = None;
            match ports::kill(pid) {
                Ok(()) => self.toasts.success(format!("Ended process {pid}")),
                Err(err) => self.toasts.error(err),
            }
            refresh = true;
        }
        if open {
            self.insight.ports = Some(shown);
            if refresh {
                self.ask_ports(ctx);
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    // ------------------------------------------------------------ project X-ray

    pub(super) fn open_xray(&mut self, ctx: &Context) {
        let Some(root) = self.folder() else {
            self.toasts.error("Open a folder first");
            return;
        };
        let (sender, receiver) = channel();
        let waking = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(survey(&root));
            waking.request_repaint();
        });
        self.insight.xray = Some(Xray {
            asking: Some(receiver),
            ..Xray::default()
        });
    }

    fn check_versions(&mut self, ctx: &Context) {
        let Some(shown) = &mut self.insight.xray else {
            return;
        };
        let Some(report) = &shown.report else {
            return;
        };
        let mut asked: Vec<(Registry, String)> = Vec::new();
        for (_, dependency) in &report.dependencies {
            let key = (dependency.registry, dependency.name.clone());
            if !asked.contains(&key) && asked.len() < MAX_ASKED {
                asked.push(key);
            }
        }
        let (sender, receiver) = channel();
        let waking = ctx.clone();
        std::thread::spawn(move || {
            for key in asked {
                let answer = latest_version(key.0, &key.1);
                if sender.send((key, answer)).is_err() {
                    return;
                }
                waking.request_repaint();
            }
        });
        shown.checking = Some(receiver);
    }

    pub(super) fn xray_window(&mut self, ctx: &Context) {
        let Some(mut shown) = self.insight.xray.take() else {
            return;
        };
        if let Some(Ok(report)) = shown.asking.as_ref().map(Receiver::try_recv) {
            shown.asking = None;
            shown.report = Some(report);
        }
        if let Some(receiver) = &shown.checking {
            loop {
                match receiver.try_recv() {
                    Ok((key, answer)) => {
                        shown.latest.insert(key, answer);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        shown.checking = None;
                        break;
                    }
                }
            }
        }
        let palette = Palette::of_ctx(ctx);
        let mut open = true;
        let mut jump = None;
        let mut todo = false;
        let mut check = false;
        egui::Window::new(with_icon(icon::CHART_PIE_SLICE, "Project X-ray"))
            .id(Id::new("xray-window"))
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .default_size(vec2(820.0, 620.0))
            .show(ctx, |ui| {
                let Some(report) = &shown.report else {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading every file of the project…");
                    });
                    return;
                };
                let dim = |text: &str| RichText::new(text.to_owned()).color(palette.text_dim);
                ui.label(
                    RichText::new(format!(
                        "{} files  ·  {} lines of code  ·  {}",
                        report.files,
                        report.lines,
                        human_size(report.bytes)
                    ))
                    .size(15.0)
                    .strong(),
                );
                ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                    ui.add_space(8.0);
                    ui.label(dim("LANGUAGES"));
                    let most = report.languages.first().map_or(1, |entry| entry.2.max(1));
                    egui::Grid::new("xray-languages").num_columns(3).spacing(vec2(14.0, 4.0)).show(ui, |ui| {
                        for (language, files, lines) in report.languages.iter().take(12) {
                            ui.label(language);
                            let (rect, _) = ui.allocate_exact_size(vec2(220.0, 8.0), Sense::hover());
                            ui.painter().rect_filled(rect, 3.0, palette.border);
                            let share = *lines as f32 / most as f32;
                            let filled = egui::Rect::from_min_size(rect.min, vec2(rect.width() * share, rect.height()));
                            ui.painter().rect_filled(filled, 3.0, palette.accent);
                            let plural = if *files == 1 { "" } else { "s" };
                            ui.label(dim(&format!("{lines} lines  ·  {files} file{plural}")));
                            ui.end_row();
                        }
                    });

                    ui.add_space(10.0);
                    ui.label(dim("BIGGEST FILES"));
                    for (shown_path, path, size, lines) in &report.biggest {
                        ui.horizontal(|ui| {
                            if ui.link(shown_path).clicked() {
                                jump = Some(path.clone());
                            }
                            ui.label(dim(&format!("{}  ·  {lines} lines", human_size(*size))));
                        });
                    }

                    ui.add_space(10.0);
                    ui.label(dim("NOTES LEFT IN THE CODE"));
                    ui.horizontal(|ui| {
                        if report.notes.is_empty() {
                            ui.label("None");
                        }
                        for (tag, count) in &report.notes {
                            ui.label(RichText::new(format!("{tag} {count}")).strong());
                        }
                        if !report.notes.is_empty() {
                            todo = ui.button("Open the TODO list").clicked();
                        }
                    });

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(dim(&format!("DEPENDENCIES ({})", report.dependencies.len())));
                        if !report.dependencies.is_empty() {
                            let label = with_icon(icon::PACKAGE, "Check versions online");
                            let tip = "Asks crates.io and the npm registry for their latest versions";
                            check = ui
                                .add_enabled(shown.checking.is_none(), Button::new(label))
                                .on_hover_text(tip)
                                .clicked();
                            if shown.checking.is_some() {
                                ui.spinner();
                            }
                        }
                    });
                    let outdated = report
                        .dependencies
                        .iter()
                        .filter(|(_, dependency)| {
                            let key = (dependency.registry, dependency.name.clone());
                            matches!(shown.latest.get(&key), Some(Ok(latest)) if xray::is_outdated(&dependency.requirement, latest))
                        })
                        .count();
                    let answered = shown.latest.values().filter(|answer| answer.is_ok()).count();
                    let failed = shown.latest.values().find_map(|answer| answer.as_ref().err());
                    if answered > 0 && shown.checking.is_none() {
                        let text = match outdated {
                            0 => "Every dependency is on its latest major version.".to_owned(),
                            1 => "1 has a new major version.".to_owned(),
                            count => format!("{count} have a new major version."),
                        };
                        ui.label(RichText::new(text).strong());
                    } else if let (0, Some(err)) = (answered, failed) {
                        ui.colored_label(palette.error, format!("The registries did not answer: {err}"));
                    }
                    egui::Grid::new("xray-dependencies").striped(true).num_columns(5).spacing(vec2(14.0, 4.0)).show(ui, |ui| {
                        for (manifest, dependency) in &report.dependencies {
                            let key = (dependency.registry, dependency.name.clone());
                            ui.label(RichText::new(&dependency.name).monospace());
                            ui.label(dim(manifest));
                            ui.label(&dependency.requirement);
                            match shown.latest.get(&key) {
                                Some(Ok(latest)) if xray::is_outdated(&dependency.requirement, latest) => {
                                    ui.label(RichText::new(format!("{latest}  new major")).color(palette.error).strong());
                                }
                                Some(Ok(latest)) => {
                                    ui.label(dim(latest));
                                }
                                Some(Err(err)) => {
                                    ui.label(dim("?")).on_hover_text(err);
                                }
                                None => {
                                    ui.label("");
                                }
                            }
                            ui.label(dim(if dependency.dev { "dev" } else { "" }));
                            ui.end_row();
                        }
                    });
                });
            });
        if let Some(path) = jump {
            self.record_jump();
            self.open_file(ctx, &path);
        }
        if todo {
            self.pending.push(Cmd::Todo);
        }
        if open {
            self.insight.xray = Some(shown);
            if check {
                self.check_versions(ctx);
            }
        }
    }

    // ------------------------------------------------------------ mock API

    pub(super) fn open_mock(&mut self) {
        let from_tab = self.tabs.get(self.active).and_then(|tab| {
            let path = tab.path.clone()?;
            let is_http = path.extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("http") || ext.eq_ignore_ascii_case("rest")
            });
            is_http.then_some(path)
        });
        let Some(path) = from_tab else {
            self.toasts
                .error("Open a .http file first: its answers become the API");
            return;
        };
        let routes = mock::parse(&self.text_of(&path));
        self.insight.mock = Some(Mock {
            path,
            port: 4010,
            server: None,
            routes,
            error: None,
            read_at: 0.0,
        });
    }

    pub(super) fn mock_window(&mut self, ctx: &Context) {
        let Some(mut shown) = self.insight.mock.take() else {
            return;
        };
        // The answers follow the file as it is edited.
        let now = ctx.input(|input| input.time);
        if now - shown.read_at > 1.0 {
            shown.read_at = now;
            let routes = mock::parse(&self.text_of(&shown.path));
            if routes != shown.routes {
                if let Some(server) = &shown.server {
                    server.set_routes(routes.clone());
                }
                shown.routes = routes;
            }
        }
        let palette = Palette::of_ctx(ctx);
        let mut open = true;
        let mut start = false;
        let mut stop = false;
        egui::Window::new(with_icon(icon::BROADCAST, "Mock API"))
            .id(Id::new("mock-window"))
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .default_size(vec2(680.0, 520.0))
            .show(ctx, |ui| {
                let dim = |text: &str| RichText::new(text.to_owned()).color(palette.text_dim);
                ui.label(dim(&format!("Answers from {}", file_name(&shown.path))));
                ui.horizontal(|ui| match &shown.server {
                    Some(server) => {
                        ui.label(RichText::new(with_icon(icon::BROADCAST, "Serving")).strong());
                        let url = server.url();
                        if ui.link(&url).clicked() {
                            let _ = tools::open_in_browser(&url);
                        }
                        if widgets::small_icon_button(ui, icon::COPY, "Copy the address").clicked() {
                            ui.ctx().copy_text(url);
                        }
                        stop = ui.button(with_icon(icon::STOP, "Stop")).clicked();
                    }
                    None => {
                        ui.label("Port");
                        ui.add(egui::DragValue::new(&mut shown.port).range(1024..=65535));
                        let ready = !shown.routes.is_empty();
                        start = ui.add_enabled(ready, Button::new(with_icon(icon::PLAY, "Start"))).clicked();
                    }
                });
                if let Some(err) = &shown.error {
                    ui.colored_label(palette.error, err);
                }
                ui.add_space(6.0);
                if shown.routes.is_empty() {
                    ui.label("No answer in this file yet. Write one under a request:");
                    ui.label(
                        RichText::new("### One user\nGET /users/{id}\n\nHTTP/1.1 200 OK\nContent-Type: application/json\n\n{\"id\": 1, \"name\": \"Ada\"}")
                            .monospace(),
                    );
                    return;
                }
                ui.label(dim(&format!("ROUTES ({})", shown.routes.len())));
                egui::Grid::new("mock-routes").striped(true).num_columns(3).show(ui, |ui| {
                    for route in &shown.routes {
                        ui.label(RichText::new(format!("{} {}", route.method, route.path)).monospace().strong());
                        ui.label(format!("{} {}", route.status, route.reason));
                        ui.label(dim(&route.name));
                        ui.end_row();
                    }
                });
                if let Some(server) = &shown.server {
                    let log = server.log();
                    ui.add_space(8.0);
                    ui.label(dim(&format!("REQUESTS ({})", log.len())));
                    ScrollArea::vertical().max_height(200.0).stick_to_bottom(true).show(ui, |ui| {
                        for hit in log.iter().rev().take(100).rev() {
                            let color = if hit.status >= 400 { palette.error } else { palette.text };
                            ui.label(RichText::new(format!("{} {}  →  {}", hit.method, hit.path, hit.status)).monospace().color(color));
                        }
                    });
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
                }
            });
        if start {
            match MockServer::start(shown.port, shown.routes.clone()) {
                Ok(server) => {
                    self.toasts.success(format!("Mock API on {}", server.url()));
                    shown.server = Some(server);
                    shown.error = None;
                }
                Err(err) => shown.error = Some(err),
            }
        }
        if stop {
            shown.server = None;
        }
        if open {
            self.insight.mock = Some(shown);
        }
    }
}

/// Programs of Windows itself, hidden unless asked for: what is left are the servers of
/// the user.
fn is_system(listener: &Listener) -> bool {
    const SYSTEM: [&str; 9] = [
        "system",
        "svchost.exe",
        "lsass.exe",
        "wininit.exe",
        "services.exe",
        "spoolsv.exe",
        "smss.exe",
        "csrss.exe",
        "winlogon.exe",
    ];
    listener.pid <= 4 || SYSTEM.contains(&listener.process.to_lowercase().as_str())
}

fn human_size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

/// Read the whole project once: languages, sizes, notes, dependencies.
fn survey(root: &Path) -> Report {
    let files = project_files(root);
    let mut languages = xray::Languages::default();
    let mut sizes = Vec::new();
    let mut notes: HashMap<String, usize> = HashMap::new();
    let mut dependencies = Vec::new();
    let mut lines = 0;
    let mut bytes = 0;
    for (shown, path) in &files {
        let size = std::fs::metadata(path).map_or(0, |meta| meta.len());
        bytes += size;
        let Some(text) = read_text(path) else {
            sizes.push((shown.clone(), path.clone(), size, 0));
            continue;
        };
        let count = xray::code_lines(&text);
        lines += count;
        let language = Language::of(path).name;
        languages.add(language, count);
        for note in xray::notes(&text) {
            *notes.entry(note.tag).or_default() += 1;
        }
        let name = shown.rsplit('/').next().unwrap_or(shown);
        let found = match name {
            "Cargo.toml" => xray::cargo_dependencies(&text),
            "package.json" => xray::npm_dependencies(&text),
            _ => Vec::new(),
        };
        dependencies.extend(
            found
                .into_iter()
                .map(|dependency| (shown.clone(), dependency)),
        );
        sizes.push((shown.clone(), path.clone(), size, count));
    }
    sizes.sort_by(|a, b| b.2.cmp(&a.2));
    sizes.truncate(10);
    let mut notes: Vec<(String, usize)> = notes.into_iter().collect();
    notes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Report {
        files: files.len(),
        lines,
        bytes,
        languages: languages.sorted(),
        biggest: sizes,
        notes,
        dependencies,
    }
}
