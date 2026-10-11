//! The file tree of the open folder.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use egui::{Align2, FontId, RichText, ScrollArea, Sense, Ui, pos2, vec2};

use crate::theme::Palette;
use crate::tools;
use crate::widgets::{self, icon, with_icon};

const ROW_HEIGHT: f32 = 24.0;
const INDENT: f32 = 14.0;

#[derive(Clone)]
struct Entry {
    path: PathBuf,
    name: String,
    is_dir: bool,
}

/// One visible line of the tree.
struct Row {
    entry: Entry,
    depth: usize,
    open: bool,
}

/// What the user asked for in the tree.
pub enum Action {
    Open(PathBuf),
    /// Create a file / folder in this folder.
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
}

pub struct Explorer {
    root: PathBuf,
    expanded: HashSet<PathBuf>,
    /// Folder contents read so far.
    listed: HashMap<PathBuf, Vec<Entry>>,
    /// The visible lines, rebuilt after a folder opens, closes or is re-read.
    rows: Option<Vec<Row>>,
    /// Bring the line of this file into view on the next frame.
    reveal: Option<PathBuf>,
}

/// Folders first, then by name without regard to case.
fn list(dir: &Path) -> Vec<Entry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut listed: Vec<Entry> = entries
        .flatten()
        .map(|entry| Entry {
            path: entry.path(),
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir: entry.file_type().is_ok_and(|kind| kind.is_dir()),
        })
        .collect();
    listed.sort_by_cached_key(|entry| (!entry.is_dir, entry.name.to_lowercase()));
    listed
}

impl Explorer {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            expanded: HashSet::new(),
            listed: HashMap::new(),
            rows: None,
            reveal: None,
        }
    }

    /// Open the folders above `file` and scroll to it.
    pub fn reveal(&mut self, file: &Path) {
        if let Some(folder) = file.parent() {
            self.expand(folder);
        }
        self.reveal = Some(file.to_path_buf());
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Read the folders again: something changed on disk.
    pub fn refresh(&mut self) {
        self.listed.clear();
        self.rows = None;
    }

    /// Open `dir` and the folders above it, so what is inside shows.
    pub fn expand(&mut self, dir: &Path) {
        for folder in dir.ancestors() {
            if folder == self.root || !folder.starts_with(&self.root) {
                break;
            }
            self.expanded.insert(folder.to_path_buf());
        }
        self.rows = None;
    }

    fn collect(&mut self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        let entries = self
            .listed
            .entry(dir.to_path_buf())
            .or_insert_with(|| list(dir))
            .clone();
        for entry in entries {
            let open = entry.is_dir && self.expanded.contains(&entry.path);
            let path = entry.path.clone();
            rows.push(Row { entry, depth, open });
            if open {
                self.collect(&path, depth + 1, rows);
            }
        }
    }

    /// Show the tree; `active` is the file of the editor in front.
    pub fn show(&mut self, ui: &mut Ui, active: Option<&Path>) -> Option<Action> {
        let palette = Palette::of(ui);
        let mut action = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(10.0);
            let name = self.root.file_name().map_or_else(
                || self.root.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            let title = RichText::new(name.to_uppercase()).size(12.0).strong();
            ui.add(egui::Label::new(title).truncate().selectable(false))
                .on_hover_text(self.root.display().to_string());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                if widgets::small_icon_button(ui, icon::ARROWS_IN_SIMPLE, "Collapse all").clicked()
                {
                    self.expanded.clear();
                    self.rows = None;
                }
                if widgets::small_icon_button(ui, icon::ARROWS_CLOCKWISE, "Refresh").clicked() {
                    self.refresh();
                }
                if widgets::small_icon_button(ui, icon::FOLDER_PLUS, "New folder").clicked() {
                    action = Some(Action::NewFolder(self.root.clone()));
                }
                if widgets::small_icon_button(ui, icon::FILE_PLUS, "New file").clicked() {
                    action = Some(Action::NewFile(self.root.clone()));
                }
            });
        });
        ui.add_space(4.0);

        if self.rows.is_none() {
            let mut rows = Vec::new();
            let root = self.root.clone();
            self.collect(&root, 0, &mut rows);
            self.rows = Some(rows);
        }
        let rows = self.rows.as_deref().unwrap_or_default();
        let mut toggled = None;
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut scroll = ScrollArea::vertical().auto_shrink(false);
        let revealed = self
            .reveal
            .take()
            .and_then(|file| rows.iter().position(|row| row.entry.path == file));
        if let Some(row) = revealed {
            // A few lines above stay in view, to see where the file sits.
            scroll = scroll.vertical_scroll_offset((row as f32 - 4.0).max(0.0) * ROW_HEIGHT);
        }
        scroll.show_rows(ui, ROW_HEIGHT, rows.len(), |ui, range| {
            for row in &rows[range] {
                let entry = &row.entry;
                let size = vec2(ui.available_width(), ROW_HEIGHT);
                let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                let painter = ui.painter();
                if active == Some(entry.path.as_path()) {
                    painter.rect_filled(rect, 0.0, palette.selected);
                } else if response.hovered() || response.context_menu_opened() {
                    painter.rect_filled(rect, 0.0, palette.hover);
                }
                let mid = rect.center().y;
                let mut x = rect.min.x + 8.0 + row.depth as f32 * INDENT;
                let glyph = |x: f32, glyph: &str, size: f32| {
                    painter.text(
                        pos2(x, mid),
                        Align2::LEFT_CENTER,
                        glyph,
                        FontId::proportional(size),
                        palette.text_dim,
                    );
                };
                if entry.is_dir {
                    let caret = if row.open {
                        icon::CARET_DOWN
                    } else {
                        icon::CARET_RIGHT
                    };
                    glyph(x, caret, 11.0);
                }
                x += 15.0;
                let center = pos2(x + 9.0, mid);
                if entry.is_dir {
                    let (kind, tint) = tools::folder_icon(&entry.name, row.open);
                    let font = widgets::filled(16.0);
                    painter.text(center, Align2::CENTER_CENTER, kind, font, tint);
                } else {
                    widgets::paint_file_icon(painter, center, &tools::file_icon(&entry.name));
                }
                x += 24.0;
                let mut job = egui::text::LayoutJob::simple_singleline(
                    entry.name.clone(),
                    FontId::proportional(14.0),
                    palette.text,
                );
                let room = (rect.max.x - x - 6.0).max(0.0);
                job.wrap = egui::text::TextWrapping::truncate_at_width(room);
                let galley = painter.layout_job(job);
                painter.galley(pos2(x, mid - galley.size().y / 2.0), galley, palette.text);

                if response.clicked() {
                    if entry.is_dir {
                        toggled = Some(entry.path.clone());
                    } else {
                        action = Some(Action::Open(entry.path.clone()));
                    }
                }
                let folder = if entry.is_dir {
                    entry.path.clone()
                } else {
                    entry
                        .path
                        .parent()
                        .map_or_else(|| self.root.clone(), Path::to_path_buf)
                };
                response.context_menu(|ui| {
                    if ui.button(with_icon(icon::FILE_PLUS, "New file…")).clicked() {
                        action = Some(Action::NewFile(folder.clone()));
                    }
                    if ui
                        .button(with_icon(icon::FOLDER_PLUS, "New folder…"))
                        .clicked()
                    {
                        action = Some(Action::NewFolder(folder.clone()));
                    }
                    ui.separator();
                    if ui
                        .button(with_icon(icon::PENCIL_SIMPLE, "Rename…"))
                        .clicked()
                    {
                        action = Some(Action::Rename(entry.path.clone()));
                    }
                    if ui.button(with_icon(icon::COPY, "Copy path")).clicked() {
                        ui.ctx().copy_text(entry.path.display().to_string());
                    }
                    ui.separator();
                    if ui.button(with_icon(icon::TRASH, "Delete")).clicked() {
                        action = Some(Action::Delete(entry.path.clone()));
                    }
                });
            }
        });
        if let Some(path) = toggled {
            if !self.expanded.remove(&path) {
                self.expanded.insert(path);
            }
            self.rows = None;
        }
        action
    }
}
