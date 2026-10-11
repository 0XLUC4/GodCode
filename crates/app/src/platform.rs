//! File dialogs and safe writes.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};

pub fn pick_files(start: Option<&Path>) -> Vec<PathBuf> {
    dialog(start).pick_files().unwrap_or_default()
}

pub fn pick_folder(start: Option<&Path>) -> Option<PathBuf> {
    dialog(start).pick_folder()
}

/// Ask for a font file, starting where the system keeps its own.
pub fn pick_font() -> Option<PathBuf> {
    let dialog = rfd::FileDialog::new().add_filter("Font", &["ttf", "otf", "ttc"]);
    let system = Path::new("C:/Windows/Fonts");
    match system.is_dir() {
        true => dialog.set_directory(system),
        false => dialog,
    }
    .pick_file()
}

/// Ask where to save a file called `name`.
pub fn pick_save(name: &str, start: Option<&Path>) -> Option<PathBuf> {
    dialog(start).set_file_name(name).save_file()
}

fn dialog(start: Option<&Path>) -> rfd::FileDialog {
    let dialog = rfd::FileDialog::new();
    match start {
        Some(folder) => dialog.set_directory(folder),
        None => dialog,
    }
}

/// Write through a temporary file so a failure never leaves a half-written file.
pub fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut temp_name = target
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from("file"));
    temp_name.push(".godcode.tmp");
    let temp = target.with_file_name(temp_name);
    let written = std::fs::File::create(&temp)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temp, target));
    written.map_err(|err| {
        // Best effort: the temp file may not exist.
        let _ = std::fs::remove_file(&temp);
        format!("Cannot write {}: {err}", target.display())
    })
}
