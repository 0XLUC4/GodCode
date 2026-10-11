//! Commands added by plugins: small WebAssembly programs run shut in, with no way out.
//!
//! A plugin is a `.wasm` file. Each of its commands takes a text (the selection, or the
//! whole file) and gives back a text: the one that replaces it, or one to insert, to show
//! or to open. A plugin can compute, within a budget, in a memory of its own that cannot
//! grow past a cap. It can be lent three things and no other: reading the files of the
//! open project, listing them, and writing lines in its log. A module that imports
//! anything else (WASI, a way to write, the network, a clock) is refused.
//!
//! # What a plugin exports
//!
//! - `memory`: its memory, where the texts are exchanged.
//! - `godcode_alloc(len: i32) -> i32`: the address of `len` free bytes in its memory,
//!   where the editor then writes the text to work on. A plugin with no room left traps.
//! - `godcode_commands() -> i64`: the names of its commands, one per line.
//! - `godcode_run(command: i32, ptr: i32, len: i32) -> i64`: run the command of that
//!   number (0 is the first line of `godcode_commands`) on the `len` bytes at `ptr`, which
//!   `godcode_alloc` gave, and return the text that replaces them.
//! - `godcode_name() -> i64`, which may be left out: the name to show for the plugin, in
//!   place of the name of its file without `.wasm`.
//!
//! Every text is UTF-8. A function that returns a text returns `(ptr << 32) | len`: the
//! address of its first byte in the upper half, its length in bytes in the lower half.
//! The text may be anywhere in the memory, and may be the text received, changed in place.
//!
//! To fail, `godcode_run` sets the highest bit of what it returns (a negative number): the
//! text is then the message shown to the user, and the text of the editor is left as it is.
//!
//! That is the whole of the first version of the interface, and a plugin written for it
//! runs as it always did. What follows may each be added or left out.
//!
//! # What a command does with its text
//!
//! `godcode_kinds() -> i64` gives one word per line, one for each command, in the order
//! of `godcode_commands`. Lines left empty are not counted, in either list.
//!
//! - `replace`: the text given back replaces the selection, or the file. This is what
//!   every command does when `godcode_kinds` is left out.
//! - `insert`: it is inserted at the caret, and the selection is kept.
//! - `show`: it is written nowhere; the editor shows it in a panel. For what counts,
//!   checks, previews or reports.
//! - `open`: it is the text of a new file, without a name yet.
//!
//! Another word, or more or fewer words than commands, and the plugin is refused.
//!
//! # Shortcuts
//!
//! `godcode_keys() -> i64` gives one line per command: line 1 is for the first command,
//! and a line left empty is a command without a shortcut (here the empty lines count;
//! lines missing at the end are empty). A shortcut is written as the editor writes them:
//! `Ctrl+Alt+U`, `Ctrl+Shift+F9`, `F8`. Before the key, any of `Ctrl`, `Alt`, `Shift`, each
//! once; the key is a letter, a digit, or `F1` to `F12`. A letter or a digit needs `Ctrl`
//! or `Alt`, or it would be taken from what is typed. Capitals do not matter. A shortcut
//! that cannot be read, or more lines than commands, and the plugin is refused. The
//! editor keeps its own shortcuts when a plugin asks for one of them.
//!
//! # What a command is told
//!
//! A plugin that exports `godcode_run_with(command: i32, ptr: i32, len: i32) -> i64` is
//! run through it, and its `godcode_run` may then be left out. It works as `godcode_run`
//! does, but what is at `ptr` is a request: lines about where the command is run, one
//! empty line, then the text.
//!
//! ```text
//! file: src/main.rs
//! language: rust
//! line: 12
//! column: 5
//! selection: yes
//!
//! the text, as godcode_run would have it
//! ```
//!
//! - Each line of the head is a name, a colon, one space, a value, and one line feed
//!   (`\n`, never `\r\n`). The value is all that stands between that space and the line
//!   feed; it may be empty, and the line then ends with the space.
//! - The head ends at the first empty line. The text starts right after that line's
//!   line feed and runs to the end, byte for byte: empty lines at its start are its own.
//! - `file`: the file being edited, as the editor names it (its path in the project,
//!   with `/`, when it is in the project); empty when it has no name yet.
//! - `language`: the language of the file, in small letters (`rust`, `markdown`); empty
//!   when it is not known.
//! - `line`, `column`: where the caret is, in decimal, counted from 1; 0 when not known.
//! - `selection`: `yes` when the text is the selection, `no` when it is the whole file.
//! - These five lines are always there, in this order. Later versions may add lines after
//!   them: skip the names you do not know.
//! - No value holds a line break: the editor takes line breaks and other control
//!   characters out of a name before it writes it.
//!
//! # What a plugin may ask of the editor
//!
//! These three functions, imported from the module `godcode`, each only when needed:
//!
//! - `read_file(path_ptr: i32, path_len: i32) -> i64`: the text of a file of the project.
//!   The editor calls `godcode_alloc` for the room, writes the text there and returns
//!   `(ptr << 32) | len`.
//! - `list_files(prefix_ptr: i32, prefix_len: i32) -> i64`: the files of the project that
//!   are in a folder or below it, given back the same way: their paths from the root of
//!   the project, with `/`, one per line, sorted by their bytes. An empty prefix is the
//!   whole project. Folders named `.git`, `node_modules` or `target` are not entered
//!   (the last two are listed when the prefix names them), and links are not followed
//!   nor listed. There are 10,000 paths at most, and 4 MB of them:
//!   in a project with more, which ones are left out is not said, so ask folder by folder.
//! - `log(ptr: i32, len: i32)`: one line for the log of the plugin, which the editor
//!   shows with what the command gives back. 200 lines of 1,024 bytes are kept for a run;
//!   the rest is dropped. A line break in a line becomes a space. Of a run that fails,
//!   only the message of failure is kept.
//!
//! A path is relative to the root of the project and uses `/`: `src/main.rs`. Empty
//! parts and `.` are passed over (`./src//main.rs` is the same file).
//!
//! `read_file` and `list_files` return a negative number when they give nothing:
//!
//! - `-1`: no project is open.
//! - `-2`: the path is refused (see the rules below), or is not in the memory, or is
//!   longer than 4,096 bytes, or is not UTF-8.
//! - `-3`: there is no such file (for `list_files`, no such folder), or it cannot be read.
//! - `-4`: the file is larger than 4 MB.
//! - `-5`: the file is not UTF-8 text.
//! - `-6`: the run has had all it may have: 2,000 calls of these two functions, those
//!   that gave nothing counted, or 32 MB of text.
//! - `-7`: asked from inside `godcode_alloc` while the editor was waiting for the room.
//!
//! # What a plugin is held to
//!
//! - Every run starts the module afresh: a new memory, the globals as written, the start
//!   function run again. Nothing is kept from one run to the next, and nothing has to be
//!   freed.
//! - A run executes two thousand million instructions at most, a few seconds, then is
//!   stopped. Listing the commands, when the plugin is loaded, has fifty million.
//! - The memory holds 64 MB at most; growing it further stops the plugin.
//! - A command is given at most 16 MB of text. The list of commands, of kinds, of
//!   shortcuts, the name and a message of failure hold 64 KB each. The file is 32 MB at
//!   most.
//! - What the editor does for a plugin is paid from the budget of the run, so that a
//!   plugin cannot keep the editor busy for ever: 10,000 instructions for each call, one
//!   for each byte of text given, 5,000 for each file or folder looked at while listing.
//!   A plugin that asks without end is stopped as one that computes without end.
//! - While a plugin is loaded no project is lent: `read_file` and `list_files` return
//!   `-1`, and what is logged is dropped.
//!
//! # What a plugin can and cannot do: the rules
//!
//! For who writes a plugin, and for who wonders whether to trust one.
//!
//! - A plugin cannot write a file, delete or rename one, start a program, reach the
//!   network, read the clock, the environment, the clipboard or the keyboard. There is
//!   no function for any of it, and a module that imports one is not loaded.
//! - All it changes is the text it gives back, which the editor puts where the kind of
//!   the command says, and which can be undone like anything typed.
//! - A plugin that imports `read_file` or `list_files` can read the project, and only
//!   the project: the folder open in the editor. The editor says so before it first runs
//!   such a plugin. A plugin that imports neither sees only the text it is given.
//! - A path that is absolute (`/etc/passwd`, `//server/share`), that holds a `\`, a `:`
//!   (a drive, `C:`, or a stream of a file), a control character such as NUL, or a part
//!   that is `..` or ends with a dot or a space, is refused before anything is looked up.
//! - What a path leads to is then resolved by the system, links and junctions followed,
//!   and refused when it is not inside the root of the project resolved the same way. A
//!   link in the project that points out of it leads nowhere.
//! - Nothing in a `.git` folder can be read or listed, by any name or link that leads
//!   there: it holds addresses and sometimes passwords of servers, not the project.
//! - Since a plugin cannot send anything anywhere, what it reads can only come back in
//!   the text it gives and in its log: read both before sharing them when the plugin is
//!   not yours.
//!
//! # An example
//!
//! One command, which turns the ASCII letters into capitals, in WebAssembly text:
//!
//! ```wat
//! (module
//!   (memory (export "memory") 1)
//!   (data (i32.const 0) "Upper case")
//!   ;; The first free byte. Nothing is freed: every run starts from this again.
//!   (global $free (mut i32) (i32.const 16))
//!
//!   (func (export "godcode_alloc") (param $len i32) (result i32)
//!     (local $at i32)
//!     (local.set $at (global.get $free))
//!     (global.set $free (i32.add (local.get $at) (local.get $len)))
//!     ;; More pages (64 KB each) when the text ends past the last one.
//!     (if (i32.gt_u (global.get $free) (i32.shl (memory.size) (i32.const 16)))
//!       (then
//!         (drop (memory.grow (i32.add (i32.shr_u (local.get $len) (i32.const 16)) (i32.const 1))))))
//!     (local.get $at))
//!
//!   ;; The 10 bytes at 0: (0 << 32) | 10.
//!   (func (export "godcode_commands") (result i64)
//!     (i64.const 10))
//!
//!   (func (export "godcode_run") (param $command i32) (param $ptr i32) (param $len i32) (result i64)
//!     (local $at i32)
//!     (local $end i32)
//!     (local.set $at (local.get $ptr))
//!     (local.set $end (i32.add (local.get $ptr) (local.get $len)))
//!     (block $done
//!       (loop $next
//!         (br_if $done (i32.ge_u (local.get $at) (local.get $end)))
//!         ;; A byte from `a` (97) to `z` is moved down by 32, to `A`..`Z`.
//!         (if (i32.lt_u (i32.sub (i32.load8_u (local.get $at)) (i32.const 97)) (i32.const 26))
//!           (then
//!             (i32.store8 (local.get $at) (i32.sub (i32.load8_u (local.get $at)) (i32.const 32)))))
//!         (local.set $at (i32.add (local.get $at) (i32.const 1)))
//!         (br $next)))
//!     ;; The text was changed where it was: (ptr << 32) | len.
//!     (i64.or
//!       (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
//!       (i64.extend_i32_u (local.get $len)))))
//! ```
//!
//! From Rust: a `#![no_std]` crate with `crate-type = ["cdylib"]`, the functions above
//! written as `#[unsafe(no_mangle)] pub extern "C" fn`, the memory exported as `memory`
//! by the compiler. Built with `cargo build --release --target wasm32-unknown-unknown`.
//! What is asked of the editor is declared in
//! `#[link(wasm_import_module = "godcode")] unsafe extern "C" { fn read_file(ptr: *const u8, len: usize) -> i64; }`.
//!
//! # A second example
//!
//! One command with a shortcut, `Ctrl+Alt+L`, which reads the file of the project whose
//! path is selected and shows how many lines it has: `12 lines`.
//!
//! ```wat
//! (module
//!   ;; Lent by the editor: the text of a file of the project, or a negative number.
//!   (import "godcode" "read_file" (func $read_file (param i32 i32) (result i64)))
//!   (memory (export "memory") 1)
//!   (data (i32.const 0) "Count the lines of the selected path")
//!   (data (i32.const 64) "show")
//!   (data (i32.const 72) "Ctrl+Alt+L")
//!   (data (i32.const 96) "cannot read that file of the project")
//!   ;; The number is written just before this, its last digit at 159.
//!   (data (i32.const 160) " lines")
//!   (global $free (mut i32) (i32.const 256))
//!
//!   ;; Called by the editor for the selection, then again for the text of the file.
//!   (func (export "godcode_alloc") (param $len i32) (result i32)
//!     (local $at i32)
//!     (local.set $at (global.get $free))
//!     (global.set $free (i32.add (local.get $at) (local.get $len)))
//!     (if (i32.gt_u (global.get $free) (i32.shl (memory.size) (i32.const 16)))
//!       (then
//!         (drop (memory.grow (i32.add (i32.shr_u (local.get $len) (i32.const 16)) (i32.const 1))))))
//!     (local.get $at))
//!
//!   ;; The 36 bytes at 0.
//!   (func (export "godcode_commands") (result i64)
//!     (i64.const 36))
//!
//!   ;; What is given back is shown, not written: the 4 bytes at 64, (64 << 32) | 4.
//!   (func (export "godcode_kinds") (result i64)
//!     (i64.const 0x0000004000000004))
//!
//!   ;; The 10 bytes at 72.
//!   (func (export "godcode_keys") (result i64)
//!     (i64.const 0x000000480000000A))
//!
//!   (func (export "godcode_run") (param $command i32) (param $ptr i32) (param $len i32) (result i64)
//!     (local $file i64)
//!     (local $at i32)
//!     (local $end i32)
//!     (local $lines i32)
//!     (local.set $file (call $read_file (local.get $ptr) (local.get $len)))
//!     ;; Negative: no such file, or not one it may read. The highest bit set says that
//!     ;; the 36 bytes at 96 are a message of failure.
//!     (if (i64.lt_s (local.get $file) (i64.const 0))
//!       (then (return (i64.const 0x8000006000000024))))
//!     (local.set $at (i32.wrap_i64 (i64.shr_u (local.get $file) (i64.const 32))))
//!     (local.set $end (i32.add (local.get $at) (i32.wrap_i64 (local.get $file))))
//!     ;; A last line that does not end with a line feed is a line too.
//!     (if (i32.lt_u (local.get $at) (local.get $end))
//!       (then
//!         (local.set $lines
//!           (i32.ne (i32.load8_u (i32.sub (local.get $end) (i32.const 1))) (i32.const 10)))))
//!     (block $done
//!       (loop $next
//!         (br_if $done (i32.ge_u (local.get $at) (local.get $end)))
//!         (local.set $lines
//!           (i32.add (local.get $lines) (i32.eq (i32.load8_u (local.get $at)) (i32.const 10))))
//!         (local.set $at (i32.add (local.get $at) (i32.const 1)))
//!         (br $next)))
//!     ;; The digits, the last one first, each put before the one written before it.
//!     (local.set $at (i32.const 160))
//!     (loop $digit
//!       (local.set $at (i32.sub (local.get $at) (i32.const 1)))
//!       (i32.store8 (local.get $at)
//!         (i32.add (i32.const 48) (i32.rem_u (local.get $lines) (i32.const 10))))
//!       (local.set $lines (i32.div_u (local.get $lines) (i32.const 10)))
//!       (br_if $digit (local.get $lines)))
//!     ;; From the first digit to the end of " lines", at 166.
//!     (i64.or
//!       (i64.shl (i64.extend_i32_u (local.get $at)) (i64.const 32))
//!       (i64.extend_i32_u (i32.sub (i32.const 166) (local.get $at))))))
//! ```

use std::io::Read as _;
use std::path::{Path, PathBuf};

use wasmi::{
    Caller, CompilationMode, Config, Engine, Error, ExternType, Instance, Linker, Memory, Module,
    Store, StoreLimits, StoreLimitsBuilder, TrapCode, ValType, WasmParams, WasmResults,
    errors::HostError,
};

/// The largest file read as a plugin, in bytes.
const MAX_MODULE: usize = 32 << 20;
/// The most memory a plugin may hold, in bytes.
const MAX_MEMORY: usize = 64 << 20;
/// The longest text given to a command, in bytes.
const MAX_TEXT: usize = 16 << 20;
/// The longest list of commands, name or message of failure, in bytes.
const MAX_NAMES: usize = 64 << 10;
/// What one run may spend: about one unit per instruction, a few seconds of work.
const RUN_FUEL: u64 = 2_000_000_000;
/// What starting a plugin and reading its names may spend.
const LOAD_FUEL: u64 = 50_000_000;
/// The bit a plugin sets in what `godcode_run` returns to say it failed.
const FAILED: u64 = 1 << 63;

/// The largest file of the project a plugin is given, and the longest list of files, in
/// bytes.
const MAX_FILE: usize = 4 << 20;
/// All the text of the project a run is given, files and lists together, in bytes.
const MAX_READ: usize = 32 << 20;
/// How many times a run may ask for a file or a list.
const MAX_CALLS: usize = 2_000;
/// The most paths in a list of files.
const MAX_PATHS: usize = 10_000;
/// The longest path a plugin may ask about, in bytes.
const MAX_PATH: usize = 4 << 10;
/// The most lines kept of what a run logs, and the bytes kept of each.
const MAX_LOG: usize = 200;
const MAX_LOG_LINE: usize = 1 << 10;
/// What a run spends each time it asks something of the editor.
const CALL_FUEL: u64 = 10_000;
/// What a run spends for each file or folder looked at while listing.
const ENTRY_FUEL: u64 = 5_000;
/// The folders whose files are never listed.
const SKIPPED: [&str; 3] = [".git", "node_modules", "target"];

/// What `read_file` and `list_files` answer when they give nothing: no project is open,
/// the path is refused, nothing is there, the file is too large, it is not text, the run
/// has had all it may, the plugin asked while the editor waited for its `godcode_alloc`.
const NO_PROJECT: i64 = -1;
const REFUSED: i64 = -2;
const NOT_FOUND: i64 = -3;
const TOO_LARGE: i64 = -4;
const NOT_TEXT: i64 = -5;
const SPENT: i64 = -6;
const BUSY: i64 = -7;

/// The functions of a plugin: the name, what it takes, what it gives. All but the first
/// two may be left out, `godcode_run` only when there is a `godcode_run_with`.
const FUNCTIONS: [(&str, &[ValType], ValType); 7] = [
    ("godcode_alloc", &[ValType::I32], ValType::I32),
    ("godcode_commands", &[], ValType::I64),
    (
        "godcode_run",
        &[ValType::I32, ValType::I32, ValType::I32],
        ValType::I64,
    ),
    ("godcode_name", &[], ValType::I64),
    (
        "godcode_run_with",
        &[ValType::I32, ValType::I32, ValType::I32],
        ValType::I64,
    ),
    ("godcode_kinds", &[], ValType::I64),
    ("godcode_keys", &[], ValType::I64),
];

/// What a plugin may ask of the editor, in the module `godcode`: the name, what it
/// takes, what it gives.
const IMPORTS: [(&str, &[ValType], &[ValType]); 3] = [
    ("read_file", &[ValType::I32, ValType::I32], &[ValType::I64]),
    ("list_files", &[ValType::I32, ValType::I32], &[ValType::I64]),
    ("log", &[ValType::I32, ValType::I32], &[]),
];

/// What the editor does with the text a command gives back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// It replaces the selection, or the file.
    Replace,
    /// It is inserted at the caret; the selection is kept.
    Insert,
    /// It is shown in a panel and written nowhere.
    Show,
    /// It is the text of a new file without a name.
    Open,
}

impl Kind {
    /// The kind a plugin names with `word`.
    fn named(word: &str) -> Option<Kind> {
        match word {
            "replace" => Some(Kind::Replace),
            "insert" => Some(Kind::Insert),
            "show" => Some(Kind::Show),
            "open" => Some(Kind::Open),
            _ => None,
        }
    }
}

/// A shortcut a plugin asks for: the keys held, and the key, in capitals (`U`, `7`, `F9`).
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: String,
}

impl Keys {
    /// The shortcut written in `text`, as `Ctrl+Alt+U` or `F9`; none when it cannot be
    /// read, or is a letter or a digit that nothing but Shift goes with.
    pub fn parse(text: &str) -> Option<Keys> {
        let mut parts = text.split('+').map(str::trim);
        let key = parts.next_back()?.to_ascii_uppercase();
        let mut keys = Keys {
            ctrl: false,
            shift: false,
            alt: false,
            key,
        };
        for held in parts {
            let held = match held.to_ascii_lowercase().as_str() {
                "ctrl" => &mut keys.ctrl,
                "shift" => &mut keys.shift,
                "alt" => &mut keys.alt,
                _ => return None,
            };
            // Named twice.
            if std::mem::replace(held, true) {
                return None;
            }
        }
        let typed = keys.key.len() == 1 && keys.key.bytes().all(|key| key.is_ascii_alphanumeric());
        let function = (1..=12).any(|number| keys.key == format!("F{number}"));
        ((typed && (keys.ctrl || keys.alt)) || function).then_some(keys)
    }
}

/// Written as the editor writes its own: `Ctrl+Alt+Shift+U`.
impl std::fmt::Display for Keys {
    fn fmt(&self, to: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let held = [
            (self.ctrl, "Ctrl+"),
            (self.alt, "Alt+"),
            (self.shift, "Shift+"),
        ];
        for (_, name) in held.iter().filter(|(held, _)| *held) {
            to.write_str(name)?;
        }
        to.write_str(&self.key)
    }
}

/// Where the command is run: what the plugin may be told and may read.
#[derive(Clone, Debug, Default)]
pub struct Context<'a> {
    /// The file being edited, as the plugin is to know it: its path from `root` with `/`
    /// when it is in the project. None when it has no name yet.
    pub file: Option<&'a str>,
    /// The language of the file, in small letters; empty when not known.
    pub language: &'a str,
    /// Where the caret is, counted from 1; 0 when not known.
    pub line: usize,
    pub column: usize,
    /// Whether the text is the selection, rather than the whole file.
    pub selection: bool,
    /// The folder of the project, which a plugin that reads the project may read. With
    /// none, it reads nothing.
    pub root: Option<&'a Path>,
}

/// What a command gave back, and what the editor is to do with it.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub kind: Kind,
    pub text: String,
    /// The lines the plugin logged during the run.
    pub log: Vec<String>,
}

/// A plugin read from its file, ready to run.
pub struct Plugin {
    name: String,
    commands: Vec<String>,
    /// For each command, what is done with its text, and the shortcut it asks for.
    kinds: Vec<Kind>,
    keys: Vec<Option<Keys>>,
    /// Whether it imports what reads the project.
    reads: bool,
    /// Whether it is run through `godcode_run_with`.
    told: bool,
    module: Module,
    /// What a run may spend.
    fuel: u64,
}

/// A command of a plugin, as the palette lists it.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub plugin: String,
    pub name: String,
}

impl Plugin {
    /// Read and check the module in `bytes`, named after `file`; refused when it asks for
    /// what no plugin is lent or lacks what a plugin must have.
    pub fn load(file: &str, bytes: &[u8]) -> Result<Plugin, String> {
        let stem = Path::new(file)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(file);
        Self::read(stem, bytes).map_err(|why| format!("The plugin {stem} {why}"))
    }

    /// The plugin in `bytes`, or what is wrong with it, to be said after its name.
    fn read(stem: &str, bytes: &[u8]) -> Result<Plugin, String> {
        if bytes.len() > MAX_MODULE {
            return Err("is larger than 32 MB".to_owned());
        }
        let mut config = Config::default();
        config
            .consume_fuel(true)
            // All of it is checked and translated now: a run pays only for what it does.
            .compilation_mode(CompilationMode::Eager)
            .wasm_memory64(false);
        let module = Module::new(&Engine::new(&config), bytes)
            .map_err(|err| format!("is not a WebAssembly module that runs here: {err}"))?;
        check(&module)?;
        let exports = |name: &str| module.exports().any(|export| export.name() == name);

        let mut run = Run::start(&module, LOAD_FUEL, None)?;
        let listed = run.call::<(), i64>("godcode_commands", ())?;
        let commands = lines(&run.text(listed, MAX_NAMES)?);
        if commands.is_empty() {
            return Err("has no command".to_owned());
        }
        let mut name = stem.to_owned();
        if exports("godcode_name") {
            let named = run.call::<(), i64>("godcode_name", ())?;
            if let Some(first) = lines(&run.text(named, MAX_NAMES)?).into_iter().next() {
                name = first;
            }
        }
        let mut kinds = vec![Kind::Replace; commands.len()];
        if exports("godcode_kinds") {
            let listed = run.call::<(), i64>("godcode_kinds", ())?;
            kinds = kinds_of(&run.text(listed, MAX_NAMES)?, commands.len())?;
        }
        let mut keys = vec![None; commands.len()];
        if exports("godcode_keys") {
            let listed = run.call::<(), i64>("godcode_keys", ())?;
            keys = keys_of(&run.text(listed, MAX_NAMES)?, commands.len())?;
        }
        Ok(Plugin {
            name,
            commands,
            kinds,
            keys,
            reads: module.imports().any(|import| import.name() != "log"),
            told: exports("godcode_run_with"),
            module,
            fuel: RUN_FUEL,
        })
    }

    /// The name to show for it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The names of its commands, in its order.
    pub fn commands(&self) -> &[String] {
        &self.commands
    }

    /// What is done with the text the command at `index` gives back.
    #[allow(dead_code)]
    pub fn kind(&self, index: usize) -> Kind {
        self.kinds.get(index).copied().unwrap_or(Kind::Replace)
    }

    /// The shortcut the command at `index` asks for, when it asks for one.
    #[allow(dead_code)]
    pub fn keys(&self, index: usize) -> Option<&Keys> {
        self.keys.get(index)?.as_ref()
    }

    /// Whether it may read the files of the project it is run in: to be told to the user
    /// before its first run.
    #[allow(dead_code)]
    pub fn reads_project(&self) -> bool {
        self.reads
    }

    /// Run the command at `index` on `text`: the text it gives back, or why it failed.
    /// It is told nothing of where it runs, and reads no project.
    pub fn run(&self, index: usize, text: &str) -> Result<String, String> {
        self.run_in(index, text, &Context::default())
            .map(|outcome| outcome.text)
    }

    /// Run the command at `index` on `text`, in `context`: what it gives back, what to do
    /// with it and what it logged, or why it failed.
    pub fn run_in(
        &self,
        index: usize,
        text: &str,
        context: &Context<'_>,
    ) -> Result<Outcome, String> {
        self.ran(index, text, context)
            .map_err(|why| format!("The plugin {} {why}", self.name))
    }

    /// What the command at `index` makes of `text`, or what went wrong, to be said after
    /// the name of the plugin.
    fn ran(&self, index: usize, text: &str, context: &Context<'_>) -> Result<Outcome, String> {
        let command = i32::try_from(index)
            .ok()
            .filter(|_| index < self.commands.len())
            .ok_or_else(|| format!("has no command {index}"))?;
        if text.len() > MAX_TEXT {
            return Err("is given more than 16 MB of text".to_owned());
        }
        if context.file.map_or(0, str::len) + context.language.len() > MAX_NAMES {
            return Err("is told of a file whose name is longer than 64 KB".to_owned());
        }
        let told;
        let (entry, given) = if self.told {
            told = request(context, text);
            ("godcode_run_with", told.as_str())
        } else {
            ("godcode_run", text)
        };
        let len = i32::try_from(given.len()).map_err(|_| "is given more than 16 MB of text")?;
        // Resolved by the system once, to compare with what its paths resolve to.
        let root = context
            .root
            .filter(|_| self.reads)
            .and_then(|root| root.canonicalize().ok());

        let mut run = Run::start(&self.module, self.fuel, root)?;
        let ptr = run.call::<i32, i32>("godcode_alloc", len)?;
        run.write(ptr, given)?;
        let answer = run.call::<(i32, i32, i32), i64>(entry, (command, ptr, len))?;
        if answer.cast_unsigned() & FAILED == 0 {
            return Ok(Outcome {
                kind: self.kinds[index],
                text: run.text(answer, MAX_MEMORY)?,
                log: std::mem::take(&mut run.store.data_mut().log),
            });
        }
        match run.text(answer, MAX_NAMES)?.trim() {
            "" => Err("failed without saying why".to_owned()),
            message => Err(format!("failed: {message}")),
        }
    }
}

/// The plugins of `folder` (its `.wasm` files, sorted by name), each loaded or told why
/// not. None when there is no such folder.
pub fn load_folder(folder: &Path) -> Vec<Result<Plugin, String>> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("wasm"))
        })
        .collect();
    files.sort();
    files.iter().map(|path| load_file(path)).collect()
}

/// The plugin in the file at `path`. A file too large is not even read.
fn load_file(path: &Path) -> Result<Plugin, String> {
    let file = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    let cannot = |err: std::io::Error| format!("Cannot read the plugin {file}: {err}");
    let size = std::fs::metadata(path).map_err(cannot)?.len();
    if size > MAX_MODULE as u64 {
        return Err(format!("The plugin {file} is larger than 32 MB"));
    }
    Plugin::load(&file, &std::fs::read(path).map_err(cannot)?)
}

/// The lines of `text` that say something, without the blanks around them.
fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The start of `word`, short enough to be said in a message.
fn start_of(word: &str) -> String {
    word.chars().take(40).collect()
}

/// The kinds a plugin lists in `text` for its `commands` commands, or what is wrong.
fn kinds_of(text: &str, commands: usize) -> Result<Vec<Kind>, String> {
    let words = lines(text);
    if words.len() != commands {
        return Err(format!(
            "says what {} commands do with their text, and has {commands}",
            words.len()
        ));
    }
    let kind = |word: &String| {
        Kind::named(word).ok_or_else(|| {
            format!(
                "says a command does `{}` with its text, which is none of replace, insert, \
                 show, open",
                start_of(word)
            )
        })
    };
    words.iter().map(kind).collect()
}

/// The shortcuts a plugin lists in `text`, line by line, for its `commands` commands, or
/// what is wrong.
fn keys_of(text: &str, commands: usize) -> Result<Vec<Option<Keys>>, String> {
    let mut lines: Vec<&str> = text.lines().map(str::trim).collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    if lines.len() > commands {
        return Err(format!(
            "gives shortcuts on {} lines, and has {commands} commands",
            lines.len()
        ));
    }
    let keys = |line: &&str| {
        if line.is_empty() {
            return Ok(None);
        }
        Keys::parse(line).map(Some).ok_or_else(|| {
            format!(
                "asks for the shortcut `{}`, which cannot be read as one like Ctrl+Alt+U or F9",
                start_of(line)
            )
        })
    };
    let mut keys = lines.iter().map(keys).collect::<Result<Vec<_>, _>>()?;
    keys.resize(commands, None);
    Ok(keys)
}

/// What `godcode_run_with` is given: the lines that tell where the command is run, an
/// empty line, then `text` as it is. No value can hold a line break.
fn request(context: &Context<'_>, text: &str) -> String {
    let plain = |value: &str| -> String {
        value
            .chars()
            .filter(|&c| !c.is_control() && c != '\u{2028}' && c != '\u{2029}')
            .collect()
    };
    format!(
        "file: {}\nlanguage: {}\nline: {}\ncolumn: {}\nselection: {}\n\n{text}",
        plain(context.file.unwrap_or_default()),
        plain(context.language),
        context.line,
        context.column,
        if context.selection { "yes" } else { "no" },
    )
}

/// How a function is written down: what it takes, what it gives.
fn signature(takes: &[ValType], gives: &[ValType]) -> String {
    format!("{takes:?} -> {gives:?}").to_lowercase()
}

/// How what a module exports or asks for is written down.
fn shape(of: &ExternType) -> String {
    match of {
        ExternType::Func(function) => signature(function.params(), function.results()),
        _ => "not a function".to_owned(),
    }
}

/// Whether `module` asks for nothing but what plugins are lent and exports what a plugin
/// must, or what is wrong.
fn check(module: &Module) -> Result<(), String> {
    for import in module.imports() {
        let lent = IMPORTS
            .iter()
            .find(|(name, ..)| import.module() == "godcode" && import.name() == *name);
        let Some((name, takes, gives)) = lent else {
            return Err(format!(
                "asks for `{}.{}` from outside, and plugins are lent nothing",
                import.module(),
                import.name()
            ));
        };
        let (found, wanted) = (shape(import.ty()), signature(takes, gives));
        if found != wanted {
            return Err(format!(
                "asks for `godcode.{name}` as {found}, where it is {wanted}"
            ));
        }
    }
    let exported = |name: &str| module.exports().find(|export| export.name() == name);
    if !matches!(
        exported("memory").as_ref().map(|export| export.ty()),
        Some(ExternType::Memory(_))
    ) {
        return Err("does not export its `memory`".to_owned());
    }
    for (place, (name, takes, gives)) in FUNCTIONS.into_iter().enumerate() {
        let Some(export) = exported(name) else {
            // The first two, and one of the two that run a command.
            if place < 2 || (name == "godcode_run" && exported("godcode_run_with").is_none()) {
                return Err(format!("does not export `{name}`"));
            }
            continue;
        };
        let (found, wanted) = (shape(export.ty()), signature(takes, &[gives]));
        if found != wanted {
            return Err(format!(
                "exports `{name}` as {found}, where {wanted} is expected"
            ));
        }
    }
    Ok(())
}

/// What the editor could not do for a plugin because of what the plugin did, to be said
/// after its name.
#[derive(Debug)]
struct Fault(&'static str);

impl std::fmt::Display for Fault {
    fn fmt(&self, to: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        to.write_str(self.0)
    }
}

impl HostError for Fault {}

/// Why a plugin was stopped, to be said after its name.
fn stopped(err: &Error) -> String {
    if let Some(Fault(why)) = err.downcast_ref() {
        return (*why).to_owned();
    }
    match err.as_trap_code() {
        Some(TrapCode::OutOfFuel) => "ran too long".to_owned(),
        Some(TrapCode::GrowthOperationLimited) => "asked for more than 64 MB of memory".to_owned(),
        _ => format!("stopped: {err}"),
    }
}

/// What the editor keeps for a run: its limits, and what it lends and has lent.
struct Host {
    limits: StoreLimits,
    /// The folder the plugin may read, as the system resolves it. None: it reads nothing.
    root: Option<PathBuf>,
    /// How many times it asked for a file or a list, and the bytes it was given.
    calls: usize,
    read: usize,
    log: Vec<String>,
    /// Whether the editor is waiting for its `godcode_alloc`, where it may ask nothing.
    busy: bool,
}

/// Take `cost` from what the run of `caller` may still spend; stopped when it has less.
fn spend(caller: &mut Caller<'_, Host>, cost: u64) -> Result<(), Error> {
    let left = caller.get_fuel()?;
    caller.set_fuel(left.saturating_sub(cost))?;
    if left < cost {
        return Err(Error::from(TrapCode::OutOfFuel));
    }
    Ok(())
}

/// The `len` bytes at `ptr` in the memory of the plugin, when they are all in it.
fn held<'a>(caller: &'a Caller<'_, Host>, ptr: i32, len: i32) -> Option<&'a [u8]> {
    let memory = caller.get_export("memory")?.into_memory()?;
    let (start, len) = (ptr.cast_unsigned() as usize, len.cast_unsigned() as usize);
    memory.data(caller).get(start..start.checked_add(len)?)
}

/// The folder a plugin may read and the path it asks about with the `len` bytes at
/// `ptr`, or the number that tells it why it is given nothing.
fn asked(caller: &mut Caller<'_, Host>, ptr: i32, len: i32) -> Result<(PathBuf, String), i64> {
    let host = caller.data_mut();
    if host.busy {
        return Err(BUSY);
    }
    if host.calls >= MAX_CALLS {
        return Err(SPENT);
    }
    host.calls += 1;
    let root = host.root.clone().ok_or(NO_PROJECT)?;
    let path = held(caller, ptr, len)
        .filter(|path| path.len() <= MAX_PATH)
        .and_then(|path| std::str::from_utf8(path).ok())
        .ok_or(REFUSED)?;
    Ok((root, path.to_owned()))
}

/// Write `text` in the memory of the plugin, at the place its `godcode_alloc` gives for
/// it, and count it: what the plugin is answered.
fn give(caller: &mut Caller<'_, Host>, text: &str) -> Result<i64, Error> {
    let host = caller.data_mut();
    if text.len() > MAX_READ - host.read {
        return Ok(SPENT);
    }
    host.read += text.len();
    spend(caller, text.len() as u64)?;
    let lacks = || Error::host(Fault("does not export what a plugin must"));
    let alloc = caller
        .get_export("godcode_alloc")
        .and_then(|export| export.into_func())
        .ok_or_else(lacks)?
        .typed::<i32, i32>(&*caller)?;
    let memory = caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
        .ok_or_else(lacks)?;
    let len = i32::try_from(text.len()).map_err(|_| lacks())?;

    // The plugin runs again, inside this call: it may not ask for more from in there.
    caller.data_mut().busy = true;
    let ptr = alloc.call(&mut *caller, len);
    caller.data_mut().busy = false;
    let start = ptr?.cast_unsigned() as usize;
    start
        .checked_add(text.len())
        .and_then(|end| memory.data_mut(&mut *caller).get_mut(start..end))
        .ok_or_else(|| Error::host(Fault("gave a place outside its memory for the text")))?
        .copy_from_slice(text.as_bytes());
    // A place in a memory of 64 MB at most: the highest bit is never set.
    Ok(((start as u64) << 32 | text.len() as u64).cast_signed())
}

/// `godcode.read_file`: the text of the file of the project at the path a plugin names.
fn read_file(mut caller: Caller<'_, Host>, ptr: i32, len: i32) -> Result<i64, Error> {
    spend(&mut caller, CALL_FUEL)?;
    let read = asked(&mut caller, ptr, len)
        .and_then(|(root, path)| file_text(&root, &path, MAX_READ - caller.data().read));
    match read {
        Ok(text) => give(&mut caller, &text),
        Err(why) => Ok(why),
    }
}

/// `godcode.list_files`: the files of the project in the folder a plugin names.
fn list_files(mut caller: Caller<'_, Host>, ptr: i32, len: i32) -> Result<i64, Error> {
    spend(&mut caller, CALL_FUEL)?;
    // It looks at no more files than the run can still pay for.
    let afford = caller.get_fuel()? / ENTRY_FUEL;
    let mut looks = usize::try_from(afford).unwrap_or(usize::MAX);
    let listed = asked(&mut caller, ptr, len)
        .and_then(|(root, folder)| files_in(&root, &folder, MAX_PATHS, &mut looks));
    spend(&mut caller, (afford - looks as u64) * ENTRY_FUEL)?;
    if looks == 0 {
        // It could not pay for the rest: the run ends here, as when it computes too long.
        spend(&mut caller, u64::MAX)?;
    }
    match listed {
        Ok(paths) => give(&mut caller, &paths),
        Err(why) => Ok(why),
    }
}

/// `godcode.log`: keep the line a plugin writes, while there is room for it.
fn log(mut caller: Caller<'_, Host>, ptr: i32, len: i32) -> Result<(), Error> {
    spend(&mut caller, CALL_FUEL)?;
    if caller.data().log.len() >= MAX_LOG {
        return Ok(());
    }
    let Some(bytes) = held(&caller, ptr, len) else {
        return Ok(());
    };
    let line = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_LOG_LINE)])
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    caller.data_mut().log.push(line);
    Ok(())
}

/// The path a plugin names, to be looked up from the root of the project; none when it
/// is written so as to lead elsewhere, or names a `.git`.
fn relative(path: &str) -> Option<PathBuf> {
    // From the root of the disk or of a server, a drive, a stream of a file, a NUL.
    if path.starts_with('/') || path.contains(|c: char| matches!(c, '\\' | ':') || c.is_control()) {
        return None;
    }
    let mut relative = PathBuf::new();
    for part in path.split('/').filter(|part| !matches!(*part, "" | ".")) {
        // `..`, and the names Windows reads without the dots and spaces at their end.
        if part.ends_with(['.', ' ']) || part.eq_ignore_ascii_case(".git") {
            return None;
        }
        relative.push(part);
    }
    Some(relative)
}

/// Whether `found` is in `root` and in no `.git`, both being written as the system
/// resolves them, links followed.
fn within(root: &Path, found: &Path) -> bool {
    found.strip_prefix(root).is_ok_and(|inside| {
        !inside
            .components()
            .any(|part| part.as_os_str().eq_ignore_ascii_case(".git"))
    })
}

/// What `path` leads to in `root`, as the system resolves it, or the number that tells
/// a plugin why it may not have it.
fn resolved(root: &Path, path: &str) -> Result<PathBuf, i64> {
    let relative = relative(path).ok_or(REFUSED)?;
    let found = root.join(relative).canonicalize().map_err(|_| NOT_FOUND)?;
    if within(root, &found) {
        Ok(found)
    } else {
        Err(REFUSED)
    }
}

/// The text of the file at `path` in `root`, when it is `left` bytes at most, or the
/// number that tells a plugin why not.
fn file_text(root: &Path, path: &str, left: usize) -> Result<String, i64> {
    let file = resolved(root, path)?;
    let fits = |size: u64| match size {
        size if size > MAX_FILE as u64 => Err(TOO_LARGE),
        size if size > left as u64 => Err(SPENT),
        _ => Ok(()),
    };
    // Weighed before it is read, and read no further than the cap if it grows meanwhile.
    let about = std::fs::metadata(&file).map_err(|_| NOT_FOUND)?;
    if !about.is_file() {
        return Err(NOT_FOUND);
    }
    fits(about.len())?;
    let mut bytes = Vec::new();
    std::fs::File::open(&file)
        .and_then(|file| file.take(MAX_FILE as u64 + 1).read_to_end(&mut bytes))
        .map_err(|_| NOT_FOUND)?;
    fits(bytes.len() as u64)?;
    String::from_utf8(bytes).map_err(|_| NOT_TEXT)
}

/// The path of `file` from `root`, with `/`, when it can be written on one line.
fn path_from(root: &Path, file: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = file
        .strip_prefix(root)
        .ok()?
        .iter()
        .map(|part| part.to_str())
        .collect();
    Some(parts?.join("/")).filter(|path| !path.contains(char::is_control))
}

/// The files in the folder at `prefix` in `root` and below, their paths from `root` one
/// per line, sorted: `most` at most, after looking at no more than `looks` files and
/// folders, which is counted down. Or the number that tells a plugin why not.
fn files_in(root: &Path, prefix: &str, most: usize, looks: &mut usize) -> Result<String, i64> {
    let folder = resolved(root, prefix)?;
    if !folder.is_dir() {
        return Err(NOT_FOUND);
    }
    let (mut paths, mut size) = (Vec::new(), 0);
    let mut folders = vec![folder];
    'walk: while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            if *looks == 0 || paths.len() >= most {
                break 'walk;
            }
            *looks -= 1;
            // A link is neither a file nor a folder here: it is not followed.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            if kind.is_dir() {
                if !SKIPPED
                    .iter()
                    .any(|skipped| name.eq_ignore_ascii_case(skipped))
                {
                    folders.push(entry.path());
                }
                continue;
            }
            let Some(path) = path_from(root, &entry.path()).filter(|_| kind.is_file()) else {
                continue;
            };
            size += path.len() + 1;
            if size > MAX_FILE {
                break 'walk;
            }
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths.join("\n"))
}

/// A plugin started afresh: a memory of its own, a budget, nothing left of earlier runs.
/// What goes wrong is told so as to be said after the name of the plugin.
struct Run {
    store: Store<Host>,
    instance: Instance,
    memory: Memory,
}

impl Run {
    /// Start `module`, which may spend `fuel` from now on and read what is in `root`.
    fn start(module: &Module, fuel: u64, root: Option<PathBuf>) -> Result<Self, String> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(MAX_MEMORY)
            .memories(1)
            .tables(8)
            .table_elements(1 << 20)
            .instances(1)
            // Growing too far stops the plugin, rather than being an answer it may ignore.
            .trap_on_grow_failure(true)
            .build();
        let host = Host {
            limits,
            root,
            calls: 0,
            read: 0,
            log: Vec::new(),
            busy: false,
        };
        let mut store = Store::new(module.engine(), host);
        store.limiter(|host| &mut host.limits);
        store.set_fuel(fuel).map_err(|err| stopped(&err))?;
        let mut linker = Linker::new(module.engine());
        linker
            .func_wrap("godcode", "read_file", read_file)
            .and_then(|linker| linker.func_wrap("godcode", "list_files", list_files))
            .and_then(|linker| linker.func_wrap("godcode", "log", log))
            .map_err(|err| format!("cannot be lent what it asks for: {err}"))?;
        let instance = linker
            .instantiate_and_start(&mut store, module)
            .map_err(|err| stopped(&err))?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or("does not export its `memory`")?;
        Ok(Self {
            store,
            instance,
            memory,
        })
    }

    /// Call the function it exports as `name`.
    fn call<Takes: WasmParams, Gives: WasmResults>(
        &mut self,
        name: &str,
        given: Takes,
    ) -> Result<Gives, String> {
        self.instance
            .get_typed_func::<Takes, Gives>(&self.store, name)
            .map_err(|err| format!("does not export a usable `{name}`: {err}"))?
            .call(&mut self.store, given)
            .map_err(|err| stopped(&err))
    }

    /// Write `text` in its memory at `ptr`, the place it gave for it.
    fn write(&mut self, ptr: i32, text: &str) -> Result<(), String> {
        let start = ptr.cast_unsigned() as usize;
        start
            .checked_add(text.len())
            .and_then(|end| self.memory.data_mut(&mut self.store).get_mut(start..end))
            .ok_or("gave a place outside its memory for the text")?
            .copy_from_slice(text.as_bytes());
        Ok(())
    }

    /// The text it points at with `answer`, `(ptr << 32) | len` under the bit of failure,
    /// when it is `most` bytes at most.
    fn text(&self, answer: i64, most: usize) -> Result<String, String> {
        let answer = answer.cast_unsigned() & !FAILED;
        let (start, len) = ((answer >> 32) as usize, (answer & 0xFFFF_FFFF) as usize);
        if len > most {
            return Err(format!("gave back {len} bytes of text, too many"));
        }
        // Looked up in the memory as it is: nothing is set aside for a length it made up.
        let bytes = start
            .checked_add(len)
            .and_then(|end| self.memory.data(&self.store).get(start..end))
            .ok_or("gave back a text outside its memory")?;
        match std::str::from_utf8(bytes) {
            Ok(text) => Ok(text.to_owned()),
            Err(_) => Err("gave back a text that is not UTF-8".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The module written in `parts`, as WebAssembly text.
    fn wasm(parts: &str) -> Vec<u8> {
        wat::parse_str(format!("(module {parts})")).expect("valid WebAssembly text")
    }

    const MEMORY: &str = r#"(memory (export "memory") 1) (data (i32.const 0) "Do")"#;
    /// Every text is put at 1024.
    const ALLOC: &str =
        r#"(func (export "godcode_alloc") (param i32) (result i32) (i32.const 1024))"#;
    /// One command, "Do".
    const COMMANDS: &str = r#"(func (export "godcode_commands") (result i64) (i64.const 2))"#;
    /// The text as it came.
    const ECHO: &str = "(i64.or (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
        (i64.extend_i32_u (local.get $len)))";

    fn run_of(body: &str) -> String {
        format!(
            r#"(func (export "godcode_run")
                (param $command i32) (param $ptr i32) (param $len i32) (result i64) {body})"#
        )
    }

    /// A plugin with one command, "Do", which does `body`, and `more` beside it.
    fn doing_with(body: &str, more: &str) -> Vec<u8> {
        wasm(&[MEMORY, ALLOC, COMMANDS, &run_of(body), more].join("\n"))
    }

    fn doing(body: &str) -> Plugin {
        Plugin::load("test.wasm", &doing_with(body, "")).expect("a plugin")
    }

    /// What a plugin returns for the `len` bytes at `ptr`.
    fn at(ptr: u64, len: u64) -> String {
        format!("(i64.const {})", (ptr << 32 | len).cast_signed())
    }

    fn refused(file: &str, bytes: &[u8]) -> String {
        match Plugin::load(file, bytes) {
            Ok(_) => panic!("{file} should be refused"),
            Err(why) => why,
        }
    }

    /// The example of the documentation of this module, as written there.
    fn documented_example() -> Vec<u8> {
        let text: Vec<&str> = include_str!("plugins.rs")
            .lines()
            .skip_while(|line| *line != "//! ```wat")
            .skip(1)
            .take_while(|line| *line != "//! ```")
            .map(|line| line.strip_prefix("//!").expect("a line of the doc"))
            .collect();
        wat::parse_str(text.join("\n")).expect("the example is valid")
    }

    #[test]
    fn the_documented_example_puts_ascii_in_capitals_and_leaves_the_rest() {
        let plugin = Plugin::load("upper.wasm", &documented_example()).expect("it loads");
        assert_eq!(plugin.name(), "upper");
        assert_eq!(plugin.commands(), ["Upper case"]);
        assert_eq!(
            plugin.run(0, "héllo, wörld ✓ {z}"),
            Ok("HéLLO, WöRLD ✓ {Z}".to_owned())
        );
        assert_eq!(plugin.run(0, ""), Ok(String::new()));
        // More than its first page holds: it grows its memory.
        let long = "ab ç\n".repeat(50_000);
        assert_eq!(plugin.run(0, &long), Ok("AB ç\n".repeat(50_000)));
    }

    #[test]
    fn a_plugin_names_itself_and_offers_two_commands() {
        let bytes = wasm(&format!(
            r#"(memory (export "memory") 1)
            (data (i32.const 0) "First\n\n  Second  \n")
            (data (i32.const 32) " Two things \nignored")
            (data (i32.const 64) "onetwo")
            {ALLOC}
            (func (export "godcode_commands") (result i64) {})
            (func (export "godcode_name") (result i64) {})
            {}"#,
            at(0, 18),
            at(32, 20),
            run_of(&format!(
                "(if (result i64) (local.get $command) (then {}) (else {}))",
                at(67, 3),
                at(64, 3)
            )),
        ));
        let plugin = Plugin::load("two.wasm", &bytes).expect("it loads");
        assert_eq!(plugin.name(), "Two things");
        assert_eq!(plugin.commands(), ["First", "Second"]);
        assert_eq!(plugin.run(0, "x"), Ok("one".to_owned()));
        assert_eq!(plugin.run(1, "x"), Ok("two".to_owned()));
        assert_eq!(
            plugin.run(2, "x"),
            Err("The plugin Two things has no command 2".to_owned())
        );
    }

    #[test]
    fn a_plugin_says_why_it_failed() {
        let failing = |len: u64| {
            let failed = (FAILED | 64 << 32 | len).cast_signed();
            let data = r#"(data (i32.const 64) "no number here")"#;
            let bytes = doing_with(&format!("(i64.const {failed})"), data);
            Plugin::load("sum.wasm", &bytes).expect("it loads")
        };
        assert_eq!(
            failing(14).run(0, "abc"),
            Err("The plugin sum failed: no number here".to_owned())
        );
        assert_eq!(
            failing(0).run(0, "abc"),
            Err("The plugin sum failed without saying why".to_owned())
        );
    }

    #[test]
    fn a_plugin_that_asks_for_anything_from_outside_is_refused() {
        let clock = r#"(import "wasi_snapshot_preview1" "clock_time_get"
            (func (param i32 i64 i32) (result i32)))"#;
        let bytes = wasm(&[clock, MEMORY, ALLOC, COMMANDS, &run_of(ECHO)].join("\n"));
        assert_eq!(
            refused("spy.wasm", &bytes),
            "The plugin spy asks for `wasi_snapshot_preview1.clock_time_get` from outside, \
             and plugins are lent nothing"
        );
        // A memory or a global of the editor is no more lent than a function.
        let memory = wasm(&[r#"(import "env" "memory" (memory 1))"#, ALLOC, COMMANDS].join("\n"));
        assert!(refused("spy.wasm", &memory).contains("asks for `env.memory`"));
    }

    #[test]
    fn a_plugin_lacking_an_export_is_refused_by_the_name_of_what_lacks() {
        let run = run_of(ECHO);
        let parts = [MEMORY, ALLOC, COMMANDS, &run];
        let names = ["memory", "godcode_alloc", "godcode_commands", "godcode_run"];
        for (lacking, name) in names.iter().enumerate() {
            let mut kept = parts.to_vec();
            kept.remove(lacking);
            let why = refused("half.wasm", &wasm(&kept.join("\n")));
            assert!(why.starts_with("The plugin half does not export"), "{why}");
            assert!(why.contains(&format!("`{name}`")), "{why}");
        }
        assert!(refused("empty.wasm", &wasm("")).contains("`memory`"));
    }

    #[test]
    fn a_function_of_the_wrong_shape_is_refused() {
        let run = run_of(ECHO);
        let narrow = r#"(func (export "godcode_alloc") (result i32) (i32.const 1024))"#;
        assert_eq!(
            refused(
                "odd.wasm",
                &wasm(&[MEMORY, narrow, COMMANDS, &run].join("\n"))
            ),
            "The plugin odd exports `godcode_alloc` as [] -> [i32], where [i32] -> [i32] is expected"
        );
        let short = r#"(func (export "godcode_commands") (result i32) (i32.const 2))"#;
        let why = refused("odd.wasm", &wasm(&[MEMORY, ALLOC, short, &run].join("\n")));
        assert!(why.contains("`godcode_commands` as [] -> [i32]"), "{why}");
        let blind = r#"(func (export "godcode_run") (param i32) (result i64) (i64.const 0))"#;
        let why = refused(
            "odd.wasm",
            &wasm(&[MEMORY, ALLOC, COMMANDS, blind].join("\n")),
        );
        assert!(
            why.contains("[i32, i32, i32] -> [i64] is expected"),
            "{why}"
        );
        let named = r#"(func (export "godcode_name") (param i32) (result i64) (i64.const 0))"#;
        let why = refused("odd.wasm", &doing_with(ECHO, named));
        assert!(why.contains("`godcode_name` as [i32] -> [i64]"), "{why}");
        // A name taken by something that is no function.
        let global = r#"(global (export "godcode_run") i64 (i64.const 0))"#;
        let why = refused(
            "odd.wasm",
            &wasm(&[MEMORY, ALLOC, COMMANDS, global].join("\n")),
        );
        assert!(why.contains("`godcode_run` as not a function"), "{why}");
        let table = r#"(table (export "memory") 1 funcref)"#;
        let why = refused(
            "odd.wasm",
            &wasm(&[table, ALLOC, COMMANDS, &run].join("\n")),
        );
        assert!(why.contains("does not export its `memory`"), "{why}");
    }

    #[test]
    fn a_plugin_that_never_ends_is_stopped_and_can_run_again() {
        // The first byte of the text says whether to hang.
        let mut plugin = doing(&format!(
            "(if (i32.eq (i32.load8_u (local.get $ptr)) (i32.const 33))
                (then (loop $ever (br $ever))))
            {ECHO}"
        ));
        plugin.fuel = 20_000_000;
        let started = std::time::Instant::now();
        assert_eq!(
            plugin.run(0, "! hang"),
            Err("The plugin test ran too long".to_owned())
        );
        assert!(started.elapsed().as_secs() < 2);
        assert_eq!(plugin.run(0, "fine"), Ok("fine".to_owned()));
    }

    #[test]
    fn a_plugin_that_hangs_while_it_starts_or_names_its_commands_is_refused() {
        let commands = r#"(func (export "godcode_commands") (result i64) (loop $ever (br $ever)) (i64.const 2))"#;
        let bytes = wasm(&[MEMORY, ALLOC, commands, &run_of(ECHO)].join("\n"));
        assert_eq!(refused("slow.wasm", &bytes), "The plugin slow ran too long");
        let start = "(func $start (loop $ever (br $ever))) (start $start)";
        assert_eq!(
            refused("slow.wasm", &doing_with(ECHO, start)),
            "The plugin slow ran too long"
        );
    }

    #[test]
    fn a_plugin_that_takes_memory_without_end_is_stopped_and_can_run_again() {
        let plugin = doing(&format!(
            "(if (i32.eq (i32.load8_u (local.get $ptr)) (i32.const 33))
                (then (loop $ever (drop (memory.grow (i32.const 16))) (br $ever))))
            {ECHO}"
        ));
        assert_eq!(
            plugin.run(0, "! grow"),
            Err("The plugin test asked for more than 64 MB of memory".to_owned())
        );
        assert_eq!(plugin.run(0, "fine"), Ok("fine".to_owned()));
        // Born with more than it may hold.
        let born = r#"(memory (export "memory") 2000)"#;
        let why = refused(
            "fat.wasm",
            &wasm(&[born, ALLOC, COMMANDS, &run_of(ECHO)].join("\n")),
        );
        assert!(why.starts_with("The plugin fat "), "{why}");
    }

    #[test]
    fn a_text_given_back_from_outside_the_memory_is_an_error() {
        let outside = "The plugin test gave back a text outside its memory";
        // One page is 65536 bytes: past its end, across its end, and very far from it.
        for answer in [at(70_000, 4), at(65_530, 10), at(0x7FFF_FFFF, 64 << 20)] {
            assert_eq!(doing(&answer).run(0, "abc"), Err(outside.to_owned()));
        }
        assert_eq!(doing(&at(65_530, 6)).run(0, "abc"), Ok("\0".repeat(6)));
        // A length no memory can hold is not even looked up.
        assert_eq!(
            doing(&at(0, 0xFFFF_FFFF)).run(0, "abc"),
            Err("The plugin test gave back 4294967295 bytes of text, too many".to_owned())
        );
        // The same for its commands, when it is loaded.
        let commands = format!(
            r#"(func (export "godcode_commands") (result i64) {})"#,
            at(65_536, 1)
        );
        let bytes = wasm(&[MEMORY, ALLOC, &commands, &run_of(ECHO)].join("\n"));
        assert_eq!(
            refused("far.wasm", &bytes),
            "The plugin far gave back a text outside its memory"
        );
    }

    #[test]
    fn a_place_for_the_text_outside_the_memory_is_an_error() {
        let outside = "The plugin wild gave a place outside its memory for the text";
        for place in ["-1", "65534", "2000000000"] {
            let alloc = format!(
                r#"(func (export "godcode_alloc") (param i32) (result i32) (i32.const {place}))"#
            );
            let bytes = wasm(&[MEMORY, &alloc, COMMANDS, &run_of(ECHO)].join("\n"));
            let plugin = Plugin::load("wild.wasm", &bytes).expect("it loads");
            assert_eq!(plugin.run(0, "abc"), Err(outside.to_owned()));
        }
    }

    #[test]
    fn a_text_given_back_that_is_not_utf8_is_an_error() {
        let plugin = Plugin::load(
            "raw.wasm",
            &doing_with(&at(64, 2), r#"(data (i32.const 64) "\ff\fe")"#),
        )
        .expect("it loads");
        assert_eq!(
            plugin.run(0, "abc"),
            Err("The plugin raw gave back a text that is not UTF-8".to_owned())
        );
        // Half of a character of two bytes.
        let half = doing(
            "(i64.or (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32)) (i64.const 1))",
        );
        assert!(half.run(0, "é").is_err());
    }

    #[test]
    fn a_plugin_that_breaks_is_an_error_and_can_run_again() {
        let plugin = doing(&format!(
            "(if (i32.eq (i32.load8_u (local.get $ptr)) (i32.const 33)) (then unreachable))
            {ECHO}"
        ));
        let why = plugin.run(0, "! break").expect_err("it broke");
        assert!(why.starts_with("The plugin test stopped: "), "{why}");
        assert_eq!(plugin.run(0, "fine"), Ok("fine".to_owned()));

        // Calling itself without end, reading outside its memory, dividing by zero.
        let deep = doing_with("(call $deep)", "(func $deep (result i64) (call $deep))");
        let deep = Plugin::load("test.wasm", &deep).expect("it loads");
        let far = doing("(i64.load (i32.const 70000))");
        let zero = doing("(i64.div_u (i64.const 1) (i64.const 0))");
        for plugin in [deep, far, zero] {
            let why = plugin.run(0, "abc").expect_err("it broke");
            assert!(why.starts_with("The plugin test stopped: "), "{why}");
        }

        let start = "(func $start unreachable) (start $start)";
        let why = refused("born.wasm", &doing_with(ECHO, start));
        assert!(why.starts_with("The plugin born stopped: "), "{why}");
    }

    #[test]
    fn bytes_that_are_no_module_are_refused() {
        let no_module = "The plugin junk is not a WebAssembly module that runs here: ";
        let whole = documented_example();
        let cut = &whole[..whole.len() / 2];
        for bytes in [&b"not wasm at all"[..], &[], &whole[..7], cut] {
            let why = refused("junk.wasm", bytes);
            assert!(why.starts_with(no_module), "{why}");
        }
        // Well formed, but its code does not hold together.
        let mut wrong = wasm(&[MEMORY, ALLOC, COMMANDS].join("\n"));
        let last = wrong.len() - 2;
        wrong[last] = 0x7E;
        assert!(Plugin::load("junk.wasm", &wrong).is_err());
    }

    #[test]
    fn what_is_too_large_is_refused_before_anything_is_done_with_it() {
        assert_eq!(
            refused("huge.wasm", &vec![0; MAX_MODULE + 1]),
            "The plugin huge is larger than 32 MB"
        );
        let plugin = doing(ECHO);
        assert_eq!(
            plugin.run(0, &" ".repeat(MAX_TEXT + 1)),
            Err("The plugin test is given more than 16 MB of text".to_owned())
        );
        // A list of commands longer than any list of commands.
        let commands = format!(
            r#"(func (export "godcode_commands") (result i64) {})"#,
            at(0, 65_537)
        );
        let bytes = wasm(
            &[
                r#"(memory (export "memory") 2)"#,
                ALLOC,
                &commands,
                &run_of(ECHO),
            ]
            .join("\n"),
        );
        assert_eq!(
            refused("long.wasm", &bytes),
            "The plugin long gave back 65537 bytes of text, too many"
        );
        let none = r#"(func (export "godcode_commands") (result i64) (i64.const 0))"#;
        let bytes = wasm(&[MEMORY, ALLOC, none, &run_of(ECHO)].join("\n"));
        assert_eq!(
            refused("mute.wasm", &bytes),
            "The plugin mute has no command"
        );
    }

    #[test]
    fn nothing_is_left_of_a_run_in_the_next_one() {
        // It counts its runs in a global and in its memory, and answers with the count.
        let plugin = Plugin::load(
            "count.wasm",
            &doing_with(
                "(global.set $runs (i32.add (global.get $runs) (i32.const 1)))
                (i32.store8 (i32.const 64)
                    (i32.add (i32.add (i32.load8_u (i32.const 64)) (global.get $runs)) (i32.const 48)))
                (i64.const 0x0000004000000001)",
                "(global $runs (mut i32) (i32.const 0))",
            ),
        )
        .expect("it loads");
        for _ in 0..3 {
            assert_eq!(plugin.run(0, "abc"), Ok("1".to_owned()));
        }
    }

    #[test]
    fn the_plugins_of_a_folder_are_loaded_in_order_each_one_or_why_not() {
        let folder = std::env::temp_dir().join(format!("godcode-plugins-{}", std::process::id()));
        std::fs::create_dir_all(folder.join("folder.wasm")).expect("folder");
        std::fs::write(folder.join("upper.wasm"), documented_example()).expect("file");
        std::fs::write(folder.join("broken.wasm"), b"\0asm, or so it says").expect("file");
        std::fs::write(folder.join("Echo.WASM"), doing_with(ECHO, "")).expect("file");
        std::fs::write(folder.join("notes.txt"), "not a plugin").expect("file");
        let loaded = load_folder(&folder);
        let _ = std::fs::remove_dir_all(&folder);

        let told: Vec<String> = loaded
            .iter()
            .map(|plugin| match plugin {
                Ok(plugin) => format!("{}: {}", plugin.name(), plugin.commands().join(", ")),
                Err(why) => why.split(':').next().unwrap_or_default().to_owned(),
            })
            .collect();
        assert_eq!(
            told,
            [
                "Echo: Do",
                "The plugin broken is not a WebAssembly module that runs here",
                "upper: Upper case",
            ]
        );
        assert!(load_folder(&folder).is_empty());
    }
    // What follows is for what came after the first version of the interface.

    /// A folder of its own for a test, removed with all that is in it when the test ends.
    struct Folder(PathBuf);

    impl Folder {
        /// A new folder holding these files, each a path with `/` and its text.
        fn with(files: &[(&str, &str)]) -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let name = format!("godcode-project-{}-{number}", std::process::id());
            let folder = Folder(std::env::temp_dir().join(name));
            let _ = std::fs::remove_dir_all(&folder.0);
            std::fs::create_dir_all(&folder.0).expect("folder");
            for (path, text) in files {
                folder.write(path, text.as_bytes());
            }
            folder
        }

        fn write(&self, path: &str, bytes: &[u8]) {
            let file = self.0.join(path);
            std::fs::create_dir_all(file.parent().expect("in a folder")).expect("folder");
            std::fs::write(file, bytes).expect("file");
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Make `link` lead to the folder `to`: a junction on Windows, which asks for no
    /// privilege, a symbolic link elsewhere. Whether it could be made.
    fn link(link: &Path, to: &Path) -> bool {
        #[cfg(windows)]
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .args([link, to])
            .output()
            .is_ok_and(|done| done.status.success());
        #[cfg(not(windows))]
        let made = std::os::unix::fs::symlink(to, link).is_ok();
        made
    }

    /// The end of a command that asked: `$got` when it is a text, else `!` and the digit
    /// of the refusal (`!3` for -3).
    const ANSWER: &str = "(if (result i64) (i64.lt_s (local.get $got) (i64.const 0))
        (then
            (i32.store8 (i32.const 65) (i32.sub (i32.const 48) (i32.wrap_i64 (local.get $got))))
            (i64.const 0x0000004000000002))
        (else (local.get $got)))";

    /// The module of a plugin lent `import` as `$ask`, with a memory of `pages` pages and
    /// one command, "Do", which does `body`.
    fn lent_with(import: &str, pages: u32, body: &str) -> Vec<u8> {
        let import =
            format!(r#"(import "godcode" "{import}" (func $ask (param i32 i32) (result i64)))"#);
        let memory = format!(
            r#"(memory (export "memory") {pages}) (data (i32.const 0) "Do") (data (i32.const 64) "!")"#
        );
        wasm(&[&import, &memory, ALLOC, COMMANDS, &run_of(body)].join("\n"))
    }

    fn lent(import: &str, body: &str) -> Plugin {
        Plugin::load("test.wasm", &lent_with(import, 1, body)).expect("a plugin")
    }

    /// A plugin that asks `import` about the text it is given, `times` times, and gives
    /// back the last answer. The path is kept at 128: the answer is written over the text.
    fn asking_often(import: &str, times: u32, pages: u32) -> Plugin {
        let body = format!(
            "(local $got i64) (local $times i32)
            (memory.copy (i32.const 128) (local.get $ptr) (local.get $len))
            (loop $again
                (local.set $got (call $ask (i32.const 128) (local.get $len)))
                (local.set $times (i32.add (local.get $times) (i32.const 1)))
                (br_if $again (i32.lt_u (local.get $times) (i32.const {times}))))
            {ANSWER}"
        );
        Plugin::load("test.wasm", &lent_with(import, pages, &body)).expect("a plugin")
    }

    fn asking(import: &str) -> Plugin {
        asking_often(import, 1, 1)
    }

    /// What the first command of `plugin` gives back for `text` when it may read `root`.
    fn given(plugin: &Plugin, root: Option<&Path>, text: &str) -> String {
        let context = Context {
            root,
            ..Context::default()
        };
        plugin.run_in(0, text, &context).expect("it runs").text
    }

    /// The module of a plugin whose commands, kinds and shortcuts are these lists, as
    /// written.
    fn listing(commands: &str, kinds: Option<&str>, keys: Option<&str>) -> Vec<u8> {
        let written = |text: &str| text.replace('\n', "\\n").replace('\r', "\\r");
        let mut parts = vec![
            format!(
                r#"(memory (export "memory") 1) (data (i32.const 0) "{}")"#,
                written(commands)
            ),
            ALLOC.to_owned(),
            format!(
                r#"(func (export "godcode_commands") (result i64) {})"#,
                at(0, commands.len() as u64)
            ),
            run_of(ECHO),
        ];
        for (name, list, place) in [("godcode_kinds", kinds, 2048), ("godcode_keys", keys, 4096)] {
            if let Some(list) = list {
                parts.push(format!(
                    r#"(data (i32.const {place}) "{}") (func (export "{name}") (result i64) {})"#,
                    written(list),
                    at(place, list.len() as u64)
                ));
            }
        }
        wasm(&parts.join("\n"))
    }

    /// The second example of the documentation of this module, as written there.
    fn second_documented_example() -> Vec<u8> {
        let text: Vec<&str> = include_str!("plugins.rs")
            .lines()
            .skip_while(|line| *line != "//! ```wat")
            .skip(1)
            .skip_while(|line| *line != "//! ```wat")
            .skip(1)
            .take_while(|line| *line != "//! ```")
            .map(|line| line.strip_prefix("//!").expect("a line of the doc"))
            .collect();
        wat::parse_str(text.join("\n")).expect("the example is valid")
    }

    #[test]
    fn the_second_documented_example_shows_how_many_lines_a_file_of_the_project_has() {
        let plugin = Plugin::load("lines.wasm", &second_documented_example()).expect("it loads");
        assert_eq!(plugin.name(), "lines");
        assert_eq!(plugin.commands(), ["Count the lines of the selected path"]);
        assert_eq!(plugin.kind(0), Kind::Show);
        assert_eq!(
            plugin.keys(0).map(Keys::to_string),
            Some("Ctrl+Alt+L".to_owned())
        );
        assert!(plugin.reads_project());

        let project = Folder::with(&[
            ("src/main.rs", "fn main() {\n    run();\n}\n"),
            ("open.txt", "one\ntwo"),
            ("empty.txt", ""),
        ]);
        // More than its first page holds: it grows its memory.
        project.write("long.txt", "line\n".repeat(100_000).as_bytes());
        let context = Context {
            root: Some(&project.0),
            ..Context::default()
        };
        let shown = |lines: &str| {
            Ok(Outcome {
                kind: Kind::Show,
                text: lines.to_owned(),
                log: Vec::new(),
            })
        };
        assert_eq!(plugin.run_in(0, "src/main.rs", &context), shown("3 lines"));
        assert_eq!(plugin.run_in(0, "open.txt", &context), shown("2 lines"));
        assert_eq!(plugin.run_in(0, "empty.txt", &context), shown("0 lines"));
        assert_eq!(
            plugin.run_in(0, "long.txt", &context),
            shown("100000 lines")
        );

        let cannot = Err("The plugin lines failed: cannot read that file of the project".into());
        assert_eq!(plugin.run_in(0, "src/none.rs", &context), cannot);
        assert_eq!(plugin.run_in(0, "../open.txt", &context), cannot);
        // With no project, as the first version of the interface runs it.
        assert_eq!(
            plugin.run(0, "open.txt"),
            cannot.map(|outcome: Outcome| outcome.text)
        );
    }

    #[test]
    fn a_plugin_of_the_first_version_runs_as_it_always_did() {
        let plugin = Plugin::load("upper.wasm", &documented_example()).expect("it loads");
        assert_eq!(plugin.kind(0), Kind::Replace);
        assert_eq!(plugin.keys(0), None);
        assert!(!plugin.reads_project());
        let project = Folder::with(&[("abc", "the file")]);
        let context = Context {
            file: Some("abc"),
            language: "text",
            line: 3,
            column: 4,
            selection: true,
            root: Some(&project.0),
        };
        // It is given the text alone, whatever is known of where it runs.
        assert_eq!(
            plugin.run_in(0, "abc", &context),
            Ok(Outcome {
                kind: Kind::Replace,
                text: "ABC".to_owned(),
                log: Vec::new(),
            })
        );
        assert_eq!(plugin.run(0, "abc"), Ok("ABC".to_owned()));
    }

    #[test]
    fn each_command_says_what_is_done_with_its_text() {
        let kinds = "replace\n insert \n\nshow\nopen\n";
        let bytes = listing("Swap\nStamp\nCount\nReport", Some(kinds), None);
        let plugin = Plugin::load("kinds.wasm", &bytes).expect("it loads");
        let kinds: Vec<Kind> = (0..4).map(|index| plugin.kind(index)).collect();
        assert_eq!(kinds, [Kind::Replace, Kind::Insert, Kind::Show, Kind::Open]);
        for (index, kind) in kinds.into_iter().enumerate() {
            let outcome = plugin.run_in(index, "text", &Context::default());
            assert_eq!(outcome.map(|outcome| outcome.kind), Ok(kind));
        }
        // A command it does not have.
        assert_eq!(plugin.kind(4), Kind::Replace);
    }

    #[test]
    fn kinds_that_are_not_one_for_each_command_or_not_known_are_refused() {
        let why = |kinds: &str| refused("odd.wasm", &listing("One\nTwo", Some(kinds), None));
        assert_eq!(
            why("show"),
            "The plugin odd says what 1 commands do with their text, and has 2"
        );
        assert_eq!(
            why("show\nopen\ninsert"),
            "The plugin odd says what 3 commands do with their text, and has 2"
        );
        assert!(why("").contains("says what 0 commands do"));
        assert_eq!(
            why("show\nprint"),
            "The plugin odd says a command does `print` with its text, which is none of \
             replace, insert, show, open"
        );
        // Written in small letters, and one word a line.
        assert!(why("Show\nopen").contains("does `Show`"));
        assert!(why("show open\ninsert").contains("does `show open`"));
        // A word without end is not said whole.
        let long = why(&format!("show\n{}", "x".repeat(5_000)));
        assert!(long.len() < 200, "{long}");
        let shape = r#"(func (export "godcode_kinds") (result i32) (i32.const 0))"#;
        let why = refused("odd.wasm", &doing_with(ECHO, shape));
        assert!(why.contains("`godcode_kinds` as [] -> [i32]"), "{why}");
    }

    #[test]
    fn a_shortcut_is_read_as_the_editor_writes_it() {
        assert_eq!(
            Keys::parse("Ctrl+Alt+U"),
            Some(Keys {
                ctrl: true,
                shift: false,
                alt: true,
                key: "U".to_owned(),
            })
        );
        let read = |text: &str| Keys::parse(text).map(|keys| keys.to_string());
        for (written, read_as) in [
            ("Ctrl+Alt+U", "Ctrl+Alt+U"),
            ("Ctrl+Shift+F9", "Ctrl+Shift+F9"),
            ("ctrl+u", "Ctrl+U"),
            ("ALT+7", "Alt+7"),
            ("F1", "F1"),
            ("f12", "F12"),
            ("Shift+F5", "Shift+F5"),
            (" Shift + Alt + Ctrl + k ", "Ctrl+Alt+Shift+K"),
            // A letter, not a key of function.
            ("Ctrl+F", "Ctrl+F"),
        ] {
            assert_eq!(read(written).as_deref(), Some(read_as), "{written}");
        }
        for unreadable in [
            "",
            "U",
            "7",
            "Shift+U",
            "Ctrl",
            "Ctrl+",
            "+U",
            "Ctrl++",
            "Ctrl+Alt",
            "Ctrl+Ctrl+U",
            "Ctrl+Meta+U",
            "Cmd+U",
            "Ctrl-U",
            "Ctrl+UU",
            "Ctrl+Enter",
            "Ctrl+é",
            "Ctrl+,",
            "F0",
            "F01",
            "F13",
            "Ctrl+F13",
        ] {
            assert_eq!(read(unreadable), None, "{unreadable}");
        }
    }

    #[test]
    fn a_plugin_asks_for_shortcuts_line_by_line() {
        let keys_of = |keys: &str| {
            let bytes = listing("One\nTwo\nThree", None, Some(keys));
            let plugin = Plugin::load("keys.wasm", &bytes).expect("it loads");
            let keys = |index| plugin.keys(index).map_or(String::new(), Keys::to_string);
            [keys(0), keys(1), keys(2), keys(3)].join(" | ")
        };
        assert_eq!(keys_of("Ctrl+Alt+U\n\nF9\n"), "Ctrl+Alt+U |  | F9 | ");
        // Lines missing at the end, and empty ones past it.
        assert_eq!(keys_of("\nctrl+shift+f2"), " | Ctrl+Shift+F2 |  | ");
        assert_eq!(keys_of("F2\r\n\n\n\n\n"), "F2 |  |  | ");
        assert_eq!(keys_of(""), " |  |  | ");

        let why = |keys: &str| refused("keys.wasm", &listing("One\nTwo", None, Some(keys)));
        assert_eq!(
            why("F2\nF3\nF4"),
            "The plugin keys gives shortcuts on 3 lines, and has 2 commands"
        );
        assert_eq!(
            why("Ctrl+Alt+U\nShift+A"),
            "The plugin keys asks for the shortcut `Shift+A`, which cannot be read as one \
             like Ctrl+Alt+U or F9"
        );
        assert!(why("Ctrl+Alt+U Ctrl+Alt+I").contains("cannot be read"));
        let long = why(&"Ctrl+".repeat(5_000));
        assert!(long.len() < 200, "{long}");
    }

    /// A plugin told where it runs, whose one command gives back all that it was given.
    fn told() -> Plugin {
        let with = run_of(ECHO).replace("godcode_run", "godcode_run_with");
        Plugin::load(
            "told.wasm",
            &wasm(&[MEMORY, ALLOC, COMMANDS, &with].join("\n")),
        )
        .expect("it loads")
    }

    #[test]
    fn a_command_is_told_where_it_runs_then_given_its_text() {
        let context = Context {
            file: Some("src/naïve café/日本.rs"),
            language: "rust",
            line: 12,
            column: 5,
            selection: true,
            root: None,
        };
        // A text that starts with empty lines keeps them.
        let request = told()
            .run_in(0, "\n\n  text\r\n", &context)
            .expect("it runs")
            .text;
        assert_eq!(
            request,
            "file: src/naïve café/日本.rs\nlanguage: rust\nline: 12\ncolumn: 5\nselection: yes\n\
             \n\n\n  text\r\n"
        );
        // Read as a plugin reads it: the head to the first empty line, a name and a value
        // on each of its lines, the text after.
        let (head, text) = request.split_once("\n\n").expect("a head");
        let head: Vec<_> = head
            .lines()
            .filter_map(|line| line.split_once(": "))
            .collect();
        assert_eq!(
            head,
            [
                ("file", "src/naïve café/日本.rs"),
                ("language", "rust"),
                ("line", "12"),
                ("column", "5"),
                ("selection", "yes"),
            ]
        );
        assert_eq!(text, "\n\n  text\r\n");
    }

    #[test]
    fn what_is_not_known_of_where_a_command_runs_is_left_empty() {
        assert_eq!(
            told().run(0, ""),
            Ok("file: \nlanguage: \nline: 0\ncolumn: 0\nselection: no\n\n".to_owned())
        );
    }

    #[test]
    fn no_line_break_gets_into_what_a_command_is_told() {
        let context = Context {
            file: Some("a\nselection: yes\r\n\n\u{2028}b\u{85}.rs"),
            language: "ru\nst\0",
            ..Context::default()
        };
        assert_eq!(
            told().run_in(0, "text", &context).expect("it runs").text,
            "file: aselection: yesb.rs\nlanguage: rust\nline: 0\ncolumn: 0\nselection: no\n\ntext"
        );
        let long = "a".repeat(MAX_NAMES + 1);
        let context = Context {
            file: Some(&long),
            ..Context::default()
        };
        assert_eq!(
            told().run_in(0, "text", &context),
            Err("The plugin told is told of a file whose name is longer than 64 KB".to_owned())
        );
    }

    #[test]
    fn a_plugin_that_can_be_told_where_it_runs_is_run_that_way() {
        let old = run_of(&at(64, 3));
        let new = run_of(&at(67, 3)).replace("godcode_run", "godcode_run_with");
        let data = r#"(data (i32.const 64) "oldnew")"#;
        let both = wasm(&[MEMORY, ALLOC, COMMANDS, data, &old, &new].join("\n"));
        let both = Plugin::load("both.wasm", &both).expect("it loads");
        assert_eq!(both.run(0, "text"), Ok("new".to_owned()));
        let outcome = both.run_in(0, "text", &Context::default());
        assert_eq!(outcome.map(|outcome| outcome.text), Ok("new".to_owned()));
        // `godcode_run` may then be left out, but not be anything.
        let alone = wasm(&[MEMORY, ALLOC, COMMANDS, data, &new].join("\n"));
        let alone = Plugin::load("alone.wasm", &alone).expect("it loads");
        assert_eq!(alone.run(0, "text"), Ok("new".to_owned()));
        let shape = r#"(func (export "godcode_run_with") (param i32) (result i64) (i64.const 0))"#;
        let why = refused("odd.wasm", &doing_with(ECHO, shape));
        assert!(
            why.contains("`godcode_run_with` as [i32] -> [i64]"),
            "{why}"
        );
        // It fails as `godcode_run` does.
        let failing = run_of(&format!(
            "(i64.const {})",
            (FAILED | 64 << 32 | 3).cast_signed()
        ))
        .replace("godcode_run", "godcode_run_with");
        let failing = wasm(&[MEMORY, ALLOC, COMMANDS, data, &failing].join("\n"));
        assert_eq!(
            Plugin::load("no.wasm", &failing)
                .expect("it loads")
                .run(0, "text"),
            Err("The plugin no failed: old".to_owned())
        );
    }

    #[test]
    fn a_plugin_reads_the_files_of_the_project() {
        let project = Folder::with(&[
            ("notes.txt", "héllo ✓\n"),
            ("src/deep/main.rs", "fn main() {}\n"),
            ("empty", ""),
        ]);
        let reader = asking("read_file");
        assert!(reader.reads_project());
        let read = |path: &str| given(&reader, Some(&project.0), path);
        assert_eq!(read("notes.txt"), "héllo ✓\n");
        assert_eq!(read("src/deep/main.rs"), "fn main() {}\n");
        assert_eq!(read("./src//deep/./main.rs"), "fn main() {}\n");
        assert_eq!(read("empty"), "");
        // Nothing there, a folder, the root itself, what is under a file.
        for path in [
            "none.txt",
            "src/none/main.rs",
            "src",
            "src/deep/",
            "",
            "notes.txt/more",
        ] {
            assert_eq!(read(path), "!3", "{path:?}");
        }
        project.write("raw.bin", &[b'a', 0xFF, 0xFE]);
        assert_eq!(read("raw.bin"), "!5");
    }

    #[test]
    fn a_path_written_so_as_to_leave_the_project_is_refused() {
        let outside = Folder::with(&[
            ("secret.txt", "secret"),
            ("project/inside.txt", "inside"),
            ("project/sub/more.txt", "more"),
            ("project/.git/config", "token"),
            ("project/sub/.git/config", "token"),
        ]);
        let root = outside.0.join("project");
        let reader = asking("read_file");
        let lister = asking("list_files");
        assert_eq!(given(&reader, Some(&root), "inside.txt"), "inside");
        assert_eq!(given(&reader, Some(&root), "sub/more.txt"), "more");

        let secret = outside.0.join("secret.txt");
        let secret = secret.to_str().expect("a path that is text");
        // The same file from the root of the disk, as each system writes it.
        let from_root = secret.replace('\\', "/");
        let escapes = [
            "..",
            "../secret.txt",
            "./../secret.txt",
            "sub/../../secret.txt",
            "sub/../inside.txt",
            "..\\secret.txt",
            "sub\\..\\..\\secret.txt",
            "sub\\more.txt",
            "/etc/passwd",
            "/secret.txt",
            "C:\\Windows\\win.ini",
            "C:/Windows/win.ini",
            "c:secret.txt",
            "//server/share/secret.txt",
            "\\\\server\\share\\secret.txt",
            "\\\\?\\C:\\Windows\\win.ini",
            "inside.txt\0",
            "\0",
            "in\nside.txt",
            "inside.txt:stream",
            "inside.txt::$DATA",
            ".git",
            ".git/config",
            ".GIT/config",
            "sub/.git/config",
            ".git./config",
            ".git /config",
            "inside.txt.",
            "inside.txt ",
            "sub/.. /secret.txt",
            ".../secret.txt",
            secret,
            &from_root,
        ];
        for path in escapes {
            assert_eq!(given(&reader, Some(&root), path), "!2", "read {path:?}");
            assert_eq!(given(&lister, Some(&root), path), "!2", "list {path:?}");
        }
    }

    #[test]
    fn a_path_is_refused_by_how_it_is_written_before_anything_is_looked_up() {
        let parts = |parts: &[&str]| Some(parts.iter().collect::<PathBuf>());
        assert_eq!(relative("src/main.rs"), parts(&["src", "main.rs"]));
        assert_eq!(relative("./src//deep/./"), parts(&["src", "deep"]));
        assert_eq!(relative(".github/été.yml"), parts(&[".github", "été.yml"]));
        assert_eq!(relative(""), parts(&[]));
        assert_eq!(relative("."), parts(&[]));
        for path in [
            "..", "a/..", "/a", "//a/b", "a\\b", "C:a", "a\0", ".git", "a/.Git/b", "a./b",
        ] {
            assert_eq!(relative(path), None, "{path:?}");
        }
    }

    #[test]
    fn what_a_path_resolves_to_must_be_in_the_root_and_in_no_git_folder() {
        // As the system writes them once resolved.
        let (root, other) = if cfg!(windows) {
            (r"\\?\C:\work\proj", r"\\?\C:\work\other\x")
        } else {
            ("/work/proj", "/work/other/x")
        };
        let under = |rest: &str| {
            let rest = rest.replace('/', std::path::MAIN_SEPARATOR_STR);
            within(Path::new(root), Path::new(&format!("{root}{rest}")))
        };
        assert!(under(""));
        assert!(under("/src/main.rs"));
        assert!(under("/.github/git/.gitignore"));
        // A folder beside it whose name starts the same way.
        assert!(!under("-secrets/x"));
        assert!(!under("/.git"));
        assert!(!under("/.git/config"));
        assert!(!under("/sub/.GIT/config"));
        assert!(!within(Path::new(root), Path::new(other)));
        assert!(!within(
            Path::new(root),
            Path::new(root).parent().expect("a parent")
        ));
        if cfg!(windows) {
            // Another disk, a server, a device, and the same folder not written the same.
            for found in [
                r"\\?\D:\work\proj\x",
                r"\\?\UNC\server\share\work\proj\x",
                r"\\.\NUL",
                r"C:\work\proj\x",
            ] {
                assert!(!within(Path::new(root), Path::new(found)), "{found}");
            }
        }
    }

    #[test]
    fn a_link_in_the_project_that_points_out_of_it_leads_nowhere() {
        let outside = Folder::with(&[
            ("secret.txt", "secret"),
            ("project/inside.txt", "inside"),
            ("project/real/file.txt", "real"),
            ("project/.git/config", "token"),
        ]);
        let root = outside.0.join("project");
        // Out of the project, into its `.git`, and to another of its folders.
        let links = [
            (root.join("out"), outside.0.clone()),
            (root.join("history"), root.join(".git")),
            (root.join("alias"), root.join("real")),
        ];
        for (from, to) in &links {
            assert!(link(from, to), "no link could be made on this machine");
        }
        // The system does follow them.
        let through = std::fs::read_to_string(root.join("out").join("secret.txt"));
        assert_eq!(through.ok().as_deref(), Some("secret"));

        let reader = asking("read_file");
        let lister = asking("list_files");
        let read = given(&reader, Some(&root), "out/secret.txt");
        let history = given(&reader, Some(&root), "history/config");
        let alias = given(&reader, Some(&root), "alias/file.txt");
        let all = given(&lister, Some(&root), "");
        let out = given(&lister, Some(&root), "out");
        let listed_history = given(&lister, Some(&root), "history");
        let listed_alias = given(&lister, Some(&root), "alias");
        for (from, _) in &links {
            let _ = std::fs::remove_dir(from).or_else(|_| std::fs::remove_file(from));
        }
        assert_eq!(read, "!2");
        assert_eq!(history, "!2");
        // A link that stays in the project is followed when it is named.
        assert_eq!(alias, "real");
        // None is followed nor listed when the files are listed.
        assert_eq!(all, "inside.txt\nreal/file.txt");
        assert_eq!(out, "!2");
        assert_eq!(listed_history, "!2");
        assert_eq!(listed_alias, "real/file.txt");
    }

    #[test]
    fn a_path_that_is_not_in_the_memory_or_not_text_is_refused() {
        let project = Folder::with(&[("Do", "done")]);
        let asked = |import: &str, before: &str, ptr: &str, len: &str| {
            let body = format!(
                "(local $got i64) {before}
                (local.set $got (call $ask (i32.const {ptr}) (i32.const {len})))
                {ANSWER}"
            );
            given(&lent(import, &body), Some(&project.0), "")
        };
        for import in ["read_file", "list_files"] {
            // One page is 65536 bytes: past its end, across its end, lengths made up.
            for (ptr, len) in [("70000", "4"), ("65530", "10"), ("-1", "1"), ("0", "-1")] {
                assert_eq!(asked(import, "", ptr, len), "!2", "{import} {ptr} {len}");
            }
            // In the memory, but longer than any path.
            assert_eq!(asked(import, "", "0", "4097"), "!2");
            let raw = "(i32.store8 (i32.const 200) (i32.const 255))";
            assert_eq!(asked(import, raw, "200", "1"), "!2");
        }
        assert_eq!(asked("read_file", "", "0", "2"), "done");
    }

    #[test]
    fn a_file_larger_than_4_mb_is_not_read() {
        let project = Folder::with(&[]);
        project.write("most.txt", &vec![b'a'; MAX_FILE]);
        project.write("more.txt", &vec![b'a'; MAX_FILE + 1]);
        let reader = asking_often("read_file", 1, 80);
        assert_eq!(given(&reader, Some(&project.0), "most.txt").len(), MAX_FILE);
        assert_eq!(given(&reader, Some(&project.0), "more.txt"), "!4");
    }

    #[test]
    fn a_run_is_given_32_mb_of_the_project_and_no_more() {
        let project = Folder::with(&[("small.txt", "small")]);
        project.write("most.txt", &vec![b'a'; MAX_FILE]);
        let eight = asking_often("read_file", 8, 80);
        assert_eq!(given(&eight, Some(&project.0), "most.txt").len(), MAX_FILE);
        let nine = asking_often("read_file", 9, 80);
        assert_eq!(given(&nine, Some(&project.0), "most.txt"), "!6");
        // The next run has it all again.
        assert_eq!(given(&eight, Some(&project.0), "small.txt"), "small");
    }

    #[test]
    fn a_run_asks_2000_times_and_no_more() {
        let project = Folder::with(&[("small.txt", "small")]);
        for import in ["read_file", "list_files"] {
            let most = asking_often(import, 2_000, 1);
            let more = asking_often(import, 2_001, 1);
            // What is asked in vain counts too.
            for path in ["small.txt", "none", "../out"] {
                assert_ne!(
                    given(&most, Some(&project.0), path),
                    "!6",
                    "{import} {path}"
                );
                assert_eq!(
                    given(&more, Some(&project.0), path),
                    "!6",
                    "{import} {path}"
                );
            }
        }
    }

    #[test]
    fn a_plugin_that_asks_without_end_is_stopped() {
        let project = Folder::with(&[("small.txt", "small"), ("src/main.rs", "")]);
        for import in ["read_file", "list_files"] {
            let plugin = lent(
                import,
                "(memory.copy (i32.const 128) (local.get $ptr) (local.get $len))
                (loop $ever
                    (drop (call $ask (i32.const 128) (local.get $len)))
                    (br $ever))
                (i64.const 0)",
            );
            let started = std::time::Instant::now();
            for (root, path) in [
                (Some(project.0.as_path()), "small.txt"),
                (None, ""),
                (None, ".."),
            ] {
                assert_eq!(
                    plugin.run_in(
                        0,
                        path,
                        &Context {
                            root,
                            ..Context::default()
                        }
                    ),
                    Err("The plugin test ran too long".to_owned())
                );
            }
            assert!(started.elapsed().as_secs() < 60);
        }
    }

    #[test]
    fn what_the_editor_does_for_a_plugin_is_paid_from_the_budget_of_the_run() {
        let too_long = Err("The plugin test ran too long".to_owned());
        // Each call: 500 of them cost five million.
        for import in ["read_file", "list_files"] {
            let mut often = asking_often(import, 500, 1);
            often.fuel = 4_000_000;
            assert_eq!(often.run(0, "notes.txt"), too_long);
            often.fuel = 6_000_000;
            assert_eq!(often.run(0, "notes.txt"), Ok("!1".to_owned()));
        }
        // Each byte given: a file of 4 MB costs four million.
        let project = Folder::with(&[]);
        project.write("most.txt", &vec![b'a'; MAX_FILE]);
        let mut reader = asking_often("read_file", 1, 80);
        reader.fuel = 4_000_000;
        let context = Context {
            root: Some(&project.0),
            ..Context::default()
        };
        assert_eq!(
            reader.run_in(0, "most.txt", &context).map(|read| read.text),
            too_long
        );
        reader.fuel = 5_000_000;
        assert!(reader.run_in(0, "most.txt", &context).is_ok());
        // Each line logged, kept or not.
        let mut logger = logging(
            "(local $times i32)
            (loop $again
                (call $log (i32.const 0) (i32.const 2))
                (local.set $times (i32.add (local.get $times) (i32.const 1)))
                (br_if $again (i32.lt_u (local.get $times) (i32.const 500))))",
        );
        logger.fuel = 4_000_000;
        assert_eq!(
            logger.run(0, ""),
            Err("The plugin log ran too long".to_owned())
        );
        logger.fuel = 6_000_000;
        assert_eq!(logger.run(0, ""), Ok(String::new()));
    }

    #[test]
    fn the_files_of_a_folder_of_the_project_are_listed_sorted() {
        let project = Folder::with(&[
            ("b.txt", ""),
            ("a.txt", ""),
            ("src/main.rs", ""),
            ("src/deep/z.rs", ""),
            ("src/deep/a.rs", ""),
            ("docs/été.md", ""),
            (".hidden/kept", ""),
            (".git/config", ""),
            ("node_modules/pkg/index.js", ""),
            ("target/debug/app", ""),
            ("src/target/out", ""),
            ("src/Node_Modules/pkg.js", ""),
            ("src/deep/.git/HEAD", ""),
        ]);
        std::fs::create_dir(project.0.join("void")).expect("folder");
        let lister = asking("list_files");
        assert!(lister.reads_project());
        let list = |prefix: &str| given(&lister, Some(&project.0), prefix);
        assert_eq!(
            list(""),
            ".hidden/kept\na.txt\nb.txt\ndocs/été.md\nsrc/deep/a.rs\nsrc/deep/z.rs\nsrc/main.rs"
        );
        let src = "src/deep/a.rs\nsrc/deep/z.rs\nsrc/main.rs";
        for prefix in ["src", "src/", "./src", "src//"] {
            assert_eq!(list(prefix), src, "{prefix:?}");
        }
        assert_eq!(list("src/deep"), "src/deep/a.rs\nsrc/deep/z.rs");
        assert_eq!(list("docs"), "docs/été.md");
        assert_eq!(list("void"), "");
        // A folder that is passed over is listed when it is the one asked for.
        assert_eq!(list("target"), "target/debug/app");
        assert_eq!(list(".git"), "!2");
        // No such folder, a file, the start of a name.
        for prefix in ["none", "a.txt", "sr", "src/deep/a.rs"] {
            assert_eq!(list(prefix), "!3", "{prefix:?}");
        }
    }

    #[test]
    fn a_list_of_files_stops_at_its_caps_and_is_paid_for() {
        let project = Folder::with(&[
            ("a", ""),
            ("b", ""),
            ("c", ""),
            ("sub/d", ""),
            ("sub/e", ""),
        ]);
        let root = project.0.canonicalize().expect("a folder");
        let mut looks = 100;
        let all = files_in(&root, "", 100, &mut looks).expect("a list");
        assert_eq!(all, "a\nb\nc\nsub/d\nsub/e");
        // Five files and one folder were looked at.
        assert_eq!(looks, 94);
        let mut looks = 100;
        let capped = files_in(&root, "", 3, &mut looks).expect("a list");
        assert_eq!(capped.lines().count(), 3);
        assert!(
            capped
                .lines()
                .all(|path| all.lines().any(|known| known == path))
        );
        let mut looks = 2;
        let short = files_in(&root, "", 100, &mut looks).expect("a list");
        assert!(short.lines().count() <= 2, "{short}");
        assert_eq!(looks, 0);

        // In a run, each look is paid for: one that cannot be ends the run.
        let mut lister = asking("list_files");
        lister.fuel = CALL_FUEL + 3 * ENTRY_FUEL;
        assert_eq!(
            lister.run_in(
                0,
                "",
                &Context {
                    root: Some(&project.0),
                    ..Context::default()
                }
            ),
            Err("The plugin test ran too long".to_owned())
        );
        lister.fuel = CALL_FUEL + 20 * ENTRY_FUEL;
        assert_eq!(given(&lister, Some(&project.0), ""), all);
    }

    #[test]
    fn with_no_project_a_plugin_reads_nothing_and_is_told_so() {
        let project = Folder::with(&[("notes.txt", "notes")]);
        for import in ["read_file", "list_files"] {
            let plugin = asking(import);
            assert_eq!(given(&plugin, None, "notes.txt"), "!1");
            assert_eq!(given(&plugin, None, ""), "!1");
            assert_eq!(given(&plugin, None, "../notes.txt"), "!1");
            // As the first version of the interface runs it.
            assert_eq!(plugin.run(0, "notes.txt"), Ok("!1".to_owned()));
            // A project that is not there.
            assert_eq!(
                given(&plugin, Some(&project.0.join("gone")), "notes.txt"),
                "!1"
            );
        }
        // Nor while it is loaded and names its commands.
        let commands = r#"(func (export "godcode_commands") (result i64)
            (if (i64.ne (call $ask (i32.const 0) (i32.const 2)) (i64.const -1)) (then unreachable))
            (i64.const 2))"#;
        let bytes = wasm(
            &[
                r#"(import "godcode" "read_file" (func $ask (param i32 i32) (result i64)))"#,
                MEMORY,
                ALLOC,
                commands,
                &run_of(ECHO),
            ]
            .join("\n"),
        );
        let plugin = Plugin::load("early.wasm", &bytes).expect("it loads");
        assert_eq!(plugin.commands(), ["Do"]);
    }

    /// A plugin lent the log, whose one command does `body` and gives its text back.
    fn logging(body: &str) -> Plugin {
        let parts = [
            r#"(import "godcode" "log" (func $log (param i32 i32)))"#,
            MEMORY,
            r#"(data (i32.const 64) "two\nlines\there")"#,
            ALLOC,
            COMMANDS,
            &run_of(&format!("{body} {ECHO}")),
        ];
        Plugin::load("log.wasm", &wasm(&parts.join("\n"))).expect("it loads")
    }

    #[test]
    fn what_a_plugin_logs_comes_back_with_what_it_gives() {
        let plugin = logging(
            "(call $log (local.get $ptr) (local.get $len))
            (call $log (i32.const 64) (i32.const 14))
            (call $log (i32.const 0) (i32.const 0))
            (call $log (i32.const 70000) (i32.const 4))
            (call $log (i32.const 65530) (i32.const 10))
            (call $log (i32.const 0) (i32.const -1))
            (call $log (i32.const 0) (i32.const 2))",
        );
        // The log alone reads nothing of the project.
        assert!(!plugin.reads_project());
        assert_eq!(
            plugin.run_in(0, "héllo", &Context::default()),
            Ok(Outcome {
                kind: Kind::Replace,
                text: "héllo".to_owned(),
                // A line break in a line is a space; what is not in the memory is no line.
                log: vec![
                    "héllo".into(),
                    "two lines here".into(),
                    String::new(),
                    "Do".into()
                ],
            })
        );
    }

    #[test]
    fn the_log_of_a_run_keeps_200_lines_of_1024_bytes() {
        let often = logging(
            "(local $times i32)
            (loop $again
                (call $log (local.get $ptr) (local.get $len))
                (local.set $times (i32.add (local.get $times) (i32.const 1)))
                (br_if $again (i32.lt_u (local.get $times) (i32.const 300))))",
        );
        let log = often
            .run_in(0, "line", &Context::default())
            .expect("it runs")
            .log;
        assert_eq!(log.len(), 200);
        assert!(log.iter().all(|line| line == "line"));

        let once = logging("(call $log (local.get $ptr) (local.get $len))");
        let long = "a".repeat(5_000);
        let log = once
            .run_in(0, &long, &Context::default())
            .expect("it runs")
            .log;
        assert_eq!(log, ["a".repeat(1_024)]);
        // Cut in the middle of a character: what is left of it is no character.
        let accents = format!("a{}", "é".repeat(1_000));
        let log = once
            .run_in(0, &accents, &Context::default())
            .expect("it runs")
            .log;
        assert_eq!(log, [format!("a{}\u{FFFD}", "é".repeat(511))]);
        let raw = logging(
            "(i32.store8 (i32.const 200) (i32.const 255))
            (call $log (i32.const 200) (i32.const 1))",
        );
        let log = raw.run_in(0, "", &Context::default()).expect("it runs").log;
        assert_eq!(log, ["\u{FFFD}"]);

        let mut endless =
            logging("(loop $ever (call $log (i32.const 0) (i32.const 2)) (br $ever))");
        endless.fuel = 50_000_000;
        assert_eq!(
            endless.run(0, ""),
            Err("The plugin log ran too long".to_owned())
        );
    }

    #[test]
    fn a_plugin_lent_the_project_is_still_refused_anything_else() {
        let spy = |import: &str| {
            refused(
                "spy.wasm",
                &wasm(&[import, MEMORY, ALLOC, COMMANDS, &run_of(ECHO)].join("\n")),
            )
        };
        for (module, name) in [
            ("godcode", "write_file"),
            ("godcode", "remove_file"),
            ("godcode", "read_file "),
            ("godcode", "godcode_alloc"),
            ("env", "read_file"),
            ("Godcode", "read_file"),
            ("", "log"),
        ] {
            let import =
                format!(r#"(import "{module}" "{name}" (func (param i32 i32) (result i64)))"#);
            assert_eq!(
                spy(&import),
                format!(
                    "The plugin spy asks for `{module}.{name}` from outside, and plugins are \
                     lent nothing"
                )
            );
        }
        // One thing it may ask for does not let another through.
        let two = r#"(import "godcode" "read_file" (func (param i32 i32) (result i64)))
            (import "godcode" "http_get" (func (param i32 i32) (result i64)))"#;
        assert!(spy(two).contains("asks for `godcode.http_get`"));

        assert_eq!(
            spy(r#"(import "godcode" "read_file" (func (param i32) (result i64)))"#),
            "The plugin spy asks for `godcode.read_file` as [i32] -> [i64], where it is \
             [i32, i32] -> [i64]"
        );
        for import in [
            r#"(import "godcode" "list_files" (func (param i32 i32) (result i32)))"#,
            r#"(import "godcode" "log" (func (param i32 i32) (result i64)))"#,
            r#"(import "godcode" "log" (func (param i64 i32)))"#,
        ] {
            assert!(
                spy(import).contains(", where it is [i32, i32] -> ["),
                "{import}"
            );
        }
        let global = spy(r#"(import "godcode" "read_file" (global i64))"#);
        assert!(
            global.contains("`godcode.read_file` as not a function"),
            "{global}"
        );
    }

    #[test]
    fn a_plugin_cannot_ask_from_inside_its_godcode_alloc() {
        let project = Folder::with(&[("Do", "done"), ("notes.txt", "notes")]);
        // Its `godcode_alloc` asks for the file "Do" each time, and keeps the answer.
        let parts = [
            r#"(import "godcode" "read_file" (func $ask (param i32 i32) (result i64)))"#,
            MEMORY,
            r#"(data (i32.const 64) "!")"#,
            "(global $seen (mut i64) (i64.const 0))",
            r#"(func (export "godcode_alloc") (param i32) (result i32)
                (global.set $seen (call $ask (i32.const 0) (i32.const 2)))
                (i32.const 1024))"#,
            COMMANDS,
            &run_of(&format!(
                "(local $got i64)
                (drop (call $ask (local.get $ptr) (local.get $len)))
                (local.set $got (global.get $seen))
                {ANSWER}"
            )),
        ];
        let plugin = Plugin::load("deep.wasm", &wasm(&parts.join("\n"))).expect("it loads");
        // The last time was for the room of "notes.txt", while the editor waited for it.
        assert_eq!(given(&plugin, Some(&project.0), "notes.txt"), "!7");
    }

    #[test]
    fn a_place_outside_the_memory_for_a_file_is_an_error() {
        let project = Folder::with(&[("notes.txt", "0123456789")]);
        // The text of the command goes to 1024; what comes after goes to `second`.
        let wild = |second: &str| {
            let parts = [
                r#"(import "godcode" "read_file" (func $ask (param i32 i32) (result i64)))"#,
                MEMORY,
                "(global $rooms (mut i32) (i32.const 0))",
                &format!(
                    r#"(func (export "godcode_alloc") (param i32) (result i32)
                        (global.set $rooms (i32.add (global.get $rooms) (i32.const 1)))
                        (if (result i32) (i32.gt_u (global.get $rooms) (i32.const 1))
                            (then {second})
                            (else (i32.const 1024))))"#
                ),
                COMMANDS,
                &run_of("(call $ask (local.get $ptr) (local.get $len))"),
            ];
            let mut plugin = Plugin::load("wild.wasm", &wasm(&parts.join("\n"))).expect("it loads");
            plugin.fuel = 20_000_000;
            plugin.run_in(
                0,
                "notes.txt",
                &Context {
                    root: Some(&project.0),
                    ..Context::default()
                },
            )
        };
        let outside = "The plugin wild gave a place outside its memory for the text";
        for place in ["-1", "65530", "2000000000"] {
            let outcome = wild(&format!("(i32.const {place})"));
            assert_eq!(outcome, Err(outside.to_owned()), "{place}");
        }
        assert_eq!(
            wild("(i32.const 65526)").map(|outcome| outcome.text),
            Ok("0123456789".to_owned())
        );
        // It breaks, or never ends, while the editor waits for the room.
        let why = wild("(unreachable)").expect_err("it broke");
        assert!(why.starts_with("The plugin wild stopped: "), "{why}");
        let why = wild("(loop $ever (br $ever)) (i32.const 0)").expect_err("it hung");
        assert_eq!(why, "The plugin wild ran too long");
    }
}
