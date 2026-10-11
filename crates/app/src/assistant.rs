//! The assistant: what is asked of a language model, local or not, and what it answers.
//!
//! Asking goes through curl, as the HTTP client does. Everything curl is told (address,
//! headers, body) is written on its input, never on its command line: the list of the
//! running programs shows command lines, and a key must not be read there.

use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// Who answers: a model running on this machine, or one of Anthropic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Ollama,
    Anthropic,
}

/// One thing said in the conversation, by the user or by the model.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub from_user: bool,
    pub text: String,
}

/// Where to ask and what.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub provider: Provider,
    pub model: String,
}

/// The models of Anthropic offered, the one to start with first.
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
/// How much of the code before the caret a completion is given, in characters.
const BEFORE_THE_CARET: usize = 6_000;
/// How much of the code after the caret a completion is given, in characters.
const AFTER_THE_CARET: usize = 2_000;
/// How long a completion may be, in tokens.
const TOKENS_OF_A_COMPLETION: u32 = 256;
/// How long a completion may be, in lines.
const LINES_OF_A_COMPLETION: usize = 40;
/// How much of what the model said about an edit is kept, in characters.
const MOST_SAID_OF_AN_EDIT: usize = 300;
/// What stands for the caret in the code a completion is asked for.
const CARET: &str = "<caret/>";
/// What the model answers when it has nothing to write at the caret.
const NOTHING: &str = "<nothing/>";
/// The longest name Anthropic takes for a tool, in characters.
const LONGEST_TOOL_NAME: usize = 64;

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ollama => "Ollama",
            Self::Anthropic => "Anthropic",
        }
    }

    /// Whether it can be asked at all: Anthropic needs a key. Whether Ollama runs is only
    /// known by asking.
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

/// The words of the model in the `answer` of its provider: none when it said nothing.
fn words(provider: Provider, answer: &Value) -> String {
    match provider {
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
    }
}

/// What the model said, read in the `answer` of its provider.
fn reply(provider: Provider, answer: &str) -> Result<String, String> {
    let said = words(provider, &understood(provider, answer)?);
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
pub fn ask(settings: &Settings, system: &str, messages: &[Message]) -> Result<String, String> {
    post(settings, &body(settings, system, messages), reply)
}

/// Send `body` to the provider of `settings` and `read` what it answers. Nothing is sent
/// when the user says nothing in it.
fn post<T>(
    settings: &Settings,
    body: &Value,
    read: impl Fn(Provider, &str) -> Result<T, String>,
) -> Result<T, String> {
    let provider = settings.provider;
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
            read(
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
            let answer = answer.and_then(|answer| read(provider, &answer));
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
pub fn system(file: Option<&str>, language: &str, text: &str, selection: Option<&str>) -> String {
    let who = "You are the assistant of GodCode, a code editor. Answer briefly and to the point. \
               Show code in fenced blocks, with the name of the language.";
    format!("{who}\n\n{}", shown(file, language, text, selection))
}

/// The file in front as a model is shown it: its name, its language, what is selected in
/// it and its text, cut when it is too long.
fn shown(file: Option<&str>, language: &str, text: &str, selection: Option<&str>) -> String {
    let mut told = format!(
        "The user is looking at the file `{}`, written in {language}.\n",
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

/// The end of `text`, up to `most` characters, from the start of a line when some is
/// left out and a line starts there.
fn ending(text: &str, most: usize) -> &str {
    let Some((at, left_out)) = text.char_indices().rev().nth(most) else {
        return text;
    };
    let end = &text[at + left_out.len_utf8()..];
    match end.split_once('\n') {
        Some((_, lines)) if left_out != '\n' => lines,
        _ => end,
    }
}

/// The beginning of `text`, up to `most` characters, up to the end of a line when some
/// is left out and a line ends there.
fn opening(text: &str, most: usize) -> &str {
    match beginning(text, most) {
        (start, true) => start.rfind('\n').map_or(start, |end| &start[..=end]),
        (whole, false) => whole,
    }
}

/// What a model completing code is told first.
fn completion_system() -> String {
    format!(
        "You complete code in GodCode, a code editor. The user gives a file with {CARET} \
         where the caret is. Answer with only the code to write at {CARET}: no explanation, \
         no fenced block, and nothing of what is already before or after {CARET}. Keep it \
         short: finish the statement or the block being written. If there is nothing to \
         write there, answer {NOTHING}."
    )
}

/// The code around the caret as a model completing it is shown it.
fn completion_prompt(file: Option<&str>, language: &str, before: &str, after: &str) -> String {
    format!(
        "The file `{}`, written in {language}:\n<file>\n{}{CARET}{}\n</file>",
        file.unwrap_or("untitled"),
        ending(before, BEFORE_THE_CARET),
        opening(after, AFTER_THE_CARET)
    )
}

/// What is sent to ask the model of `settings` what to write at the caret.
fn completion_body(
    settings: &Settings,
    file: Option<&str>,
    language: &str,
    before: &str,
    after: &str,
) -> Value {
    let system = completion_system();
    let asked = json!({
        "role": "user",
        "content": completion_prompt(file, language, before, after),
    });
    match settings.provider {
        Provider::Ollama => json!({
            "model": settings.model,
            "messages": [{ "role": "system", "content": system }, asked],
            "stream": false,
            "options": { "temperature": 0, "num_predict": TOKENS_OF_A_COMPLETION },
        }),
        // No temperature: the recent models of Anthropic refuse one.
        Provider::Anthropic => json!({
            "model": settings.model,
            "max_tokens": TOKENS_OF_A_COMPLETION,
            "system": system,
            "messages": [asked],
        }),
    }
}

/// `answer` without the fence around it, when it is given in a fenced block.
fn unfenced(answer: &str) -> &str {
    let inside = answer.trim();
    if !inside.starts_with("```") {
        return answer;
    }
    match inside.split_once('\n') {
        Some((_, code)) => code.strip_suffix("```").unwrap_or(code),
        None => inside.trim_matches('`'),
    }
}

/// `answer` without the line the caret is on, when the model typed it again before going
/// on: the whole of it, or all of it but its indentation.
fn without_the_line_typed_again<'a>(answer: &'a str, before: &str) -> &'a str {
    let line = before.rsplit('\n').next().unwrap_or_default();
    let typed = line.trim_start();
    // One character is too little to tell a repetition from code that starts the same.
    if typed.chars().count() == 1 {
        return answer;
    }
    let unindented = || answer.trim_start().strip_prefix(typed);
    let again = answer
        .strip_prefix(line)
        .or_else(|| unindented().filter(|_| !typed.is_empty()));
    again.unwrap_or(answer)
}

/// `answer` without its end, when that end is the beginning of what follows the caret:
/// more than one character of it, not all white space.
fn without_what_follows<'a>(answer: &'a str, after: &str) -> &'a str {
    let kept = |answer: &'a str| {
        (1..=answer.len().min(after.len()))
            .rev()
            .filter(|&length| after.is_char_boundary(length))
            .map(|length| &after[..length])
            .filter(|follows| follows.chars().nth(1).is_some() && !follows.trim().is_empty())
            .find_map(|follows| answer.strip_suffix(follows))
    };
    kept(answer)
        .or_else(|| kept(answer.trim_end()))
        .unwrap_or(answer)
}

/// What is to be written at the caret, from the `answer` of a model: without the fence
/// around it, without the line before the caret typed again, without the code after the
/// caret repeated, without white space at its end, and not longer than forty lines.
/// Nothing when the model said it had nothing to write.
fn completion(answer: &str, before: &str, after: &str) -> String {
    let answer = answer.replace(CARET, "");
    let answer = unfenced(&answer);
    if answer.trim() == NOTHING {
        return String::new();
    }
    let answer = without_the_line_typed_again(answer, before);
    let answer = without_what_follows(answer, after);
    let kept: String = answer
        .split_inclusive('\n')
        .take(LINES_OF_A_COMPLETION)
        .collect();
    kept.trim_end().to_owned()
}

/// What a model would write at the caret, given the code before and after it; empty when
/// it has nothing to add.
#[allow(dead_code)]
pub fn complete(
    settings: &Settings,
    file: Option<&str>,
    language: &str,
    before: &str,
    after: &str,
) -> Result<String, String> {
    let body = completion_body(settings, file, language, before, after);
    let answer = post(settings, &body, |provider, answer| {
        Ok(words(provider, &understood(provider, answer)?))
    })?;
    Ok(completion(&answer, before, after))
}

/// Whether `text` is too long for a model to be given it whole, and so to answer with
/// the whole of it changed.
#[allow(dead_code)]
pub fn too_long_to_edit(text: &str) -> bool {
    beginning(text, MOST_OF_A_FILE).1
}

/// The instructions for a model asked to change the file in front: it answers with the
/// whole new text of the file in one fenced block, after one short sentence saying what
/// it changed.
#[allow(dead_code)]
pub fn edit_system(
    file: Option<&str>,
    language: &str,
    text: &str,
    selection: Option<&str>,
) -> String {
    let how = match too_long_to_edit(text) {
        false => {
            "The user asks for a change to the file in front. Answer with one short sentence \
             saying what you changed, then the whole new text of the file in one fenced \
             block: every line of it, changed or not, nothing left out, nothing summarised. \
             Open and close that block with four backticks (````), so that a fence inside \
             the file does not end it. Change only what is asked. If nothing is to be \
             changed, answer in words, without any fenced block."
        }
        true => {
            "The user asks for a change to the file in front, but the file is too long to \
             be given whole. Do NOT answer with the whole text of a file, and use no fenced \
             block: say in one short sentence that the file is too long to be changed here."
        }
    };
    format!(
        "You are the assistant of GodCode, a code editor. {how}\n\n{}",
        shown(file, language, text, selection)
    )
}

/// The fence `line` opens or closes: its mark, how many times it is written, and what
/// follows it on the line.
fn fence(line: &str) -> Option<(char, usize, &str)> {
    let line = line.trim();
    let mark = line.chars().next().filter(|it| matches!(it, '`' | '~'))?;
    let count = line.chars().take_while(|&it| it == mark).count();
    // The mark is one byte long.
    let rest = &line[count..];
    // Backticks after the fence: code within a line, not a fence.
    (count >= 3 && !(mark == '`' && rest.contains('`'))).then_some((mark, count, rest))
}

/// The new text of the file an answer proposes, with what the model said about it;
/// `None` when the answer holds no whole-file block.
///
/// The text is what the last fenced block of the answer holds, as it is written there.
/// Any fenced block is taken for a whole file: nothing here knows the file.
#[allow(dead_code)]
pub fn proposed(answer: &str) -> Option<(String, String)> {
    // The fence that is open: its mark, its length, where its line starts and ends.
    let mut open = None;
    // The last block closed: where it starts and ends, and where its content does.
    let mut last = None;
    let mut at = 0;
    for line in answer.split_inclusive('\n') {
        let end = at + line.len();
        match (open, fence(line)) {
            (None, Some((mark, count, _))) => open = Some((mark, count, at, end)),
            (Some((mark, count, start, content)), Some((closing, closed, "")))
                if closing == mark && closed >= count =>
            {
                last = Some((start, end, content, at));
                open = None;
            }
            _ => {}
        }
        at = end;
    }
    if open.is_some() {
        return None;
    }
    let (start, end, content, content_end) = last?;
    let around = [answer[..start].trim(), answer[end..].trim()];
    let said = around.into_iter().filter(|it| !it.is_empty());
    let said = said.collect::<Vec<_>>().join("\n\n");
    Some((
        beginning(&said, MOST_SAID_OF_AN_EDIT).0.to_owned(),
        answer[content..content_end].to_owned(),
    ))
}

/// A tool the model may ask for: its name, what it does, and the JSON schema of what it
/// takes.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// One step of a talk in which the model may use tools.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub enum Turn {
    User(String),
    Said(String),
    /// The model asked for tools to be run (with what it said before asking, possibly
    /// empty).
    Asked {
        said: String,
        calls: Vec<Call>,
    },
    /// What the tools answered, in the order asked.
    Results(Vec<ToolResult>),
}

/// One tool the model asked to be run, and what to give it.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub id: String,
    pub tool: String,
    pub arguments: Value,
}

/// What a tool answered to the call `id`, or why it failed.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub id: String,
    pub tool: String,
    pub text: String,
    pub failed: bool,
}

/// The name a model knows the `tool` of the MCP `server` by: `server__tool`, with `_` for
/// every character a name cannot hold, cut to 64 characters.
///
/// A name cannot be turned back into its server and tool: whoever offers the tools keeps
/// each name with the server and the tool it stands for, and looks it up there.
#[allow(dead_code)]
pub fn tool_name(server: &str, tool: &str) -> String {
    let allowed = |it: char| it.is_ascii_alphanumeric() || it == '_' || it == '-';
    format!("{server}__{tool}")
        .chars()
        .map(|it| if allowed(it) { it } else { '_' })
        .take(LONGEST_TOOL_NAME)
        .collect()
}

/// `value` as the JSON object a tool takes: itself when it is one, what it holds when it
/// is one written as a text, an empty one otherwise.
fn object(value: &Value) -> Value {
    let written = value.as_str().and_then(|it| serde_json::from_str(it).ok());
    match written.as_ref().unwrap_or(value) {
        Value::Object(fields) => Value::Object(fields.clone()),
        _ => json!({}),
    }
}

/// Add `text`, said by `role`, to `messages`: joined to what the same side just said in
/// words, left out when it is empty.
fn spoken(messages: &mut Vec<Value>, role: &str, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    match messages.last_mut() {
        Some(last) if last["role"] == role && last["content"].is_string() => {
            let said = last["content"].as_str().unwrap_or_default();
            last["content"] = format!("{said}\n\n{text}").into();
        }
        _ => messages.push(json!({ "role": role, "content": text })),
    }
}

/// `turns` as the messages Anthropic takes: the tools asked for are blocks of what the
/// model said, their results blocks of what the user says next.
fn anthropic_messages<'a>(turns: impl Iterator<Item = &'a Turn>) -> Vec<Value> {
    let mut messages = Vec::new();
    for turn in turns {
        match turn {
            Turn::User(text) => spoken(&mut messages, "user", text),
            Turn::Said(text) => spoken(&mut messages, "assistant", text),
            Turn::Asked { said, calls } if calls.is_empty() => {
                spoken(&mut messages, "assistant", said);
            }
            Turn::Asked { said, calls } => {
                let mut blocks = Vec::new();
                if !said.trim().is_empty() {
                    blocks.push(json!({ "type": "text", "text": said }));
                }
                blocks.extend(calls.iter().map(|call| {
                    json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.tool,
                        "input": object(&call.arguments),
                    })
                }));
                messages.push(json!({ "role": "assistant", "content": blocks }));
            }
            Turn::Results(results) if results.is_empty() => {}
            Turn::Results(results) => {
                let blocks: Vec<Value> = results
                    .iter()
                    .map(|result| {
                        json!({
                            "type": "tool_result",
                            "tool_use_id": result.id,
                            "content": result.text,
                            "is_error": result.failed,
                        })
                    })
                    .collect();
                messages.push(json!({ "role": "user", "content": blocks }));
            }
        }
    }
    messages
}

/// `turns` as the messages Ollama takes: the tools asked for go with what the model
/// said, each result is a message of its own, said by the tool.
fn ollama_messages<'a>(turns: impl Iterator<Item = &'a Turn>) -> Vec<Value> {
    let mut messages = Vec::new();
    for turn in turns {
        match turn {
            Turn::User(text) => spoken(&mut messages, "user", text),
            Turn::Said(text) => spoken(&mut messages, "assistant", text),
            Turn::Asked { said, calls } if calls.is_empty() => {
                spoken(&mut messages, "assistant", said);
            }
            Turn::Asked { said, calls } => {
                let calls: Vec<Value> = calls
                    .iter()
                    .map(|call| {
                        let arguments = object(&call.arguments);
                        json!({ "function": { "name": call.tool, "arguments": arguments } })
                    })
                    .collect();
                messages.push(json!({ "role": "assistant", "content": said, "tool_calls": calls }));
            }
            Turn::Results(results) => messages.extend(results.iter().map(|result| {
                // Ollama has no word for a failure: the text says it.
                let text = match result.failed {
                    true => format!("The tool failed: {}", result.text),
                    false => result.text.clone(),
                };
                json!({ "role": "tool", "tool_name": result.tool, "content": text })
            })),
        }
    }
    messages
}

/// What is sent to ask `turns` of the model of `settings`, told `system` first, with
/// `tools` on offer. What comes before the user first speaks is left out.
fn tool_body(settings: &Settings, system: &str, turns: &[Turn], tools: &[ToolSpec]) -> Value {
    let speaks = |turn: &&Turn| matches!(turn, Turn::User(text) if !text.trim().is_empty());
    let turns = turns.iter().skip_while(|turn| !speaks(turn));
    let mut body = match settings.provider {
        Provider::Ollama => {
            let mut messages = ollama_messages(turns);
            if !system.is_empty() {
                messages.insert(0, json!({ "role": "system", "content": system }));
            }
            json!({ "model": settings.model, "messages": messages, "stream": false })
        }
        Provider::Anthropic => {
            let messages = anthropic_messages(turns);
            let mut body =
                json!({ "model": settings.model, "max_tokens": 4096, "messages": messages });
            if !system.is_empty() {
                body["system"] = system.into();
            }
            body
        }
    };
    if !tools.is_empty() {
        let offered = |tool: &ToolSpec| {
            // A tool that says nothing of what it takes takes nothing.
            let schema = match &tool.schema {
                Value::Object(_) => tool.schema.clone(),
                _ => json!({ "type": "object", "properties": {} }),
            };
            match settings.provider {
                Provider::Ollama => json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": schema,
                    },
                }),
                Provider::Anthropic => json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": schema,
                }),
            }
        };
        body["tools"] = tools.iter().map(offered).collect();
    }
    body
}

/// What the model answered when it could ask for tools, read in the `answer` of its
/// provider: its words, or the tools it wants run. Ollama gives its calls nothing to
/// answer them by: they are numbered, after the `asked_before` of the talk so far.
fn tool_reply(provider: Provider, answer: &str, asked_before: usize) -> Result<Turn, String> {
    let name = provider.name();
    let answer = understood(provider, answer)?;
    let said = words(provider, &answer);
    let calls: Vec<Call> = match provider {
        Provider::Ollama => answer["message"]["tool_calls"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|call| &call["function"])
            .filter_map(|function| Some((function["name"].as_str()?, &function["arguments"])))
            .enumerate()
            .map(|(at, (tool, arguments))| Call {
                id: format!("call-{}", asked_before + at + 1),
                tool: tool.to_owned(),
                arguments: object(arguments),
            })
            .collect(),
        Provider::Anthropic => answer["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|block| block["type"] == "tool_use")
            .filter_map(|block| {
                Some(Call {
                    id: block["id"].as_str()?.to_owned(),
                    tool: block["name"].as_str()?.to_owned(),
                    arguments: object(&block["input"]),
                })
            })
            .collect(),
    };
    if calls.is_empty() {
        return match said.trim().is_empty() {
            true => Err(format!("{name} gave no answer.")),
            false => Ok(Turn::Said(said)),
        };
    }
    // Cut short, what a tool is to be given may be missing its end.
    if answer["stop_reason"] == "max_tokens" || answer["done_reason"] == "length" {
        return Err(format!("{name} was cut short while asking for a tool."));
    }
    Ok(Turn::Asked { said, calls })
}

/// `error` in clearer words when it is Ollama saying its model takes no tools.
fn tool_failure(error: String) -> String {
    match error.contains("does not support tools") {
        true => "This model cannot use tools: choose another one.".to_owned(),
        false => error,
    }
}

/// Ask with tools on offer: the answer is either words (`Turn::Said`) or a request to
/// run tools (`Turn::Asked`).
#[allow(dead_code)]
pub fn ask_with_tools(
    settings: &Settings,
    system: &str,
    turns: &[Turn],
    tools: &[ToolSpec],
) -> Result<Turn, String> {
    let asked_before: usize = turns
        .iter()
        .map(|turn| match turn {
            Turn::Asked { calls, .. } => calls.len(),
            _ => 0,
        })
        .sum();
    let body = tool_body(settings, system, turns, tools);
    let answer = post(settings, &body, |provider, answer| {
        tool_reply(provider, answer, asked_before)
    });
    answer.map_err(tool_failure)
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

    #[test]
    fn what_is_before_the_caret_is_given_from_its_end_and_from_the_start_of_a_line() {
        let text = "ab\ncd\nef";
        assert_eq!(ending(text, 100), text);
        assert_eq!(ending(text, 8), text);
        // Cut inside a line: from the next one.
        assert_eq!(ending(text, 4), "ef");
        // Cut where a line starts: nothing more is left out.
        assert_eq!(ending(text, 5), "cd\nef");
        // No line starts in what is kept.
        assert_eq!(ending("abcdef", 3), "def");
        assert_eq!(ending(&"é".repeat(10), 3), "ééé");
        assert_eq!(ending(text, 0), "");
    }

    #[test]
    fn what_follows_the_caret_is_given_from_its_start_and_up_to_the_end_of_a_line() {
        let text = "ab\ncd\nef";
        assert_eq!(opening(text, 100), text);
        assert_eq!(opening(text, 4), "ab\n");
        assert_eq!(opening(text, 3), "ab\n");
        // No line ends in what is kept.
        assert_eq!(opening(text, 2), "ab");
        assert_eq!(opening(&"é".repeat(10), 3), "ééé");
    }

    #[test]
    fn a_completion_is_asked_with_the_caret_marked_in_the_code_around_it() {
        let prompt = completion_prompt(
            Some("main.rs"),
            "Rust",
            "fn main() {\n    let x = ",
            ";\n}\n",
        );
        assert_eq!(
            prompt,
            "The file `main.rs`, written in Rust:\n<file>\nfn main() {\n    let x = <caret/>;\n}\n\n</file>"
        );
        // Of a long file, only whole lines near the caret: 6,000 characters before it at
        // most, 2,000 after.
        let before = "let a;\n".repeat(2_000);
        let after = "what follows\n".repeat(1_000);
        let prompt = completion_prompt(None, "Rust", &before, &after);
        assert!(prompt.starts_with("The file `untitled`"), "{prompt}");
        assert_eq!(prompt.matches("let a;\n").count(), 857);
        assert_eq!(prompt.matches("what follows\n").count(), 153);
        assert!(prompt.contains("<file>\nlet a;\n"));
        assert!(prompt.contains("let a;\n<caret/>what follows\n"));
        assert!(prompt.ends_with("what follows\n\n</file>"));
    }

    #[test]
    fn a_model_completing_is_told_to_answer_with_the_code_alone() {
        let told = completion_system();
        assert!(
            told.contains("only the code to write at <caret/>"),
            "{told}"
        );
        assert!(told.contains("no explanation"), "{told}");
        assert!(told.contains("no fenced block"), "{told}");
        assert!(told.contains("before or after <caret/>"), "{told}");
        assert!(told.contains("answer <nothing/>."), "{told}");
    }

    #[test]
    fn ollama_is_asked_a_short_completion_with_no_chance_in_it() {
        let ollama = settings(Provider::Ollama);
        assert_eq!(
            completion_body(&ollama, Some("a.py"), "Python", "x = ", "\n"),
            json!({
                "model": "a-model",
                "messages": [
                    { "role": "system", "content": completion_system() },
                    {
                        "role": "user",
                        "content": "The file `a.py`, written in Python:\n<file>\nx = <caret/>\n\n</file>",
                    },
                ],
                "stream": false,
                "options": { "temperature": 0, "num_predict": 256 },
            })
        );
    }

    #[test]
    fn anthropic_is_asked_a_short_completion_and_no_temperature() {
        let anthropic = settings(Provider::Anthropic);
        assert_eq!(
            completion_body(&anthropic, Some("a.py"), "Python", "x = ", "\n"),
            json!({
                "model": "a-model",
                "max_tokens": 256,
                "system": completion_system(),
                "messages": [{
                    "role": "user",
                    "content": "The file `a.py`, written in Python:\n<file>\nx = <caret/>\n\n</file>",
                }],
            })
        );
    }

    #[test]
    fn a_model_that_says_nothing_has_no_words_and_that_is_no_error() {
        let silent = json!({ "content": [], "stop_reason": "end_turn" });
        assert_eq!(words(Provider::Anthropic, &silent), "");
        let silent = json!({ "message": { "role": "assistant", "content": "" } });
        assert_eq!(words(Provider::Ollama, &silent), "");
        assert_eq!(completion("", "x = ", "\n"), "");
    }

    #[test]
    fn a_completion_is_kept_as_written_but_for_the_space_at_its_end() {
        assert_eq!(completion("foo();\n  \n", "", ""), "foo();");
        assert_eq!(completion("\n    foo();", "", ""), "\n    foo();");
        assert_eq!(completion("   \n", "x", "y"), "");
    }

    #[test]
    fn the_fence_around_a_completion_is_taken_off() {
        assert_eq!(completion("```rust\nlet x = 1;\n```", "", ""), "let x = 1;");
        assert_eq!(
            completion("```\n    indented\n```\n", "", ""),
            "    indented"
        );
        assert_eq!(
            completion("```python\r\nprint(1)\r\n```\r\n", "", ""),
            "print(1)"
        );
        // Cut before the fence was closed.
        assert_eq!(completion("```rust\nlet x = 1;", "", ""), "let x = 1;");
        assert_eq!(completion("```x```", "", ""), "x");
    }

    #[test]
    fn a_model_with_nothing_to_write_gives_an_empty_completion() {
        assert_eq!(completion("<nothing/>", "a", "b"), "");
        assert_eq!(completion(" <nothing/>\n", "a", "b"), "");
        assert_eq!(completion("```\n<nothing/>\n```", "a", "b"), "");
        // The mark of the caret is not code.
        assert_eq!(completion("foo(<caret/>)", "", ""), "foo()");
        // A word that is code somewhere is kept.
        assert_eq!(completion("None", "return ", ""), "None");
    }

    #[test]
    fn the_line_before_the_caret_typed_again_is_taken_off() {
        let before = "fn main() {\n    let total = pri";
        assert_eq!(
            completion("    let total = price * 2;", before, ""),
            "ce * 2;"
        );
        assert_eq!(completion("let total = price * 2;", before, ""), "ce * 2;");
        assert_eq!(completion("ce * 2;", before, ""), "ce * 2;");
        // The indentation alone, typed again.
        assert_eq!(
            completion("    done();\n    more();", "{\n    ", ""),
            "done();\n    more();"
        );
        // One character is not enough to tell.
        assert_eq!(completion("xx = 1", "x", ""), "xx = 1");
        // Nothing is typed yet on the line.
        assert_eq!(completion("  let a;", "x\n", ""), "  let a;");
    }

    #[test]
    fn the_code_after_the_caret_repeated_is_taken_off() {
        assert_eq!(completion("a, b);", "call(", ");\n}"), "a, b");
        assert_eq!(
            completion("x += 1;\n}\n", "{\n    ", "\n}\nfn next() {}"),
            "x += 1;"
        );
        // Whatever white space the model ended with.
        assert_eq!(completion("x += 1;\n}  \n", "", "\n}"), "x += 1;");
        // One character may as well be code of its own.
        assert_eq!(completion("inner(x)", "outer(", ")"), "inner(x)");
        assert_eq!(completion("é", "", "é"), "é");
        // White space alone is nothing repeated.
        assert_eq!(completion("a\n\n", "", "\n\nb"), "a");
    }

    #[test]
    fn a_completion_is_never_longer_than_forty_lines() {
        let kept = completion(&"line\n".repeat(100), "", "");
        assert_eq!(kept.lines().count(), 40);
        assert!(kept.ends_with("line"), "{kept}");
    }

    #[test]
    fn a_model_changing_the_file_is_told_to_answer_with_the_whole_of_it() {
        let told = edit_system(Some("main.rs"), "Rust", "fn main() {}", Some("main"));
        assert!(
            told.starts_with("You are the assistant of GodCode"),
            "{told}"
        );
        assert!(
            told.contains("the whole new text of the file in one fenced block"),
            "{told}"
        );
        assert!(told.contains("four backticks"), "{told}");
        // The file is shown as it is in a conversation.
        assert!(
            told.contains("the file `main.rs`, written in Rust."),
            "{told}"
        );
        assert!(told.contains("<selection>\nmain\n</selection>"), "{told}");
        assert!(told.contains("<file>\nfn main() {}\n</file>"), "{told}");
        assert!(!told.contains("Do NOT"), "{told}");
        assert!(!too_long_to_edit("fn main() {}"));
    }

    #[test]
    fn a_file_too_long_to_be_given_whole_is_not_to_be_answered_whole() {
        let long = "é".repeat(MOST_OF_A_FILE + 1);
        assert!(too_long_to_edit(&long));
        assert!(!too_long_to_edit(&"é".repeat(MOST_OF_A_FILE)));
        let told = edit_system(None, "Plain Text", &long, None);
        assert!(
            told.contains("Do NOT answer with the whole text of a file"),
            "{}",
            &told[..400]
        );
        assert!(!told.contains("four backticks"));
        assert!(told.contains("(Cut: only the first 60000 characters are given.)"));
    }

    fn proposal(said: &str, text: &str) -> Option<(String, String)> {
        Some((said.to_owned(), text.to_owned()))
    }

    #[test]
    fn the_file_proposed_is_what_the_fenced_block_of_the_answer_holds() {
        assert_eq!(
            proposed("I renamed the function.\n\n```rust\nfn start() {}\n```\n"),
            proposal("I renamed the function.", "fn start() {}\n")
        );
        // Tildes, no language, words after the block, blank lines and indentation kept.
        assert_eq!(
            proposed("~~~\na\n\n  b\n\n\n~~~\nDone."),
            proposal("Done.", "a\n\n  b\n\n\n")
        );
        assert_eq!(proposed("Emptied.\n```\n```"), proposal("Emptied.", ""));
    }

    #[test]
    fn the_last_block_of_several_is_the_file_and_the_others_are_what_was_said() {
        let answer = "First:\n```sh\nls\n```\nthen the file:\n````md\nnew\n````\nbye";
        assert_eq!(
            proposed(answer),
            proposal("First:\n```sh\nls\n```\nthen the file:\n\nbye", "new\n")
        );
    }

    #[test]
    fn a_longer_fence_holds_the_fences_of_the_file() {
        let file = "# Title\n\n```rust\nfn a() {}\n```\n\n~~~\nplain\n~~~\n";
        let answer = format!("Added an example.\n````markdown\n{file}````\n");
        assert_eq!(proposed(&answer), proposal("Added an example.", file));
        // Backticks do not close tildes, and a longer fence closes a shorter one.
        assert_eq!(
            proposed("~~~\n```\nx\n```\n~~~~"),
            proposal("", "```\nx\n```\n")
        );
    }

    #[test]
    fn the_ends_of_line_of_the_file_proposed_are_kept() {
        assert_eq!(
            proposed("Changed.\r\n```c\r\nint a;\r\n\r\nint b;\r\n```\r\n"),
            proposal("Changed.", "int a;\r\n\r\nint b;\r\n")
        );
    }

    #[test]
    fn an_answer_without_a_closed_block_proposes_nothing() {
        assert_eq!(proposed("Nothing to change."), None);
        assert_eq!(proposed(""), None);
        // Cut before the end of the file.
        assert_eq!(proposed("Here:\n```rust\nfn a() {}\n"), None);
        assert_eq!(proposed("```\na\n```\nand\n```\nb"), None);
        // A shorter fence does not close a longer one.
        assert_eq!(proposed("````\na\n```\n"), None);
        // Code within a line is not a block.
        assert_eq!(proposed("```x``` is the fix."), None);
    }

    #[test]
    fn what_was_said_about_an_edit_is_kept_short() {
        let answer = format!("{}\n```\nx\n```", "é".repeat(400));
        let (said, text) = proposed(&answer).expect("a block");
        assert_eq!(said, "é".repeat(300));
        assert_eq!(text, "x\n");
    }

    #[test]
    fn a_tool_is_named_after_its_server_in_the_letters_a_model_takes() {
        assert_eq!(tool_name("postgres", "query"), "postgres__query");
        assert_eq!(tool_name("git-hub", "list_PRs2"), "git-hub__list_PRs2");
        assert_eq!(
            tool_name("my server", "read.file/é"),
            "my_server__read_file__"
        );
        let long = tool_name(&"s".repeat(60), "tool-name");
        assert_eq!(long.len(), 64);
        assert!(long.ends_with("s__to"), "{long}");
        assert_eq!(tool_name("", ""), "__");
    }

    // What the providers answer below is written from the shapes their documentation
    // gives, not captured from real calls.
    const ANTHROPIC_ASKS: &str = r#"{"id":"msg_01Aq9w938a90dw8q","type":"message","role":"assistant","model":"claude-sonnet-5-5","content":[{"type":"text","text":"I will count them."},{"type":"tool_use","id":"toolu_01A09q90qw90lq917835lq9","name":"postgres__query","input":{"sql":"select count(*) from users"}},{"type":"tool_use","id":"toolu_01B7xTq3","name":"files__read","input":{}}],"stop_reason":"tool_use","stop_sequence":null,"usage":{"input_tokens":472,"output_tokens":89}}"#;
    const OLLAMA_ASKS: &str = r#"{"model":"qwen3:8b","created_at":"2025-07-07T20:32:53.844124Z","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"postgres__query","arguments":{"sql":"select count(*) from users"}}},{"function":{"name":"files__read","arguments":{"path":"a.txt"}}}]},"done_reason":"stop","done":true,"total_duration":3244883583,"prompt_eval_count":169,"eval_count":44}"#;

    fn offered() -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "postgres__query".to_owned(),
            description: "Run a read-only SQL query.".to_owned(),
            schema: json!({
                "type": "object",
                "properties": { "sql": { "type": "string" } },
                "required": ["sql"],
            }),
        }]
    }

    fn call(id: &str, tool: &str, arguments: Value) -> Call {
        Call {
            id: id.to_owned(),
            tool: tool.to_owned(),
            arguments,
        }
    }

    fn result(id: &str, text: &str, failed: bool) -> ToolResult {
        ToolResult {
            id: id.to_owned(),
            tool: "postgres__query".to_owned(),
            text: text.to_owned(),
            failed,
        }
    }

    /// The user asks, the model asks for a tool, the tool answers.
    fn talk() -> Vec<Turn> {
        let count = json!({ "sql": "select count(*) from users" });
        vec![
            Turn::User("How many users?".to_owned()),
            Turn::Asked {
                said: "I will count them.".to_owned(),
                calls: vec![call("toolu_01", "postgres__query", count)],
            },
            Turn::Results(vec![result("toolu_01", "42", false)]),
        ]
    }

    #[test]
    fn anthropic_is_given_the_tools_and_what_was_asked_and_answered_as_blocks() {
        let body = tool_body(
            &settings(Provider::Anthropic),
            "be brief",
            &talk(),
            &offered(),
        );
        assert_eq!(
            body,
            json!({
                "model": "a-model",
                "max_tokens": 4096,
                "system": "be brief",
                "messages": [
                    { "role": "user", "content": "How many users?" },
                    { "role": "assistant", "content": [
                        { "type": "text", "text": "I will count them." },
                        {
                            "type": "tool_use",
                            "id": "toolu_01",
                            "name": "postgres__query",
                            "input": { "sql": "select count(*) from users" },
                        },
                    ] },
                    { "role": "user", "content": [{
                        "type": "tool_result",
                        "tool_use_id": "toolu_01",
                        "content": "42",
                        "is_error": false,
                    }] },
                ],
                "tools": [{
                    "name": "postgres__query",
                    "description": "Run a read-only SQL query.",
                    "input_schema": {
                        "type": "object",
                        "properties": { "sql": { "type": "string" } },
                        "required": ["sql"],
                    },
                }],
            })
        );
    }

    #[test]
    fn ollama_is_given_the_tools_as_functions_and_each_result_as_a_message() {
        let body = tool_body(&settings(Provider::Ollama), "be brief", &talk(), &offered());
        assert_eq!(
            body,
            json!({
                "model": "a-model",
                "stream": false,
                "messages": [
                    { "role": "system", "content": "be brief" },
                    { "role": "user", "content": "How many users?" },
                    {
                        "role": "assistant",
                        "content": "I will count them.",
                        "tool_calls": [{ "function": {
                            "name": "postgres__query",
                            "arguments": { "sql": "select count(*) from users" },
                        } }],
                    },
                    { "role": "tool", "tool_name": "postgres__query", "content": "42" },
                ],
                "tools": [{
                    "type": "function",
                    "function": {
                        "name": "postgres__query",
                        "description": "Run a read-only SQL query.",
                        "parameters": {
                            "type": "object",
                            "properties": { "sql": { "type": "string" } },
                            "required": ["sql"],
                        },
                    },
                }],
            })
        );
    }

    #[test]
    fn a_failed_tool_is_told_and_what_is_not_an_object_is_never_sent_as_one() {
        let turns = [
            Turn::User("go".to_owned()),
            Turn::Asked {
                said: " ".to_owned(),
                calls: vec![
                    call("a", "one", json!("{\"n\": 1}")),
                    call("b", "two", Value::Null),
                ],
            },
            Turn::Results(vec![
                result("a", "no such table", true),
                result("b", "", false),
            ]),
        ];
        let tools = [ToolSpec {
            name: "one".to_owned(),
            description: String::new(),
            schema: Value::Null,
        }];
        let body = tool_body(&settings(Provider::Anthropic), "", &turns, &tools);
        assert_eq!(
            body["messages"][1]["content"],
            json!([
                { "type": "tool_use", "id": "a", "name": "one", "input": { "n": 1 } },
                { "type": "tool_use", "id": "b", "name": "two", "input": {} },
            ])
        );
        assert_eq!(
            body["messages"][2]["content"],
            json!([
                { "type": "tool_result", "tool_use_id": "a", "content": "no such table", "is_error": true },
                { "type": "tool_result", "tool_use_id": "b", "content": "", "is_error": false },
            ])
        );
        assert_eq!(
            body["tools"][0]["input_schema"],
            json!({ "type": "object", "properties": {} })
        );
        let body = tool_body(&settings(Provider::Ollama), "", &turns, &tools);
        assert_eq!(
            body["messages"][2],
            json!({
                "role": "tool",
                "tool_name": "postgres__query",
                "content": "The tool failed: no such table",
            })
        );
        assert_eq!(
            body["messages"][1]["tool_calls"][1]["function"]["arguments"],
            json!({})
        );
    }

    #[test]
    fn without_tools_what_is_sent_is_what_a_plain_question_sends() {
        let messages = [
            said(false, "Hello, ask me."),
            said(true, "one"),
            said(true, "two"),
            said(false, "  "),
            said(false, "answer"),
            said(true, "three"),
        ];
        let turns: Vec<Turn> = messages
            .iter()
            .map(|message| match message.from_user {
                true => Turn::User(message.text.clone()),
                false => Turn::Said(message.text.clone()),
            })
            .collect();
        for provider in [Provider::Ollama, Provider::Anthropic] {
            for system in ["", "be brief"] {
                assert_eq!(
                    tool_body(&settings(provider), system, &turns, &[]),
                    body(&settings(provider), system, &messages)
                );
            }
        }
    }

    #[test]
    fn nothing_is_asked_with_tools_when_the_user_said_nothing() {
        let nothing = Err("There is nothing to ask.".to_owned());
        assert_eq!(
            ask_with_tools(&settings(Provider::Ollama), "system", &[], &offered()),
            nothing
        );
        // What the model said and what tools answered before the user spoke is not a
        // question.
        let turns = [
            Turn::Said("Hello".to_owned()),
            Turn::Results(vec![result("a", "42", false)]),
            Turn::User(" ".to_owned()),
        ];
        assert_eq!(
            ask_with_tools(&settings(Provider::Anthropic), "system", &turns, &offered()),
            nothing
        );
    }

    #[test]
    fn the_tools_anthropic_asks_for_are_read_with_what_it_said_first() {
        let count = json!({ "sql": "select count(*) from users" });
        assert_eq!(
            tool_reply(Provider::Anthropic, ANTHROPIC_ASKS, 0),
            Ok(Turn::Asked {
                said: "I will count them.".to_owned(),
                calls: vec![
                    call("toolu_01A09q90qw90lq917835lq9", "postgres__query", count),
                    call("toolu_01B7xTq3", "files__read", json!({})),
                ],
            })
        );
    }

    #[test]
    fn the_tools_ollama_asks_for_are_numbered_after_those_asked_before() {
        let count = json!({ "sql": "select count(*) from users" });
        assert_eq!(
            tool_reply(Provider::Ollama, OLLAMA_ASKS, 2),
            Ok(Turn::Asked {
                said: String::new(),
                calls: vec![
                    call("call-3", "postgres__query", count),
                    call("call-4", "files__read", json!({ "path": "a.txt" })),
                ],
            })
        );
    }

    #[test]
    fn an_answer_in_words_is_what_the_model_said_whatever_tools_were_offered() {
        let words = r#"{"id":"msg_01","type":"message","role":"assistant","content":[{"type":"text","text":"There are 42."}],"stop_reason":"end_turn"}"#;
        assert_eq!(
            tool_reply(Provider::Anthropic, words, 1),
            Ok(Turn::Said("There are 42.".to_owned()))
        );
        let words = r#"{"model":"qwen3:8b","message":{"role":"assistant","content":"There are 42."},"done_reason":"stop","done":true}"#;
        assert_eq!(
            tool_reply(Provider::Ollama, words, 1),
            Ok(Turn::Said("There are 42.".to_owned()))
        );
        assert_eq!(
            tool_reply(Provider::Anthropic, r#"{"content":[]}"#, 0),
            Err("Anthropic gave no answer.".to_owned())
        );
        assert_eq!(
            tool_reply(Provider::Ollama, "<html>", 0),
            Err("Ollama did not answer in JSON.".to_owned())
        );
    }

    #[test]
    fn a_request_for_a_tool_cut_short_is_an_error() {
        let cut = ANTHROPIC_ASKS.replace(
            r#""stop_reason":"tool_use""#,
            r#""stop_reason":"max_tokens""#,
        );
        assert_eq!(
            tool_reply(Provider::Anthropic, &cut, 0),
            Err("Anthropic was cut short while asking for a tool.".to_owned())
        );
        let cut = OLLAMA_ASKS.replace(r#""done_reason":"stop""#, r#""done_reason":"length""#);
        assert_eq!(
            tool_reply(Provider::Ollama, &cut, 0),
            Err("Ollama was cut short while asking for a tool.".to_owned())
        );
    }

    #[test]
    fn a_model_of_ollama_that_takes_no_tools_is_told_in_clear_words() {
        let refused =
            r#"{"error":"registry.ollama.ai/library/gemma3:latest does not support tools"}"#;
        let refused = tool_reply(Provider::Ollama, refused, 0).map_err(tool_failure);
        assert_eq!(
            refused,
            Err("This model cannot use tools: choose another one.".to_owned())
        );
        // Any other error is told as it is.
        let other = tool_reply(Provider::Ollama, r#"{"error":"model 'm' not found"}"#, 0);
        assert_eq!(
            other.map_err(tool_failure),
            Err("Ollama: model 'm' not found".to_owned())
        );
    }

    #[test]
    fn a_talk_with_tools_goes_through_curl_and_the_tools_asked_for_come_back() {
        if !curl_is_here() {
            return;
        }
        let (url, asked) = listener(OLLAMA_ASKS);
        let body = tool_body(&settings(Provider::Ollama), "be brief", &talk(), &offered());
        let headers = ["content-type: application/json".to_owned()];
        let answer = fetch(
            Provider::Ollama,
            &config(&url, &headers, Some(&body.to_string())),
        );
        let asked = asked.join().expect("listener");
        let (_, sent) = asked.split_once("\r\n\r\n").expect("a request");
        assert_eq!(serde_json::from_str::<Value>(sent).ok(), Some(body));
        let Ok(Turn::Asked { said, calls }) =
            tool_reply(Provider::Ollama, &answer.unwrap_or_default(), 1)
        else {
            panic!("no tool was asked for");
        };
        assert_eq!(said, "");
        let ids: Vec<_> = calls.iter().map(|call| call.id.as_str()).collect();
        assert_eq!(ids, ["call-2", "call-3"]);
    }
}
