//! Requests written in a `.http` file: a method and an address, headers, a blank line, a
//! body. `###` on a line of its own separates one request from the next.

/// A request to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub url: String,
    /// As written: `Name: value`.
    pub headers: Vec<String>,
    pub body: String,
}

/// The request written around line `line` (counted from 0) of `text`.
pub fn request_at(text: &str, line: usize) -> Option<Request> {
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
    let comment = |content: &str| content.starts_with('#') || content.starts_with("//");
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
}
