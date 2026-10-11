//! Commands added by plugins: small WebAssembly programs run shut in, with no way out.
//!
//! A plugin is a `.wasm` file. Each of its commands takes a text (the selection, or the
//! whole file) and gives back the text that replaces it. A plugin is lent nothing: a
//! module that imports anything (WASI, a file, the network, a clock) is refused. All it
//! can do is compute, within a budget, in a memory of its own that cannot grow past a cap.
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
//! # What a plugin is held to
//!
//! - Every run starts the module afresh: a new memory, the globals as written, the start
//!   function run again. Nothing is kept from one run to the next, and nothing has to be
//!   freed.
//! - A run executes two thousand million instructions at most, a few seconds, then is
//!   stopped. Listing the commands, when the plugin is loaded, has fifty million.
//! - The memory holds 64 MB at most; growing it further stops the plugin.
//! - A command is given at most 16 MB of text. The list of commands, the name and a
//!   message of failure hold 64 KB each. The file is 32 MB at most.
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

use std::path::{Path, PathBuf};

use wasmi::{
    CompilationMode, Config, Engine, ExternType, Instance, Linker, Memory, Module, Store,
    StoreLimits, StoreLimitsBuilder, TrapCode, ValType, WasmParams, WasmResults,
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

/// The functions of a plugin: the name, what it takes, what it gives, whether it may be
/// left out.
const FUNCTIONS: [(&str, &[ValType], ValType, bool); 4] = [
    ("godcode_alloc", &[ValType::I32], ValType::I32, false),
    ("godcode_commands", &[], ValType::I64, false),
    (
        "godcode_run",
        &[ValType::I32, ValType::I32, ValType::I32],
        ValType::I64,
        false,
    ),
    ("godcode_name", &[], ValType::I64, true),
];

/// A plugin read from its file, ready to run.
pub struct Plugin {
    name: String,
    commands: Vec<String>,
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
    /// anything from outside or lacks what a plugin must have.
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

        let mut run = Run::start(&module, LOAD_FUEL)?;
        let listed = run.call::<(), i64>("godcode_commands", ())?;
        let commands = lines(&run.text(listed, MAX_NAMES)?);
        if commands.is_empty() {
            return Err("has no command".to_owned());
        }
        let mut name = stem.to_owned();
        if module
            .exports()
            .any(|export| export.name() == "godcode_name")
        {
            let named = run.call::<(), i64>("godcode_name", ())?;
            if let Some(first) = lines(&run.text(named, MAX_NAMES)?).into_iter().next() {
                name = first;
            }
        }
        Ok(Plugin {
            name,
            commands,
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

    /// Run the command at `index` on `text`: the text it gives back, or why it failed.
    pub fn run(&self, index: usize, text: &str) -> Result<String, String> {
        self.ran(index, text)
            .map_err(|why| format!("The plugin {} {why}", self.name))
    }

    /// What the command at `index` makes of `text`, or what went wrong, to be said after
    /// the name of the plugin.
    fn ran(&self, index: usize, text: &str) -> Result<String, String> {
        let command = i32::try_from(index)
            .ok()
            .filter(|_| index < self.commands.len())
            .ok_or_else(|| format!("has no command {index}"))?;
        let len = i32::try_from(text.len())
            .ok()
            .filter(|_| text.len() <= MAX_TEXT)
            .ok_or("is given more than 16 MB of text")?;

        let mut run = Run::start(&self.module, self.fuel)?;
        let ptr = run.call::<i32, i32>("godcode_alloc", len)?;
        run.write(ptr, text)?;
        let answer = run.call::<(i32, i32, i32), i64>("godcode_run", (command, ptr, len))?;
        if answer.cast_unsigned() & FAILED == 0 {
            return run.text(answer, MAX_MEMORY);
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

/// How a function is written down: what it takes, what it gives.
fn signature(takes: &[ValType], gives: &[ValType]) -> String {
    format!("{takes:?} -> {gives:?}").to_lowercase()
}

/// Whether `module` asks for nothing and exports what a plugin must, or what is wrong.
fn check(module: &Module) -> Result<(), String> {
    if let Some(import) = module.imports().next() {
        return Err(format!(
            "asks for `{}.{}` from outside, and plugins are lent nothing",
            import.module(),
            import.name()
        ));
    }
    let exported = |name: &str| module.exports().find(|export| export.name() == name);
    if !matches!(
        exported("memory").as_ref().map(|export| export.ty()),
        Some(ExternType::Memory(_))
    ) {
        return Err("does not export its `memory`".to_owned());
    }
    for (name, takes, gives, optional) in FUNCTIONS {
        let Some(export) = exported(name) else {
            if optional {
                continue;
            }
            return Err(format!("does not export `{name}`"));
        };
        let wanted = signature(takes, &[gives]);
        let found = match export.ty() {
            ExternType::Func(function) => signature(function.params(), function.results()),
            _ => "not a function".to_owned(),
        };
        if found != wanted {
            return Err(format!(
                "exports `{name}` as {found}, where {wanted} is expected"
            ));
        }
    }
    Ok(())
}

/// Why a plugin was stopped, to be said after its name.
fn stopped(err: &wasmi::Error) -> String {
    match err.as_trap_code() {
        Some(TrapCode::OutOfFuel) => "ran too long".to_owned(),
        Some(TrapCode::GrowthOperationLimited) => "asked for more than 64 MB of memory".to_owned(),
        _ => format!("stopped: {err}"),
    }
}

/// A plugin started afresh: a memory of its own, a budget, nothing left of earlier runs.
/// What goes wrong is told so as to be said after the name of the plugin.
struct Run {
    store: Store<StoreLimits>,
    instance: Instance,
    memory: Memory,
}

impl Run {
    /// Start `module`, which may spend `fuel` from now on.
    fn start(module: &Module, fuel: u64) -> Result<Self, String> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(MAX_MEMORY)
            .memories(1)
            .tables(8)
            .table_elements(1 << 20)
            .instances(1)
            // Growing too far stops the plugin, rather than being an answer it may ignore.
            .trap_on_grow_failure(true)
            .build();
        let mut store = Store::new(module.engine(), limits);
        store.limiter(|limits| limits);
        store.set_fuel(fuel).map_err(|err| stopped(&err))?;
        let instance = Linker::new(module.engine())
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
}
