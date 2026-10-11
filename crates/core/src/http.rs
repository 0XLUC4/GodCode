//! Requests written in a `.http` file: a method and an address, headers, a blank line, a
//! body. `###` on a line of its own separates one request from the next.
//!
//! `@name = value` on a line of its own, anywhere in the file, names a value: `{{name}}`
//! stands for it in every request. The method `GRAPHQL` posts its body as a query, with
//! the variables written as JSON after a blank line when there are some.

/// A request to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub url: String,
    /// As written: `Name: value`.
    pub headers: Vec<String>,
    pub body: String,
}

/// A line that names a value: `@host = https://a.test`.
fn named(content: &str) -> Option<(&str, &str)> {
    let (name, value) = content.trim().strip_prefix('@')?.split_once('=')?;
    let name = name.trim();
    let word = !name.is_empty() && name.chars().all(|ch| ch.is_alphanumeric() || ch == '_');
    word.then_some((name, value.trim()))
}

/// `written` between quotes, as JSON wants a text.
fn quoted(written: &str) -> String {
    let mut quoted = String::from('"');
    for ch in written.chars() {
        match ch {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => {}
            '\t' => quoted.push_str("\\t"),
            ch => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

/// What is left of a `http-client.env.json` file being read.
struct Json<'a> {
    rest: &'a str,
}

impl Json<'_> {
    fn eat(&mut self, wanted: char) -> bool {
        self.rest = self.rest.trim_start();
        let rest = self.rest.strip_prefix(wanted);
        rest.inspect(|rest| self.rest = rest).is_some()
    }

    /// A text between quotes, its escapes read.
    fn text(&mut self) -> Option<String> {
        let mut chars = self.rest.trim_start().strip_prefix('"')?.chars();
        let mut text = String::new();
        loop {
            match chars.next()? {
                '"' => break,
                '\\' => text.push(match chars.next()? {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'u' => {
                        let unit = hex(&mut chars)?;
                        // A char past U+FFFF is written as two halves, each its own `\u`.
                        let mut after = chars.clone();
                        let low = (after.next() == Some('\\') && after.next() == Some('u'))
                            .then(|| hex(&mut after))
                            .flatten();
                        let code = match low {
                            Some(low)
                                if (0xD800..0xDC00).contains(&unit)
                                    && (0xDC00..0xE000).contains(&low) =>
                            {
                                chars = after;
                                0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00)
                            }
                            _ => unit,
                        };
                        char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER)
                    }
                    escaped @ ('"' | '\\' | '/') => escaped,
                    _ => return None,
                }),
                ch => text.push(ch),
            }
        }
        self.rest = chars.as_str();
        Some(text)
    }

    /// Goes past an object or an array, whatever it holds.
    fn skip(&mut self) -> Option<()> {
        let mut depth = 0usize;
        loop {
            self.rest = self.rest.trim_start();
            let mut chars = self.rest.chars();
            match chars.next()? {
                '"' => {
                    self.text()?;
                    continue;
                }
                '{' | '[' => depth += 1,
                '}' | ']' => depth = depth.checked_sub(1)?,
                _ => {}
            }
            self.rest = chars.as_str();
            if depth == 0 {
                return Some(());
            }
        }
    }

    /// A value: a text, or a number or a boolean as written. `Some(None)` is a value that
    /// is none of these and was gone past.
    fn value(&mut self) -> Option<Option<String>> {
        self.rest = self.rest.trim_start();
        match self.rest.chars().next()? {
            '"' => return self.text().map(Some),
            '{' | '[' => return self.skip().map(|()| None),
            _ => {}
        }
        let ends = |ch: char| matches!(ch, ',' | '}' | ']') || ch.is_whitespace();
        let end = self.rest.find(ends).unwrap_or(self.rest.len());
        let (word, rest) = self.rest.split_at(end);
        self.rest = rest;
        let number = word.starts_with(|ch: char| ch == '-' || ch.is_ascii_digit())
            && word
                .chars()
                .all(|ch| ch.is_ascii_digit() || "+-.eE".contains(ch))
            && word.parse::<f64>().is_ok();
        match word {
            "null" => Some(None),
            "true" | "false" => Some(Some(word.to_owned())),
            _ => number.then(|| Some(word.to_owned())),
        }
    }

    /// An object, `entry` reading the value of each of its names.
    fn object(&mut self, mut entry: impl FnMut(&mut Self, String) -> Option<()>) -> Option<()> {
        if !self.eat('{') {
            return None;
        }
        if self.eat('}') {
            return Some(());
        }
        loop {
            let name = self.text()?;
            if !self.eat(':') {
                return None;
            }
            entry(self, name)?;
            if !self.eat(',') {
                return self.eat('}').then_some(());
            }
        }
    }
}

/// The four hex digits after `\u`.
fn hex(chars: &mut std::str::Chars<'_>) -> Option<u32> {
    (0..4).try_fold(0, |code, _| Some(code * 16 + chars.next()?.to_digit(16)?))
}

/// A set of values chosen by name: the name, then each value under its own name.
pub type Environment = (String, Vec<(String, String)>);

/// The environments a `http-client.env.json` file holds, in the order it writes them:
/// `{ "dev": { "host": "http://localhost:3000" }, "prod": { ... } }`.
///
/// A value is a text, or a number or a boolean kept as written; one that is an object, an
/// array or `null` is left out. A file that is not well formed holds no environment.
pub fn environments(json: &str) -> Vec<Environment> {
    let mut json = Json {
        rest: json.trim_start_matches('\u{feff}'),
    };
    let mut all = Vec::new();
    let read = json.object(|json, name| {
        // What is no object at this level is no environment.
        if !json.rest.trim_start().starts_with('{') {
            return json.value().map(drop);
        }
        let mut values: Vec<(String, String)> = Vec::new();
        json.object(|json, name| {
            let Some(value) = json.value()? else {
                return Some(());
            };
            // A name written twice keeps its place and takes the last value.
            match values.iter_mut().find(|(known, _)| *known == name) {
                Some(known) => known.1 = value,
                None => values.push((name, value)),
            }
            Some(())
        })?;
        all.push((name, values));
        Some(())
    });
    if read.is_none() || !json.rest.trim().is_empty() {
        return Vec::new();
    }
    all
}

/// The request written around line `line` (counted from 0) of `text`.
pub fn request_at(text: &str, line: usize) -> Option<Request> {
    request_in(text, line, &[])
}

/// The request written around line `line` (counted from 0) of `text`, in an environment:
/// its values win over the values the file names the same.
pub fn request_in(text: &str, line: usize, environment: &[(String, String)]) -> Option<Request> {
    let mut request = written_at(text, line)?;
    // The values the file names stand in for their names, wherever they are named, but
    // for the names the environment has. Its values come last, so that they stand in a
    // value of the file that is written with them too: `@api = {{host}}/api`.
    let chosen = |name: &str| environment.iter().any(|(known, _)| known == name);
    let written = text.lines().filter_map(named);
    let values: Vec<(&str, &str)> = written
        .filter(|(name, _)| !chosen(name))
        .chain(environment.iter().map(|(name, value)| (&**name, &**value)))
        .collect();
    let filled = |written: &str| {
        let fill = |written: String, (name, value): &(&str, &str)| {
            written.replace(&format!("{{{{{name}}}}}"), value)
        };
        values.iter().fold(written.to_owned(), fill)
    };
    request.url = filled(&request.url);
    request.body = filled(&request.body);
    request.headers = request
        .headers
        .iter()
        .map(|header| filled(header))
        .collect();
    if request.method == "GRAPHQL" {
        // The query, then after a blank line the variables it takes, as JSON.
        let (query, variables) = match request.body.split_once("\n\n") {
            Some((query, variables)) => (query, variables.trim()),
            None => (request.body.as_str(), ""),
        };
        let query = quoted(query.trim());
        request.body = match variables {
            "" => format!("{{\"query\":{query}}}"),
            variables => format!("{{\"query\":{query},\"variables\":{variables}}}"),
        };
        request.method = "POST".to_owned();
        let typed = |header: &String| header.to_lowercase().starts_with("content-type:");
        if !request.headers.iter().any(typed) {
            request
                .headers
                .push("Content-Type: application/json".to_owned());
        }
    }
    Some(request)
}

/// The request around a line, as it is written.
fn written_at(text: &str, line: usize) -> Option<Request> {
    let lines: Vec<&str> = text.lines().collect();
    let separates = |content: &&str| content.trim_start().starts_with("###");
    let line = line.min(lines.len().saturating_sub(1));
    let start = lines[..=line.min(lines.len().checked_sub(1)?)]
        .iter()
        .rposition(separates)
        .map_or(0, |at| at + 1);
    let end = lines[start..]
        .iter()
        .position(separates)
        .map_or(lines.len(), |at| start + at);
    // A line that names a value is no part of a request.
    let comment = |content: &str| {
        content.starts_with('#') || content.starts_with("//") || named(content).is_some()
    };
    let mut block = lines[start..end]
        .iter()
        .map(|content| content.trim_end())
        .skip_while(|content| content.trim().is_empty() || comment(content.trim_start()));
    let mut first = block.next()?.split_whitespace();
    let word = first.next()?;
    // An address alone is fetched.
    let (method, url) = if word.contains("://") {
        ("GET", word)
    } else {
        (word, first.next()?)
    };
    let mut headers = Vec::new();
    for content in block.by_ref() {
        if content.trim().is_empty() {
            break;
        }
        if !comment(content.trim_start()) {
            headers.push(content.trim().to_owned());
        }
    }
    let body: Vec<&str> = block.collect();
    Some(Request {
        method: method.to_uppercase(),
        url: url.to_owned(),
        headers,
        body: body.join("\n").trim().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_around_a_line_is_read() {
        let text = "# list\nGET https://a.test/items\nAccept: application/json\n\n###\n\n\
                    post https://a.test/items HTTP/1.1\nContent-Type: application/json\n// note\n\n\
                    {\n  \"name\": \"x\"\n}\n\n### last\nhttps://a.test/";
        let first = request_at(text, 0).expect("request");
        assert_eq!(
            (first.method.as_str(), first.url.as_str()),
            ("GET", "https://a.test/items")
        );
        assert_eq!(first.headers, ["Accept: application/json"]);
        assert_eq!(first.body, "");
        let second = request_at(text, 9).expect("request");
        assert_eq!(second.method, "POST");
        assert_eq!(second.headers, ["Content-Type: application/json"]);
        assert_eq!(second.body, "{\n  \"name\": \"x\"\n}");
        let last = request_at(text, 99).expect("request");
        assert_eq!(
            (last.method.as_str(), last.url.as_str()),
            ("GET", "https://a.test/")
        );
        assert_eq!(request_at("", 0), None);
        assert_eq!(request_at("# nothing\n", 0), None);
    }

    #[test]
    fn named_values_stand_in_and_a_query_is_posted() {
        let text = "@host = https://a.test\n@token = abc 123\n\nGET {{host}}/items/{{id}}\n\
                    Authorization: Bearer {{token}}\n\n###\n@id = 7\n\
                    GRAPHQL {{host}}/graphql\n\nquery Item($id: ID!) {\n  item(id: $id) { name \"x\" }\n}\n\n\
                    { \"id\": \"{{id}}\" }\n";
        let first = request_at(text, 3).expect("request");
        // A value named further down, in another request, stands in all the same.
        assert_eq!(first.url, "https://a.test/items/7");
        assert_eq!(first.headers, ["Authorization: Bearer abc 123"]);
        let query = request_at(text, 9).expect("request");
        assert_eq!(
            (query.method.as_str(), query.url.as_str()),
            ("POST", "https://a.test/graphql")
        );
        assert_eq!(query.headers, ["Content-Type: application/json"]);
        assert_eq!(
            query.body,
            "{\"query\":\"query Item($id: ID!) {\\n  item(id: $id) { name \\\"x\\\" }\\n}\",\
             \"variables\":{ \"id\": \"7\" }}"
        );
        // Without variables the query goes alone; a name that names nothing stays.
        let alone = "GRAPHQL https://a.test/q\nContent-Type: application/graphql+json\n\n{ me { {{who}} } }";
        let alone = request_at(alone, 0).expect("request");
        assert_eq!(alone.body, "{\"query\":\"{ me { {{who}} } }\"}");
        assert_eq!(alone.headers, ["Content-Type: application/graphql+json"]);
        assert_eq!(named("@a_b = c = d"), Some(("a_b", "c = d")));
        assert_eq!(
            (named("@ = x"), named("a@b = c"), named("@a b = c")),
            (None, None, None)
        );
    }

    fn pairs(values: &[(&str, &str)]) -> Vec<(String, String)> {
        let owned = |(name, value): &(&str, &str)| (name.to_string(), value.to_string());
        values.iter().map(owned).collect()
    }

    #[test]
    fn environments_are_read_in_the_order_of_the_file() {
        let json = r#"{
            "dev": { "host": "http://localhost:3000", "token": "abc" },
            "prod": { "host": "https://a.test", "token": "xyz" },
            "empty": {}
        }"#;
        let all = environments(json);
        let names: Vec<&str> = all.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["dev", "prod", "empty"]);
        assert_eq!(
            all[0].1,
            pairs(&[("host", "http://localhost:3000"), ("token", "abc")])
        );
        assert_eq!(
            all[1].1,
            pairs(&[("host", "https://a.test"), ("token", "xyz")])
        );
        assert_eq!(all[2].1, []);
        // The same without a space or a line, and behind a byte order mark.
        let tight = "\u{feff}{\"dev\":{\"host\":\"h\",\"zeta\":\"1\",\"alpha\":\"2\"}}";
        assert_eq!(
            environments(tight),
            [(
                "dev".to_owned(),
                pairs(&[("host", "h"), ("zeta", "1"), ("alpha", "2")])
            )]
        );
        assert_eq!(environments("{}"), []);
    }

    #[test]
    fn escapes_in_an_environment_are_read() {
        let json = r#"{ "d\u00e9v": {
            "quoted": "say \"hi\" \\ \/ done",
            "spaced": "a\tb\nc\r\b\f",
            "accent": "caf\u00E9 \u4e16\u754c",
            "plain": "café 世界 😀",
            "pair": "\ud83d\ude00!",
            "half": "\ud83d \ude00 \ud83d\u0041"
        } }"#;
        let all = environments(json);
        assert_eq!(all[0].0, "dév");
        assert_eq!(
            all[0].1,
            pairs(&[
                ("quoted", "say \"hi\" \\ / done"),
                ("spaced", "a\tb\nc\r\u{8}\u{c}"),
                ("accent", "café 世界"),
                ("plain", "café 世界 😀"),
                ("pair", "😀!"),
                // Half of a pair alone is no char.
                ("half", "\u{fffd} \u{fffd} \u{fffd}A"),
            ])
        );
    }

    #[test]
    fn numbers_and_booleans_are_kept_as_written_and_nested_values_skipped() {
        let json = r#"{
            "$schema": "https://a.test/schema.json",
            "version": 2,
            "dev": {
                "port": 3000,
                "ratio": -1.50e+3,
                "secure": false,
                "debug": true,
                "nothing": null,
                "headers": { "a": [1, {"b": "}]\""}], "c": null },
                "list": ["x", "]", ["y"]],
                "none": [],
                "last": "kept",
                "port": 8080
            }
        }"#;
        // What is no object beside the environments is no environment; a name written
        // twice keeps its place and takes its last value.
        assert_eq!(
            environments(json),
            [(
                "dev".to_owned(),
                pairs(&[
                    ("port", "8080"),
                    ("ratio", "-1.50e+3"),
                    ("secure", "false"),
                    ("debug", "true"),
                    ("last", "kept"),
                ])
            )]
        );
    }

    #[test]
    fn a_malformed_environment_file_holds_no_environment() {
        for json in [
            "",
            "   ",
            "null",
            "[]",
            "\"dev\"",
            "{",
            "{ \"dev\" }",
            "{ \"dev\": }",
            "{ \"dev\": { \"host\": \"h\" }",
            "{ \"dev\": { \"host\": \"h\" } } more",
            "{ \"dev\": { \"host\": \"h\", } }",
            "{ \"dev\": { \"host\": \"h\" }, }",
            "{ \"dev\": { \"host\" \"h\" } }",
            "{ \"dev\": { \"host\": \"h\" \"port\": 1 } }",
            "{ \"dev\": { host: \"h\" } }",
            "{ \"dev\": { \"host\": 'h' } }",
            "{ \"dev\": { \"host\": \"unclosed } }",
            "{ \"dev\": { \"host\": \"bad \\x escape\" } }",
            "{ \"dev\": { \"host\": \"\\u12\" } }",
            "{ \"dev\": { \"host\": \"\\u00zz\" } }",
            "{ \"dev\": { \"host\": \"h\\",
            "{ \"dev\": { \"host\": yes } }",
            "{ \"dev\": { \"port\": 1x } }",
            "{ \"dev\": { \"port\": inf } }",
            "{ \"dev\": { \"port\": -nan } }",
            "{ \"dev\": { \"list\": [1, 2 } }",
            "{ \"dev\": { \"deep\": { \"a\": 1 } }",
        ] {
            assert_eq!(environments(json), [], "{json}");
        }
        // Brackets nested past any depth are gone past without trouble.
        let deep = format!("{{ \"dev\": {{ \"deep\": {} }} }}", "[".repeat(200_000));
        assert_eq!(environments(&deep), []);
        let closed = format!(
            "{{ \"dev\": {{ \"deep\": {}{}, \"host\": \"h\" }} }}",
            "[".repeat(200_000),
            "]".repeat(200_000)
        );
        assert_eq!(
            environments(&closed),
            [("dev".to_owned(), pairs(&[("host", "h")]))]
        );
    }

    #[test]
    fn the_values_of_an_environment_win_over_those_of_the_file() {
        let text = "@host = https://a.test\n@token = abc\n@api = {{host}}/api\n\n\
                    POST {{api}}/items/{{id}}\nAuthorization: Bearer {{token}}\n\n\
                    { \"port\": {{port}}, \"who\": \"{{who}}\" }";
        let json = r#"{ "dev": { "host": "http://localhost:3000", "port": 3000, "id": "7" } }"#;
        let all = environments(json);
        let request = request_in(text, 4, &all[0].1).expect("request");
        // The environment's host stands in, even inside a value the file wrote with it;
        // the token is the file's, the environment having none.
        assert_eq!(request.url, "http://localhost:3000/api/items/7");
        assert_eq!(request.headers, ["Authorization: Bearer abc"]);
        // A name that neither names stays as written.
        assert_eq!(request.body, "{ \"port\": 3000, \"who\": \"{{who}}\" }");
        assert_eq!(request_in("", 0, &all[0].1), None);
    }

    #[test]
    fn a_request_without_an_environment_is_the_request_of_the_file() {
        let text = "@host = https://a.test\n\nGET {{host}}/items/{{id}}\nAccept: {{kind}}\n\n\
                    ###\nGRAPHQL {{host}}/graphql\n\n{ me { name } }";
        for line in [2, 7] {
            assert_eq!(request_at(text, line), request_in(text, line, &[]));
        }
        let first = request_at(text, 2).expect("request");
        assert_eq!(first.url, "https://a.test/items/{{id}}");
        assert_eq!(first.headers, ["Accept: {{kind}}"]);
        // A query is posted in an environment as it is without one.
        let query = request_in(text, 7, &pairs(&[("host", "http://localhost")])).expect("request");
        assert_eq!(
            (query.method.as_str(), query.url.as_str()),
            ("POST", "http://localhost/graphql")
        );
        assert_eq!(query.body, "{\"query\":\"{ me { name } }\"}");
    }
}
