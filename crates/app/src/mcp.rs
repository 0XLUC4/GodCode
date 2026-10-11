//! Tools lent by a Model Context Protocol server, a program talked to over its pipes.
//!
//! JSON-RPC messages, one a line each way. Nothing here blocks: messages are written and
//! read on threads of their own, and what the server said is collected with
//! [`Server::poll`]. What a tool returns is data: it is handed over as text, never read
//! into here.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The revision of the protocol spoken here; a server may answer with another.
const PROTOCOL: &str = "2025-06-18";
/// How much of what a server complains about is kept, in bytes: its last words.
const KEPT: usize = 4096;
/// A list of tools is not followed further than this many pages.
const MAX_PAGES: usize = 100;
/// How long a server is given to leave by itself once its input is closed.
const GRACE: Duration = Duration::from_millis(250);
/// A value of the environment this long or longer is taken out of what a server
/// complains about: shorter ones (`1`, `true`) are no secrets, and are in every text.
const SECRET_LENGTH: usize = 8;

/// A server to start, as the user wrote it down.
#[derive(Clone, PartialEq)]
pub struct Config {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    /// Added to the environment of the editor. The values are often tokens.
    pub env: Vec<(String, String)>,
}

/// Printed without its arguments and the values of its environment: they hold tokens.
impl std::fmt::Debug for Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.env.iter().map(|(name, _)| name.as_str()).collect();
        formatter
            .debug_struct("Config")
            .field("name", &self.name)
            .field("command", &self.command)
            .field("args", &self.args.len())
            .field("env", &names)
            .finish()
    }
}

/// A value of a settings file as the text a program is given: `3000` for a number.
fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Read the servers of a file written like Claude Desktop's:
/// `{"mcpServers": {"name": {"command": "...", "args": [...], "env": {...}}}}`, sorted by
/// name. Entries that are not that, such as the ones reached at a `url`, are left out.
pub fn configs(json: &str) -> Result<Vec<Config>, String> {
    let file: Value = serde_json::from_str(json).map_err(|err| format!("Not JSON: {err}"))?;
    let entries = file["mcpServers"].as_object().into_iter().flatten();
    let mut configs: Vec<Config> = entries
        .filter_map(|(name, entry)| {
            let command = entry["command"].as_str().filter(|it| !it.is_empty())?;
            let args = entry["args"].as_array().into_iter().flatten();
            let env = entry["env"].as_object().into_iter().flatten();
            Some(Config {
                name: name.clone(),
                command: command.to_owned(),
                args: args.map(plain).collect(),
                env: env
                    .map(|(key, value)| (key.clone(), plain(value)))
                    .collect(),
            })
        })
        .collect();
    configs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(configs)
}

/// Something a server can do when asked.
#[derive(Clone, Debug, PartialEq)]
pub struct Tool {
    /// The name of the server that lends it.
    pub server: String,
    pub name: String,
    pub description: String,
    /// What it takes, as a JSON Schema.
    pub schema: Value,
}

/// What a server said.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The server listens, and lends these tools.
    Ready { server: String, tools: Vec<Tool> },
    /// Its tools are now these: the list replaces the previous one.
    ToolsChanged { server: String, tools: Vec<Tool> },
    /// What the tool asked for by the call `id` returned, or why it failed.
    Called {
        id: u64,
        result: Result<String, String>,
    },
    /// The server cannot be used: it refused, or stopped before it was ready.
    Failed { server: String, why: String },
    /// Its program is gone. Said once.
    Exited { server: String },
}

/// What a request was about, to read its answer.
enum Asked {
    Initialize,
    /// A page of the tools: the ones `found` so far, and whether the list replaces one
    /// already told.
    Tools {
        found: Vec<Tool>,
        changed: bool,
        page: usize,
    },
    Call,
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// What a server is told first: who talks to it.
fn hello() -> Value {
    json!({
        "protocolVersion": PROTOCOL,
        "capabilities": {},
        "clientInfo": { "name": "GodCode", "version": env!("CARGO_PKG_VERSION") },
    })
}

/// What a call of `tool` is asked with. No arguments is an empty object, not `null`.
fn call_of(tool: &str, arguments: Value) -> Value {
    let arguments = match arguments {
        Value::Null => json!({}),
        given => given,
    };
    json!({ "name": tool, "arguments": arguments })
}

/// The answer to something the server asks: a `ping` is answered, anything else (its
/// roots, a sampling, a question to the user) is not something this client does. `None`
/// for a notification, which wants no answer.
fn answer(message: &Value) -> Option<Value> {
    let id = message.get("id").filter(|id| !id.is_null())?;
    Some(if message["method"] == "ping" {
        json!({ "jsonrpc": "2.0", "id": id, "result": {} })
    } else {
        let error = json!({ "code": -32601, "message": "Method not found" });
        json!({ "jsonrpc": "2.0", "id": id, "error": error })
    })
}

/// The tools of one page of a `tools/list` answer. One without a name is left out.
fn tools(server: &str, result: &Value) -> Vec<Tool> {
    let list = result["tools"].as_array().into_iter().flatten();
    let tools = list.filter_map(|found| {
        Some(Tool {
            server: server.to_owned(),
            name: found["name"].as_str()?.to_owned(),
            description: found["description"].as_str().unwrap_or("").to_owned(),
            schema: match &found["inputSchema"] {
                Value::Null => json!({ "type": "object" }),
                schema => schema.clone(),
            },
        })
    });
    tools.collect()
}

/// What a tool returned, as text: its text blocks, one a line; a picture, a sound or a
/// file is only named (`[image image/png]`). An error when the tool said it failed.
fn told(result: &Value) -> Result<String, String> {
    let blocks = result["content"].as_array().into_iter().flatten();
    let parts: Vec<String> = blocks
        .map(|block| {
            let kind = block["type"].as_str().unwrap_or("content");
            let about = match kind {
                "text" => return block["text"].as_str().unwrap_or("").to_owned(),
                "image" | "audio" => block["mimeType"].as_str(),
                "resource" => block["resource"]["uri"].as_str(),
                "resource_link" => block["uri"].as_str(),
                _ => None,
            };
            match about {
                Some(about) => format!("[{kind} {about}]"),
                None => format!("[{kind}]"),
            }
        })
        .collect();
    let mut text = parts.join("\n");
    // A tool may answer with data only, and no text for it.
    if text.is_empty() && !result["structuredContent"].is_null() {
        text = result["structuredContent"].to_string();
    }
    if result["isError"] != true {
        Ok(text)
    } else if text.is_empty() {
        Err("The tool failed.".to_owned())
    } else {
        Err(text)
    }
}

/// Read one message: the next line that is JSON. A server may print other things, which
/// are skipped. `None` once it is gone.
fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    loop {
        let mut line = Vec::new();
        if reader.read_until(b'\n', &mut line).ok()? == 0 {
            return None;
        }
        if let Ok(message) = serde_json::from_slice(&line) {
            return Some(message);
        }
    }
}

/// Keep the end of what is read from `stream`, until it closes.
fn keep(mut stream: impl Read, kept: &Mutex<Vec<u8>>) {
    let mut chunk = [0; 1024];
    while let Ok(read @ 1..) = stream.read(&mut chunk) {
        let mut kept = kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.extend_from_slice(&chunk[..read]);
        let extra = kept.len().saturating_sub(KEPT);
        kept.drain(..extra);
    }
}

/// Start the program of `config` in `folder`, its three streams piped.
fn spawn(config: &Config, folder: &Path) -> Result<Child, String> {
    let mut names = vec![config.command.clone()];
    // As installed by npm on Windows, a program such as `npx` is a `.cmd` file.
    if cfg!(windows) {
        names.push(format!("{}.cmd", config.command));
        names.push(format!("{}.exe", config.command));
    }
    let mut refused = String::new();
    for name in names {
        let mut command = Command::new(name);
        command
            .args(&config.args)
            .envs(config.env.iter().map(|(name, value)| (name, value)))
            .current_dir(folder)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::tools::hidden(&mut command);
        match command.spawn() {
            Ok(child) => return Ok(child),
            // Why the name as written could not start says the most.
            Err(err) if refused.is_empty() => refused = err.to_string(),
            Err(_) => {}
        }
    }
    Err(format!("Cannot start {}: {refused}", config.command))
}

/// End `child`, and on Windows the programs it started: a `.cmd` file is run by a shell,
/// and the server is the child of that shell.
fn end(child: &mut Child) {
    #[cfg(windows)]
    {
        let mut kill = Command::new("taskkill");
        kill.args(["/PID", &child.id().to_string(), "/T", "/F"]);
        crate::tools::hidden(&mut kill);
        let _ = kill.output();
    }
    let _ = child.kill();
}

/// A running Model Context Protocol server.
pub struct Server {
    name: String,
    child: Child,
    /// Messages on their way to the server, written by a thread of their own. Nothing
    /// once its input is closed.
    outgoing: Option<Sender<String>>,
    incoming: Receiver<Value>,
    /// The end of what the server wrote on its error stream.
    complaints: Arc<Mutex<Vec<u8>>>,
    /// The values lent to its environment that are never to be shown.
    secrets: Vec<String>,
    next_id: u64,
    pending: HashMap<u64, Asked>,
    /// The server answered `initialize`: it listens.
    ready: bool,
    /// What was asked before it was ready.
    queued: Vec<Value>,
    /// Why the server can no longer be used, once it cannot.
    stopped: Option<String>,
    exited: bool,
}

impl Server {
    /// Start the program and shake hands; [`Event::Ready`] or [`Event::Failed`] follows.
    /// `wake` is called, from another thread, whenever there is something to
    /// [`Self::poll`].
    pub fn start(
        config: &Config,
        folder: &Path,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let mut child = spawn(config, folder)?;
        let streams = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(mut stdin), Some(stdout), Some(stderr)) = streams else {
            end(&mut child);
            return Err(format!("Cannot talk to {}", config.command));
        };
        let (outgoing, to_write) = channel::<String>();
        std::thread::spawn(move || {
            for line in to_write {
                if stdin.write_all(line.as_bytes()).is_err() || stdin.flush().is_err() {
                    break;
                }
            }
        });
        let complaints = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&complaints);
        let complaining = std::thread::spawn(move || keep(stderr, &kept));
        let (sender, incoming) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(message) = read_message(&mut reader) {
                if sender.send(message).is_err() {
                    return;
                }
                wake();
            }
            // Its last words are waited for, then the sender is dropped: the next poll
            // sees that the program is gone, and can say why.
            let _ = complaining.join();
            drop(sender);
            wake();
        });
        let secrets = config.env.iter().map(|(_, value)| value.clone());
        let mut server = Self {
            name: config.name.clone(),
            child,
            outgoing: Some(outgoing),
            incoming,
            complaints,
            secrets: secrets
                .filter(|value| value.len() >= SECRET_LENGTH)
                .collect(),
            next_id: 1,
            pending: HashMap::from([(1, Asked::Initialize)]),
            ready: false,
            queued: Vec::new(),
            stopped: None,
            exited: false,
        };
        server.write(&request(1, "initialize", hello()));
        Ok(server)
    }

    /// The name the user gave it.
    pub fn name(&self) -> &str {
        &self.name
    }

    fn write(&mut self, message: &Value) {
        // A server that is gone is noticed by `poll`, not here.
        if let Some(outgoing) = &self.outgoing {
            let _ = outgoing.send(format!("{message}\n"));
        }
    }

    fn request(&mut self, method: &str, params: Value, asked: Asked) -> u64 {
        self.next_id += 1;
        self.pending.insert(self.next_id, asked);
        let message = request(self.next_id, method, params);
        if self.ready {
            self.write(&message);
        } else {
            self.queued.push(message);
        }
        self.next_id
    }

    /// Ask for a tool to be run; answered by [`Event::Called`] with this id. On a server
    /// that stopped, the answer is an error, at the next [`Self::poll`].
    pub fn call(&mut self, tool: &str, arguments: Value) -> u64 {
        self.request("tools/call", call_of(tool, arguments), Asked::Call)
    }

    /// `text` without the values lent to the server, should it have printed one.
    fn clean(&self, text: &str) -> String {
        let clean = |text: String, secret: &String| text.replace(secret.as_str(), "***");
        self.secrets.iter().fold(text.trim().to_owned(), clean)
    }

    /// Everything the server said since the last call.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        let mut gone = false;
        loop {
            match self.incoming.try_recv() {
                Ok(message) => self.handle(&message, &mut events),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    gone = !std::mem::replace(&mut self.exited, true);
                    break;
                }
            }
        }
        if gone && self.stopped.is_none() {
            let kept = self.complaints.lock();
            let kept = kept.unwrap_or_else(PoisonError::into_inner).clone();
            let said = self.clean(&String::from_utf8_lossy(&kept));
            let why = if said.is_empty() {
                "The server stopped.".to_owned()
            } else {
                format!("The server stopped: {said}")
            };
            let starting = |asked: &Asked| {
                let first = matches!(asked, Asked::Tools { changed: false, .. });
                first || matches!(asked, Asked::Initialize)
            };
            if self.pending.values().any(starting) {
                let server = self.name.clone();
                let why = why.clone();
                events.push(Event::Failed { server, why });
            }
            self.stopped = Some(why);
        }
        if let Some(why) = &self.stopped {
            // Nothing will answer these any more.
            let asked = self.pending.drain();
            let mut calls: Vec<u64> = asked
                .filter(|(_, asked)| matches!(asked, Asked::Call))
                .map(|(id, _)| id)
                .collect();
            calls.sort_unstable();
            let failed = |id| {
                let result = Err(why.clone());
                Event::Called { id, result }
            };
            events.extend(calls.into_iter().map(failed));
            self.queued.clear();
        }
        if gone {
            let server = self.name.clone();
            events.push(Event::Exited { server });
        }
        events
    }

    /// The server cannot be used: say so, and close its input for it to leave.
    fn fail(&mut self, why: String, events: &mut Vec<Event>) {
        let server = self.name.clone();
        self.stopped = Some(why.clone());
        self.outgoing = None;
        events.push(Event::Failed { server, why });
    }

    fn handle(&mut self, message: &Value, events: &mut Vec<Event>) {
        if let Some(method) = message["method"].as_str() {
            if let Some(answer) = answer(message) {
                self.write(&answer);
            } else if method == "notifications/tools/list_changed" && self.ready {
                let asked = Asked::Tools {
                    found: Vec::new(),
                    changed: true,
                    page: 1,
                };
                self.request("tools/list", json!({}), asked);
            }
            return;
        }
        // An answer to nothing that was asked is not for this client.
        let Some((id, asked)) = message["id"]
            .as_u64()
            .and_then(|id| Some((id, self.pending.remove(&id)?)))
        else {
            return;
        };
        let result = match &message["error"] {
            Value::Null => Ok(&message["result"]),
            error => Err(self.clean(error["message"].as_str().unwrap_or("The server refused."))),
        };
        match (asked, result) {
            (Asked::Initialize, Ok(result)) => {
                self.ready = true;
                self.write(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
                for queued in std::mem::take(&mut self.queued) {
                    self.write(&queued);
                }
                let asked = Asked::Tools {
                    found: Vec::new(),
                    changed: false,
                    page: 1,
                };
                if result["capabilities"]["tools"].is_null() {
                    // It lends none, and is not to be asked for them.
                    let server = self.name.clone();
                    let tools = Vec::new();
                    events.push(Event::Ready { server, tools });
                } else {
                    self.request("tools/list", json!({}), asked);
                }
            }
            (
                Asked::Tools {
                    mut found,
                    changed,
                    page,
                },
                Ok(result),
            ) => {
                found.extend(tools(&self.name, result));
                let next = result["nextCursor"].as_str();
                match next.filter(|cursor| !cursor.is_empty() && page < MAX_PAGES) {
                    Some(cursor) => {
                        let asked = Asked::Tools {
                            found,
                            changed,
                            page: page + 1,
                        };
                        self.request("tools/list", json!({ "cursor": cursor }), asked);
                    }
                    None => {
                        let (server, tools) = (self.name.clone(), found);
                        events.push(if changed {
                            Event::ToolsChanged { server, tools }
                        } else {
                            Event::Ready { server, tools }
                        });
                    }
                }
            }
            (Asked::Call, result) => {
                let result = result.and_then(told);
                events.push(Event::Called { id, result });
            }
            // A new list that could not be read: the tools told before stay.
            (Asked::Tools { changed: true, .. }, Err(_)) => {}
            (Asked::Initialize | Asked::Tools { .. }, Err(why)) => self.fail(why, events),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Its input closed, a server leaves by itself; one that lingers is ended.
        self.outgoing = None;
        let deadline = Instant::now() + GRACE;
        let running = |child: &mut Child| matches!(child.try_wait(), Ok(None));
        while running(&mut self.child) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if running(&mut self.child) {
            end(&mut self.child);
        }
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A small server, for Node: it lends a few tools, and does what a client must cope
    /// with (lines that are no messages, answers to nothing, requests of its own).
    const FAKE: &str = r#"
const lines = require('readline').createInterface({ input: process.stdin });
const send = (message) => process.stdout.write(JSON.stringify(message) + '\n');
const answer = (id, result) => send({ jsonrpc: '2.0', id, result });
const refuse = (id, message) => send({ jsonrpc: '2.0', id, error: { code: -32602, message } });
const said = (text, isError) => ({ content: [{ type: 'text', text }], isError });
const tool = (name) => ({ name, description: 'Does ' + name, inputSchema: { type: 'object', title: name } });
let initialized = false;
let changed = false;
const waiting = {};
process.stderr.write('fake: listening\n');
lines.on('line', (line) => {
  const message = JSON.parse(line);
  const { id, method, params } = message;
  if (method === undefined) {
    // An answer to what was asked of the client: told to the call that waits for it.
    const call = waiting[id];
    delete waiting[id];
    const heard = message.error ? String(message.error.code) : JSON.stringify(message.result);
    if (call !== undefined) answer(call, said(heard));
  } else if (method === 'initialize') {
    process.stdout.write('starting, and this line is not JSON\n');
    answer(999, {});
    const capabilities = { tools: { listChanged: true } };
    answer(id, { protocolVersion: params.protocolVersion, capabilities, serverInfo: { name: 'fake', version: '1' } });
  } else if (method === 'notifications/initialized') {
    initialized = true;
  } else if (method === 'tools/list') {
    if (!initialized) refuse(id, 'not initialized');
    else if (!params.cursor) answer(id, { tools: [tool('echo'), tool('fail')], nextCursor: 'second' });
    else if (params.cursor === 'second') answer(id, { tools: [tool('picture'), { name: 'bare' }].concat(changed ? [tool('new')] : []) });
    else refuse(id, 'no such page');
  } else if (method === 'tools/call') {
    const name = params.name;
    const given = params.arguments;
    if (name === 'echo') answer(id, said(given.text));
    else if (name === 'fail') answer(id, said('it broke', true));
    else if (name === 'picture') answer(id, { content: [{ type: 'image', data: 'AAAA', mimeType: 'image/png' }] });
    else if (name === 'env') answer(id, said(String(process.env.GODCODE_MCP_TEST)));
    else if (name === 'pid') answer(id, said(String(process.pid)));
    else if (name === 'ask') {
      waiting['asked-' + id] = id;
      send({ jsonrpc: '2.0', id: 'asked-' + id, method: given.method });
    } else if (name === 'change') {
      changed = true;
      send({ jsonrpc: '2.0', method: 'notifications/message', params: { level: 'info', data: 'noise' } });
      send({ jsonrpc: '2.0', method: 'notifications/tools/list_changed' });
      answer(id, said('changed'));
    } else if (name === 'quit') {
      process.stderr.write('fake: leaving\n', () => process.exit(0));
    } else refuse(id, 'Unknown tool: ' + name);
  }
});
"#;

    /// Write `text` as a script for Node; `None`, with a note, when Node is not installed.
    fn script(name: &str, text: &str) -> Option<PathBuf> {
        if Command::new("node").arg("--version").output().is_err() {
            eprintln!("skipped: `node` is not installed");
            return None;
        }
        let name = format!("godcode-mcp-{}-{name}.js", std::process::id());
        let file = std::env::temp_dir().join(name);
        std::fs::write(&file, text).expect("script");
        Some(file)
    }

    fn config(command: &str, args: &[&str]) -> Config {
        Config {
            name: "fake".to_owned(),
            command: command.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: vec![("GODCODE_MCP_TEST".to_owned(), "lent".to_owned())],
        }
    }

    fn start(file: &Path) -> Server {
        let config = config("node", &[&file.to_string_lossy()]);
        Server::start(&config, &std::env::temp_dir(), || {}).expect("the server starts")
    }

    /// Everything `server` says until `wanted` is said, which has to be soon.
    fn until(server: &mut Server, wanted: impl Fn(&Event) -> bool) -> Vec<Event> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut seen = Vec::new();
        loop {
            seen.extend(server.poll());
            if seen.iter().any(&wanted) {
                return seen;
            }
            assert!(Instant::now() < deadline, "it never came: {seen:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn answer_to(events: &[Event], call: u64) -> Option<Result<String, String>> {
        events.iter().find_map(|event| match event {
            Event::Called { id, result } if *id == call => Some(result.clone()),
            _ => None,
        })
    }

    /// Run a tool and wait for what it returns.
    fn called(server: &mut Server, tool: &str, arguments: Value) -> Result<String, String> {
        let call = server.call(tool, arguments);
        let seen = until(server, |event| {
            answer_to(std::slice::from_ref(event), call).is_some()
        });
        answer_to(&seen, call).expect("an answer")
    }

    fn names(tools: &[Tool]) -> Vec<&str> {
        tools.iter().map(|tool| tool.name.as_str()).collect()
    }

    /// The program numbered `pid` still runs.
    fn alive(pid: u32) -> bool {
        let mut look = if cfg!(windows) {
            let mut look = Command::new("tasklist");
            look.args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"]);
            look
        } else {
            let mut look = Command::new("kill");
            look.args(["-0", &pid.to_string()]);
            look
        };
        let output = look.output().expect("the list of programs");
        if cfg!(windows) {
            String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
        } else {
            output.status.success()
        }
    }

    /// Wait for the program numbered `pid` to be gone; `false` when it stays.
    fn gone(pid: u32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(15);
        while alive(pid) {
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

    #[test]
    fn the_servers_of_a_settings_file_are_read() {
        let file = r#"{
            "globalShortcut": "Ctrl+Space",
            "mcpServers": {
                "github": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-github"],
                    "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_abc123", "PORT": 3000, "DEBUG": true }
                },
                "files": { "command": "C:\\tools\\files.exe", "args": ["C:\\work", 2] },
                "remote": { "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer x" } },
                "typed": { "type": "http", "url": "https://example.com/mcp" },
                "empty": { "command": "" },
                "odd": "npx"
            }
        }"#;
        let found = configs(file).expect("servers");
        let names: Vec<&str> = found.iter().map(|config| config.name.as_str()).collect();
        // Only the ones that are programs to start, by name.
        assert_eq!(names, ["files", "github"]);
        assert_eq!(found[0].command, r"C:\tools\files.exe");
        assert_eq!(found[0].args, [r"C:\work", "2"]);
        assert!(found[0].env.is_empty());
        assert_eq!(found[1].command, "npx");
        assert_eq!(found[1].args, ["-y", "@modelcontextprotocol/server-github"]);
        // The values are given as they are, numbers and all, for the program to read.
        let env = |name: &str| {
            let mut values = found[1].env.iter();
            let value = values.find(|(key, _)| key == name);
            value.map(|(_, value)| value.as_str())
        };
        assert_eq!(env("GITHUB_PERSONAL_ACCESS_TOKEN"), Some("ghp_abc123"));
        assert_eq!((env("PORT"), env("DEBUG")), (Some("3000"), Some("true")));
    }

    #[test]
    fn a_file_that_is_not_json_is_refused_and_one_without_servers_has_none() {
        let refused = configs(r#"{ "mcpServers": { "a": { "command": "secret-token" "#);
        let why = refused.expect_err("not JSON");
        assert!(why.starts_with("Not JSON"), "{why}");
        // Why it is not JSON is said by where, never by quoting the file.
        assert!(!why.contains("secret-token"), "{why}");
        assert_eq!(configs("{}"), Ok(Vec::new()));
        assert_eq!(configs(r#"{ "mcpServers": [] }"#), Ok(Vec::new()));
        assert_eq!(configs("null"), Ok(Vec::new()));
    }

    #[test]
    fn tokens_are_never_printed_with_a_server_or_with_why_it_did_not_start() {
        let config = Config {
            name: "github".to_owned(),
            command: "godcode-no-such-program".to_owned(),
            args: vec!["--token=arg-s3cret".to_owned()],
            env: vec![("API_KEY".to_owned(), "env-s3cret".to_owned())],
        };
        let printed = format!("{config:?}");
        assert!(printed.contains("github") && printed.contains("API_KEY"));
        assert!(!printed.contains("s3cret"), "{printed}");
        // A program that does not exist is said at once, by its name alone.
        let Err(why) = Server::start(&config, &std::env::temp_dir(), || {}) else {
            panic!("a program that does not exist started");
        };
        assert!(why.starts_with("Cannot start godcode-no-such-program: "));
        assert!(!why.contains("s3cret"), "{why}");
    }

    #[test]
    fn messages_are_written_as_the_protocol_wants_them() {
        assert_eq!(
            request(1, "initialize", hello()),
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "GodCode", "version": env!("CARGO_PKG_VERSION") },
                },
            })
        );
        assert_eq!(
            call_of("echo", json!({ "text": "é" })),
            json!({ "name": "echo", "arguments": { "text": "é" } })
        );
        // No arguments is an empty object: a server refuses `null`.
        assert_eq!(
            call_of("now", Value::Null),
            json!({ "name": "now", "arguments": {} })
        );
        // One message a line: nothing written breaks the line.
        let line = request(2, "tools/call", call_of("echo", json!({ "text": "a\nb" })));
        assert!(!line.to_string().contains('\n'));
    }

    #[test]
    fn what_a_server_asks_is_answered_or_refused() {
        let asked =
            |id: Value, method: &str| json!({ "jsonrpc": "2.0", "id": id, "method": method });
        assert_eq!(
            answer(&asked(json!(7), "ping")),
            Some(json!({ "jsonrpc": "2.0", "id": 7, "result": {} }))
        );
        // The id is given back as it came, a text as well as a number.
        for method in ["roots/list", "sampling/createMessage", "elicitation/create"] {
            let refused = answer(&asked(json!("a-1"), method)).expect("an answer");
            assert_eq!(refused["id"], "a-1");
            assert_eq!(refused["error"]["code"], -32601);
            assert!(refused.get("result").is_none());
        }
        // A notification wants no answer.
        let notified = json!({ "jsonrpc": "2.0", "method": "notifications/progress" });
        assert_eq!(answer(&notified), None);
    }

    #[test]
    fn the_tools_of_a_page_are_read() {
        let page = json!({ "tools": [
            { "name": "search", "title": "Search", "description": "Finds things",
              "inputSchema": { "type": "object", "properties": { "query": { "type": "string" } } } },
            { "name": "now" },
            { "description": "has no name" },
            "nothing",
        ], "nextCursor": "more" });
        let found = tools("web", &page);
        assert_eq!(names(&found), ["search", "now"]);
        assert_eq!(found[0].server, "web");
        assert_eq!(found[0].description, "Finds things");
        assert_eq!(found[0].schema["properties"]["query"]["type"], "string");
        // One that says nothing of what it takes, takes nothing.
        assert_eq!(found[1].description, "");
        assert_eq!(found[1].schema, json!({ "type": "object" }));
        assert!(tools("web", &Value::Null).is_empty());
    }

    #[test]
    fn what_a_tool_returns_is_read_block_by_block() {
        let text = |text: &str| json!({ "type": "text", "text": text });
        let of = |content: Value| told(&json!({ "content": content }));
        assert_eq!(of(json!([text("one"), text("two")])), Ok("one\ntwo".into()));
        let image = json!({ "type": "image", "data": "AAAA", "mimeType": "image/png" });
        assert_eq!(of(json!([image])), Ok("[image image/png]".into()));
        let audio = json!({ "type": "audio", "data": "AAAA", "mimeType": "audio/wav" });
        assert_eq!(of(json!([audio])), Ok("[audio audio/wav]".into()));
        let resource = json!({ "type": "resource", "resource": {
            "uri": "file:///a.rs", "mimeType": "text/x-rust", "text": "fn main() {}",
        } });
        assert_eq!(of(json!([resource])), Ok("[resource file:///a.rs]".into()));
        let link = json!({ "type": "resource_link", "uri": "file:///b.rs", "name": "b.rs" });
        assert_eq!(
            of(json!([text("see"), link, { "type": "hologram" }, {}])),
            Ok("see\n[resource_link file:///b.rs]\n[hologram]\n[content]".into())
        );
        // Nothing, or data alone.
        assert_eq!(of(json!([])), Ok(String::new()));
        assert_eq!(told(&Value::Null), Ok(String::new()));
        assert_eq!(
            told(&json!({ "content": [], "structuredContent": { "degrees": 21 } })),
            Ok(r#"{"degrees":21}"#.into())
        );
    }

    #[test]
    fn a_tool_that_says_it_failed_is_an_error_with_what_it_said() {
        let failed =
            json!({ "content": [{ "type": "text", "text": "No such city" }], "isError": true });
        assert_eq!(told(&failed), Err("No such city".into()));
        assert_eq!(
            told(&json!({ "isError": true })),
            Err("The tool failed.".into())
        );
        let fine = json!({ "content": [{ "type": "text", "text": "ok" }], "isError": false });
        assert_eq!(told(&fine), Ok("ok".into()));
    }

    #[test]
    fn messages_are_read_line_by_line_and_what_is_no_message_is_skipped() {
        let stream = "{\"id\":1,\"a\":\"é\"}\r\nDebugger listening\n\n\u{feff}{}\n{\"id\":2}";
        let mut reader = stream.as_bytes();
        assert_eq!(
            read_message(&mut reader),
            Some(json!({ "id": 1, "a": "é" }))
        );
        assert_eq!(read_message(&mut reader), Some(json!({ "id": 2 })));
        assert_eq!(read_message(&mut reader), None);
        // Bytes that are no text do not stop the reading.
        let mut reader: &[u8] = b"\xff\xfe\n{}\n";
        assert_eq!(read_message(&mut reader), Some(json!({})));
    }

    #[test]
    fn only_the_last_words_of_a_server_are_kept() {
        let kept = Mutex::new(Vec::new());
        let said = format!("{}the end", "x".repeat(3 * KEPT));
        keep(said.as_bytes(), &kept);
        let kept = kept.into_inner().expect("kept");
        assert_eq!(kept.len(), KEPT);
        assert!(kept.ends_with(b"the end"));
    }

    #[test]
    fn a_server_is_started_asked_for_its_tools_and_has_them_run() {
        let Some(file) = script("fake", FAKE) else {
            return;
        };
        let woken = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&woken);
        let wake = move || {
            count.fetch_add(1, Ordering::Relaxed);
        };
        let config = config("node", &[&file.to_string_lossy()]);
        let mut server = Server::start(&config, &std::env::temp_dir(), wake).expect("started");
        assert_eq!(server.name(), "fake");
        // Asked before the server is ready: kept, and said once it listens.
        let early = server.call("echo", json!({ "text": "early" }));

        let seen = until(&mut server, |event| matches!(event, Event::Ready { .. }));
        assert_eq!(answer_to(&seen, early), Some(Ok("early".to_owned())));
        let Some(Event::Ready {
            server: from,
            tools,
        }) = seen.last()
        else {
            panic!("not ready: {seen:?}");
        };
        // Both pages of its list, in order; the lines that were no answers did no harm.
        assert_eq!(from, "fake");
        assert_eq!(names(tools), ["echo", "fail", "picture", "bare"]);
        assert!(tools.iter().all(|tool| tool.server == "fake"));
        assert_eq!(tools[0].description, "Does echo");
        assert_eq!(
            tools[0].schema,
            json!({ "type": "object", "title": "echo" })
        );
        assert!(
            !seen
                .iter()
                .any(|event| matches!(event, Event::Failed { .. }))
        );
        assert!(woken.load(Ordering::Relaxed) > 0);

        let text = "héllo ✓ 😀\nsecond line";
        assert_eq!(
            called(&mut server, "echo", json!({ "text": text })),
            Ok(text.to_owned())
        );
        assert_eq!(
            called(&mut server, "fail", Value::Null),
            Err("it broke".to_owned())
        );
        assert_eq!(
            called(&mut server, "picture", json!({})),
            Ok("[image image/png]".to_owned())
        );
        // A tool the server does not have: its refusal is the error.
        assert_eq!(
            called(&mut server, "nope", json!({})),
            Err("Unknown tool: nope".to_owned())
        );
        // Its environment is the one of the editor, and what the user added.
        assert_eq!(called(&mut server, "env", json!({})), Ok("lent".to_owned()));
        // What the server asks: a ping is answered, the rest is refused.
        let ask =
            |server: &mut Server, method: &str| called(server, "ask", json!({ "method": method }));
        assert_eq!(ask(&mut server, "ping"), Ok("{}".to_owned()));
        for method in ["roots/list", "sampling/createMessage", "elicitation/create"] {
            assert_eq!(ask(&mut server, method), Ok("-32601".to_owned()));
        }

        // Its tools change: the new list is asked for, and told.
        server.call("change", json!({}));
        let changed = |event: &Event| matches!(event, Event::ToolsChanged { .. });
        let seen = until(&mut server, changed);
        let Some(Event::ToolsChanged { tools, .. }) = seen.iter().find(|event| changed(event))
        else {
            panic!("no new list: {seen:?}");
        };
        assert_eq!(names(tools), ["echo", "fail", "picture", "bare", "new"]);

        // It leaves while a tool runs: the call fails with its last words, then it is gone.
        let last = server.call("quit", json!({}));
        let exited = |event: &Event| matches!(event, Event::Exited { .. });
        let seen = until(&mut server, exited);
        let why = answer_to(&seen, last)
            .expect("an answer")
            .expect_err("gone");
        assert!(why.starts_with("The server stopped: "), "{why}");
        assert!(
            why.contains("fake: listening") && why.contains("fake: leaving"),
            "{why}"
        );
        assert_eq!(
            seen.last(),
            Some(&Event::Exited {
                server: "fake".to_owned()
            })
        );
        assert!(
            !seen
                .iter()
                .any(|event| matches!(event, Event::Failed { .. }))
        );
        // Said once; what is asked of it now fails at once.
        let late = server.call("echo", json!({ "text": "late" }));
        let seen = server.poll();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(matches!(answer_to(&seen, late), Some(Err(_))));
        assert!(server.poll().is_empty());
        drop(server);
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn a_program_that_leaves_at_once_has_failed_and_its_complaint_is_the_reason() {
        let leaving = "process.stderr.write('fatal: bad key ' + process.env.GODCODE_MCP_KEY + '\\n');\nprocess.exitCode = 3;\n";
        let Some(file) = script("leaving", leaving) else {
            return;
        };
        let mut config = config("node", &[&file.to_string_lossy()]);
        let key = ("GODCODE_MCP_KEY".to_owned(), "sk-s3cret-token".to_owned());
        config.env.push(key);
        let mut server = Server::start(&config, &std::env::temp_dir(), || {}).expect("started");
        let call = server.call("echo", json!({}));
        let seen = until(&mut server, |event| matches!(event, Event::Exited { .. }));
        // Why, with the token it printed taken out; then what was asked; then that it is gone.
        let why = "The server stopped: fatal: bad key ***".to_owned();
        let server_name = || "fake".to_owned();
        assert_eq!(
            seen,
            [
                Event::Failed {
                    server: server_name(),
                    why: why.clone()
                },
                Event::Called {
                    id: call,
                    result: Err(why)
                },
                Event::Exited {
                    server: server_name()
                },
            ]
        );
        drop(server);
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn a_server_that_refuses_the_handshake_has_failed() {
        let refusing = r#"
require('readline').createInterface({ input: process.stdin }).on('line', (line) => {
  const { id } = JSON.parse(line);
  const error = { code: -32602, message: 'Unsupported protocol version' };
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id, error }) + '\n');
});
"#;
        let Some(file) = script("refusing", refusing) else {
            return;
        };
        let mut server = start(&file);
        let call = server.call("echo", json!({}));
        let seen = until(&mut server, |event| matches!(event, Event::Exited { .. }));
        let why = "Unsupported protocol version".to_owned();
        assert_eq!(
            seen[..2],
            [
                Event::Failed {
                    server: "fake".to_owned(),
                    why: why.clone()
                },
                Event::Called {
                    id: call,
                    result: Err(why)
                },
            ]
        );
        // Its input closed, it left by itself; it failed once, not twice.
        assert_eq!(seen.len(), 3, "{seen:?}");
        drop(server);
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn a_server_that_is_dropped_is_ended_whether_it_leaves_by_itself_or_not() {
        let Some(polite) = script("polite", FAKE) else {
            return;
        };
        let mut server = start(&polite);
        until(&mut server, |event| matches!(event, Event::Ready { .. }));
        let pid = server.child.id();
        assert!(alive(pid));
        drop(server);
        assert!(gone(pid), "the server outlived its client");

        // One that never reads its input does not see it close.
        let stubborn = script("stubborn", "setInterval(() => {}, 1000);\n").expect("script");
        let server = start(&stubborn);
        let pid = server.child.id();
        assert!(alive(pid));
        let asked = Instant::now();
        drop(server);
        assert!(gone(pid), "the server outlived its client");
        assert!(asked.elapsed() < Duration::from_secs(20));
        for file in [polite, stubborn] {
            let _ = std::fs::remove_file(file);
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_command_that_is_a_cmd_file_is_found_and_what_it_started_ends_with_it() {
        let Some(file) = script("wrapped", FAKE) else {
            return;
        };
        // As npm installs a program: a `.cmd` file, named without its extension.
        let wrapper = file.with_extension("cmd");
        let name = file.file_name().expect("name").to_string_lossy();
        std::fs::write(
            &wrapper,
            format!("@echo off\r\nnode \"%~dp0{name}\" %*\r\n"),
        )
        .expect("wrapper");
        let command = file.with_extension("");
        let config = config(&command.to_string_lossy(), &[]);
        let mut server = Server::start(&config, &std::env::temp_dir(), || {}).expect("started");
        until(&mut server, |event| matches!(event, Event::Ready { .. }));
        // The server is not the program started, but one that program started.
        let shell = server.child.id();
        let node: u32 = called(&mut server, "pid", json!({}))
            .expect("its number")
            .parse()
            .expect("a number");
        assert_ne!(shell, node);
        assert!(alive(node));
        drop(server);
        assert!(gone(node) && gone(shell), "the server outlived its client");
        for file in [file, wrapper] {
            let _ = std::fs::remove_file(file);
        }
    }
}
