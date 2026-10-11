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

/// The request written around line `line` (counted from 0) of `text`.
pub fn request_at(text: &str, line: usize) -> Option<Request> {
    let mut request = written_at(text, line)?;
    // The values the file names stand in for their names, wherever they are named.
    let values: Vec<(&str, &str)> = text.lines().filter_map(named).collect();
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
}
