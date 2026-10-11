//! The assistant: what is asked of a language model, local or not, and what it answers.
//!
//! Asking goes through curl, as the HTTP client does. Everything curl is told (address,
//! headers, body) is written on its input, never on its command line: the list of the
//! running programs shows command lines, and a key must not be read there.

use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// Who answers: a model running on this machine, or one of Anthropic.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Ollama,
    Anthropic,
}

/// One thing said in the conversation, by the user or by the model.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub from_user: bool,
    pub text: String,
}

/// Where to ask and what.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub provider: Provider,
    pub model: String,
}

/// The models of Anthropic offered, the one to start with first.
#[allow(dead_code)]
pub const ANTHROPIC_MODELS: [&str; 3] = [
    "claude-sonnet-5-5",
    "claude-opus-5-5",
    "claude-haiku-4-5-20251001",
];

/// The variable of the environment holding the key of the user for Anthropic.
const KEY_VARIABLE: &str = "ANTHROPIC_API_KEY";
const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
/// How long an answer is waited for, in seconds.
const PATIENCE: &str = "120";
/// How much of a file the model is given, in characters.
const MOST_OF_A_FILE: usize = 60_000;

impl Provider {
    #[allow(dead_code)]
    pub fn name(self) -> &'static str {
        match self {
            Self::Ollama => "Ollama",
            Self::Anthropic => "Anthropic",
        }
    }

    /// Whether it can be asked at all: Anthropic needs a key. Whether Ollama runs is only
    /// known by asking.
    #[allow(dead_code)]
    pub fn available(self) -> bool {
        match self {
            Self::Ollama => true,
            Self::Anthropic => key().is_some(),
        }
    }
}

/// The key of the user for Anthropic, when the environment holds one that can be sent.
fn key() -> Option<String> {
    let key = std::env::var(KEY_VARIABLE).ok()?;
    let key = key.trim();
    // A line break in it would start another header.
    (!key.is_empty() && !key.contains(char::is_control)).then(|| key.to_owned())
}

/// The address Ollama is asked at, from what `OLLAMA_HOST` holds: `host`, `host:port` or
/// a whole address.
fn host(set: Option<&str>) -> String {
    let set = set.unwrap_or_default().trim().trim_end_matches('/');
    if set.is_empty() {
        return "http://127.0.0.1:11434".to_owned();
    }
    // What a server listens on for every machine is not an address to call.
    let set = set.replace("0.0.0.0", "127.0.0.1");
    if set.contains("://") {
        return set;
    }
    let has_port = set
        .rsplit_once(':')
        .is_some_and(|(_, port)| !port.is_empty() && port.bytes().all(|it| it.is_ascii_digit()));
    match has_port {
        true => format!("http://{set}"),
        false => format!("http://{set}:11434"),
    }
}

/// The conversation as a model takes it: the user speaks first, then each in turn.
/// What follows from the same side is joined, what is empty is left out.
fn turns(messages: &[Message]) -> Vec<Value> {
    let mut turns: Vec<(bool, String)> = Vec::new();
    for message in messages {
        if message.text.trim().is_empty() {
            continue;
        }
        match turns.last_mut() {
            Some((from_user, said)) if *from_user == message.from_user => {
                said.push_str("\n\n");
                said.push_str(&message.text);
            }
            None if !message.from_user => {}
            _ => turns.push((message.from_user, message.text.clone())),
        }
    }
    let role = |from_user| if from_user { "user" } else { "assistant" };
    turns
        .into_iter()
        .map(|(from_user, text)| json!({ "role": role(from_user), "content": text }))
        .collect()
}

/// What is sent to ask `messages` of the model of `settings`, told `system` first.
fn body(settings: &Settings, system: &str, messages: &[Message]) -> Value {
    let mut turns = turns(messages);
    match settings.provider {
        Provider::Ollama => {
            if !system.is_empty() {
                turns.insert(0, json!({ "role": "system", "content": system }));
            }
            json!({ "model": settings.model, "messages": turns, "stream": false })
        }
        Provider::Anthropic => {
            let mut body =
                json!({ "model": settings.model, "max_tokens": 4096, "messages": turns });
            if !system.is_empty() {
                body["system"] = system.into();
            }
            body
        }
    }
}

/// `answer` read as JSON, unless it is the report of an error: Ollama gives it as a text,
/// Anthropic as an object holding a message.
fn understood(provider: Provider, answer: &str) -> Result<Value, String> {
    let name = provider.name();
    let answer: Value =
        serde_json::from_str(answer).map_err(|_| format!("{name} did not answer in JSON."))?;
    match answer.get("error") {
        Some(error) => {
            let said = error.as_str().or_else(|| error["message"].as_str());
            Err(format!("{name}: {}", said.unwrap_or("unknown error")))
        }
        None => Ok(answer),
    }
}

/// What the model said, read in the `answer` of its provider.
fn reply(provider: Provider, answer: &str) -> Result<String, String> {
    let answer = understood(provider, answer)?;
    let said: String = match provider {
        Provider::Ollama => answer["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        Provider::Anthropic => answer["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect(),
    };
    match said.trim().is_empty() {
        true => Err(format!("{} gave no answer.", provider.name())),
        false => Ok(said),
    }
}

/// The names of the models Ollama holds, read in its `answer`.
fn model_names(answer: &str) -> Result<Vec<String>, String> {
    let answer = understood(Provider::Ollama, answer)?;
    let models = answer["models"]
        .as_array()
        .ok_or("Ollama did not list its models.")?;
    Ok(models
        .iter()
        .filter_map(|model| model["name"].as_str())
        .map(str::to_owned)
        .collect())
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

/// What curl reads on its input: where to ask, with which headers, and what to post when
/// there is a `body`. The answer comes back followed by a line holding the HTTP status.
fn config(url: &str, headers: &[String], body: Option<&str>) -> String {
    let mut config = format!("url = {}\n", quoted(url));
    for header in headers {
        config.push_str(&format!("header = {}\n", quoted(header)));
    }
    if let Some(body) = body {
        // `data-raw`: a body starting with `@` is not the name of a file to send.
        config.push_str(&format!("data-raw = {}\n", quoted(body)));
    }
    config.push_str("write-out = \"\\n%{http_code}\"\n");
    config
}

/// curl, about to read on its input what it has to do. Nothing of the request is here.
fn curl() -> Command {
    let mut command = Command::new("curl");
    command
        // `-q`: the options the user keeps for curl are not ours.
        .args(["-q", "-sS", "--max-time", PATIENCE, "--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::tools::hidden(&mut command);
    command
}

/// Why curl failed, in words: `code` is what it exited with, `complaint` what it printed.
fn failure(provider: Provider, code: Option<i32>, complaint: &str) -> String {
    let name = provider.name();
    match (code, provider) {
        (Some(7), Provider::Ollama) => format!(
            "Ollama is not running: start it, or set {KEY_VARIABLE} to ask Anthropic instead."
        ),
        (Some(6 | 7), _) => format!("Cannot reach {name}: check the connection."),
        (Some(28), _) => format!("{name} did not answer in {PATIENCE} seconds."),
        _ => {
            let complaint = complaint.lines().next().unwrap_or_default().trim();
            match complaint.is_empty() {
                true => format!("Cannot reach {name}: curl failed."),
                false => complaint.to_owned(),
            }
        }
    }
}

/// The answer in what curl `printed`, without the line of the status that follows it;
/// the status alone when it tells of an error and the answer is not JSON to read.
fn received(provider: Provider, printed: &str) -> Result<String, String> {
    let (answer, status) = printed.rsplit_once('\n').unwrap_or((printed, ""));
    let failed = status.trim().parse::<u16>().is_ok_and(|it| it >= 400);
    if failed && !answer.trim_start().starts_with('{') {
        return Err(format!(
            "{} answered HTTP {}.",
            provider.name(),
            status.trim()
        ));
    }
    Ok(answer.to_owned())
}

/// Run curl on `config` and return what `provider` answered.
fn fetch(provider: Provider, config: &str) -> Result<String, String> {
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
        return Err(failure(provider, output.status.code(), &complaint));
    }
    received(provider, &String::from_utf8_lossy(&output.stdout))
}

fn ollama() -> String {
    host(std::env::var("OLLAMA_HOST").ok().as_deref())
}

/// The models `provider` can be asked: those Ollama holds, or the ones of Anthropic.
#[allow(dead_code)]
pub fn models(provider: Provider) -> Result<Vec<String>, String> {
    match provider {
        Provider::Anthropic => Ok(ANTHROPIC_MODELS.map(str::to_owned).to_vec()),
        Provider::Ollama => {
            let url = format!("{}/api/tags", ollama());
            model_names(&fetch(provider, &config(&url, &[], None))?)
        }
    }
}

/// Ask `messages`, the conversation so far, of the model of `settings`, told `system`
/// first, and wait for what it answers: up to two minutes.
#[allow(dead_code)]
pub fn ask(settings: &Settings, system: &str, messages: &[Message]) -> Result<String, String> {
    let provider = settings.provider;
    let body = body(settings, system, messages);
    if body["messages"]
        .as_array()
        .is_none_or(|turns| turns.iter().all(|it| it["role"] != "user"))
    {
        return Err("There is nothing to ask.".to_owned());
    }
    let json = "content-type: application/json".to_owned();
    let body = body.to_string();
    match provider {
        Provider::Ollama => {
            let url = format!("{}/api/chat", ollama());
            reply(
                provider,
                &fetch(provider, &config(&url, &[json], Some(&body)))?,
            )
        }
        Provider::Anthropic => {
            let key = key().ok_or(format!("Set {KEY_VARIABLE} to ask Anthropic."))?;
            let headers = [
                format!("x-api-key: {key}"),
                "anthropic-version: 2023-06-01".to_owned(),
                json,
            ];
            let answer = fetch(provider, &config(ANTHROPIC_URL, &headers, Some(&body)));
            // Nothing is known to repeat the key, and nothing may.
            let answer = answer.and_then(|answer| reply(provider, &answer));
            answer.map_err(|err| err.replace(&key, "(the key)"))
        }
    }
}

/// The beginning of `text`, up to `most` characters, and whether some was left out.
fn beginning(text: &str, most: usize) -> (&str, bool) {
    match text.char_indices().nth(most) {
        Some((end, _)) => (&text[..end], true),
        None => (text, false),
    }
}

/// What the model is told before the conversation: who it is, and the file in front,
/// named `file`, written in `language`, holding `text`, with what is selected in it.
#[allow(dead_code)]
pub fn system(file: Option<&str>, language: &str, text: &str, selection: Option<&str>) -> String {
    let mut told = format!(
        "You are the assistant of GodCode, a code editor. Answer briefly and to the point. \
         Show code in fenced blocks, with the name of the language.\n\n\
         The user is looking at the file `{}`, written in {language}.\n",
        file.unwrap_or("untitled")
    );
    let mut give = |what: &str, tag: &str, text: &str| {
        let (text, cut) = beginning(text, MOST_OF_A_FILE);
        told.push_str(&format!("\n{what}:\n<{tag}>\n{text}\n</{tag}>\n"));
        if cut {
            told.push_str(&format!(
                "(Cut: only the first {MOST_OF_A_FILE} characters are given.)\n"
            ));
        }
    };
    if let Some(selection) = selection.filter(|it| !it.trim().is_empty()) {
        give("What is selected in it", "selection", selection);
    }
    give("The text of the file", "file", text);
    told
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(from_user: bool, text: &str) -> Message {
        Message {
            from_user,
            text: text.to_owned(),
        }
    }

    fn settings(provider: Provider) -> Settings {
        Settings {
            provider,
            model: "a-model".to_owned(),
        }
    }

    /// Answer the one request made to a port of this machine with `answer`, and give the
    /// address to ask and what was asked there.
    fn listener(answer: &'static str) -> (String, std::thread::JoinHandle<String>) {
        use std::io::Read as _;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("port");
        let url = format!(
            "http://{}/api/chat",
            listener.local_addr().expect("address")
        );
        let asked = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request");
            let mut request = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                let text = String::from_utf8_lossy(&request).into_owned();
                let whole = text.split_once("\r\n\r\n").is_some_and(|(head, body)| {
                    let length = head
                        .lines()
                        .filter_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")?
                                .trim()
                                .parse()
                                .ok()
                        })
                        .next()
                        .unwrap_or(0);
                    body.len() >= length
                });
                if whole {
                    break;
                }
                match stream.read(&mut chunk).expect("read") {
                    0 => break,
                    read => request.extend_from_slice(&chunk[..read]),
                }
            }
            let answer = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                answer.len()
            );
            stream.write_all(answer.as_bytes()).expect("answer");
            String::from_utf8_lossy(&request).into_owned()
        });
        (url, asked)
    }

    fn curl_is_here() -> bool {
        Command::new("curl").arg("--version").output().is_ok()
    }

    #[test]
    fn a_provider_has_a_name_and_ollama_can_always_be_tried() {
        assert_eq!(Provider::Ollama.name(), "Ollama");
        assert_eq!(Provider::Anthropic.name(), "Anthropic");
        assert!(Provider::Ollama.available());
        assert_eq!(
            models(Provider::Anthropic),
            Ok(ANTHROPIC_MODELS.map(str::to_owned).to_vec())
        );
    }

    #[test]
    fn the_host_of_ollama_is_read_with_or_without_its_scheme_and_port() {
        assert_eq!(host(None), "http://127.0.0.1:11434");
        assert_eq!(host(Some("  ")), "http://127.0.0.1:11434");
        assert_eq!(host(Some("localhost:8080")), "http://localhost:8080");
        assert_eq!(host(Some("http://box:8080/")), "http://box:8080");
        assert_eq!(
            host(Some("https://models.example")),
            "https://models.example"
        );
        assert_eq!(host(Some("box")), "http://box:11434");
        assert_eq!(host(Some("[::1]")), "http://[::1]:11434");
        // The address a server listens on is turned into one that can be called.
        assert_eq!(host(Some("0.0.0.0:11434")), "http://127.0.0.1:11434");
    }

    #[test]
    fn the_conversation_starts_with_the_user_and_each_speaks_in_turn() {
        let messages = [
            said(false, "Hello, ask me."),
            said(true, "one"),
            said(true, "two"),
            said(false, "   "),
            said(true, ""),
            said(false, "answer"),
            said(false, "more"),
            said(true, "three"),
        ];
        assert_eq!(
            Value::from(turns(&messages)),
            json!([
                { "role": "user", "content": "one\n\ntwo" },
                { "role": "assistant", "content": "answer\n\nmore" },
                { "role": "user", "content": "three" },
            ])
        );
        assert!(turns(&[said(false, "alone")]).is_empty());
    }

    #[test]
    fn ollama_is_given_the_system_as_the_first_message_and_no_stream() {
        let body = body(&settings(Provider::Ollama), "be brief", &[said(true, "hi")]);
        assert_eq!(
            body,
            json!({
                "model": "a-model",
                "messages": [
                    { "role": "system", "content": "be brief" },
                    { "role": "user", "content": "hi" },
                ],
                "stream": false,
            })
        );
    }

    #[test]
    fn anthropic_is_given_the_system_apart_and_a_limit_of_tokens() {
        let anthropic = settings(Provider::Anthropic);
        let body_told = body(&anthropic, "be brief", &[said(true, "hi")]);
        assert_eq!(
            body_told,
            json!({
                "model": "a-model",
                "max_tokens": 4096,
                "system": "be brief",
                "messages": [{ "role": "user", "content": "hi" }],
            })
        );
        // Nothing to tell: no empty system is sent.
        assert!(
            body(&anthropic, "", &[said(true, "hi")])
                .get("system")
                .is_none()
        );
    }

    #[test]
    fn the_answer_of_ollama_is_its_message_and_its_error_is_told() {
        let answer =
            r#"{"model":"m","message":{"role":"assistant","content":"Hello"},"done":true}"#;
        assert_eq!(reply(Provider::Ollama, answer), Ok("Hello".to_owned()));
        assert_eq!(
            reply(Provider::Ollama, r#"{"error":"model 'm' not found"}"#),
            Err("Ollama: model 'm' not found".to_owned())
        );
        assert_eq!(
            reply(Provider::Ollama, r#"{"message":{"content":""}}"#),
            Err("Ollama gave no answer.".to_owned())
        );
    }

    #[test]
    fn the_answer_of_anthropic_is_its_blocks_of_text_put_together() {
        let answer = r#"{"type":"message","content":[
            {"type":"thinking","thinking":"hm"},
            {"type":"text","text":"Hello"},
            {"type":"tool_use","id":"t","name":"n","input":{}},
            {"type":"text","text":" there"}]}"#;
        assert_eq!(
            reply(Provider::Anthropic, answer),
            Ok("Hello there".to_owned())
        );
        let error = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        assert_eq!(
            reply(Provider::Anthropic, error),
            Err("Anthropic: invalid x-api-key".to_owned())
        );
        assert_eq!(
            reply(Provider::Anthropic, r#"{"content":[]}"#),
            Err("Anthropic gave no answer.".to_owned())
        );
    }

    #[test]
    fn an_answer_that_is_not_json_is_an_error_and_not_a_panic() {
        for answer in [
            "",
            "<html>Bad gateway</html>",
            "[1, 2]",
            "null",
            "{\"content\":3}",
        ] {
            assert!(reply(Provider::Anthropic, answer).is_err(), "{answer}");
            assert!(reply(Provider::Ollama, answer).is_err(), "{answer}");
            assert!(model_names(answer).is_err(), "{answer}");
        }
        assert_eq!(
            reply(Provider::Ollama, "<html>"),
            Err("Ollama did not answer in JSON.".to_owned())
        );
    }

    #[test]
    fn the_models_of_ollama_are_read_by_name() {
        let answer = r#"{"models":[{"name":"llama3.2:latest","size":1},{"size":2},{"name":"qwen2.5-coder:7b"}]}"#;
        assert_eq!(
            model_names(answer),
            Ok(vec![
                "llama3.2:latest".to_owned(),
                "qwen2.5-coder:7b".to_owned()
            ])
        );
        assert_eq!(model_names(r#"{"models":[]}"#), Ok(Vec::new()));
        assert_eq!(
            model_names(r#"{"error":"busy"}"#),
            Err("Ollama: busy".to_owned())
        );
    }

    #[test]
    fn what_curl_printed_is_the_answer_then_the_status() {
        let ollama = Provider::Ollama;
        assert_eq!(
            received(ollama, "{\"a\":1}\n200"),
            Ok("{\"a\":1}".to_owned())
        );
        // An error told in JSON is left to be read; one that is not is told by its status.
        assert_eq!(
            received(ollama, "{\"error\":\"no\"}\n404"),
            Ok("{\"error\":\"no\"}".to_owned())
        );
        assert_eq!(
            received(Provider::Anthropic, "<html>\nBad gateway</html>\n502"),
            Err("Anthropic answered HTTP 502.".to_owned())
        );
    }

    #[test]
    fn a_failure_of_curl_is_told_in_words() {
        let refused = failure(Provider::Ollama, Some(7), "curl: (7) Failed to connect");
        assert!(refused.contains("start it"), "{refused}");
        assert!(refused.contains("ANTHROPIC_API_KEY"), "{refused}");
        assert_eq!(
            failure(
                Provider::Anthropic,
                Some(6),
                "curl: (6) Could not resolve host"
            ),
            "Cannot reach Anthropic: check the connection."
        );
        assert_eq!(
            failure(Provider::Anthropic, Some(28), ""),
            "Anthropic did not answer in 120 seconds."
        );
        assert_eq!(
            failure(Provider::Ollama, Some(35), "curl: (35) TLS failed\nmore"),
            "curl: (35) TLS failed"
        );
        assert_eq!(
            failure(Provider::Ollama, None, ""),
            "Cannot reach Ollama: curl failed."
        );
    }

    #[test]
    fn values_are_escaped_for_the_options_of_curl() {
        assert_eq!(quoted("plain"), "\"plain\"");
        assert_eq!(quoted("a \"b\" \\n"), r#""a \"b\" \\n""#);
        assert_eq!(quoted("one\r\n\ttwo"), r#""one\r\n\ttwo""#);
        // One option a line, whatever the body holds.
        let config = config(
            "http://h/p",
            &["a: b".to_owned()],
            Some("{\"t\":\"x\\ny\"}\n"),
        );
        assert_eq!(
            config,
            "url = \"http://h/p\"\nheader = \"a: b\"\ndata-raw = \"{\\\"t\\\":\\\"x\\\\ny\\\"}\\n\"\nwrite-out = \"\\n%{http_code}\"\n"
        );
        assert!(!super::config("http://h/p", &[], None).contains("data-raw"));
    }

    #[test]
    fn the_key_is_never_on_the_command_line() {
        let key = "sk-ant-not-a-real-key";
        let headers = [format!("x-api-key: {key}")];
        let config = config(ANTHROPIC_URL, &headers, Some("{}"));
        assert!(config.contains("header = \"x-api-key: sk-ant-not-a-real-key\"\n"));
        let command = curl();
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect();
        assert_eq!(args, ["-q", "-sS", "--max-time", "120", "--config", "-"]);
        // Nothing of the request is among them: no key, no address, no body.
        assert!(
            args.iter()
                .all(|arg| !arg.contains(key) && !arg.contains("http"))
        );
        assert_eq!(command.get_envs().count(), 0);
    }

    #[test]
    fn curl_sends_the_headers_and_the_body_as_they_were_written() {
        if !curl_is_here() {
            return;
        }
        let (url, asked) = listener(r#"{"message":{"content":"It works"}}"#);
        let nasty = "quotes \" and ' \\ back\\slash \\n\nline\r\n\ttab é 日本 🦀 \u{1a} @file %{http_code} $HOME";
        let body = body(&settings(Provider::Ollama), nasty, &[said(true, nasty)]).to_string();
        let headers = [
            "x-api-key: sk-ant-not-a-real-key".to_owned(),
            "content-type: application/json".to_owned(),
        ];
        let answer = fetch(Provider::Ollama, &config(&url, &headers, Some(&body)));
        let asked = asked.join().expect("listener");
        assert_eq!(
            answer,
            Ok(r#"{"message":{"content":"It works"}}"#.to_owned())
        );
        assert_eq!(
            reply(Provider::Ollama, &answer.unwrap_or_default()),
            Ok("It works".to_owned())
        );
        let (head, sent) = asked.split_once("\r\n\r\n").expect("a request");
        assert!(head.starts_with("POST /api/chat HTTP/1.1\r\n"), "{head}");
        assert!(
            head.contains("x-api-key: sk-ant-not-a-real-key\r\n"),
            "{head}"
        );
        assert!(
            head.contains("content-type: application/json\r\n"),
            "{head}"
        );
        assert_eq!(sent, body);
    }

    #[test]
    fn a_port_nobody_listens_on_tells_to_start_ollama() {
        if !curl_is_here() {
            return;
        }
        // Bound then let go: nobody answers there.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("port");
        let url = format!(
            "http://{}/api/tags",
            listener.local_addr().expect("address")
        );
        drop(listener);
        let answer = fetch(Provider::Ollama, &config(&url, &[], None));
        let refused = answer.expect_err("nobody listens");
        assert!(refused.starts_with("Ollama is not running"), "{refused}");
    }

    #[test]
    fn nothing_is_asked_when_the_user_said_nothing() {
        let nothing = Err("There is nothing to ask.".to_owned());
        assert_eq!(ask(&settings(Provider::Ollama), "system", &[]), nothing);
        let only_the_model = [said(false, "Hello"), said(true, "  ")];
        assert_eq!(
            ask(&settings(Provider::Anthropic), "system", &only_the_model),
            nothing
        );
    }

    #[test]
    fn the_model_is_told_the_file_its_language_and_the_selection() {
        let told = system(Some("main.rs"), "Rust", "fn main() {}", Some("main"));
        assert!(
            told.starts_with("You are the assistant of GodCode"),
            "{told}"
        );
        assert!(told.contains("fenced blocks"), "{told}");
        assert!(
            told.contains("the file `main.rs`, written in Rust."),
            "{told}"
        );
        assert!(told.contains("<selection>\nmain\n</selection>"), "{told}");
        assert!(told.contains("<file>\nfn main() {}\n</file>"), "{told}");
        assert!(!told.contains("Cut"), "{told}");
        // No name, nothing selected.
        let told = system(None, "Plain Text", "notes", Some(" "));
        assert!(told.contains("`untitled`"), "{told}");
        assert!(!told.contains("<selection>"), "{told}");
    }

    #[test]
    fn a_long_file_is_cut_and_the_model_is_told_so() {
        let long = "é".repeat(MOST_OF_A_FILE + 10);
        let told = system(Some("a.txt"), "Plain Text", &long, None);
        assert_eq!(told.matches('é').count(), MOST_OF_A_FILE);
        assert!(
            told.contains("(Cut: only the first 60000 characters are given.)"),
            "{told}"
        );
        let whole = "é".repeat(MOST_OF_A_FILE);
        assert!(!system(None, "Plain Text", &whole, None).contains("Cut"));
    }
}
