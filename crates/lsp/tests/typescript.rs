//! The client against a real server. Ignored by default: it needs
//! `typescript-language-server` (`npm install -g typescript-language-server typescript@5`),
//! or its path in `GODCODE_TS_SERVER`. Run with `cargo test -p godcode-lsp -- --ignored`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use godcode_lsp::{Event, Server};

/// Ask again until the server, once it has read the project, gives an answer worth keeping.
fn until<T>(
    server: &mut Server,
    mut ask: impl FnMut(&mut Server) -> u64,
    mut keep: impl FnMut(u64, Event) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        let id = ask(server);
        let asked = Instant::now();
        while asked.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(50));
            for event in server.poll() {
                if let Event::Exited { why } = &event {
                    panic!("the server left: {why}");
                }
                if let Some(kept) = keep(id, event) {
                    return kept;
                }
            }
        }
    }
    panic!("no answer from the server");
}

/// The server to run: the one named in `GODCODE_TS_SERVER`, or the one installed.
fn program() -> String {
    std::env::var("GODCODE_TS_SERVER").unwrap_or_else(|_| {
        let name = "typescript-language-server";
        if cfg!(windows) {
            format!("{name}.cmd")
        } else {
            name.to_owned()
        }
    })
}

/// An empty folder for a project, named as the server will name it: it follows links.
fn folder(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("godcode-lsp-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("folder");
    let root = root.canonicalize().expect("folder");
    PathBuf::from(root.to_string_lossy().trim_start_matches(r"\\?\"))
}

#[test]
#[ignore = "needs typescript-language-server"]
fn a_real_server_documents_implements_and_finds_symbols() {
    let (program, root) = (program(), folder("pets"));
    let file = root.join("pets.ts");
    let text = "/** Makes a sound. */\ninterface Animal { speak(): string }\nclass Dog \
                implements Animal { speak() { return \"woof\" } }\nexport const pet: Animal = \
                new Dog()\n";
    std::fs::write(&file, text).expect("file");

    let mut server = Server::start(&program, &["--stdio"], &root, "", || {}).expect("server");
    server.open(&file, "typescript", text);

    // `Animal`, on the line of the interface.
    let documented = until(
        &mut server,
        |server| server.hover(&file, 1, 12),
        |id, event| match event {
            Event::Hover { id: of, text } if of == id && !text.is_empty() => Some(text),
            _ => None,
        },
    );
    assert!(documented.contains("Makes a sound"), "{documented}");
    assert!(documented.contains("interface Animal"), "{documented}");

    let implemented = until(
        &mut server,
        |server| server.implementation(&file, 1, 12),
        |id, event| match event {
            Event::Definition { id: of, places } if of == id && !places.is_empty() => Some(places),
            _ => None,
        },
    );
    assert_eq!(implemented.len(), 1, "{implemented:?}");
    assert_eq!((implemented[0].line, implemented[0].column), (2, 6));
    assert_eq!(implemented[0].path, file);

    let named = until(
        &mut server,
        |server| server.symbols("Dog"),
        |id, event| match event {
            Event::Symbols { id: of, list } if of == id && !list.is_empty() => Some(list),
            _ => None,
        },
    );
    let dog = named.iter().find(|found| found.name == "Dog");
    let dog = dog.unwrap_or_else(|| panic!("{named:?}"));
    assert_eq!((dog.place.line, &dog.place.path), (2, &file));

    // A line on top, in chars that take two units, then a name changed in two places: the
    // server is told only what differs, and still knows where everything is.
    let moved = format!("// é😀\n{text}");
    server.change(&file, &moved);
    let renamed = moved.replace("Dog", "Hound");
    server.change(&file, &renamed);
    let implemented = until(
        &mut server,
        |server| server.implementation(&file, 2, 12),
        |id, event| match event {
            Event::Definition { id: of, places } if of == id && !places.is_empty() => Some(places),
            _ => None,
        },
    );
    assert_eq!((implemented[0].line, implemented[0].column), (3, 6));
    let documented = until(
        &mut server,
        |server| server.hover(&file, 3, 8),
        |id, event| match event {
            Event::Hover { id: of, text } if of == id && !text.is_empty() => Some(text),
            _ => None,
        },
    );
    assert!(documented.contains("class Hound"), "{documented}");

    // Who calls `inner`, and what `outer` calls.
    let calling = format!(
        "{renamed}function inner() {{ return 1 }}\nfunction outer() {{ return inner() + inner() }}\n"
    );
    server.change(&file, &calling);
    let mut calls = |line: usize, incoming: bool| {
        let places = until(
            &mut server,
            |server| server.calls(&file, line, 9, incoming),
            |id, event| match event {
                Event::References { id: of, places } if of == id && !places.is_empty() => {
                    Some(places)
                }
                _ => None,
            },
        );
        let mut places: Vec<_> = places.iter().map(|at| (at.line, at.column)).collect();
        places.sort_unstable();
        places
    };
    assert_eq!(calls(5, true), [(6, 26), (6, 36)]);
    assert_eq!(calls(6, false), [(5, 9)]);

    drop(server);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[ignore = "needs typescript-language-server"]
fn a_real_server_imports_what_a_completion_needs() {
    let root = folder("import");
    std::fs::write(root.join("tsconfig.json"), "{}").expect("project");
    let (library, file) = (root.join("library.ts"), root.join("main.ts"));
    let exported = "export function helper() { return 1 }\n";
    let text = "export const total = hel\n";
    std::fs::write(&library, exported).expect("file");
    std::fs::write(&file, text).expect("file");

    let mut server = Server::start(&program(), &["--stdio"], &root, "", || {}).expect("server");
    server.open(&library, "typescript", exported);
    server.open(&file, "typescript", text);

    let written = "export const total = helper\n";
    let mut edits = Vec::new();
    // tsserver fails this question for a moment after a project opens (its map of what is
    // exported is not ready): asked again, as someone picking the word again would.
    let deadline = Instant::now() + Duration::from_secs(60);
    while edits.is_empty() && Instant::now() < deadline {
        // `helper` is in another file: the list has it once the project is read.
        let picked = until(
            &mut server,
            |server| server.completion(&file, 0, 24),
            |id, event| match event {
                Event::Completion { id: of, items } if of == id => {
                    items.into_iter().find(|item| item.label == "helper")
                }
                _ => None,
            },
        );
        assert_eq!(picked.insert, "helper");
        // Asked first, the word written after: the server answers about the text of its list.
        edits = server.accepted(&file, &picked);
        if edits.is_empty() {
            // The word waits for the answer: told now, the server would fail.
            assert!(!server.change(&file, written));
            let files = edits_asked(&mut server);
            edits = files.into_iter().flat_map(|file| file.edits).collect();
        }
    }
    assert!(server.change(&file, written));
    assert_eq!(edits.len(), 1, "{edits:?}");
    assert_eq!((edits[0].start, edits[0].end), ((0, 0), (0, 0)));
    assert!(edits[0].text.contains("import { helper }"), "{edits:?}");

    drop(server);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[ignore = "needs typescript-language-server"]
fn a_real_server_writes_types_and_argument_names() {
    let root = folder("hints");
    let file = root.join("hints.ts");
    let text = "function twice(count: number) { return count * 2 }\nconst four = twice(2)\n";
    std::fs::write(&file, text).expect("file");

    // This server writes nothing unless told what to write.
    let options = r#"{ "preferences": {
        "includeInlayParameterNameHints": "literals",
        "includeInlayVariableTypeHints": true,
        "includeInlayFunctionLikeReturnTypeHints": true
    } }"#;
    let mut server =
        Server::start(&program(), &["--stdio"], &root, options, || {}).expect("server");
    server.open(&file, "typescript", text);
    let hints = until(
        &mut server,
        |server| server.hints(&file, 0..2),
        |id, event| match event {
            Event::Hints { id: of, list } if of == id && !list.is_empty() => Some(list),
            _ => None,
        },
    );
    let found: Vec<_> = hints
        .iter()
        .map(|hint| (hint.line, hint.column, hint.label.trim()))
        .collect();
    // What the function gives back, the type of the name, the argument the 2 is for.
    assert_eq!(
        found,
        [(0, 29, ": number"), (1, 10, ": number"), (1, 19, "count:")]
    );

    drop(server);
    let _ = std::fs::remove_dir_all(root);
}

/// The changes the server asks for within a few seconds.
fn edits_asked(server: &mut Server) -> Vec<godcode_lsp::FileEdits> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        // The first answer is the one, even when it says nothing is to change.
        let mut asked = server.poll().into_iter().filter_map(|event| match event {
            Event::Edits { files, .. } => Some(files),
            _ => None,
        });
        if let Some(files) = asked.next() {
            return files;
        }
    }
    Vec::new()
}

#[test]
#[ignore = "needs typescript-language-server"]
fn a_real_server_fixes_a_missing_import() {
    let root = folder("fix");
    std::fs::write(root.join("tsconfig.json"), "{}").expect("project");
    let (library, file) = (root.join("library.ts"), root.join("main.ts"));
    let exported = "export function helper() { return 1 }\n";
    let text = "export const total = helper()\n";
    std::fs::write(&library, exported).expect("file");
    std::fs::write(&file, text).expect("file");

    let mut server = Server::start(&program(), &["--stdio"], &root, "", || {}).expect("server");
    server.open(&library, "typescript", exported);
    server.open(&file, "typescript", text);

    // Fixes are asked with the problems of the line: those come first.
    let problems = until(
        &mut server,
        |_| 0,
        |_, event| match event {
            Event::Diagnostics { path, list } if path == file && !list.is_empty() => Some(list),
            _ => None,
        },
    );
    assert!(problems[0].message.contains("helper"), "{problems:?}");
    let actions = until(
        &mut server,
        |server| server.actions(&file, 0),
        |id, event| match event {
            Event::Actions { id: of, actions } if of == id && !actions.is_empty() => Some(actions),
            _ => None,
        },
    );
    let import = actions
        .iter()
        .find(|action| action.title.contains("import"));
    let import = import.unwrap_or_else(|| panic!("{actions:?}"));

    // Its changes are here already, or come once the server has done its part.
    let mut files = import.files.clone();
    server.carry_out(import);
    if files.is_empty() {
        files = edits_asked(&mut server);
    }
    let written = files.iter().flat_map(|file| &file.edits);
    let written: Vec<&str> = written.map(|edit| edit.text.as_str()).collect();
    assert_eq!(files[0].path, file, "{files:?}");
    assert!(
        written.concat().contains("import { helper }"),
        "{import:?} wrote {written:?}"
    );

    // A rework comes without its changes: the server is asked for them, or does them.
    let reworks = until(
        &mut server,
        |server| server.actions(&library, 0),
        |id, event| match event {
            Event::Actions { id: of, actions } if of == id && !actions.is_empty() => Some(actions),
            _ => None,
        },
    );
    let titled = |title: &str| reworks.iter().find(|action| action.title == title);
    let lazy = titled("Convert named export to default export");
    let lazy = lazy.unwrap_or_else(|| panic!("{reworks:?}"));
    assert!(lazy.files.is_empty());
    server.carry_out(lazy);
    let files = edits_asked(&mut server);
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(
        (&files[0].path, files[0].edits[0].text.as_str()),
        (&library, " default")
    );

    let moving = titled("Move to a new file").unwrap_or_else(|| panic!("{reworks:?}"));
    server.carry_out(moving);
    // Code goes to a file that does not exist yet: the app is to make it.
    let files = edits_asked(&mut server);
    let made = files
        .iter()
        .find(|file| file.path == root.join("helper.ts"));
    let made = made.unwrap_or_else(|| panic!("{files:?}"));
    assert!(made.edits[0].text.contains("function helper"), "{made:?}");
    assert!(files.iter().any(|file| file.path == library), "{files:?}");

    drop(server);
    let _ = std::fs::remove_dir_all(root);
}
