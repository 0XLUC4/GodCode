//! What belongs to a kind of file: how to run it, how to check it, its icon.

use std::path::{Path, PathBuf};
use std::process::Command;

use egui::Color32;

use crate::widgets::icon;

/// A compiler or linter run to find the problems of a file.
pub struct Checker {
    pub program: &'static str,
    pub args: Vec<String>,
    /// Where it runs; the paths it prints are relative to this folder.
    pub cwd: PathBuf,
}

/// Keep a console program from opening a console window of its own.
pub fn hidden(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// The outermost folder above `file` holding a `Cargo.toml`: the workspace when there is one.
fn cargo_root(file: &Path) -> Option<PathBuf> {
    file.ancestors()
        .skip(1)
        .filter(|folder| folder.join("Cargo.toml").is_file())
        .last()
        .map(Path::to_path_buf)
}

fn parent(file: &Path) -> PathBuf {
    file.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// The shell command that runs `file`, for the languages that have an obvious one.
pub fn run_command(language: &str, file: &Path) -> Option<String> {
    let with = |program: &str| Some(format!("{program} \"{}\"", file.display()));
    match language {
        "Rust" => cargo_root(file).map(|root| {
            let manifest = root.join("Cargo.toml");
            format!("cargo run --manifest-path \"{}\"", manifest.display())
        }),
        "Python" => with("python"),
        "JavaScript" => with("node"),
        "TypeScript" => with("npx tsx"),
        "Go" => with("go run"),
        "Ruby" => with("ruby"),
        "PHP" => with("php"),
        "Lua" => with("lua"),
        "Java" => with("java"),
        "Shell" if file.extension().is_some() => with("bash"),
        "PowerShell" | "Batch" => with("&"),
        // Opens in the default browser.
        "HTML" if cfg!(windows) => with("Start-Process"),
        "HTML" => with("xdg-open"),
        _ => None,
    }
}

/// The program that finds the problems of `file`, when its language has a common one.
pub fn checker(language: &str, file: &Path) -> Option<Checker> {
    let path = file.display().to_string();
    let (program, args, cwd) = match language {
        "Rust" => (
            "cargo",
            vec!["check", "--message-format=short", "--color=never"],
            cargo_root(file)?,
        ),
        // Parsing only: nothing is run and no cache file is written.
        "Python" => (
            "python",
            vec![
                "-c",
                "import ast,sys; ast.parse(open(sys.argv[1], encoding='utf-8').read(), sys.argv[1])",
                &path,
            ],
            parent(file),
        ),
        "JavaScript" => ("node", vec!["--check", &path], parent(file)),
        "Go" => ("go", vec!["vet", "."], parent(file)),
        _ => return None,
    };
    Some(Checker {
        program,
        args: args.into_iter().map(str::to_owned).collect(),
        cwd,
    })
}

/// Run `checker` to its end and return everything it printed.
pub fn check(checker: &Checker) -> Result<String, String> {
    let mut command = Command::new(checker.program);
    command.args(&checker.args).current_dir(&checker.cwd);
    hidden(&mut command);
    let output = command
        .output()
        .map_err(|err| format!("Cannot run {}: {err}", checker.program))?;
    let mut printed = String::from_utf8_lossy(&output.stdout).into_owned();
    printed.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(printed)
}

/// Run `git` in `folder` and return what it printed; what it complained about when it fails.
pub fn git(folder: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command.args(args).current_dir(folder);
    hidden(&mut command);
    let output = command
        .output()
        .map_err(|err| format!("Cannot run git: {err}"))?;
    let printed = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        return Ok(printed);
    }
    let complaint = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(if complaint.is_empty() {
        printed.trim().to_owned()
    } else {
        complaint
    })
}

/// Format `text` with the formatter of its language, when one is installed: `Some(Err)`
/// is what it complained about, `None` that there is none to run.
pub fn format_external(language: &str, text: &str) -> Option<Result<String, String>> {
    use std::io::Write as _;
    use std::process::Stdio;
    let (program, args): (&str, &[&str]) = match language {
        "Rust" => ("rustfmt", &["--edition", "2024", "--emit", "stdout"]),
        "Go" => ("gofmt", &[]),
        _ => return None,
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hidden(&mut command);
    let mut child = command.spawn().ok()?;
    let mut stdin = child.stdin.take()?;
    let source = text.to_owned();
    // Written from another thread: the formatter may fill its output before reading it all.
    let writer = std::thread::spawn(move || stdin.write_all(source.as_bytes()));
    let output = child.wait_with_output().ok()?;
    let _ = writer.join();
    Some(if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"))
    } else {
        let complaint = String::from_utf8_lossy(&output.stderr);
        Err(complaint.lines().next().unwrap_or(program).to_owned())
    })
}

/// How a file shows in lists.
pub struct FileIcon {
    /// Drawn alone when there is no badge, and wherever only text can go.
    pub glyph: &'static str,
    pub color: Color32,
    /// One or two letters on a square of `color`, for the languages known by their initials.
    pub badge: Option<&'static str>,
}

fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Icon and brand colour of a file, from its name.
pub fn file_icon(name: &str) -> FileIcon {
    const GREY: u32 = 0x8F_9B_A8;
    let lower = name.to_lowercase();
    let extension = lower
        .rsplit_once('.')
        .map_or("", |(_, extension)| extension);
    let badge = |label, hex| (icon::FILE_CODE, hex, Some(label));
    let glyph = |glyph, hex| (glyph, hex, None);
    let (glyph, hex, badge) = match (lower.as_str(), extension) {
        ("cargo.toml" | "package.json" | "go.mod" | "pyproject.toml", _) => {
            glyph(icon::PACKAGE, 0xC9_8A_4B)
        }
        ("package-lock.json" | "cargo.lock" | "pnpm-lock.yaml" | "yarn.lock", _) => {
            glyph(icon::LOCK_SIMPLE, GREY)
        }
        (".gitignore" | ".gitattributes" | ".gitmodules", _) => glyph(icon::GIT_BRANCH, 0xF0_50_33),
        ("dockerfile" | "makefile", _) => glyph(icon::TERMINAL_WINDOW, 0x4E_AA_25),
        ("license" | "license.md" | "license.txt", _) => glyph(icon::SCALES, 0xD0_A8_3F),
        ("readme.md" | "readme" | "lisezmoi.txt", _) => glyph(icon::INFO, 0x51_9A_BA),
        (_, "rs") => badge("RS", 0xDE_A5_84),
        (_, "py" | "pyw" | "pyi") => badge("PY", 0x37_76_AB),
        (_, "js" | "mjs" | "cjs") => badge("JS", 0xF1_D9_3A),
        (_, "ts" | "mts" | "cts") => badge("TS", 0x31_78_C6),
        (_, "jsx" | "tsx") => glyph(icon::ATOM, 0x61_DA_FB),
        (_, "vue") => badge("V", 0x41_B8_83),
        (_, "svelte") => badge("S", 0xFF_3E_00),
        (_, "html" | "htm") => badge("<>", 0xE4_4D_26),
        (_, "xml" | "xaml" | "plist" | "csproj") => badge("<>", GREY),
        (_, "css") => badge("#", 0x26_4D_E4),
        (_, "scss" | "less") => badge("#", 0xCF_64_9A),
        (_, "c" | "h") => badge("C", 0x5C_6B_C0),
        (_, "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx") => badge("C+", 0xF3_4B_7D),
        (_, "cs") => badge("C#", 0x9B_4F_96),
        (_, "go") => badge("GO", 0x00_AD_D8),
        (_, "java") => badge("J", 0xE7_6F_00),
        (_, "kt" | "kts") => badge("K", 0x7F_52_FF),
        (_, "swift") => badge("S", 0xF0_51_38),
        (_, "php") => badge("P", 0x77_7B_B4),
        (_, "rb") => badge("RB", 0xCC_34_2D),
        (_, "lua") => badge("L", 0x2C_2D_72),
        (_, "sql") => glyph(icon::DATABASE, 0xE3_8C_00),
        (_, "md" | "markdown") => badge("M", 0x51_9A_BA),
        (_, "json" | "jsonc" | "webmanifest") => badge("{}", 0xCB_CB_41),
        (_, "toml" | "yml" | "yaml" | "ini" | "cfg" | "conf" | "env") => glyph(icon::GEAR, GREY),
        (_, "sh" | "bash" | "zsh" | "fish" | "ps1" | "psm1" | "bat" | "cmd") => {
            glyph(icon::TERMINAL_WINDOW, 0x4E_AA_25)
        }
        (_, "png" | "gif" | "bmp" | "ico" | "webp" | "jpg" | "jpeg") => {
            glyph(icon::IMAGE, 0xA0_74_C4)
        }
        (_, "svg") => glyph(icon::SHAPES, 0xFF_B1_3B),
        (_, "pdf") => glyph(icon::FILE_PDF, 0xE5_39_35),
        (_, "zip" | "gz" | "tar" | "7z" | "rar") => glyph(icon::FILE_ZIP, GREY),
        (_, "csv") => glyph(icon::FILE_CSV, 0x4E_AA_25),
        (_, "lock") => glyph(icon::LOCK_SIMPLE, GREY),
        (_, "txt" | "log") => glyph(icon::FILE_TEXT, GREY),
        _ => glyph(icon::FILE, GREY),
    };
    FileIcon {
        glyph,
        color: rgb(hex),
        badge,
    }
}

/// Icon and colour of a folder: the ones every project has stand out.
pub fn folder_icon(name: &str, open: bool) -> (&'static str, Color32) {
    let hex = match name.to_lowercase().as_str() {
        "src" | "lib" | "app" | "crates" | "packages" => 0x4F_9C_E8,
        "test" | "tests" | "spec" | "__tests__" => 0x4E_AA_25,
        "public" | "assets" | "static" | "images" | "img" => 0xD0_A8_3F,
        ".git" | ".github" => 0xF0_50_33,
        "node_modules" | "target" | "dist" | "build" | ".venv" | "__pycache__" | ".wrangler" => {
            0x5F_67_70
        }
        _ => 0x8F_9B_A8,
    };
    let glyph = if open {
        icon::FOLDER_OPEN
    } else {
        icon::FOLDER
    };
    (glyph, rgb(hex))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_get_a_run_command_a_checker_and_an_icon() {
        let script = Path::new("C:/work/my app/main.py");
        assert_eq!(
            run_command("Python", script).as_deref(),
            Some("python \"C:/work/my app/main.py\"")
        );
        assert_eq!(run_command("Markdown", script), None);
        assert_eq!(
            checker("JavaScript", script).map(|it| it.program),
            Some("node")
        );
        assert!(checker("Markdown", script).is_none());
        assert_eq!(file_icon("Main.RS").badge, Some("RS"));
        assert_eq!(file_icon("Cargo.toml").glyph, icon::PACKAGE);
        assert_eq!(file_icon("notes").glyph, icon::FILE);
        assert!(file_icon("notes").badge.is_none());
        assert_eq!(folder_icon("src", true).0, icon::FOLDER_OPEN);
    }
}
