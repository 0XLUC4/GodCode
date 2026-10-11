//! The language servers of the open files: which program serves a language, started the
//! first time a file of that language shows.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use egui::Context;
use godcode_lsp::{Edit, Event, Server};

use crate::field::Hint;

/// The server of a language.
#[derive(Clone, Copy)]
pub struct Known {
    pub program: &'static str,
    args: &'static [&'static str],
    /// The command that installs it.
    pub install: &'static str,
    /// What it is set up with when it starts, as JSON; empty for nothing.
    options: &'static str,
}

const fn known(
    program: &'static str,
    args: &'static [&'static str],
    install: &'static str,
) -> Known {
    Known {
        program,
        args,
        install,
        options: "",
    }
}

const WEB: &str = "npm install -g vscode-langservers-extracted";
/// The TypeScript server writes no hint in the lines unless told which ones.
const TYPESCRIPT_OPTIONS: &str = r#"{ "preferences": {
    "includeInlayParameterNameHints": "literals",
    "includeInlayVariableTypeHints": true,
    "includeInlayFunctionLikeReturnTypeHints": true
} }"#;

/// The server that understands `language`, when a common one exists.
pub fn server_of(language: &str) -> Option<Known> {
    Some(match language {
        "Rust" => known("rust-analyzer", &[], "rustup component add rust-analyzer"),
        "TypeScript" | "JavaScript" => Known {
            options: TYPESCRIPT_OPTIONS,
            ..known(
                "typescript-language-server",
                &["--stdio"],
                // TypeScript 7 no longer ships the `tsserver` this server drives.
                "npm install -g typescript-language-server typescript@5",
            )
        },
        "Python" => known("pyright-langserver", &["--stdio"], "npm install -g pyright"),
        "Go" => known("gopls", &[], "go install golang.org/x/tools/gopls@latest"),
        "C" | "C++" | "Objective-C" => known("clangd", &[], "winget install LLVM.LLVM"),
        "HTML" => known("vscode-html-language-server", &["--stdio"], WEB),
        "CSS" => known("vscode-css-language-server", &["--stdio"], WEB),
        "JSON" => known("vscode-json-language-server", &["--stdio"], WEB),
        "Shell" => known(
            "bash-language-server",
            &["start"],
            "npm install -g bash-language-server",
        ),
        "YAML" => known(
            "yaml-language-server",
            &["--stdio"],
            "npm install -g yaml-language-server",
        ),
        "TOML" => known(
            "taplo",
            &["lsp", "stdio"],
            "cargo install taplo-cli --features lsp",
        ),
        "Lua" => known(
            "lua-language-server",
            &[],
            "winget install LuaLS.lua-language-server",
        ),
        "Zig" => known("zls", &[], "winget install zigtools.zls"),
        "Dart" => known("dart", &["language-server"], "install the Dart SDK"),
        "PHP" => known("intelephense", &["--stdio"], "npm install -g intelephense"),
        "Ruby" => known("solargraph", &["stdio"], "gem install solargraph"),
        "Java" => known("jdtls", &[], "install Eclipse JDT Language Server (jdtls)"),
        "Kotlin" => known(
            "kotlin-language-server",
            &[],
            "install kotlin-language-server",
        ),
        "C#" => known("csharp-ls", &[], "dotnet tool install -g csharp-ls"),
        "Haskell" => known(
            "haskell-language-server-wrapper",
            &["--lsp"],
            "ghcup install hls",
        ),
        "Elixir" => known("elixir-ls", &[], "install elixir-ls"),
        _ => return None,
    })
}

/// The name the protocol gives to the language of `path`.
fn language_id(language: &str, path: &Path) -> String {
    let extension = path.extension().and_then(|extension| extension.to_str());
    match (language, extension.unwrap_or("")) {
        ("TypeScript", "tsx") => "typescriptreact".to_owned(),
        ("JavaScript", "jsx") => "javascriptreact".to_owned(),
        ("CSS", extension @ ("scss" | "less")) => extension.to_owned(),
        ("C++", _) => "cpp".to_owned(),
        ("C#", _) => "csharp".to_owned(),
        ("Shell", _) => "shellscript".to_owned(),
        _ => language.to_lowercase(),
    }
}

/// Line (from 0) and UTF-16 column of the char at `index` of `text`: a place as the
/// protocol gives it.
pub fn position(text: &str, index: usize) -> (usize, usize) {
    let before = &text[..godcode_core::byte_index(text, index)];
    let line = before.rsplit('\n').next().unwrap_or("");
    (before.matches('\n').count(), line.encode_utf16().count())
}

/// The reverse of [`position`]: the char at a line and a UTF-16 column, with its column
/// in chars.
pub fn index_of(text: &str, line: usize, column: usize) -> (usize, usize) {
    let start = godcode_core::line_start(text, line + 1);
    let mut units = 0;
    let chars = text
        .chars()
        .skip(start)
        .take_while(|ch| {
            units += ch.len_utf16();
            *ch != '\n' && units <= column
        })
        .count();
    (start + chars, chars)
}

/// `text` once `edits` are made in it.
pub fn rewritten(text: &str, edits: &[Edit]) -> String {
    let byte = |(line, column): (usize, usize)| {
        godcode_core::byte_index(text, index_of(text, line, column).0)
    };
    let mut ranges: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|edit| (byte(edit.start), byte(edit.end), edit.text.as_str()))
        .collect();
    // From the end, so that the places still to change do not move.
    ranges.sort_by(|a, b| b.0.cmp(&a.0));
    let mut text = text.to_owned();
    for (start, end, with) in ranges {
        text.replace_range(start..end.max(start), &with.replace("\r\n", "\n"));
    }
    text
}

/// Where the char `caret` of `text` is once `edits` are made: what is written or taken out
/// before it moves it along.
pub fn moved(text: &str, edits: &[Edit], caret: usize) -> usize {
    let shifts = edits.iter().map(|edit| {
        let start = index_of(text, edit.start.0, edit.start.1).0;
        let end = index_of(text, edit.end.0, edit.end.1).0.max(start);
        let written = edit.text.replace("\r\n", "\n").chars().count();
        if end <= caret {
            written as isize - (end - start) as isize
        } else {
            0
        }
    });
    caret.saturating_add_signed(shifts.sum())
}

/// The hints of a server, sorted by line, placed in `text`: their columns counted in chars.
/// The text is read through once, however many they are.
pub fn placed(text: &str, hints: Vec<godcode_lsp::Hint>) -> Vec<Hint> {
    let mut lines = text.split('\n').enumerate().peekable();
    let placed = hints.into_iter().filter_map(|hint| {
        while lines.next_if(|(line, _)| *line < hint.line).is_some() {}
        let (_, line) = lines.peek().filter(|(line, _)| *line == hint.line)?;
        let mut units = 0;
        let column = line.chars().take_while(|ch| {
            units += ch.len_utf16();
            units <= hint.column
        });
        Some(Hint {
            line: hint.line,
            column: column.count(),
            label: hint.label,
        })
    });
    placed.collect()
}

/// Hints are asked for this many lines above the top of the view, and below it.
const HINTS_ABOVE: usize = 100;
const HINTS_BELOW: usize = 300;

struct Running {
    program: &'static str,
    root: PathBuf,
    server: Server,
    /// The open files, with the revision of their text the server knows.
    sent: HashMap<PathBuf, u64>,
}

/// How a language is served.
pub enum State {
    /// Its server runs.
    On(&'static str),
    /// Its server is not installed, or stopped: with what it said, if anything.
    Off(Known, String),
}

#[derive(Default)]
pub struct Servers {
    running: Vec<Running>,
    /// Programs that could not start, or stopped, with what they said: not tried again.
    missing: HashMap<&'static str, String>,
}

impl Servers {
    /// Keep the server of `language` in step with the file at `path`: start it for the
    /// project in `root` when needed, tell it the text when its `revision` moved.
    pub fn sync(
        &mut self,
        ctx: &Context,
        root: &Path,
        path: &Path,
        language: &str,
        text: &str,
        revision: u64,
    ) {
        let Some(known) = server_of(language) else {
            return;
        };
        if self.missing.contains_key(known.program) {
            return;
        }
        let serves = |running: &Running| running.program == known.program && running.root == root;
        let index = match self.running.iter().position(serves) {
            Some(index) => index,
            None => {
                // As installed by npm on Windows, a server is a `.cmd` file.
                let names = [
                    known.program.to_owned(),
                    format!("{}.cmd", known.program),
                    format!("{}.exe", known.program),
                ];
                let started = names.iter().find_map(|name| {
                    let ctx = ctx.clone();
                    let wake = move || ctx.request_repaint();
                    Server::start(name, known.args, root, known.options, wake).ok()
                });
                let Some(server) = started else {
                    self.missing.insert(known.program, String::new());
                    return;
                };
                self.running.push(Running {
                    program: known.program,
                    root: root.to_path_buf(),
                    server,
                    sent: HashMap::new(),
                });
                self.running.len() - 1
            }
        };
        let running = &mut self.running[index];
        match running.sent.get(path) {
            Some(sent) if *sent == revision => return,
            // Not taken yet: said again at the next call.
            Some(_) if !running.server.change(path, text) => return,
            Some(_) => {}
            None => running
                .server
                .open(path, &language_id(language, path), text),
        }
        running.sent.insert(path.to_path_buf(), revision);
    }

    fn of(&mut self, path: &Path) -> Option<&mut Server> {
        let mut running = self.running.iter_mut();
        let serving = running.find(|running| running.sent.contains_key(path))?;
        Some(&mut serving.server)
    }

    /// A server knows the file at `path`.
    pub fn serves(&self, path: &Path) -> bool {
        self.running
            .iter()
            .any(|running| running.sent.contains_key(path))
    }

    /// How `language` is served; `None` when no server is known for it.
    pub fn state(&self, language: &str) -> Option<State> {
        let known = server_of(language)?;
        let runs = self
            .running
            .iter()
            .any(|running| running.program == known.program);
        Some(if runs {
            State::On(known.program)
        } else {
            let why = self.missing.get(known.program).cloned();
            State::Off(known, why.unwrap_or_default())
        })
    }

    pub fn saved(&mut self, path: &Path) {
        if let Some(server) = self.of(path) {
            server.saved(path);
        }
    }

    pub fn closed(&mut self, path: &Path) {
        for running in &mut self.running {
            if running.sent.remove(path).is_some() {
                running.server.close(path);
            }
        }
    }

    /// Ask for the documentation of what is at a place; `None` when no server knows the file.
    pub fn hover(&mut self, path: &Path, line: usize, column: usize) -> Option<u64> {
        Some(self.of(path)?.hover(path, line, column))
    }

    pub fn definition(&mut self, path: &Path, line: usize, column: usize) -> Option<u64> {
        Some(self.of(path)?.definition(path, line, column))
    }

    pub fn completion(&mut self, path: &Path, line: usize, column: usize) -> Option<u64> {
        Some(self.of(path)?.completion(path, line, column))
    }

    /// The server of `path`, when it said it can do `provider`.
    fn able(&mut self, path: &Path, provider: &str) -> Option<&mut Server> {
        self.of(path).filter(|server| server.can(provider))
    }

    pub fn references(&mut self, path: &Path, line: usize, column: usize) -> Option<u64> {
        let server = self.able(path, "referencesProvider")?;
        Some(server.references(path, line, column))
    }

    pub fn rename(&mut self, path: &Path, line: usize, column: usize, name: &str) -> Option<u64> {
        let server = self.able(path, "renameProvider")?;
        Some(server.rename(path, line, column, name))
    }

    pub fn format(&mut self, path: &Path, tab_size: usize, spaces: bool) -> Option<u64> {
        let server = self.able(path, "documentFormattingProvider")?;
        Some(server.format(path, tab_size, spaces))
    }

    pub fn implementation(&mut self, path: &Path, line: usize, column: usize) -> Option<u64> {
        let server = self.able(path, "implementationProvider")?;
        Some(server.implementation(path, line, column))
    }

    /// Ask who calls what is at a place (`incoming`), or what it calls.
    pub fn calls(
        &mut self,
        path: &Path,
        line: usize,
        column: usize,
        incoming: bool,
    ) -> Option<u64> {
        let server = self.able(path, "callHierarchyProvider")?;
        Some(server.calls(path, line, column, incoming))
    }

    pub fn signature(&mut self, path: &Path, line: usize, column: usize) -> Option<u64> {
        let server = self.able(path, "signatureHelpProvider")?;
        Some(server.signature(path, line, column))
    }

    /// Ask what is to be written in the lines of `path` around the line `around`; `None`
    /// as long as its server has not been told the text of this `revision`. Returns the
    /// request, the lines asked about, and whether they go to the end of the text.
    pub fn hints(
        &mut self,
        path: &Path,
        text: &str,
        revision: u64,
        around: usize,
    ) -> Option<(u64, Range<usize>, bool)> {
        let mut running = self.running.iter_mut();
        let serving = running.find(|running| running.sent.get(path) == Some(&revision))?;
        let server = Some(&mut serving.server).filter(|server| server.can("inlayHintProvider"))?;
        let total = text.matches('\n').count() + 1;
        let end = around.saturating_add(HINTS_BELOW).min(total);
        let start = around
            .saturating_sub(HINTS_ABOVE)
            .min(end.saturating_sub(HINTS_ABOVE + HINTS_BELOW));
        Some((server.hints(path, start..end), start..end, end == total))
    }

    /// Ask the server of `path` for the definitions of its project named like `query`.
    pub fn symbols(&mut self, path: &Path, query: &str) -> Option<u64> {
        Some(self.able(path, "workspaceSymbolProvider")?.symbols(query))
    }

    pub fn actions(&mut self, path: &Path, line: usize) -> Option<u64> {
        let server = self.able(path, "codeActionProvider")?;
        Some(server.actions(path, line))
    }

    /// A completion of the server was picked in `path`: what else it changes there, as far
    /// as it is known already.
    pub fn accepted(&mut self, path: &Path, item: &godcode_lsp::Item) -> Vec<Edit> {
        self.of(path)
            .map_or_else(Vec::new, |server| server.accepted(path, item))
    }

    /// Have the server of `path` do what is left of an action once its edits are made.
    pub fn carry_out(&mut self, path: &Path, action: &godcode_lsp::Action) {
        if let Some(server) = self.of(path) {
            server.carry_out(action);
        }
    }

    /// What the servers said since the last call. One that stopped is not started again.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        let missing = &mut self.missing;
        self.running.retain_mut(|running| {
            let mut exited = false;
            for event in running.server.poll() {
                match event {
                    Event::Exited { why } => {
                        missing.insert(running.program, why);
                        exited = true;
                    }
                    other => events.push(other),
                }
            }
            !exited
        });
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_convert_between_chars_and_the_protocol() {
        // The emoji is one char and two UTF-16 units.
        let text = "ab\n😀é x";
        assert_eq!(position(text, 5), (1, 3));
        assert_eq!(index_of(text, 1, 3), (5, 2));
        assert_eq!(
            index_of(text, 1, 99),
            (7, 4),
            "a column past the end stays on the line"
        );
        assert_eq!(index_of(text, 0, 1), (1, 1));
        let edit = |start, end, text: &str| Edit {
            start,
            end,
            text: text.to_owned(),
        };
        let edits = [edit((0, 0), (0, 2), "AB"), edit((1, 2), (1, 3), "e\r\n")];
        assert_eq!(rewritten(text, &edits), "AB\n😀e\n x");
        // A caret after what changed follows it; one before stays.
        assert_eq!((moved(text, &edits, 7), moved(text, &edits, 1)), (8, 1));
        let import = [edit((0, 0), (0, 0), "import a;\n")];
        assert_eq!(moved("b()", &import, 3), 13);
        let cut = [edit((0, 0), (1, 0), "")];
        assert_eq!(moved(text, &cut, 5), 2);
        assert_eq!(
            language_id("TypeScript", Path::new("a.tsx")),
            "typescriptreact"
        );
        assert_eq!(language_id("Rust", Path::new("a.rs")), "rust");
        // Hints are placed the same way; one on a line the text does not have is left out.
        let hint = |line, column| godcode_lsp::Hint {
            line,
            column,
            label: ":".to_owned(),
        };
        let hints = placed(text, vec![hint(0, 2), hint(1, 3), hint(1, 5), hint(7, 0)]);
        let found: Vec<_> = hints.iter().map(|hint| (hint.line, hint.column)).collect();
        assert_eq!(found, [(0, 2), (1, 2), (1, 4)]);
        assert!(server_of("Rust").is_some() && server_of("Markdown").is_none());
    }
}
