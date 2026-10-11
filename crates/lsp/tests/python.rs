//! The debugger client against a real debug adapter. Ignored by default: it needs Python
//! with debugpy (`pip install debugpy`), or a folder that holds it in `GODCODE_DEBUGPY`
//! (`pip install --target <folder> debugpy`). Run with
//! `cargo test -p godcode-lsp --test python -- --ignored`.

use std::time::{Duration, Instant};

use godcode_lsp::dap::{Event, Session};

/// What the debugger says until `wanted` keeps something of it.
fn until<T>(session: &mut Session, mut wanted: impl FnMut(Event) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        for event in session.poll() {
            if let Event::Failed(why) = &event {
                panic!("the debugger refused: {why}");
            }
            if let Some(kept) = wanted(event) {
                return kept;
            }
        }
    }
    panic!("the debugger did not say what was waited for");
}

#[test]
#[ignore = "needs python with debugpy"]
fn a_real_debugger_stops_and_tells_what_expressions_are_worth() {
    let folder = std::env::temp_dir().join(format!("godcode-dap-{}", std::process::id()));
    std::fs::create_dir_all(&folder).expect("folder");
    let file = folder.join("sum.py");
    let script = "items = [1, 2, 3]\ntotal = sum(items)\nprint(total)\n";
    std::fs::write(&file, script).expect("file");
    if let Ok(packages) = std::env::var("GODCODE_DEBUGPY") {
        // The adapter and the program it runs both find debugpy there.
        // SAFETY: nothing else of this test program reads or writes the environment.
        unsafe { std::env::set_var("PYTHONPATH", packages) };
    }
    let launch = serde_json::json!({
        "request": "launch",
        "program": file,
        "cwd": folder,
        "console": "internalConsole",
        "justMyCode": true,
    });
    // Stopped on the last line, the two above it have run.
    let breakpoints = vec![(file.clone(), vec![2])];
    let adapter = ["-m", "debugpy.adapter"];
    let mut session =
        Session::start("python", &adapter, launch, breakpoints, || {}).expect("python");

    let frames = until(&mut session, |event| match event {
        Event::Stack(frames) if !frames.is_empty() => Some(frames),
        _ => None,
    });
    assert_eq!(
        (frames[0].line, frames[0].path.as_deref()),
        (2, Some(&*file))
    );
    let variables = until(&mut session, |event| match event {
        Event::Variables(variables) => Some(variables),
        _ => None,
    });
    let total = variables.iter().find(|(name, _)| name == "total");
    assert_eq!(
        total.map(|(_, value)| value.as_str()),
        Some("6"),
        "{variables:?}"
    );

    // What is watched is worked out in that call; what cannot be says why.
    let watched = ["total * 2", "len(items)", "missing + 1"].map(str::to_owned);
    session.watch(&watched);
    let mut worth = std::collections::HashMap::new();
    until(&mut session, |event| {
        if let Event::Watched { expression, value } = event {
            worth.insert(expression, value);
        }
        (worth.len() == 3).then_some(())
    });
    assert_eq!(worth["total * 2"], "12");
    assert_eq!(worth["len(items)"], "3");
    assert!(worth["missing + 1"].contains("missing"), "{worth:?}");

    // Let go, it runs to its end.
    session.resume();
    until(&mut session, |event| {
        matches!(event, Event::Ended).then_some(())
    });
    drop(session);
    let _ = std::fs::remove_dir_all(folder);
}
