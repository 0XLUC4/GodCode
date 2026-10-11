//! Tools lent by a Model Context Protocol server: a program talked to over its pipes, or
//! a server reached over HTTP.
//!
//! JSON-RPC messages either way: one a line with a program, one a `POST` with a remote
//! server (the Streamable HTTP transport, asked through curl as the assistant is). Nothing
//! here blocks: messages are written and read on threads of their own, and what the server
//! said is collected with [`Server::poll`]. What a tool returns is data: it is handed over
//! as text, never read into here.
//!
//! A remote server is only heard when it answers: the stream on which it could speak first
//! is not opened, so the tools it adds later are not told before it is started again. It
//! is given a token in its `headers`; signing in through a browser (OAuth) is not done.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
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
/// How long a remote server is given to answer while hands are shaken, in seconds.
const HANDSHAKE: u32 = 30;
/// How long a remote server is given to run a tool, in seconds.
const PATIENCE: u32 = 120;
/// How long telling a remote server goodbye may take, in seconds. Nobody waits for it.
const FAREWELL: u32 = 5;

/// A server to start, as the user wrote it down.
#[derive(Clone, PartialEq)]
pub struct Config {
    pub name: String,
    /// The program to start. Empty for a server reached at a `url`.
    pub command: String,
    pub args: Vec<String>,
    /// Added to the environment of the editor. The values are often tokens.
    pub env: Vec<(String, String)>,
    /// Where a server reached over HTTP listens, instead of a program to start.
    pub url: Option<String>,
    /// Sent with every request to the `url`. The values are often tokens.
    pub headers: Vec<(String, String)>,
}

/// The names of `pairs`, without their values.
fn keys(pairs: &[(String, String)]) -> Vec<&str> {
    pairs.iter().map(|(name, _)| name.as_str()).collect()
}

/// Printed without its arguments, the values of its environment and of its headers, and
/// what follows the path of its address: they hold tokens.
impl std::fmt::Debug for Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Config")
            .field("name", &self.name)
            .field("command", &self.command)
            .field("args", &self.args.len())
            .field("env", &keys(&self.env))
            .field("url", &self.url.as_deref().map(shown))
            .field("headers", &keys(&self.headers))
            .finish()
    }
}

/// The parts of the address of a remote server.
struct Address<'a> {
    /// Asked over `https`, not `http`.
    secure: bool,
    /// A name and a password written before the host, if any.
    login: &'a str,
    /// The host, with its port when one is written.
    host: &'a str,
    path: &'a str,
    /// What follows the `?`, often a key.
    query: &'a str,
}

/// Read `url` as the address of a remote server. `None` for what is not one, and for what
/// two programs could read in two ways: a token is not sent where that is not sure.
fn address(url: &str) -> Option<Address<'_>> {
    let odd = |ch: char| ch.is_whitespace() || ch.is_control() || ch == '\\';
    let (scheme, rest) = url.split_once("://")?;
    let secure = match scheme.to_ascii_lowercase().as_str() {
        "https" => true,
        "http" => false,
        _ => return None,
    };
    let (authority, rest) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
    let (login, host) = authority.rsplit_once('@').unwrap_or(("", authority));
    let rest = rest.split('#').next().unwrap_or(rest);
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let fine = !url.contains(odd) && !host.is_empty() && !login.contains('@');
    fine.then_some(Address {
        secure,
        login,
        host,
        path,
        query,
    })
}

/// `url` as it can be shown: without the name and the password before its host, and
/// without what follows its path.
fn shown(url: &str) -> String {
    match address(url) {
        Some(address) => {
            let scheme = if address.secure { "https" } else { "http" };
            format!("{scheme}://{}{}", address.host, address.path)
        }
        None => "an address that cannot be read".to_owned(),
    }
}

/// `host`, written with its port or without, is this computer.
fn loopback(host: &str) -> bool {
    if let Some(rest) = host.strip_prefix('[') {
        let (ip, port) = rest.split_once(']').unwrap_or((rest, "?"));
        let local = ip.parse::<Ipv6Addr>().is_ok_and(|ip| ip.is_loopback());
        return local && (port.is_empty() || port.starts_with(':'));
    }
    let name = host.rsplit_once(':').map_or(host, |(name, _)| name);
    let local = name.parse::<Ipv4Addr>().is_ok_and(|ip| ip.is_loopback());
    local || name.eq_ignore_ascii_case("localhost")
}

/// The address of the remote server `name` when a token may be sent there, or why not.
/// Over `http`, only a server on this computer is asked.
fn reachable<'a>(name: &str, url: &'a str) -> Result<Address<'a>, String> {
    let Some(address) = address(url) else {
        return Err(format!(
            "The address of {name} is not one of a server: it starts with https://"
        ));
    };
    if !address.secure && !loopback(address.host) {
        return Err(format!(
            "{name} is not asked at {}: over http, what is sent would travel in the clear, \
             tokens and all. Write https://, or run the server on this computer.",
            shown(url)
        ));
    }
    Ok(address)
}

/// A value of a settings file as the text a program is given: `3000` for a number.
fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The names and the values of an object of a settings file, such as an environment.
fn pairs(object: &Value) -> Vec<(String, String)> {
    let entries = object.as_object().into_iter().flatten();
    entries
        .map(|(key, value)| (key.clone(), plain(value)))
        .collect()
}

/// Read the servers of a file written like Claude Desktop's, sorted by name: programs to
/// start, `{"mcpServers": {"name": {"command": "...", "args": [...], "env": {...}}}}`, and
/// servers reached over HTTP, `{"name": {"url": "https://...", "headers": {...}}}`, whose
/// `type` is not looked at. Entries that are neither are left out.
pub fn configs(json: &str) -> Result<Vec<Config>, String> {
    let file: Value = serde_json::from_str(json).map_err(|err| format!("Not JSON: {err}"))?;
    let entries = file["mcpServers"].as_object().into_iter().flatten();
    let mut configs: Vec<Config> = entries
        .filter_map(|(name, entry)| {
            let written = |key: &str| entry[key].as_str().filter(|it| !it.is_empty());
            let command = written("command");
            // A program to start is started, whatever else its entry says.
            let url = written("url").filter(|_| command.is_none());
            let args = entry["args"].as_array().into_iter().flatten();
            Some(Config {
                name: name.clone(),
                command: command.or(url.map(|_| ""))?.to_owned(),
                args: args.map(plain).collect(),
                env: pairs(&entry["env"]),
                url: url.map(str::to_owned),
                headers: pairs(&entry["headers"]),
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

/// What the threads that talk to a server hand over.
enum Heard {
    /// Something the server said.
    Message(Value),
    /// The server can no longer be asked, and why.
    Lost(String),
}

/// How a server is talked to.
enum Link {
    /// A program started here, and the end of what it wrote on its error stream.
    Program {
        child: Child,
        complaints: Arc<Mutex<Vec<u8>>>,
    },
    /// A server asked over HTTP, and the session it opened.
    Remote {
        remote: Remote,
        session: Arc<Mutex<Session>>,
    },
}

/// Lock `mutex`, whatever became of the thread that held it last.
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Start the program of `config` in `folder`, with the threads that write what is
/// `to_write` to it and hand over what is `heard` from it.
fn run(
    config: &Config,
    folder: &Path,
    to_write: Receiver<Value>,
    heard: Sender<Heard>,
    wake: impl Fn() + Send + 'static,
) -> Result<Link, String> {
    let mut child = spawn(config, folder)?;
    let streams = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let (Some(mut stdin), Some(stdout), Some(stderr)) = streams else {
        end(&mut child);
        return Err(format!("Cannot talk to {}", config.command));
    };
    std::thread::spawn(move || {
        for message in to_write {
            let line = format!("{message}\n");
            if stdin.write_all(line.as_bytes()).is_err() || stdin.flush().is_err() {
                break;
            }
        }
    });
    let complaints = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&complaints);
    let complaining = std::thread::spawn(move || keep(stderr, &kept));
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        while let Some(message) = read_message(&mut reader) {
            if heard.send(Heard::Message(message)).is_err() {
                return;
            }
            wake();
        }
        // Its last words are waited for, then the sender is dropped: the next poll
        // sees that the program is gone, and can say why.
        let _ = complaining.join();
        drop(heard);
        wake();
    });
    Ok(Link::Program { child, complaints })
}

/// A server asked over HTTP: where, and what the user has every request carry.
#[derive(Clone)]
struct Remote {
    url: String,
    headers: Vec<(String, String)>,
}

/// What a remote server is told with every request once hands are shaken.
#[derive(Clone, Default)]
struct Session {
    /// The name the server gave to this talk, if it gave one. Not to be shown.
    id: Option<String>,
    /// The revision of the protocol that was agreed on.
    protocol: Option<String>,
}

/// `text` between quotes, as a file of options of curl writes a value.
fn quoted(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

/// What curl reads on its input to post `body` to `remote`, or, without one, to tell it
/// that the session is over. curl prints the headers of the answer, the answer, then a
/// line of its own: the HTTP status and how many bytes the headers took.
fn curl_config(remote: &Remote, session: &Session, body: Option<&str>, patience: u32) -> String {
    let mut headers = vec![("Accept", "application/json, text/event-stream")];
    if body.is_some() {
        headers.push(("Content-Type", "application/json"));
    }
    let asked = remote.headers.iter();
    headers.extend(asked.map(|(name, value)| (name.as_str(), value.as_str())));
    if let Some(id) = &session.id {
        headers.push(("Mcp-Session-Id", id));
    }
    if let Some(protocol) = &session.protocol {
        headers.push(("MCP-Protocol-Version", protocol));
    }
    // `globoff`: braces and brackets in an address are part of it, not a list of addresses.
    let mut config = format!("globoff\nurl = {}\n", quoted(&remote.url));
    for (name, value) in headers {
        let header = quoted(&format!("{name}: {value}"));
        config.push_str(&format!("header = {header}\n"));
    }
    match body {
        // `data-raw`: a body starting with `@` is not the name of a file to send.
        Some(body) => config.push_str(&format!("data-raw = {}\n", quoted(body))),
        None => config.push_str("request = \"DELETE\"\n"),
    }
    config.push_str(&format!("max-time = {patience}\n"));
    config.push_str("dump-header = \"-\"\nwrite-out = \"\\n%{http_code} %{size_header}\"\n");
    config
}

/// curl, about to read on its input what it has to do. Nothing of the request is here,
/// and a redirection is not followed: a token goes nowhere but where the user sent it.
fn curl() -> Command {
    let mut command = Command::new("curl");
    command
        // `-q`: the options the user keeps for curl are not ours.
        .args(["-q", "-sS", "--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::tools::hidden(&mut command);
    command
}

/// What a server answered over HTTP.
struct Reply {
    status: u16,
    /// Its `Content-Type`, in lower case.
    kind: String,
    /// The `Mcp-Session-Id` it gave, if any.
    session: Option<String>,
    body: String,
}

/// Read what curl `printed`: the headers, the answer, then the line of curl. The headers
/// are as many bytes as that line says, so nothing in an answer is taken for one.
fn reply(printed: &[u8]) -> Option<Reply> {
    let cut = printed.iter().rposition(|byte| *byte == b'\n')?;
    let line = std::str::from_utf8(&printed[cut + 1..]).ok()?;
    let (status, size) = line.split_once(' ')?;
    let (head, body) = printed[..cut].split_at_checked(size.parse().ok()?)?;
    let head = String::from_utf8_lossy(head).replace("\r\n", "\n");
    // Headers start with a line of status and end with an empty one.
    if !head.starts_with("HTTP/") || !head.ends_with("\n\n") {
        return None;
    }
    // The last ones are those of the answer: a server may first say that it goes on.
    let last = head.trim_end().rsplit("\n\n").next()?;
    let header = |name: &str| {
        let mut headers = last.lines().skip(1).filter_map(|line| line.split_once(':'));
        let found = headers.find(|(key, _)| key.trim().eq_ignore_ascii_case(name));
        found.map(|(_, value)| value.trim().to_owned())
    };
    Some(Reply {
        status: status.parse().ok()?,
        kind: header("content-type")
            .unwrap_or_default()
            .to_ascii_lowercase(),
        session: header("mcp-session-id").filter(|id| !id.is_empty()),
        body: String::from_utf8_lossy(body).into_owned(),
    })
}

/// Why curl failed, in words: `code` is what it exited with, `complaint` what it printed.
fn failure(code: Option<i32>, complaint: &str, patience: u32) -> String {
    match code {
        Some(6) => "Cannot find the server: check its address, and the connection.".to_owned(),
        Some(7) => "Cannot reach the server: nothing answers at its address.".to_owned(),
        Some(28) => format!("The server did not answer in {patience} seconds."),
        _ => {
            let complaint = complaint.lines().next().unwrap_or_default().trim();
            match complaint.is_empty() {
                true => "Cannot reach the server: curl failed.".to_owned(),
                false => complaint.to_owned(),
            }
        }
    }
}

/// Run curl on `config` and return what the server answered, whatever its status.
fn fetch(config: &str, patience: u32) -> Result<Reply, String> {
    let cannot = |err: std::io::Error| format!("Cannot run curl: {err}");
    let mut child = curl().spawn().map_err(cannot)?;
    // curl reads all of its input before it prints anything: writing cannot block on it.
    // If it left early, the reason is in what it printed.
    if let Some(mut input) = child.stdin.take() {
        let _ = input.write_all(config.as_bytes());
    }
    let output = child.wait_with_output().map_err(cannot)?;
    if !output.status.success() {
        let complaint = String::from_utf8_lossy(&output.stderr);
        return Err(failure(output.status.code(), &complaint, patience));
    }
    reply(&output.stdout).ok_or_else(|| "The server did not answer over HTTP.".to_owned())
}

/// The messages of a stream of events: each is the `data:` lines of an event put
/// together, an event ending with an empty line. Comments, the names and the ids of
/// events are skipped, and so is what is not JSON, such as an event cut short.
fn events(stream: &str) -> Vec<Value> {
    let mut found = Vec::new();
    let mut data: Vec<&str> = Vec::new();
    let lines = stream.trim_start_matches('\u{feff}').lines();
    // One more empty line: a stream may end without the one that ends its last event.
    for line in lines.chain([""]) {
        if line.is_empty() {
            found.extend(serde_json::from_str::<Value>(&data.join("\n")));
            data.clear();
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    found
}

/// The messages in what a server answered to one: none when it only took note, one, or
/// those of a stream. An error, in words, when it refused or answered something else.
fn messages(reply: &Reply) -> Result<Vec<Value>, String> {
    let status = reply.status;
    if matches!(status, 401 | 403) {
        return Err(format!(
            "The server refused the credentials (HTTP {status})."
        ));
    }
    let json = serde_json::from_str::<Value>(&reply.body).ok();
    if !(200..300).contains(&status) {
        let said = json.as_ref().and_then(|it| it["error"]["message"].as_str());
        return Err(match said {
            Some(said) => format!("The server answered HTTP {status}: {said}"),
            None => format!("The server answered HTTP {status}."),
        });
    }
    if reply.body.trim().is_empty() {
        return Ok(Vec::new());
    }
    if reply.kind.starts_with("text/event-stream") {
        return Ok(events(&reply.body));
    }
    match json {
        Some(Value::Array(several)) => Ok(several),
        Some(message @ Value::Object(_)) => Ok(vec![message]),
        _ => Err("The server did not answer with JSON.".to_owned()),
    }
}

/// The answer to the request `id` when the server gave none: why, as it would refuse.
fn refusal(id: &Value, why: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": why } })
}

/// Post to `remote` each message there is `to_send`, one at a time, and hand over what it
/// answers. Whatever is asked gets an answer: the reason, when the server gave none.
fn talk(
    remote: &Remote,
    session: &Mutex<Session>,
    to_send: Receiver<Value>,
    heard: &Sender<Heard>,
    wake: &impl Fn(),
) {
    for message in to_send {
        let method = message["method"].as_str();
        let shaking = ["initialize", "notifications/initialized", "tools/list"];
        let shaking = method.is_some_and(|method| shaking.contains(&method));
        let patience = if shaking { HANDSHAKE } else { PATIENCE };
        let known = locked(session).clone();
        let config = curl_config(remote, &known, Some(&message.to_string()), patience);
        let answered = fetch(&config, patience);
        // The server no longer knows the session: nothing more can be asked in it.
        if known.id.is_some() && answered.as_ref().is_ok_and(|reply| reply.status == 404) {
            locked(session).id = None;
            let _ = heard.send(Heard::Lost("The server ended the session.".to_owned()));
            return;
        }
        let said = answered.and_then(|reply| {
            let said = messages(&reply)?;
            let mut results = said.iter().map(|message| &message["result"]);
            let shaken = |result: &&Value| method == Some("initialize") && !result.is_null();
            if let Some(agreed) = results.find(shaken) {
                let protocol = agreed["protocolVersion"].as_str().unwrap_or(PROTOCOL);
                let protocol = Some(protocol.to_owned());
                *locked(session) = Session {
                    id: reply.session,
                    protocol,
                };
            }
            Ok(said)
        });
        // Only a request waits for an answer: a note that did not arrive is nobody's.
        let asked = method.and_then(|_| message.get("id"));
        let mut said = said.unwrap_or_else(|why| {
            let refused = asked.map(|id| refusal(id, &why));
            refused.into_iter().collect()
        });
        let answers = |said: &Value| said.get("method").is_none() && said.get("id") == asked;
        if let Some(id) = asked.filter(|_| !said.iter().any(answers)) {
            said.push(refusal(id, "The server did not answer."));
        }
        for message in said {
            if heard.send(Heard::Message(message)).is_err() {
                return;
            }
        }
        wake();
    }
}

/// Have the remote server of `config`, at `url`, talked to on a thread of its own. With
/// the link come the texts never to be shown: what its headers and its address hold.
fn reach(
    config: &Config,
    url: &str,
    to_send: Receiver<Value>,
    heard: Sender<Heard>,
    wake: impl Fn() + Send + 'static,
) -> Result<(Link, Vec<String>), String> {
    let address = reachable(&config.name, url)?;
    let one_line = |text: &str| !text.contains(|ch: char| ch.is_control());
    let written = |(name, value): &(String, String)| {
        !name.is_empty() && !name.contains(':') && one_line(name) && one_line(value)
    };
    if !config.headers.iter().all(written) {
        return Err(format!(
            "A header of {} is not a name and a value on one line.",
            config.name
        ));
    }
    // A token is often the last word of a header, and the value of a key of the address.
    let mut secrets = vec![address.login, address.query];
    secrets.extend(address.login.rsplit(':').next());
    let keys = address.query.split('&');
    secrets.extend(keys.filter_map(|key| Some(key.split_once('=')?.1)));
    for (_, value) in &config.headers {
        secrets.push(value.as_str());
        secrets.extend(value.split_whitespace().last());
    }
    let remote = Remote {
        url: url.to_owned(),
        headers: config.headers.clone(),
    };
    let session = Arc::new(Mutex::new(Session::default()));
    let (asked, known) = (remote.clone(), Arc::clone(&session));
    std::thread::spawn(move || {
        talk(&asked, &known, to_send, &heard, &wake);
        // The sender is dropped: the next poll sees that nothing more will be heard.
        drop(heard);
        wake();
    });
    let secrets = secrets.into_iter().map(str::to_owned).collect();
    Ok((Link::Remote { remote, session }, secrets))
}

/// Tell a remote server that its `session` is over. What it answers is not waited for.
fn farewell(remote: &Remote, session: &Session) {
    let mut command = curl();
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return;
    };
    if let Some(mut input) = child.stdin.take() {
        let config = curl_config(remote, session, None, FAREWELL);
        let _ = input.write_all(config.as_bytes());
    }
    // Waited for on a thread of its own, for it not to linger once it is done.
    std::thread::spawn(move || child.wait());
}

/// A running Model Context Protocol server.
pub struct Server {
    name: String,
    link: Link,
    /// Messages on their way to the server, sent by a thread of their own. Nothing once
    /// it is no longer talked to.
    outgoing: Option<Sender<Value>>,
    incoming: Receiver<Heard>,
    /// What was lent to the server and is never to be shown: the values of its
    /// environment, of its headers, and what its address holds. The longest first.
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
    /// Start the program, or reach the server at the `url` of `config`, and shake hands;
    /// [`Event::Ready`] or [`Event::Failed`] follows. `wake` is called, from another
    /// thread, whenever there is something to [`Self::poll`].
    pub fn start(
        config: &Config,
        folder: &Path,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let (outgoing, to_send) = channel();
        let (heard, incoming) = channel();
        let mut secrets: Vec<String> = config.env.iter().map(|(_, value)| value.clone()).collect();
        let link = match &config.url {
            Some(url) => {
                let (link, held) = reach(config, url, to_send, heard, wake)?;
                secrets.extend(held);
                link
            }
            None => run(config, folder, to_send, heard, wake)?,
        };
        secrets.retain(|value| value.len() >= SECRET_LENGTH);
        // The longest first: one may hold another, and is taken out whole.
        secrets.sort_by_key(|value| std::cmp::Reverse(value.len()));
        let mut server = Self {
            name: config.name.clone(),
            link,
            outgoing: Some(outgoing),
            incoming,
            secrets,
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
            let _ = outgoing.send(message.clone());
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

    /// `text` without the values lent to the server, should it have printed one, and
    /// without the name of its session.
    fn clean(&self, text: &str) -> String {
        let clean = |text: String, secret: &String| text.replace(secret.as_str(), "***");
        let text = self.secrets.iter().fold(text.trim().to_owned(), clean);
        let session = match &self.link {
            Link::Remote { session, .. } => locked(session).id.clone(),
            Link::Program { .. } => None,
        };
        session.iter().fold(text, clean)
    }

    /// The end of what a program wrote on its error stream, to say why it stopped.
    fn last_words(&self) -> String {
        match &self.link {
            Link::Program { complaints, .. } => {
                let kept = locked(complaints).clone();
                self.clean(&String::from_utf8_lossy(&kept))
            }
            Link::Remote { .. } => String::new(),
        }
    }

    /// Everything the server said since the last call.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        let mut gone = false;
        loop {
            match self.incoming.try_recv() {
                Ok(Heard::Message(message)) => self.handle(&message, &mut events),
                Ok(Heard::Lost(why)) if self.stopped.is_none() => {
                    self.fail(self.clean(&why), &mut events);
                }
                Ok(Heard::Lost(_)) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    gone = !std::mem::replace(&mut self.exited, true);
                    break;
                }
            }
        }
        if gone && self.stopped.is_none() {
            let said = self.last_words();
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
        let child = match &mut self.link {
            Link::Program { child, .. } => child,
            Link::Remote { remote, session } => {
                // A remote one is told that its session is over, if it opened one.
                let session = locked(session).clone();
                if session.id.is_some() {
                    farewell(remote, &session);
                }
                return;
            }
        };
        let deadline = Instant::now() + GRACE;
        let running = |child: &mut Child| matches!(child.try_wait(), Ok(None));
        while running(child) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if running(child) {
            end(child);
        }
        let _ = child.wait();
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
            url: None,
            headers: Vec::new(),
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

    /// The number of the program `server` is.
    fn pid_of(server: &Server) -> u32 {
        match &server.link {
            Link::Program { child, .. } => child.id(),
            Link::Remote { .. } => panic!("a remote server is no program"),
        }
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
        // The programs to start and the servers reached at a `url`, by name.
        assert_eq!(names, ["files", "github", "remote", "typed"]);
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
            url: None,
            headers: Vec::new(),
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
        let pid = pid_of(&server);
        assert!(alive(pid));
        drop(server);
        assert!(gone(pid), "the server outlived its client");

        // One that never reads its input does not see it close.
        let stubborn = script("stubborn", "setInterval(() => {}, 1000);\n").expect("script");
        let server = start(&stubborn);
        let pid = pid_of(&server);
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
        let shell = pid_of(&server);
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

    /// A small server reached over HTTP, for Node. It listens on a port of this computer
    /// that it prints, wants what the transport says a client sends, and notes in the
    /// file it is given what it could not answer with: goodbyes, notes, answers.
    const REMOTE_FAKE: &str = r#"
const http = require('http');
const fs = require('fs');
const note = (line) => fs.appendFileSync(process.argv[2], line + '\n');
const SESSION = 'sess-9f8e7d6c5b4a';
const VERSION = '2025-03-26';
const tool = (name) => ({ name, description: 'Does ' + name, inputSchema: { type: 'object' } });
const said = (text, isError) => ({ content: [{ type: 'text', text }], isError });
let open = false;
const server = http.createServer((req, res) => {
  let body = '';
  req.on('data', (chunk) => { body += chunk; });
  req.on('end', () => {
    const plain = (status, text) => { res.writeHead(status, { 'Content-Type': 'text/plain' }); res.end(text); };
    const header = (name) => String(req.headers[name]);
    if (req.url !== '/mcp?key=query-s3cret-9') { note('elsewhere ' + req.url); return plain(410, 'gone'); }
    if (header('authorization') !== 'Bearer s3cret-token-1') return plain(401, 'who are you');
    if (req.method === 'DELETE') {
      note('DELETE ' + header('mcp-session-id') + ' ' + header('mcp-protocol-version'));
      open = false;
      return plain(200, '');
    }
    if (req.method !== 'POST') return plain(405, 'no');
    const accept = header('accept');
    if (!accept.includes('application/json') || !accept.includes('text/event-stream')) return plain(406, 'no');
    if (header('content-type') !== 'application/json') return plain(415, 'no');
    const message = JSON.parse(body);
    const { id, method, params } = message;
    const json = (answer, headers, status) => {
      res.writeHead(status || 200, Object.assign({ 'Content-Type': 'application/json; charset=utf-8' }, headers));
      res.end(JSON.stringify(Object.assign({ jsonrpc: '2.0', id }, answer)));
    };
    const noted = () => { res.writeHead(202); res.end(); };
    const event = (message) => 'data: ' + JSON.stringify(message) + '\n\n';
    if (method === 'initialize') {
      if (req.headers['mcp-session-id'] !== undefined) return plain(400, 'a session already');
      open = true;
      const result = { protocolVersion: VERSION, capabilities: { tools: {} }, serverInfo: { name: 'far', version: '1' } };
      return json({ result }, { 'Mcp-Session-Id': SESSION });
    }
    if (req.headers['mcp-session-id'] === undefined) return plain(400, 'no session');
    if (!open || header('mcp-session-id') !== SESSION) return plain(404, 'no such session');
    if (header('mcp-protocol-version') !== VERSION) return plain(400, 'no version');
    if (method === undefined) { note('answer ' + id + ' ' + JSON.stringify(message.result)); return noted(); }
    if (method === 'notifications/initialized') { note('initialized'); return noted(); }
    if (method === 'tools/list') {
      if (!params.cursor) return json({ result: { tools: [tool('echo'), tool('stream')], nextCursor: 'second' } });
      if (params.cursor === 'second') return json({ result: { tools: [tool('fail')] } });
      return json({ error: { code: -32602, message: 'no such page' } });
    }
    const name = params.name;
    if (name === 'echo') return json({ result: said(params.arguments.text) });
    if (name === 'fail') return json({ result: said('it broke', true) });
    if (name === 'stream') {
      res.writeHead(200, { 'Content-Type': 'text/event-stream' });
      res.write(': a comment\r\n\r\n');
      res.write('event: message\r\nid: 1\r\n' + event({ jsonrpc: '2.0', method: 'notifications/progress', params: { progressToken: 1, progress: 1 } }));
      res.write(event({ jsonrpc: '2.0', id: 'ping-1', method: 'ping' }));
      // The answer, over three lines of data.
      return res.end('id: 2\ndata: {"jsonrpc":"2.0",\ndata: "id":' + id + ',\ndata:"result":' + JSON.stringify(said('streamed')) + '}\n\n');
    }
    if (name === 'silent') {
      res.writeHead(200, { 'Content-Type': 'text/event-stream' });
      return res.end(event({ jsonrpc: '2.0', method: 'notifications/message', params: { level: 'info', data: 'noise' } }));
    }
    if (name === 'secret') {
      const message = 'bad ' + header('authorization') + ' in ' + header('mcp-session-id') + ' at ' + req.url;
      return json({ error: { code: -32000, message } });
    }
    if (name === 'bad') return json({ error: { code: -32602, message: 'Bad arguments' } }, {}, 400);
    if (name === 'page') { res.writeHead(200, { 'Content-Type': 'text/html' }); return res.end('<html>Sign in</html>'); }
    if (name === 'moved') { res.writeHead(307, { Location: '/elsewhere' }); return res.end(); }
    if (name === 'kill-session') { open = false; return json({ result: said('killed') }); }
    return json({ error: { code: -32602, message: 'Unknown tool: ' + name } });
  });
});
server.listen(0, '127.0.0.1', () => console.log(server.address().port));
"#;

    /// The small remote server, running. Ended when dropped.
    struct Far {
        child: Child,
        port: u16,
        script: PathBuf,
        notes: PathBuf,
    }

    impl Far {
        /// Start it; `None`, with a note, when Node or curl is not installed.
        fn start(name: &str) -> Option<Self> {
            if Command::new("curl").arg("--version").output().is_err() {
                eprintln!("skipped: `curl` is not installed");
                return None;
            }
            let script = script(name, REMOTE_FAKE)?;
            let notes = script.with_extension("notes");
            std::fs::write(&notes, "").expect("notes");
            let mut node = Command::new("node");
            node.arg(&script).arg(&notes).stdout(Stdio::piped());
            let mut child = node.spawn().expect("node");
            let mut printed = BufReader::new(child.stdout.take().expect("its output"));
            let mut line = String::new();
            printed.read_line(&mut line).expect("a line");
            let port = line.trim().parse().expect("its port");
            Some(Self {
                child,
                port,
                script,
                notes,
            })
        }

        /// How the user would write it down, with `token` to be let in.
        fn config(&self, token: &str) -> Config {
            let url = format!("http://127.0.0.1:{}/mcp?key=query-s3cret-9", self.port);
            Config {
                name: "far".to_owned(),
                command: String::new(),
                args: Vec::new(),
                env: Vec::new(),
                url: Some(url),
                headers: vec![("Authorization".to_owned(), format!("Bearer {token}"))],
            }
        }

        fn notes(&self) -> String {
            std::fs::read_to_string(&self.notes).expect("notes")
        }

        /// Wait for it to note `line`; `false` when it never does.
        fn noted(&self, line: &str) -> bool {
            let deadline = Instant::now() + Duration::from_secs(15);
            while !self.notes().lines().any(|noted| noted == line) {
                if Instant::now() > deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            true
        }
    }

    impl Drop for Far {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            for file in [&self.script, &self.notes] {
                let _ = std::fs::remove_file(file);
            }
        }
    }

    fn remote(url: &str) -> Config {
        Config {
            name: "far".to_owned(),
            command: String::new(),
            args: Vec::new(),
            env: Vec::new(),
            url: Some(url.to_owned()),
            headers: Vec::new(),
        }
    }

    fn far() -> String {
        "far".to_owned()
    }

    #[test]
    fn the_servers_reached_at_a_url_are_read_with_their_headers() {
        let file = r#"{ "mcpServers": {
            "far": {
                "type": "http",
                "url": "https://example.com/mcp?key=abc",
                "headers": { "Authorization": "Bearer tok", "X-Count": 3 }
            },
            "old": { "type": "sse", "url": "http://localhost:8080/sse" },
            "bare": { "url": "https://example.com/" },
            "stream": { "type": "streamable-http", "url": "https://example.com/s", "headers": [] },
            "near": { "command": "npx", "args": ["x"], "url": "https://ignored.example", "headers": { "A": "b" } },
            "nowhere": { "url": "" },
            "odd": { "url": 3 }
        } }"#;
        let found = configs(file).expect("servers");
        let names: Vec<&str> = found.iter().map(|config| config.name.as_str()).collect();
        assert_eq!(names, ["bare", "far", "near", "old", "stream"]);
        // One reached at a `url` is no program: nothing to start, and no arguments.
        let bare = Config {
            name: "bare".to_owned(),
            ..remote("https://example.com/")
        };
        assert_eq!(found[0], bare);
        assert_eq!(found[1].command, "");
        assert_eq!(
            found[1].url.as_deref(),
            Some("https://example.com/mcp?key=abc")
        );
        let header = |name: &str, value: &str| (name.to_owned(), value.to_owned());
        assert_eq!(
            found[1].headers,
            [
                header("Authorization", "Bearer tok"),
                header("X-Count", "3")
            ]
        );
        // A program is started, whatever address its entry holds.
        assert_eq!((found[2].command.as_str(), &found[2].url), ("npx", &None));
        // The kind of transport written is not looked at.
        assert_eq!(found[3].url.as_deref(), Some("http://localhost:8080/sse"));
        assert_eq!(found[4].url.as_deref(), Some("https://example.com/s"));
        assert!(found[4].headers.is_empty());
    }

    #[test]
    fn only_a_server_on_this_computer_is_asked_over_plain_http() {
        let allowed = |url: &str| reachable("far", url).is_ok();
        for url in [
            "https://example.com/mcp",
            "HTTPS://example.com",
            "https://user:password@example.com:8443/mcp?key=1#part",
            "http://localhost/mcp",
            "http://LocalHost:3000",
            "http://127.0.0.1:8080/mcp?key=1",
            "http://127.8.9.1",
            "http://user@127.0.0.1/",
            "http://[::1]:8080/mcp",
            "http://[::1]",
        ] {
            assert!(allowed(url), "{url}");
        }
        for url in [
            "http://example.com/mcp",
            "http://192.168.1.4:8080/",
            "http://0.0.0.0/",
            "http://localhost.example.com/",
            "http://127.0.0.1.example.com/",
            "http://localhost@example.com/",
            "http://127.0.0.1:80@example.com/",
            "http://example.com/?http://localhost/",
            "http://example.com#@localhost/",
            "http://[::2]/",
            "http://[::1]example.com/",
            "http://[::1",
            // What is no address, or one that programs read in more than one way.
            "example.com/mcp",
            "ftp://localhost/",
            "file:///etc/passwd",
            "http://",
            "https:///mcp",
            "http://local host/",
            "http://localhost\\@example.com/",
            "http://a@b@localhost/",
            "https://example.com/\nmcp",
        ] {
            assert!(!allowed(url), "{url}");
        }
        // Why is said with the address as it can be shown, or without it.
        let Err(why) = reachable("far", "http://me:pass-w0rd@example.com/mcp?key=s3cret-key")
        else {
            panic!("allowed");
        };
        assert!(
            why.starts_with("far is not asked at http://example.com/mcp: "),
            "{why}"
        );
        assert!(!why.contains("s3cret") && !why.contains("w0rd"), "{why}");
        let Err(why) = reachable("far", "s3cret-key://nonsense") else {
            panic!("allowed");
        };
        assert!(
            why.starts_with("The address of far ") && !why.contains("s3cret"),
            "{why}"
        );
    }

    #[test]
    fn the_tokens_of_a_remote_server_are_never_printed_with_it() {
        let mut config = remote("https://me:pass-s3cret@example.com:8443/v1/mcp?key=s3cret#s3cret");
        config.headers = vec![("Authorization".to_owned(), "Bearer s3cret".to_owned())];
        let printed = format!("{config:?}");
        assert!(
            printed.contains("https://example.com:8443/v1/mcp"),
            "{printed}"
        );
        assert!(printed.contains("Authorization"), "{printed}");
        assert!(
            !printed.contains("s3cret") && !printed.contains('@'),
            "{printed}"
        );
        // An address that cannot be read is not printed at all.
        let printed = format!("{:?}", remote("s3cret nonsense"));
        assert!(!printed.contains("s3cret"), "{printed}");
        assert_eq!(shown("http://LOCALHOST/a/b?c#d"), "http://LOCALHOST/a/b");
        assert_eq!(shown("https://example.com"), "https://example.com");
    }

    #[test]
    fn a_stream_of_events_is_read_message_by_message() {
        let stream = concat!(
            "\u{feff}: a comment\r\n",
            "event: message\r\nid: 7\r\nretry: 10\r\n",
            // One message over two lines of data, the space after the colon or not.
            "data: {\"id\":1,\r\ndata:\"a\":\"é: b\"}\r\n\r\n",
            ": keep-alive\n\n",
            "data:{\"id\":2}\n\n\n",
            "event: other\ndata: not JSON\n\n",
            "data\ndatum: {\"id\":9}\n\n",
            "data: {\"id\":3}\n\n",
            // Cut short: the stream ended in the middle of it.
            "data: {\"id\":4,\"cut\":",
        );
        assert_eq!(
            events(stream),
            [
                json!({ "id": 1, "a": "é: b" }),
                json!({ "id": 2 }),
                json!({ "id": 3 })
            ]
        );
        // The last event is read though the empty line that ends it is missing.
        assert_eq!(events("data: {\"id\":5}"), [json!({ "id": 5 })]);
        assert!(events("").is_empty());
        assert!(events(": nothing\n\n").is_empty());
    }

    #[test]
    fn the_headers_of_an_answer_are_told_from_the_answer_by_their_length() {
        let head = concat!(
            "HTTP/1.1 100 Continue\r\nMcp-Session-Id: early\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Type: Text/Event-Stream; charset=utf-8\r\n",
            "mcp-session-id:  sess-1 \r\nX-Odd\r\n\r\n",
        );
        // An answer that looks like headers, and ends like the line of curl.
        let body = "HTTP/1.1 404 Not Found\r\nMcp-Session-Id: forged\r\nContent-Type: text/plain\r\n\r\ndata: {}\n\n404 3";
        let printed = format!("{head}{body}\n200 {}", head.len());
        let read = reply(printed.as_bytes()).expect("an answer");
        assert_eq!(read.status, 200);
        assert_eq!(read.kind, "text/event-stream; charset=utf-8");
        assert_eq!(read.session.as_deref(), Some("sess-1"));
        assert_eq!(read.body, body);
        // Noted, with nothing said: no answer, no kind, no session.
        let head = "HTTP/1.1 202 Accepted\r\n\r\n";
        let read = reply(format!("{head}\n202 {}", head.len()).as_bytes()).expect("an answer");
        assert_eq!((read.status, read.body.as_str()), (202, ""));
        assert_eq!((read.kind.as_str(), read.session), ("", None));
        // Headers as HTTP/2 has them printed.
        let head = "HTTP/2 200 \ncontent-type: application/json\n\n";
        let read = reply(format!("{head}{{}}\n200 {}", head.len()).as_bytes()).expect("an answer");
        assert_eq!(
            (read.kind.as_str(), read.body.as_str()),
            ("application/json", "{}")
        );
        // What does not add up is no answer: nothing is guessed.
        for printed in [
            "",
            "HTTP/1.1 200 OK\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\n\r\n{}\n200",
            "HTTP/1.1 200 OK\r\n\r\n{}\n200 999",
            "HTTP/1.1 200 OK\r\n\r\n{}\n200 5",
            "HTTP/1.1 200 OK\r\n\r\n{}\nabc 19",
            "{}\n000 0",
            "<html>\r\n\r\n{}\n200 10",
        ] {
            assert!(reply(printed.as_bytes()).is_none(), "{printed}");
        }
    }

    #[test]
    fn what_a_remote_server_answered_is_its_messages_or_why_there_are_none() {
        let answered = |status: u16, kind: &str, body: &str| {
            messages(&Reply {
                status,
                kind: kind.to_owned(),
                session: None,
                body: body.to_owned(),
            })
        };
        let json = "application/json";
        assert_eq!(
            answered(200, json, r#"{"id":1}"#),
            Ok(vec![json!({ "id": 1 })])
        );
        assert_eq!(
            answered(200, json, r#"[{"id":1},{"id":2}]"#),
            Ok(vec![json!({ "id": 1 }), json!({ "id": 2 })])
        );
        let stream = "data: {\"method\":\"m\"}\n\ndata: {\"id\":1}\n\n";
        assert_eq!(
            answered(200, "text/event-stream; charset=utf-8", stream),
            Ok(vec![json!({ "method": "m" }), json!({ "id": 1 })])
        );
        // A note was taken: nothing is said.
        assert_eq!(answered(202, "", ""), Ok(Vec::new()));
        assert_eq!(answered(200, json, " \n"), Ok(Vec::new()));
        let refused = |status| {
            Err(format!(
                "The server refused the credentials (HTTP {status})."
            ))
        };
        assert_eq!(
            answered(401, json, r#"{"error":{"message":"token abc"}}"#),
            refused(401)
        );
        assert_eq!(answered(403, "text/html", "<html>"), refused(403));
        assert_eq!(
            answered(500, "text/html", "<html>"),
            Err("The server answered HTTP 500.".to_owned())
        );
        assert_eq!(
            answered(307, "", ""),
            Err("The server answered HTTP 307.".to_owned())
        );
        assert_eq!(
            answered(
                400,
                json,
                r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"No session"}}"#
            ),
            Err("The server answered HTTP 400: No session".to_owned())
        );
        for body in ["<html>Sign in</html>", "3", "\"text\"", "{\"cut\":"] {
            assert_eq!(
                answered(200, "text/html", body),
                Err("The server did not answer with JSON.".to_owned())
            );
        }
    }

    #[test]
    fn a_failure_of_curl_to_reach_a_server_is_told_in_words() {
        assert_eq!(
            failure(Some(6), "curl: (6) Could not resolve host: example.com", 30),
            "Cannot find the server: check its address, and the connection."
        );
        assert_eq!(
            failure(Some(7), "curl: (7) Failed to connect", 30),
            "Cannot reach the server: nothing answers at its address."
        );
        assert_eq!(
            failure(Some(28), "curl: (28) Operation timed out", 120),
            "The server did not answer in 120 seconds."
        );
        assert_eq!(
            failure(Some(35), "curl: (35) TLS failed\nmore", 30),
            "curl: (35) TLS failed"
        );
        assert_eq!(
            failure(None, " \n", 30),
            "Cannot reach the server: curl failed."
        );
    }

    #[test]
    fn what_curl_is_told_holds_the_tokens_and_its_arguments_hold_none() {
        let remote = Remote {
            url: "https://example.com/mcp?key=s3cret-{key}".to_owned(),
            headers: vec![(
                "Authorization".to_owned(),
                "Bearer s3cret-\"token\"".to_owned(),
            )],
        };
        let session = Session {
            id: Some("sess-1".to_owned()),
            protocol: Some("2025-06-18".to_owned()),
        };
        let config = curl_config(&remote, &session, Some("{\"a\":\"@b\\n\"}"), 30);
        let lines: Vec<&str> = config.lines().collect();
        assert_eq!(
            lines,
            [
                "globoff",
                r#"url = "https://example.com/mcp?key=s3cret-{key}""#,
                r#"header = "Accept: application/json, text/event-stream""#,
                r#"header = "Content-Type: application/json""#,
                r#"header = "Authorization: Bearer s3cret-\"token\"""#,
                r#"header = "Mcp-Session-Id: sess-1""#,
                r#"header = "MCP-Protocol-Version: 2025-06-18""#,
                r#"data-raw = "{\"a\":\"@b\\n\"}""#,
                "max-time = 30",
                r#"dump-header = "-""#,
                r#"write-out = "\n%{http_code} %{size_header}""#,
            ]
        );
        // Before hands are shaken there is no session to name; a goodbye has no body.
        let first = curl_config(&remote, &Session::default(), Some("{}"), 30);
        assert!(!first.contains("Mcp-Session-Id") && !first.contains("MCP-Protocol-Version"));
        let goodbye = curl_config(&remote, &session, None, 5);
        assert!(goodbye.contains("request = \"DELETE\"\n") && goodbye.contains("max-time = 5\n"));
        assert!(goodbye.contains("Mcp-Session-Id: sess-1") && !goodbye.contains("data-raw"));
        assert!(!goodbye.contains("Content-Type"), "{goodbye}");
        // Nothing of a request is among the arguments, which other programs can read.
        let command = curl();
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect();
        assert_eq!(args, ["-q", "-sS", "--config", "-"]);
        assert_eq!(command.get_envs().count(), 0);
    }

    #[test]
    fn a_remote_server_is_asked_over_http_for_its_tools_and_has_them_run() {
        let Some(fake) = Far::start("far") else {
            return;
        };
        let woken = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&woken);
        let wake = move || {
            count.fetch_add(1, Ordering::Relaxed);
        };
        let config = fake.config("s3cret-token-1");
        let mut server = Server::start(&config, &std::env::temp_dir(), wake).expect("started");
        assert_eq!(server.name(), "far");
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
        // Both pages of its list, in order. It would have given none without the token,
        // the headers of the transport, the session it opened and the revision it chose.
        assert_eq!(from, "far");
        assert_eq!(names(tools), ["echo", "stream", "fail"]);
        assert!(tools.iter().all(|tool| tool.server == "far"));
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert!(woken.load(Ordering::Relaxed) > 0);
        assert!(fake.noted("initialized"));

        let text = "héllo ✓ 😀\n\"second\" line \\ @file %{http_code}";
        let mut results = vec![called(&mut server, "echo", json!({ "text": text }))];
        assert_eq!(results[0], Ok(text.to_owned()));
        // A stream: what comes before the answer does no harm, and its ping is answered.
        results.push(called(&mut server, "stream", Value::Null));
        assert_eq!(results[1], Ok("streamed".to_owned()));
        assert!(fake.noted("answer ping-1 {}"));
        for (tool, error) in [
            ("fail", "it broke"),
            ("nope", "Unknown tool: nope"),
            // What it was lent comes back hidden.
            ("secret", "bad *** in *** at /mcp?***"),
            ("bad", "The server answered HTTP 400: Bad arguments"),
            ("page", "The server did not answer with JSON."),
            ("silent", "The server did not answer."),
            // Not followed: a token is not carried elsewhere.
            ("moved", "The server answered HTTP 307."),
        ] {
            results.push(called(&mut server, tool, json!({})));
            assert_eq!(results.last(), Some(&Err(error.to_owned())), "{tool}");
        }
        let printed = format!("{results:?} {seen:?}");
        assert!(
            !printed.contains("s3cret") && !printed.contains("sess-"),
            "{printed}"
        );
        // It still listens after all that, and is told goodbye when dropped.
        assert_eq!(
            called(&mut server, "echo", json!({ "text": "still" })),
            Ok("still".to_owned())
        );
        assert!(server.poll().is_empty());
        drop(server);
        assert!(fake.noted("DELETE sess-9f8e7d6c5b4a 2025-03-26"));
        assert!(!fake.notes().contains("elsewhere"), "{}", fake.notes());
    }

    #[test]
    fn a_remote_server_that_refuses_the_token_has_failed() {
        let Some(fake) = Far::start("refusing-far") else {
            return;
        };
        let config = fake.config("wrong-s3cret-token");
        let mut server = Server::start(&config, &std::env::temp_dir(), || {}).expect("started");
        let call = server.call("echo", json!({}));
        let seen = until(&mut server, |event| matches!(event, Event::Exited { .. }));
        let why = "The server refused the credentials (HTTP 401).".to_owned();
        assert_eq!(
            seen,
            [
                Event::Failed {
                    server: far(),
                    why: why.clone()
                },
                Event::Called {
                    id: call,
                    result: Err(why)
                },
                Event::Exited { server: far() },
            ]
        );
        // No session was opened: there is none to end.
        drop(server);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(fake.notes(), "");
    }

    #[test]
    fn a_remote_server_that_ends_the_session_has_failed_and_is_gone() {
        let Some(fake) = Far::start("ending-far") else {
            return;
        };
        let config = fake.config("s3cret-token-1");
        let mut server = Server::start(&config, &std::env::temp_dir(), || {}).expect("started");
        until(&mut server, |event| matches!(event, Event::Ready { .. }));
        assert_eq!(
            called(&mut server, "kill-session", json!({})),
            Ok("killed".to_owned())
        );
        // The session it no longer knows: what is asked fails, and so does the server.
        let first = server.call("echo", json!({ "text": "a" }));
        let second = server.call("echo", json!({ "text": "b" }));
        let seen = until(&mut server, |event| matches!(event, Event::Exited { .. }));
        let why = "The server ended the session.".to_owned();
        let failed = |id| Event::Called {
            id,
            result: Err(why.clone()),
        };
        assert_eq!(
            seen,
            [
                Event::Failed {
                    server: far(),
                    why: why.clone()
                },
                failed(first),
                failed(second),
                Event::Exited { server: far() },
            ]
        );
        // Said once; what is asked of it now fails at once.
        let late = server.call("echo", json!({ "text": "late" }));
        assert_eq!(server.poll(), [failed(late)]);
        assert!(server.poll().is_empty());
        // A session that ended is not ended again.
        drop(server);
        std::thread::sleep(Duration::from_millis(200));
        assert!(!fake.notes().contains("DELETE"), "{}", fake.notes());
    }

    #[test]
    fn a_remote_server_that_is_not_there_has_failed() {
        if Command::new("curl").arg("--version").output().is_err() {
            eprintln!("skipped: `curl` is not installed");
            return;
        }
        // A port of this computer where nothing listens: it was just let go.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = listener.local_addr().expect("its address").port();
        drop(listener);
        let config = remote(&format!("http://127.0.0.1:{port}/mcp"));
        let mut server = Server::start(&config, &std::env::temp_dir(), || {}).expect("started");
        let seen = until(&mut server, |event| matches!(event, Event::Exited { .. }));
        let why = "Cannot reach the server: nothing answers at its address.".to_owned();
        assert_eq!(
            seen,
            [
                Event::Failed { server: far(), why },
                Event::Exited { server: far() },
            ]
        );
    }

    #[test]
    fn a_remote_server_asked_over_plain_http_elsewhere_is_not_started() {
        let mut config = remote("http://example.com/mcp?key=s3cret-key");
        config.headers = vec![("Authorization".to_owned(), "Bearer s3cret-token".to_owned())];
        let Err(why) = Server::start(&config, &std::env::temp_dir(), || {}) else {
            panic!("a token was about to travel in the clear");
        };
        assert!(
            why.contains("http://example.com/mcp") && why.contains("https://"),
            "{why}"
        );
        assert!(!why.contains("s3cret"), "{why}");
        // Nor one with a header that is more than a line: it would be more than a header.
        let mut config = remote("https://example.com/mcp");
        config.headers = vec![("X-Key".to_owned(), "s3cret-a\r\nX-Other: b".to_owned())];
        let Err(why) = Server::start(&config, &std::env::temp_dir(), || {}) else {
            panic!("a header of two lines was sent");
        };
        assert_eq!(
            why,
            "A header of far is not a name and a value on one line."
        );
    }
}
