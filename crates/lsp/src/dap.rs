//! A client of the Debug Adapter Protocol: the same framing as the language servers,
//! other messages. One [`Session`] runs one program under a debugger.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use serde_json::{Value, json};

use crate::{connect, trace};

/// A call on the stack of the stopped program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub id: u64,
    pub name: String,
    pub path: Option<PathBuf>,
    /// Counted from 0.
    pub line: usize,
}

/// What the debugger said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The program stopped, on a breakpoint or after a step.
    Stopped {
        reason: String,
    },
    Running,
    /// Where the stopped program is, innermost call first.
    Stack(Vec<Frame>),
    /// The variables of a call: their names and values.
    Variables(Vec<(String, String)>),
    /// What the program printed.
    Output(String),
    /// What an expression is worth where the program is stopped, or why it cannot be told.
    Watched {
        expression: String,
        value: String,
    },
    Ended,
    /// The debugger refused: why.
    Failed(String),
}

enum Asked {
    Initialize,
    Launch,
    Stack,
    Scopes,
    Variables,
    /// The worth of this expression.
    Evaluate(String),
    Other,
}

/// A program running under a debugger.
pub struct Session {
    child: Child,
    outgoing: Sender<String>,
    incoming: Receiver<Value>,
    seq: u64,
    pending: HashMap<u64, Asked>,
    /// The thread that stopped last.
    thread: u64,
    /// The call of the stack being looked at: expressions are worked out there.
    frame: u64,
    /// What to launch, sent once the debugger answered `initialize`.
    launch: Value,
    /// The breakpoints to set once the debugger is ready for them.
    breakpoints: Vec<(PathBuf, Vec<usize>)>,
    ended: bool,
}

impl Session {
    /// Start the debug adapter `program` and have it launch what `launch` describes, with
    /// `breakpoints` set: files and their lines, counted from 0.
    pub fn start(
        program: &str,
        args: &[&str],
        launch: Value,
        breakpoints: Vec<(PathBuf, Vec<usize>)>,
        wake: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self> {
        let mut command = Command::new(program);
        command.args(args);
        let (child, outgoing, incoming) = connect(command, wake)?;
        let mut session = Self {
            child,
            outgoing,
            incoming,
            seq: 0,
            pending: HashMap::new(),
            thread: 0,
            frame: 0,
            launch,
            breakpoints,
            ended: false,
        };
        let arguments = json!({
            "clientID": "godcode",
            "adapterID": "godcode",
            "linesStartAt1": true,
            "columnsStartAt1": true,
            "pathFormat": "path",
        });
        session.request("initialize", arguments, Asked::Initialize);
        Ok(session)
    }

    fn request(&mut self, command: &str, arguments: Value, asked: Asked) {
        self.seq += 1;
        self.pending.insert(self.seq, asked);
        let message = json!({
            "seq": self.seq,
            "type": "request",
            "command": command,
            "arguments": arguments,
        });
        trace("->", &message);
        // A debugger that is gone is noticed by `poll`, not here.
        let _ = self.outgoing.send(message.to_string());
    }

    /// Set the breakpoints of one file: all of them, replacing those it had.
    pub fn set_breakpoints(&mut self, path: &Path, lines: &[usize]) {
        let places: Vec<Value> = lines
            .iter()
            .map(|line| json!({ "line": line + 1 }))
            .collect();
        let arguments = json!({ "source": { "path": path }, "breakpoints": places });
        self.request("setBreakpoints", arguments, Asked::Other);
    }

    fn step(&mut self, command: &str) {
        self.request(command, json!({ "threadId": self.thread }), Asked::Other);
    }

    pub fn resume(&mut self) {
        self.step("continue");
    }

    /// Run the line, calls included.
    pub fn step_over(&mut self) {
        self.step("next");
    }

    pub fn step_into(&mut self) {
        self.step("stepIn");
    }

    pub fn step_out(&mut self) {
        self.step("stepOut");
    }

    /// Ask for the variables of a call of the stack; answered by [`Event::Variables`].
    pub fn inspect(&mut self, frame: u64) {
        self.frame = frame;
        self.request("scopes", json!({ "frameId": frame }), Asked::Scopes);
    }

    /// Ask what `expressions` are worth in the call being looked at; each is answered by
    /// an [`Event::Watched`].
    pub fn watch(&mut self, expressions: &[String]) {
        for expression in expressions {
            let arguments = json!({
                "expression": expression,
                "frameId": self.frame,
                "context": "watch",
            });
            self.request("evaluate", arguments, Asked::Evaluate(expression.clone()));
        }
    }

    /// Stop the program and the debugger.
    pub fn stop(&mut self) {
        let arguments = json!({ "terminateDebuggee": true });
        self.request("disconnect", arguments, Asked::Other);
    }

    /// Everything the debugger said since the last call.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            match self.incoming.try_recv() {
                Ok(message) => self.handle(&message, &mut events),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.end(&mut events);
                    break;
                }
            }
        }
        events
    }

    fn end(&mut self, events: &mut Vec<Event>) {
        if !std::mem::replace(&mut self.ended, true) {
            events.push(Event::Ended);
        }
    }

    fn handle(&mut self, message: &Value, events: &mut Vec<Event>) {
        let body = &message["body"];
        match message["type"].as_str() {
            Some("event") => match message["event"].as_str().unwrap_or("") {
                // Ready for the breakpoints: once they are set the program may run.
                "initialized" => {
                    for (path, lines) in std::mem::take(&mut self.breakpoints) {
                        self.set_breakpoints(&path, &lines);
                    }
                    self.request("configurationDone", json!({}), Asked::Other);
                }
                "stopped" => {
                    self.thread = body["threadId"].as_u64().unwrap_or(self.thread);
                    let reason = body["reason"].as_str().unwrap_or("").to_owned();
                    events.push(Event::Stopped { reason });
                    let arguments = json!({ "threadId": self.thread, "levels": 50 });
                    self.request("stackTrace", arguments, Asked::Stack);
                }
                "continued" => events.push(Event::Running),
                "output" if body["category"].as_str() != Some("telemetry") => {
                    events.push(Event::Output(
                        body["output"].as_str().unwrap_or("").to_owned(),
                    ));
                }
                "terminated" | "exited" => self.end(events),
                _ => {}
            },
            Some("response") => {
                let asked = message["request_seq"].as_u64();
                let asked = asked.and_then(|seq| self.pending.remove(&seq));
                let failed = message["success"].as_bool() == Some(false);
                match asked {
                    Some(Asked::Initialize | Asked::Launch) if failed => {
                        let why = message["message"].as_str().unwrap_or("it gave no reason");
                        events.push(Event::Failed(why.to_owned()));
                    }
                    Some(Asked::Initialize) => {
                        let launch = std::mem::take(&mut self.launch);
                        self.request("launch", launch, Asked::Launch);
                    }
                    Some(Asked::Stack) => {
                        let frames: Vec<Frame> = frames(body);
                        if let Some(top) = frames.first() {
                            self.inspect(top.id);
                        }
                        events.push(Event::Stack(frames));
                    }
                    // The first scope is the local one.
                    Some(Asked::Scopes) => {
                        if let Some(scope) = body["scopes"][0]["variablesReference"].as_u64() {
                            let arguments = json!({ "variablesReference": scope });
                            self.request("variables", arguments, Asked::Variables);
                        }
                    }
                    Some(Asked::Evaluate(expression)) => events.push(Event::Watched {
                        expression,
                        value: worth(message),
                    }),
                    Some(Asked::Variables) => {
                        let list = body["variables"].as_array().into_iter().flatten();
                        let text = |value: &Value| value.as_str().unwrap_or("").to_owned();
                        let list = list.map(|found| (text(&found["name"]), text(&found["value"])));
                        events.push(Event::Variables(list.collect()));
                    }
                    _ => {}
                }
            }
            // The debugger asks something this client cannot do: it is told so.
            Some("request") => {
                self.seq += 1;
                let answer = json!({
                    "seq": self.seq,
                    "type": "response",
                    "request_seq": message["seq"],
                    "command": message["command"],
                    "success": false,
                    "message": "not supported",
                });
                let _ = self.outgoing.send(answer.to_string());
            }
            _ => {}
        }
    }
}

/// What the answer to `evaluate` says an expression is worth, or why it has no worth.
fn worth(message: &Value) -> String {
    let said = match message["success"].as_bool() {
        Some(false) => message["message"]
            .as_str()
            .unwrap_or("cannot be worked out"),
        _ => message["body"]["result"].as_str().unwrap_or(""),
    };
    // One line: the first of what was said.
    said.lines().next().unwrap_or("").to_owned()
}

fn frames(body: &Value) -> Vec<Frame> {
    let list = body["stackFrames"].as_array().into_iter().flatten();
    list.map(|frame| Frame {
        id: frame["id"].as_u64().unwrap_or(0),
        name: frame["name"].as_str().unwrap_or("").to_owned(),
        path: frame["source"]["path"].as_str().map(PathBuf::from),
        line: (frame["line"].as_u64().unwrap_or(1) as usize).saturating_sub(1),
    })
    .collect()
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worth_of_an_expression_is_read_or_why_it_has_none() {
        let known = json!({ "success": true, "body": { "result": "[1, 2]", "type": "list" } });
        assert_eq!(worth(&known), "[1, 2]");
        let unknown =
            json!({ "success": false, "message": "NameError: name 'x' is not defined\nmore" });
        assert_eq!(worth(&unknown), "NameError: name 'x' is not defined");
        assert_eq!(worth(&json!({ "success": false })), "cannot be worked out");
    }

    #[test]
    fn the_stack_is_read_with_lines_from_zero() {
        let body = json!({ "stackFrames": [
            { "id": 7, "name": "main", "line": 12, "source": { "path": "/work/a.py" } },
            { "id": 8, "name": "<module>", "line": 1 },
        ]});
        let found = frames(&body);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].id, found[0].line), (7, 11));
        assert_eq!(found[0].path.as_deref(), Some(Path::new("/work/a.py")));
        assert_eq!(found[1].path, None);
    }
}
