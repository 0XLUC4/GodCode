//! A client of the Language Server Protocol: JSON-RPC messages over the standard streams
//! of a server process.
//!
//! Nothing here blocks: messages are written and read on threads of their own, and what
//! the server said is collected with [`Server::poll`]. Columns are in UTF-16 units, as the
//! protocol counts them.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use serde_json::{Value, json};

pub mod dap;

/// A problem the server found in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Counted from 0.
    pub line: usize,
    pub column: usize,
    /// An error; a warning otherwise.
    pub error: bool,
    pub message: String,
}

/// A position in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
}

/// Something the server offers to write at the caret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    /// What is written when it is picked.
    pub insert: String,
    /// The item as the server gave it, when picking it may change more than the word: an
    /// import to add. See [`Server::accepted`].
    rest: Value,
}

/// A stretch of a file to replace: from `start` up to `end`, both (line, column).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub text: String,
}

/// The edits of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEdits {
    pub path: PathBuf,
    pub edits: Vec<Edit>,
}

/// A fix or a refactoring the server offers, with what it changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub title: String,
    /// What it changes, as far as the server said: to be made by the app.
    pub files: Vec<FileEdits>,
    /// What is left for the server, once `files` are changed: a command to run, or the
    /// action itself when its changes are still to be asked. See [`Server::carry_out`].
    rest: Value,
}

/// A definition of the project, found by its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Named {
    pub name: String,
    /// What it is in: a class, a module.
    pub container: String,
    pub place: Place,
}

/// A note the server writes in a line: the type of a name, the name of an argument.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub line: usize,
    pub column: usize,
    /// With the space the server wants around it.
    pub label: String,
}

/// What a stretch of lines that folds is, when the server says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldKind {
    Comment,
    Imports,
    /// Between two marks written for that: `// #region` and `// #endregion`.
    Region,
}

/// Lines the server says can be folded away: from `first` to `last`, both counted from 0
/// and both part of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fold {
    pub first: usize,
    pub last: usize,
    /// Nothing for plain code: a function, a block, a list.
    pub kind: Option<FoldKind>,
}

/// What the server said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Every problem of a file: the list replaces the previous one.
    Diagnostics {
        path: PathBuf,
        list: Vec<Diagnostic>,
    },
    /// The documentation asked for by request `id`, as Markdown.
    Hover {
        id: u64,
        text: String,
    },
    Definition {
        id: u64,
        places: Vec<Place>,
    },
    Completion {
        id: u64,
        items: Vec<Item>,
    },
    References {
        id: u64,
        places: Vec<Place>,
    },
    /// Changes to make: the answer to a rename or a formatting (`id`), or something the
    /// server asks for by itself (`id` 0).
    Edits {
        id: u64,
        files: Vec<FileEdits>,
    },
    Actions {
        id: u64,
        actions: Vec<Action>,
    },
    /// The call being written: its signature, and the chars of the argument being typed.
    Signature {
        id: u64,
        label: String,
        active: Option<(usize, usize)>,
    },
    Symbols {
        id: u64,
        list: Vec<Named>,
    },
    /// The notes to write in the lines asked about.
    Hints {
        id: u64,
        list: Vec<Hint>,
    },
    /// What can be folded in the file asked about, from its top down.
    Folds {
        id: u64,
        list: Vec<Fold>,
    },
    /// The server is gone, or refused to start: what it said, when it said something.
    Exited {
        why: String,
    },
}

/// What a request was about, to read its answer.
enum Asked {
    Initialize,
    Hover,
    Definition,
    Completion,
    References,
    Rename,
    /// The edits of a formatting come without the name of their file.
    Format(PathBuf),
    Actions,
    Signature,
    Symbols,
    Hints,
    Folds,
    /// What can be called at a position was asked, to then ask for its callers (`true`) or
    /// for what it calls.
    Callable(bool),
    /// The calls of the request `origin`, the one the answer goes to.
    Calls {
        origin: u64,
        incoming: bool,
    },
    /// The changes of an action that came without them.
    Resolve,
    /// What else a completion picked in this file changes.
    Imports(PathBuf),
    /// A command the server runs: what it changes, it asks for by itself.
    Command,
}

/// The address of a file, as the protocol names it.
pub fn uri_of(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from("file://");
    if !text.starts_with('/') {
        uri.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => {
                uri.push(byte as char);
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

/// The file an address names; `None` for what is not a local file.
pub fn path_of(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?.as_bytes();
    let mut bytes = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        let escaped = (raw[index] == b'%')
            .then(|| std::str::from_utf8(raw.get(index + 1..index + 3)?).ok())
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                bytes.push(byte);
                index += 3;
            }
            None => {
                bytes.push(raw[index]);
                index += 1;
            }
        }
    }
    let mut text = String::from_utf8(bytes).ok()?;
    // `/c:/work/a.rs` is `C:\work\a.rs`: servers do not agree on the case of the drive.
    if text.starts_with('/') && text.as_bytes().get(2) == Some(&b':') {
        text.remove(0);
        let drive = text[..1].to_uppercase();
        text.replace_range(..1, &drive);
        text = text.replace('/', "\\");
    }
    Some(PathBuf::from(text))
}

/// Start `command` and talk to it over its standard streams: what is sent is written by
/// a thread, what it says is read by another, which calls `wake` at each message and once
/// more when the program is gone.
pub(crate) fn connect(
    mut command: Command,
    wake: impl Fn() + Send + 'static,
) -> std::io::Result<(Child, Sender<String>, Receiver<Value>)> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // No console window of its own.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn()?;
    let streams = child.stdin.take().zip(child.stdout.take());
    let (mut stdin, stdout) = streams.ok_or_else(|| std::io::Error::other("no streams"))?;
    let (outgoing, to_write) = channel::<String>();
    std::thread::spawn(move || {
        for body in to_write {
            let framed = format!("Content-Length: {}\r\n\r\n{body}", body.len());
            if stdin.write_all(framed.as_bytes()).is_err() || stdin.flush().is_err() {
                break;
            }
        }
    });
    let (sender, incoming) = channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        while let Some(message) = read_message(&mut reader) {
            trace("<-", &message);
            if sender.send(message).is_err() {
                return;
            }
            wake();
        }
        // Dropped first: the next poll then sees that the program is gone.
        drop(sender);
        wake();
    });
    Ok((child, outgoing, incoming))
}

/// Read one message: headers, a blank line, then as many bytes as `Content-Length` says.
fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

/// With `GODCODE_LSP_LOG` set, print every message exchanged: to see why a server stays
/// silent.
pub(crate) fn trace(way: &str, message: &Value) {
    if std::env::var_os("GODCODE_LSP_LOG").is_some() {
        eprintln!("lsp {way} {message}");
    }
}

/// The text of a hover: a string, a `{ value }`, or a list of those.
fn plain(contents: &Value) -> String {
    match contents {
        Value::String(text) => text.clone(),
        Value::Object(_) => match (contents["language"].as_str(), contents["value"].as_str()) {
            (Some(language), Some(code)) => format!("```{language}\n{code}\n```"),
            (None, Some(text)) => text.to_owned(),
            _ => String::new(),
        },
        Value::Array(parts) => {
            let parts: Vec<String> = parts.iter().map(plain).collect();
            let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
            parts.join("\n\n").trim().to_owned()
        }
        _ => String::new(),
    }
}

fn end_of(range: &Value) -> (usize, usize) {
    let at = |key: &str| range["end"][key].as_u64().unwrap_or(0) as usize;
    (at("line"), at("character"))
}

fn edits(list: &Value) -> Vec<Edit> {
    let list = list.as_array().into_iter().flatten();
    list.map(|edit| Edit {
        start: start_of(&edit["range"]),
        end: end_of(&edit["range"]),
        text: edit["newText"].as_str().unwrap_or("").to_owned(),
    })
    .collect()
}

/// A `WorkspaceEdit`, in either of its two shapes.
fn workspace_edit(value: &Value) -> Vec<FileEdits> {
    let by_file = value["changes"].as_object().into_iter().flatten();
    let by_file = by_file.map(|(uri, list)| (uri.as_str(), list));
    let by_document = value["documentChanges"].as_array().into_iter().flatten();
    let by_document = by_document
        .filter_map(|change| Some((change["textDocument"]["uri"].as_str()?, &change["edits"])));
    by_file
        .chain(by_document)
        .filter_map(|(uri, list)| {
            Some(FileEdits {
                path: path_of(uri)?,
                edits: edits(list),
            })
        })
        .collect()
}

fn start_of(range: &Value) -> (usize, usize) {
    let at = |key: &str| range["start"][key].as_u64().unwrap_or(0) as usize;
    (at("line"), at("character"))
}

/// A `Location` or a `LocationLink`.
fn place(value: &Value) -> Option<Place> {
    let uri = value["uri"].as_str().or(value["targetUri"].as_str())?;
    let range = if value["range"].is_object() {
        &value["range"]
    } else {
        &value["targetSelectionRange"]
    };
    let (line, column) = start_of(range);
    Some(Place {
        path: path_of(uri)?,
        line,
        column,
    })
}

/// The one stretch that differs between two texts: where it starts and ends in `old`, as
/// (line, column), and what stands there in `new`.
fn difference<'new>(old: &str, new: &'new str) -> ((usize, usize), (usize, usize), &'new str) {
    let (old_bytes, new_bytes) = (old.as_bytes(), new.as_bytes());
    let same = |(a, b): &(&u8, &u8)| a == b;
    let mut prefix = old_bytes.iter().zip(new_bytes).take_while(same).count();
    // Never between the bytes of a char, nor between the two chars of a line end: a
    // position there has no column the server would agree on.
    while !old.is_char_boundary(prefix) || old[..prefix].ends_with('\r') {
        prefix -= 1;
    }
    let ends = old_bytes[prefix..]
        .iter()
        .rev()
        .zip(new_bytes[prefix..].iter().rev());
    let mut suffix = ends.take_while(same).count();
    while !old.is_char_boundary(old.len() - suffix) || old[old.len() - suffix..].starts_with('\n') {
        suffix -= 1;
    }
    let place = |end: usize| {
        let before = &old[..end];
        let line_start = before.rfind('\n').map_or(0, |found| found + 1);
        let line = before.bytes().filter(|byte| *byte == b'\n').count();
        (line, before[line_start..].encode_utf16().count())
    };
    let written = &new[prefix..new.len() - suffix];
    (place(prefix), place(old.len() - suffix), written)
}

/// A running language server.
pub struct Server {
    child: Child,
    /// Messages on their way to the server, written by a thread of their own.
    outgoing: Sender<String>,
    incoming: Receiver<Value>,
    next_id: u64,
    pending: HashMap<u64, Asked>,
    /// The server answered `initialize`: it listens.
    ready: bool,
    /// What was said before it was ready.
    queued: Vec<Value>,
    /// Each open file: its version, bumped at every change, and its text as the server
    /// has it.
    open: HashMap<PathBuf, (i64, String)>,
    exited: bool,
    /// What the server said it can do, when it answered `initialize`.
    capabilities: Value,
    /// The problems of each file as the server sent them: fixes are asked with them.
    reported: HashMap<PathBuf, Vec<Value>>,
}

impl Server {
    /// Start `program` for the project in `root`. `options` is what this server is set up
    /// with, as JSON (its `initializationOptions`), or nothing. `wake` is called, from
    /// another thread, whenever there is something to [`Self::poll`].
    pub fn start(
        program: &str,
        args: &[&str],
        root: &Path,
        options: &str,
        wake: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self> {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            // No console window of its own.
            command.creation_flags(0x0800_0000);
        }
        let (child, outgoing, incoming) = connect(command, wake)?;
        let mut server = Self {
            child,
            outgoing,
            incoming,
            next_id: 0,
            pending: HashMap::new(),
            ready: false,
            queued: Vec::new(),
            open: HashMap::new(),
            exited: false,
            capabilities: Value::Null,
            reported: HashMap::new(),
        };
        let uri = uri_of(root);
        let name = root
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let options: Value = serde_json::from_str(options).unwrap_or(Value::Null);
        let params = json!({
            "processId": std::process::id(),
            "initializationOptions": options,
            "rootUri": uri,
            "workspaceFolders": [{ "uri": uri, "name": name }],
            "capabilities": {
                "general": { "positionEncodings": ["utf-16"] },
                "workspace": {
                    "configuration": true,
                    "workspaceFolders": true,
                    "symbol": {},
                    "applyEdit": true,
                    "executeCommand": {},
                },
                "textDocument": {
                    "synchronization": { "didSave": true },
                    "publishDiagnostics": {},
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "definition": { "linkSupport": true },
                    "completion": { "completionItem": {
                        "snippetSupport": false,
                        "resolveSupport": { "properties": ["additionalTextEdits"] },
                    } },
                    "references": {},
                    "implementation": { "linkSupport": true },
                    "callHierarchy": {},
                    "signatureHelp": {},
                    "inlayHint": {},
                    "foldingRange": { "lineFoldingOnly": true },
                    "rename": {},
                    "formatting": {},
                    "codeAction": {
                        "codeActionLiteralSupport": { "codeActionKind": {
                            "valueSet": ["quickfix", "refactor", "source"],
                        } },
                        "dataSupport": true,
                        "resolveSupport": { "properties": ["edit", "command"] },
                    },
                },
            },
        });
        server.next_id += 1;
        server.pending.insert(server.next_id, Asked::Initialize);
        server.write(&json!({
            "jsonrpc": "2.0",
            "id": server.next_id,
            "method": "initialize",
            "params": params,
        }));
        Ok(server)
    }

    fn write(&mut self, message: &Value) {
        trace("->", message);
        // A server that is gone is noticed by `poll`, not here.
        let _ = self.outgoing.send(message.to_string());
    }

    fn send(&mut self, message: Value) {
        if self.ready {
            self.write(&message);
        } else {
            self.queued.push(message);
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn request(&mut self, method: &str, params: Value, asked: Asked) -> u64 {
        self.next_id += 1;
        self.pending.insert(self.next_id, asked);
        self.send(json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": method,
            "params": params,
        }));
        self.next_id
    }

    /// Tell the server a file is being edited, with its text.
    pub fn open(&mut self, path: &Path, language_id: &str, text: &str) {
        self.open.insert(path.to_path_buf(), (1, text.to_owned()));
        let document = json!({
            "uri": uri_of(path),
            "languageId": language_id,
            "version": 1,
            "text": text,
        });
        self.notify("textDocument/didOpen", json!({ "textDocument": document }));
    }

    /// Tell the server the text of an open file changed: only what differs, when the
    /// server takes that, the whole text otherwise.
    ///
    /// Returns `false`, the server told nothing, while it is asked what a completion picked
    /// in this file changes: it answers about the text it has, and fails on a newer one.
    /// The change is then to be said again later.
    pub fn change(&mut self, path: &Path, text: &str) -> bool {
        let mut pending = self.pending.values();
        if pending.any(|asked| matches!(asked, Asked::Imports(of) if of == path)) {
            return false;
        }
        let sync = &self.capabilities["textDocumentSync"];
        let incremental = sync.as_u64().or(sync["change"].as_u64()) == Some(2);
        let Some((version, sent)) = self.open.get_mut(path) else {
            return true;
        };
        *version += 1;
        let change = if incremental {
            let (start, end, written) = difference(sent, text);
            let at =
                |(line, character): (usize, usize)| json!({ "line": line, "character": character });
            json!({ "range": { "start": at(start), "end": at(end) }, "text": written })
        } else {
            json!({ "text": text })
        };
        let document = json!({ "uri": uri_of(path), "version": *version });
        text.clone_into(sent);
        let params = json!({ "textDocument": document, "contentChanges": [change] });
        self.notify("textDocument/didChange", params);
        true
    }

    pub fn saved(&mut self, path: &Path) {
        if self.open.contains_key(path) {
            let document = json!({ "uri": uri_of(path) });
            self.notify("textDocument/didSave", json!({ "textDocument": document }));
        }
    }

    pub fn close(&mut self, path: &Path) {
        if self.open.remove(path).is_some() {
            let document = json!({ "uri": uri_of(path) });
            self.notify("textDocument/didClose", json!({ "textDocument": document }));
        }
    }

    fn at(path: &Path, line: usize, column: usize) -> Value {
        json!({
            "textDocument": { "uri": uri_of(path) },
            "position": { "line": line, "character": column },
        })
    }

    /// Ask for the documentation of what is at a position; answered by [`Event::Hover`].
    pub fn hover(&mut self, path: &Path, line: usize, column: usize) -> u64 {
        self.request(
            "textDocument/hover",
            Self::at(path, line, column),
            Asked::Hover,
        )
    }

    /// Ask where what is at a position is defined; answered by [`Event::Definition`].
    pub fn definition(&mut self, path: &Path, line: usize, column: usize) -> u64 {
        let params = Self::at(path, line, column);
        self.request("textDocument/definition", params, Asked::Definition)
    }

    /// Ask what can be written at a position; answered by [`Event::Completion`].
    pub fn completion(&mut self, path: &Path, line: usize, column: usize) -> u64 {
        let params = Self::at(path, line, column);
        self.request("textDocument/completion", params, Asked::Completion)
    }

    /// The server said it has this capability (`renameProvider`…). Nothing is known before
    /// it answered `initialize`.
    pub fn can(&self, provider: &str) -> bool {
        !matches!(
            self.capabilities[provider],
            Value::Null | Value::Bool(false)
        )
    }

    /// Ask where what is at a position is used; answered by [`Event::References`].
    pub fn references(&mut self, path: &Path, line: usize, column: usize) -> u64 {
        let mut params = Self::at(path, line, column);
        params["context"] = json!({ "includeDeclaration": true });
        self.request("textDocument/references", params, Asked::References)
    }

    /// Ask to rename what is at a position, wherever it is used; answered by
    /// [`Event::Edits`].
    pub fn rename(&mut self, path: &Path, line: usize, column: usize, name: &str) -> u64 {
        let mut params = Self::at(path, line, column);
        params["newName"] = json!(name);
        self.request("textDocument/rename", params, Asked::Rename)
    }

    /// Ask where what is at a position is implemented; answered by [`Event::Definition`].
    pub fn implementation(&mut self, path: &Path, line: usize, column: usize) -> u64 {
        let params = Self::at(path, line, column);
        self.request("textDocument/implementation", params, Asked::Definition)
    }

    /// Ask who calls what is at a position (`incoming`), or what it calls; answered by
    /// [`Event::References`]: the calls themselves for the callers, the definitions for
    /// what is called.
    pub fn calls(&mut self, path: &Path, line: usize, column: usize, incoming: bool) -> u64 {
        let params = Self::at(path, line, column);
        let asked = Asked::Callable(incoming);
        self.request("textDocument/prepareCallHierarchy", params, asked)
    }

    /// Ask which call is being written at a position; answered by [`Event::Signature`].
    pub fn signature(&mut self, path: &Path, line: usize, column: usize) -> u64 {
        let params = Self::at(path, line, column);
        self.request("textDocument/signatureHelp", params, Asked::Signature)
    }

    /// Ask what is to be written in the lines `lines` of a file; answered by
    /// [`Event::Hints`].
    pub fn hints(&mut self, path: &Path, lines: std::ops::Range<usize>) -> u64 {
        let params = json!({
            "textDocument": { "uri": uri_of(path) },
            "range": {
                "start": { "line": lines.start, "character": 0 },
                "end": { "line": lines.end, "character": 0 },
            },
        });
        self.request("textDocument/inlayHint", params, Asked::Hints)
    }

    /// Ask what can be folded in a file; answered by [`Event::Folds`].
    pub fn folds(&mut self, path: &Path) -> u64 {
        let params = json!({ "textDocument": { "uri": uri_of(path) } });
        self.request("textDocument/foldingRange", params, Asked::Folds)
    }

    /// Ask for the definitions of the project whose name looks like `query`; answered by
    /// [`Event::Symbols`].
    pub fn symbols(&mut self, query: &str) -> u64 {
        self.request(
            "workspace/symbol",
            json!({ "query": query }),
            Asked::Symbols,
        )
    }

    /// Ask to format a file; answered by [`Event::Edits`].
    pub fn format(&mut self, path: &Path, tab_size: usize, spaces: bool) -> u64 {
        let params = json!({
            "textDocument": { "uri": uri_of(path) },
            "options": { "tabSize": tab_size, "insertSpaces": spaces },
        });
        let asked = Asked::Format(path.to_path_buf());
        self.request("textDocument/formatting", params, asked)
    }

    /// Ask what can be fixed or reworked on a line; answered by [`Event::Actions`].
    pub fn actions(&mut self, path: &Path, line: usize) -> u64 {
        let on_line = |found: &&Value| {
            let range = &found["range"];
            (start_of(range).0..=end_of(range).0).contains(&line)
        };
        let reported = self.reported.get(path).into_iter().flatten();
        let reported: Vec<&Value> = reported.filter(on_line).collect();
        let params = json!({
            "textDocument": { "uri": uri_of(path) },
            "range": {
                "start": { "line": line, "character": 0 },
                "end": { "line": line + 1, "character": 0 },
            },
            "context": { "diagnostics": reported },
        });
        self.request("textDocument/codeAction", params, Asked::Actions)
    }

    /// A completion was picked in `path`. What else it changes there (an import to add) is
    /// returned when the server said it with the item; otherwise it is asked for, and
    /// comes as [`Event::Edits`]. To call before [`Self::change`] says the word is written:
    /// a server answers about the text its list was made for.
    pub fn accepted(&mut self, path: &Path, item: &Item) -> Vec<Edit> {
        let known = edits(&item.rest["additionalTextEdits"]);
        let resolves = self.capabilities["completionProvider"]["resolveProvider"] == true;
        if known.is_empty() && resolves && !item.rest.is_null() {
            let asked = Asked::Imports(path.to_path_buf());
            self.request("completionItem/resolve", item.rest.clone(), asked);
        }
        known
    }

    /// Have the server do what is left of an action once its `files` are changed: run its
    /// command, or say what it changes. The changes come as [`Event::Edits`].
    pub fn carry_out(&mut self, action: &Action) {
        if let Some(command) = action.rest.get("command") {
            self.execute(command);
        } else if let Some(lazy) = action.rest.get("resolve") {
            self.request("codeAction/resolve", lazy.clone(), Asked::Resolve);
        }
    }

    fn execute(&mut self, command: &Value) {
        let mut params = json!({ "command": command["command"] });
        if command["arguments"].is_array() {
            params["arguments"] = command["arguments"].clone();
        }
        self.request("workspace/executeCommand", params, Asked::Command);
    }

    /// Everything the server said since the last call.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            match self.incoming.try_recv() {
                Ok(message) => self.handle(&message, &mut events),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !std::mem::replace(&mut self.exited, true) {
                        events.push(Event::Exited { why: String::new() });
                    }
                    break;
                }
            }
        }
        events
    }

    fn handle(&mut self, message: &Value, events: &mut Vec<Event>) {
        let id = &message["id"];
        if let Some(method) = message["method"].as_str() {
            let params = &message["params"];
            if method == "textDocument/publishDiagnostics" {
                if let Some(path) = params["uri"].as_str().and_then(path_of) {
                    let list = params["diagnostics"].as_array().cloned();
                    self.reported.insert(path, list.unwrap_or_default());
                }
                events.extend(diagnostics(params));
            } else if method == "workspace/applyEdit" {
                // A fix carried out by the server: it hands back what to change.
                let files = workspace_edit(&params["edit"]);
                events.push(Event::Edits { id: 0, files });
                let result = json!({ "applied": true });
                self.write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            } else if !id.is_null() {
                // The server asks something: it gets an empty answer rather than none.
                let result = match message["params"]["items"].as_array() {
                    Some(items) if method == "workspace/configuration" => {
                        json!(vec![Value::Null; items.len()])
                    }
                    _ => Value::Null,
                };
                self.write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            }
            return;
        }
        let Some((id, asked)) = id
            .as_u64()
            .and_then(|id| Some((id, self.pending.remove(&id)?)))
        else {
            return;
        };
        let result = &message["result"];
        match asked {
            // A server that cannot serve this project says so here, then leaves.
            Asked::Initialize if message["error"].is_object() => {
                let why = message["error"]["message"].as_str().unwrap_or("");
                self.exited = true;
                events.push(Event::Exited {
                    why: why.to_owned(),
                });
            }
            Asked::Initialize => {
                self.capabilities = result["capabilities"].clone();
                self.ready = true;
                self.write(&json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));
                for queued in std::mem::take(&mut self.queued) {
                    self.write(&queued);
                }
            }
            Asked::Hover => events.push(Event::Hover {
                id,
                text: plain(&result["contents"]),
            }),
            Asked::Definition => {
                let places = match result {
                    Value::Array(list) => list.iter().filter_map(place).collect(),
                    other => place(other).into_iter().collect(),
                };
                events.push(Event::Definition { id, places });
            }
            Asked::Signature => {
                let (label, active) = signature(result).unwrap_or_default();
                events.push(Event::Signature { id, label, active });
            }
            Asked::Symbols => {
                let list = result.as_array().into_iter().flatten();
                let list = list.filter_map(|found| {
                    Some(Named {
                        name: found["name"].as_str()?.to_owned(),
                        container: found["containerName"].as_str().unwrap_or("").to_owned(),
                        place: place(&found["location"])?,
                    })
                });
                events.push(Event::Symbols {
                    id,
                    list: list.collect(),
                });
            }
            Asked::Hints => events.push(Event::Hints {
                id,
                list: hints(result),
            }),
            Asked::Folds => events.push(Event::Folds {
                id,
                list: folds(result),
            }),
            Asked::References => {
                let list = result.as_array().into_iter().flatten();
                let places = list.filter_map(place).collect();
                events.push(Event::References { id, places });
            }
            // The server names what is at the position first; its calls are asked with
            // that name, and answer the request the app knows.
            Asked::Callable(incoming) => match result.get(0) {
                Some(item) => {
                    let method = if incoming {
                        "callHierarchy/incomingCalls"
                    } else {
                        "callHierarchy/outgoingCalls"
                    };
                    let asked = Asked::Calls {
                        origin: id,
                        incoming,
                    };
                    self.request(method, json!({ "item": item }), asked);
                }
                None => events.push(Event::References {
                    id,
                    places: Vec::new(),
                }),
            },
            Asked::Calls { origin, incoming } => events.push(Event::References {
                id: origin,
                places: calls(result, incoming),
            }),
            Asked::Rename => events.push(Event::Edits {
                id,
                files: workspace_edit(result),
            }),
            Asked::Format(path) => {
                let edits = edits(result);
                events.push(Event::Edits {
                    id,
                    files: vec![FileEdits { path, edits }],
                });
            }
            Asked::Actions => {
                let resolves = self.capabilities["codeActionProvider"]["resolveProvider"] == true;
                let list = result.as_array().into_iter().flatten();
                let actions = list.filter_map(|found| action(found, resolves));
                events.push(Event::Actions {
                    id,
                    actions: actions.collect(),
                });
            }
            Asked::Resolve => {
                let files = workspace_edit(&result["edit"]);
                if !files.is_empty() {
                    events.push(Event::Edits { id, files });
                }
                if result["command"].is_object() {
                    self.execute(&result["command"]);
                }
            }
            Asked::Command => {}
            Asked::Imports(path) => {
                let edits = edits(&result["additionalTextEdits"]);
                // Said even when there is nothing to change, or the server failed: the
                // question is answered either way.
                let files = if edits.is_empty() {
                    Vec::new()
                } else {
                    vec![FileEdits { path, edits }]
                };
                events.push(Event::Edits { id, files });
                if result["command"].is_object() {
                    self.execute(&result["command"]);
                }
            }
            Asked::Completion => {
                let list = result.as_array().or(result["items"].as_array());
                let items = list.into_iter().flatten().filter_map(item).collect();
                events.push(Event::Completion { id, items });
            }
        }
    }
}

/// A `CodeAction`, or a bare `Command`. `resolves`: the server can be asked later for the
/// changes of an action that comes without them. One nothing can be done with is left out.
fn action(found: &Value, resolves: bool) -> Option<Action> {
    if found["disabled"].is_object() {
        return None;
    }
    let files = workspace_edit(&found["edit"]);
    let rest = match &found["command"] {
        Value::String(_) => json!({ "command": found }),
        command @ Value::Object(_) => json!({ "command": command }),
        _ if files.is_empty() && resolves => json!({ "resolve": found }),
        _ => Value::Null,
    };
    (!files.is_empty() || !rest.is_null()).then(|| Action {
        title: found["title"].as_str().unwrap_or("Fix").to_owned(),
        files,
        rest,
    })
}

/// Where the callers of a function call it (`incoming`), or where what it calls is defined.
fn calls(result: &Value, incoming: bool) -> Vec<Place> {
    let list = result.as_array().into_iter().flatten();
    let places = list.flat_map(|call| {
        let (uri, ranges) = if incoming {
            let from = &call["from"];
            let sites = call["fromRanges"].as_array();
            let sites = sites.filter(|sites| !sites.is_empty()).cloned();
            let ranges = sites.unwrap_or_else(|| vec![from["selectionRange"].clone()]);
            (&from["uri"], ranges)
        } else {
            let to = &call["to"];
            (&to["uri"], vec![to["selectionRange"].clone()])
        };
        let path = uri.as_str().and_then(path_of);
        ranges.into_iter().filter_map(move |range| {
            let (line, column) = start_of(&range);
            Some(Place {
                path: path.clone()?,
                line,
                column,
            })
        })
    });
    places.collect()
}

/// The signature a `SignatureHelp` points at, and where its active parameter is in it.
fn signature(result: &Value) -> Option<(String, Option<(usize, usize)>)> {
    let index = result["activeSignature"].as_u64().unwrap_or(0) as usize;
    let list = &result["signatures"];
    let chosen = list.get(index).or_else(|| list.get(0))?;
    let label = chosen["label"].as_str()?.to_owned();
    let which = chosen["activeParameter"].as_u64();
    let which = which.or(result["activeParameter"].as_u64()).unwrap_or(0) as usize;
    let active = match &chosen["parameters"][which]["label"] {
        Value::String(name) => label.find(name.as_str()).map(|byte| {
            let start = label[..byte].chars().count();
            (start, start + name.chars().count())
        }),
        Value::Array(ends) => {
            let end = |index: usize| Some(ends.get(index)?.as_u64()? as usize);
            end(0).zip(end(1))
        }
        _ => None,
    };
    Some((label, active))
}

/// The `InlayHint`s of an answer, in the order of the text. A label comes whole or in parts.
fn hints(result: &Value) -> Vec<Hint> {
    let list = result.as_array().into_iter().flatten();
    let mut hints: Vec<Hint> = list
        .filter_map(|found| {
            let label = match &found["label"] {
                Value::String(label) => label.clone(),
                Value::Array(parts) => {
                    let values = parts.iter().filter_map(|part| part["value"].as_str());
                    values.collect()
                }
                _ => return None,
            };
            let space = |key: &str| if found[key] == true { " " } else { "" };
            let position = &found["position"];
            Some(Hint {
                line: position["line"].as_u64()? as usize,
                column: position["character"].as_u64()? as usize,
                label: format!("{}{label}{}", space("paddingLeft"), space("paddingRight")),
            })
        })
        .filter(|hint| !hint.label.is_empty())
        .collect();
    hints.sort_by_key(|hint| (hint.line, hint.column));
    hints
}

/// The `FoldingRange`s of an answer, from the top of the file down, each stretch of lines
/// once. One that ends on the line it starts on folds nothing, and is left out.
fn folds(result: &Value) -> Vec<Fold> {
    let list = result.as_array().into_iter().flatten();
    let mut folds: Vec<Fold> = list
        .filter_map(|found| {
            let line = |key: &str| Some(found[key].as_u64()? as usize);
            let kind = match found["kind"].as_str() {
                Some("comment") => Some(FoldKind::Comment),
                Some("imports") => Some(FoldKind::Imports),
                Some("region") => Some(FoldKind::Region),
                _ => None,
            };
            Some(Fold {
                first: line("startLine")?,
                last: line("endLine")?,
                kind,
            })
        })
        .filter(|fold| fold.last > fold.first)
        .collect();
    folds.sort_by_key(|fold| (fold.first, fold.last));
    folds.dedup_by_key(|fold| (fold.first, fold.last));
    folds
}

fn diagnostics(params: &Value) -> Option<Event> {
    let path = path_of(params["uri"].as_str()?)?;
    let list = params["diagnostics"].as_array()?;
    let list = list
        .iter()
        // Hints are suggestions, not problems.
        .filter(|found| found["severity"].as_u64() != Some(4))
        .map(|found| {
            let (line, column) = start_of(&found["range"]);
            Diagnostic {
                line,
                column,
                error: found["severity"].as_u64().unwrap_or(1) == 1,
                message: found["message"].as_str().unwrap_or("").to_owned(),
            }
        })
        .collect();
    Some(Event::Diagnostics { path, list })
}

fn item(value: &Value) -> Option<Item> {
    let label = value["label"].as_str()?.to_owned();
    let written = value["insertText"]
        .as_str()
        .or(value["textEdit"]["newText"].as_str());
    // A snippet has place holders this client does not fill, and an edit may rewrite what
    // stands before the word (`.log` for `log`): the label is written instead.
    let snippet = value["insertTextFormat"].as_u64() == Some(2);
    let continues = |text: &&str| text.chars().next() == label.chars().next();
    let insert = written
        .filter(|_| !snippet)
        .filter(continues)
        .unwrap_or(&label)
        .to_owned();
    // Kept only when there is, or may be, more to it than the word.
    let more = value["additionalTextEdits"].is_array() || !value["data"].is_null();
    let rest = if more { value.clone() } else { Value::Null };
    Some(Item {
        label,
        insert,
        rest,
    })
}

impl Drop for Server {
    fn drop(&mut self) {
        self.ready = true;
        self.notify("exit", Value::Null);
        let _ = self.child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_and_addresses_convert_both_ways() {
        let path = Path::new(r"C:\work\my app\é.rs");
        let uri = uri_of(path);
        assert_eq!(uri, "file:///C:/work/my%20app/%C3%A9.rs");
        if cfg!(windows) {
            assert_eq!(path_of(&uri).as_deref(), Some(path));
            assert_eq!(
                path_of("file:///c%3A/work/my%20app/%C3%A9.rs").as_deref(),
                Some(path)
            );
        }
        assert_eq!(
            path_of("file:///home/a.rs"),
            Some(PathBuf::from("/home/a.rs"))
        );
        assert_eq!(path_of("untitled:1"), None);
    }

    /// `old` once the stretch `start..end` is replaced, the way a server does it.
    fn replaced(old: &str, start: (usize, usize), end: (usize, usize), text: &str) -> String {
        let byte_of = |(line, column): (usize, usize)| {
            let lines = old.split_inclusive('\n').take(line);
            let line_start = lines.map(str::len).sum::<usize>();
            let mut units = 0;
            let chars = old[line_start..].chars().take_while(|char| {
                let before = units < column;
                units += char.len_utf16();
                before
            });
            line_start + chars.map(char::len_utf8).sum::<usize>()
        };
        format!("{}{text}{}", &old[..byte_of(start)], &old[byte_of(end)..])
    }

    #[test]
    fn only_what_differs_is_sent() {
        assert_eq!(
            difference("let a = 1;\nlet b;\n", "let a = 12;\nlet b;\n"),
            ((0, 9), (0, 9), "2")
        );
        assert_eq!(difference("a\nbc\nd", "a\nd"), ((1, 0), (2, 0), ""));
        // Columns count UTF-16 units: two for the face.
        assert_eq!(difference("é😀 x", "é😀 y"), ((0, 4), (0, 5), "y"));
        assert_eq!(difference("same", "same"), ((0, 4), (0, 4), ""));
        let texts = [
            "",
            "a",
            "fn main() {\n    one();\n}\n",
            "fn main() {\n    one();\n    two();\n}\n",
            "fn main() {\r\n    one();\r\n}\r\n",
            "fn main() {\r\n\r\n}\r\n",
            "é😀\nñ\n",
            "é😁\nñ\nñ\n",
            "\n\n\n",
            "\n\n",
            "aaa",
            "aa",
        ];
        for old in texts {
            for new in texts {
                let (start, end, text) = difference(old, new);
                assert_eq!(replaced(old, start, end, text), new, "{old:?} -> {new:?}");
            }
        }
    }

    #[test]
    fn messages_are_read_one_by_one() {
        let stream = "Content-Length: 17\r\nContent-Type: x\r\n\r\n{\"id\":1,\"a\":\"é\"}Content-Length: 2\r\n\r\n{}";
        let mut reader = stream.as_bytes();
        assert_eq!(
            read_message(&mut reader),
            Some(json!({ "id": 1, "a": "é" }))
        );
        assert_eq!(read_message(&mut reader), Some(json!({})));
        assert_eq!(read_message(&mut reader), None);
    }

    #[test]
    fn edits_are_read_in_both_their_shapes() {
        let range = json!({
            "start": { "line": 1, "character": 2 },
            "end": { "line": 1, "character": 5 },
        });
        let edit = json!({ "range": range, "newText": "new" });
        let by_file = json!({ "changes": { "file:///a.rs": [edit] } });
        let by_document = json!({ "documentChanges": [
            { "textDocument": { "uri": "file:///b.rs", "version": 3 }, "edits": [edit] },
            { "kind": "create", "uri": "file:///c.rs" },
        ]});
        for (value, name) in [(by_file, "/a.rs"), (by_document, "/b.rs")] {
            let files = workspace_edit(&value);
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].path, PathBuf::from(name));
            let found = &files[0].edits[0];
            assert_eq!(
                (found.start, found.end, found.text.as_str()),
                ((1, 2), (1, 5), "new")
            );
        }
        assert!(workspace_edit(&Value::Null).is_empty());
    }

    #[test]
    fn the_argument_being_typed_is_found_in_the_signature() {
        let named = json!({ "signatures": [
            { "label": "add(a: number, b: number): number",
              "parameters": [{ "label": "a: number" }, { "label": "b: number" }] }
        ], "activeParameter": 1 });
        let found = signature(&named).expect("signature");
        assert_eq!(found.1, Some((15, 24)));
        let by_offsets = json!({ "activeSignature": 0, "signatures": [
            { "label": "f(x)", "activeParameter": 0, "parameters": [{ "label": [2, 3] }] }
        ]});
        assert_eq!(
            signature(&by_offsets),
            Some(("f(x)".to_owned(), Some((2, 3))))
        );
        assert_eq!(signature(&Value::Null), None);
    }

    #[test]
    fn hints_are_read_whole_or_in_parts_and_put_in_order() {
        let at = |line: usize, character: usize| json!({ "line": line, "character": character });
        let answer = json!([
            { "position": at(2, 4), "label": [{ "value": "count" }, { "value": ":" }],
              "paddingRight": true },
            { "position": at(0, 9), "label": ": number", "paddingLeft": false },
            { "position": at(1, 0), "label": "" },
            { "label": "nowhere" },
        ]);
        let hint = |line, column, label: &str| Hint {
            line,
            column,
            label: label.to_owned(),
        };
        assert_eq!(
            hints(&answer),
            [hint(0, 9, ": number"), hint(2, 4, "count: ")]
        );
        assert!(hints(&Value::Null).is_empty());
    }

    #[test]
    fn folds_are_read_in_order_and_one_line_ones_left_out() {
        let answer = json!([
            { "startLine": 9, "endLine": 12, "kind": "region" },
            { "startLine": 4, "endLine": 7, "startCharacter": 20, "endCharacter": 1 },
            { "startLine": 0, "endLine": 2, "kind": "imports" },
            { "startLine": 4, "endLine": 6, "kind": "comment" },
            // The same lines twice: the first one said is kept.
            { "startLine": 4, "endLine": 7, "kind": "comment" },
            // A kind this client does not know is plain code.
            { "startLine": 14, "endLine": 15, "kind": "custom" },
            // Nothing to hide: one line, an end before its start.
            { "startLine": 8, "endLine": 8 },
            { "startLine": 8, "endLine": 3 },
            // Not ranges at all.
            { "startLine": 5 },
            { "endLine": 5, "kind": "comment" },
            { "startLine": "1", "endLine": 5 },
            "nothing",
        ]);
        let fold = |first, last, kind| Fold { first, last, kind };
        assert_eq!(
            folds(&answer),
            [
                fold(0, 2, Some(FoldKind::Imports)),
                fold(4, 6, Some(FoldKind::Comment)),
                fold(4, 7, None),
                fold(9, 12, Some(FoldKind::Region)),
                fold(14, 15, None),
            ]
        );
        assert!(folds(&Value::Null).is_empty());
        assert!(folds(&json!([])).is_empty());
    }

    #[test]
    fn actions_keep_what_is_left_for_the_server() {
        let edit = json!({ "changes": { "file:///home/a.rs": [{
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
            "newText": "use b;",
        }] } });
        let run = json!({ "title": "Run", "command": "tidy", "arguments": [1] });
        let rest = |found: &Value, resolves| action(found, resolves).map(|action| action.rest);

        // Changes to make here, and nothing more.
        let plain = json!({ "title": "Import b", "edit": edit });
        let read = action(&plain, true).expect("an action");
        assert_eq!((read.files.len(), read.rest), (1, Value::Null));
        // A command, bare or in an action: the server runs it.
        assert_eq!(rest(&run, false), Some(json!({ "command": run })));
        let both = json!({ "title": "Fix", "edit": edit, "command": run });
        assert_eq!(rest(&both, false), Some(json!({ "command": run })));
        // Neither: its changes are asked later, when the server answers that.
        let lazy = json!({ "title": "Extract", "data": 7 });
        assert_eq!(rest(&lazy, true), Some(json!({ "resolve": lazy })));
        assert_eq!(action(&lazy, false), None);
        let disabled = json!({ "title": "No", "edit": edit, "disabled": { "reason": "x" } });
        assert_eq!(action(&disabled, true), None);
    }

    #[test]
    fn calls_are_read_both_ways() {
        let range = |line: u64| json!({ "start": { "line": line, "character": 4 } });
        let item = |uri: &str, line: u64| json!({ "uri": uri, "selectionRange": range(line) });
        let incoming = json!([
            { "from": item("file:///home/a.rs", 1), "fromRanges": [range(5), range(9)] },
            { "from": item("file:///home/b.rs", 2), "fromRanges": [] },
        ]);
        let lines = |found: &[Place]| -> Vec<(PathBuf, usize)> {
            let named = |place: &Place| (place.path.clone(), place.line);
            found.iter().map(named).collect()
        };
        // Every call of a caller, or the caller itself when the server names no call.
        let (a, b) = (PathBuf::from("/home/a.rs"), PathBuf::from("/home/b.rs"));
        assert_eq!(
            lines(&calls(&incoming, true)),
            [(a.clone(), 5), (a, 9), (b.clone(), 2)]
        );
        let outgoing = json!([{ "to": item("file:///home/b.rs", 7), "fromRanges": [range(3)] }]);
        assert_eq!(lines(&calls(&outgoing, false)), [(b, 7)]);
        assert_eq!(calls(&Value::Null, true), []);
    }

    #[test]
    fn answers_are_read_in_all_their_shapes() {
        assert_eq!(plain(&json!("text")), "text");
        assert_eq!(
            plain(&json!({ "kind": "markdown", "value": "**a**" })),
            "**a**"
        );
        assert_eq!(
            plain(&json!([{ "language": "rust", "value": "fn a()" }, "doc"])),
            "```rust\nfn a()\n```\n\ndoc"
        );
        let range = json!({ "start": { "line": 3, "character": 7 } });
        let link = json!({ "targetUri": "file:///a.rs", "targetSelectionRange": range });
        assert_eq!(
            place(&link).map(|place| (place.line, place.column)),
            Some((3, 7))
        );
        let snippet = json!({ "label": "for", "insertText": "for ${1:i}", "insertTextFormat": 2 });
        assert_eq!(
            item(&snippet).map(|item| item.insert).as_deref(),
            Some("for")
        );
        let edit = json!({ "label": "log", "textEdit": { "newText": "log()" } });
        assert_eq!(
            item(&edit).map(|item| item.insert).as_deref(),
            Some("log()")
        );
        let dotted = json!({ "label": "log", "textEdit": { "newText": ".log" } });
        assert_eq!(
            item(&dotted).map(|item| item.insert).as_deref(),
            Some("log")
        );
        let params = json!({ "uri": "file:///a.rs", "diagnostics": [
            { "range": range, "severity": 2, "message": "unused" },
            { "range": range, "severity": 4, "message": "hint" },
        ]});
        let Some(Event::Diagnostics { list, .. }) = diagnostics(&params) else {
            panic!("diagnostics");
        };
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].line, list[0].column, list[0].error), (3, 7, false));
    }
}
