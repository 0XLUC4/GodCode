//! A pretend API served on this computer, from the answers written in a `.http` file:
//! a front end can be built before the real server exists.
//!
//! ```text
//! ### One user
//! GET /users/{id}
//!
//! HTTP/1.1 200 OK
//! Content-Type: application/json
//!
//! {"id": 1, "name": "Ada"}
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub name: String,
    pub method: String,
    /// `/users/{id}`: a `{…}` part matches any one segment, a final `*` matches the rest.
    pub path: String,
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Blocks that have both a request and an answer; the others are left to the HTTP client.
pub fn parse(text: &str) -> Vec<Route> {
    text.split("\n###")
        .filter_map(|block| {
            let block = block.trim_start_matches("###");
            let mut lines = block.lines();
            let name = lines.next().unwrap_or_default().trim().to_owned();
            let mut lines = lines.skip_while(|line| {
                let line = line.trim();
                line.is_empty() || line.starts_with('#') || line.starts_with("//")
            });
            let request = lines.next()?.trim();
            let (method, target) = request.split_once(char::is_whitespace)?;
            if !method.chars().all(|ch| ch.is_ascii_uppercase()) {
                return None;
            }
            let target = target.split_whitespace().next()?;
            // A full address keeps only its path.
            let path = match target.split_once("://") {
                Some((_, rest)) => rest.find('/').map_or("/", |at| &rest[at..]),
                None => target,
            };
            let path = path.split('?').next().unwrap_or(path).to_owned();
            let rest: Vec<&str> = lines.collect();
            let start = rest
                .iter()
                .position(|line| line.trim_start().starts_with("HTTP/"))?;
            let status_line = rest[start].trim();
            let mut parts = status_line.splitn(3, ' ');
            parts.next();
            let status: u16 = parts.next()?.parse().ok()?;
            let reason = parts.next().unwrap_or("").to_owned();
            let mut headers = Vec::new();
            let mut body_start = rest.len();
            for (offset, line) in rest[start + 1..].iter().enumerate() {
                if line.trim().is_empty() {
                    body_start = start + 2 + offset;
                    break;
                }
                if let Some((key, value)) = line.split_once(':') {
                    headers.push((key.trim().to_owned(), value.trim().to_owned()));
                }
            }
            let body = rest
                .get(body_start..)
                .unwrap_or_default()
                .join("\n")
                .trim()
                .to_owned();
            Some(Route {
                name: if name.is_empty() {
                    format!("{method} {path}")
                } else {
                    name
                },
                method: method.to_owned(),
                path,
                status,
                reason,
                headers,
                body,
            })
        })
        .collect()
}

fn matches(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.trim_matches('/').split('/').collect();
    let path: Vec<&str> = path.trim_matches('/').split('/').collect();
    for (index, part) in pattern.iter().enumerate() {
        if *part == "*" && index == pattern.len() - 1 {
            return true;
        }
        match path.get(index) {
            Some(segment) if part.starts_with('{') && part.ends_with('}') => {
                if segment.is_empty() {
                    return false;
                }
            }
            Some(segment) if segment == part => {}
            _ => return false,
        }
    }
    pattern.len() == path.len()
}

/// The route that answers `method path`, exact paths before patterns.
pub fn find<'a>(routes: &'a [Route], method: &str, path: &str) -> Option<&'a Route> {
    let path = path.split('?').next().unwrap_or(path);
    let candidates = routes
        .iter()
        .filter(|route| route.method.eq_ignore_ascii_case(method) && matches(&route.path, path));
    candidates.min_by_key(|route| route.path.contains(['{', '*']))
}

/// One request the server saw.
#[derive(Clone, Debug)]
pub struct Hit {
    pub method: String,
    pub path: String,
    pub status: u16,
}

pub struct MockServer {
    pub port: u16,
    stop: Arc<AtomicBool>,
    routes: Arc<Mutex<Vec<Route>>>,
    log: Arc<Mutex<Vec<Hit>>>,
}

impl MockServer {
    /// Listens on `127.0.0.1:port` (`0`: any free port).
    pub fn start(port: u16, routes: Vec<Route>) -> Result<Self, String> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .map_err(|err| format!("Cannot listen on port {port}: {err}"))?;
        let port = listener.local_addr().map_err(|err| err.to_string())?.port();
        listener
            .set_nonblocking(true)
            .map_err(|err| err.to_string())?;
        let server = Self {
            port,
            stop: Arc::default(),
            routes: Arc::new(Mutex::new(routes)),
            log: Arc::default(),
        };
        let (stop, routes, log) = (
            server.stop.clone(),
            server.routes.clone(),
            server.log.clone(),
        );
        std::thread::Builder::new()
            .name("mock-api".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let (routes, log) = (routes.clone(), log.clone());
                            std::thread::spawn(move || serve(stream, &routes, &log));
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(30)),
                    }
                }
            })
            .map_err(|err| format!("Cannot start the server: {err}"))?;
        Ok(server)
    }

    /// Answers change at once when the file changes.
    pub fn set_routes(&self, routes: Vec<Route>) {
        if let Ok(mut current) = self.routes.lock() {
            *current = routes;
        }
    }

    pub fn log(&self) -> Vec<Hit> {
        self.log.lock().map(|log| log.clone()).unwrap_or_default()
    }

    pub fn url(&self) -> String {
        format!("http://localhost:{}", self.port)
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn serve(stream: TcpStream, routes: &Mutex<Vec<Route>>, log: &Mutex<Vec<Hit>>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    });
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return;
    };
    let mut length = 0usize;
    let mut line = String::new();
    while reader.read_line(&mut line).is_ok_and(|read| read > 0) && line != "\r\n" && line != "\n" {
        if let Some((key, value)) = line.split_once(':')
            && key.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0).min(16 * 1024 * 1024);
        }
        line.clear();
    }
    let mut body = vec![0; length];
    let _ = reader.read_exact(&mut body);

    let cors = "Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: *\r\nAccess-Control-Allow-Headers: *\r\n";
    let (status, reason, headers, body) = if method == "OPTIONS" {
        (204, "No Content".to_owned(), String::new(), String::new())
    } else {
        let routes = routes
            .lock()
            .map(|routes| routes.clone())
            .unwrap_or_default();
        match find(&routes, method, path) {
            Some(route) => {
                let headers: String = route
                    .headers
                    .iter()
                    .filter(|(key, _)| !key.eq_ignore_ascii_case("content-length"))
                    .map(|(key, value)| format!("{key}: {value}\r\n"))
                    .collect();
                (
                    route.status,
                    route.reason.clone(),
                    headers,
                    route.body.clone(),
                )
            }
            None => {
                let known: Vec<String> = routes
                    .iter()
                    .map(|route| format!("{} {}", route.method, route.path))
                    .collect();
                let body = format!(
                    "No answer for {method} {path} in the .http file.\nKnown: {}\n",
                    known.join(", ")
                );
                (
                    404,
                    "Not Found".to_owned(),
                    "Content-Type: text/plain\r\n".to_owned(),
                    body,
                )
            }
        }
    };
    let answer = format!(
        "HTTP/1.1 {status} {reason}\r\n{headers}{cors}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = stream;
    let _ = stream.write_all(answer.as_bytes());
    if let Ok(mut log) = log.lock() {
        log.push(Hit {
            method: method.to_owned(),
            path: path.to_owned(),
            status,
        });
        let excess = log.len().saturating_sub(200);
        log.drain(..excess);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
### All users
GET http://localhost:8080/users

HTTP/1.1 200 OK
Content-Type: application/json

[{\"id\": 1}]

### One user
GET /users/{id}

HTTP/1.1 200 OK
Content-Type: application/json

{\"id\": 7}

### Only a request, for the client
POST https://api.example.com/login
Content-Type: application/json

{\"user\": \"ada\"}

### Me
GET /users/me

HTTP/1.1 401 Unauthorized
";

    #[test]
    fn reads_answers_and_skips_plain_requests() {
        let routes = parse(FILE);
        assert_eq!(routes.len(), 3);
        assert_eq!(routes[0].name, "All users");
        assert_eq!(routes[0].path, "/users");
        assert_eq!(routes[0].body, "[{\"id\": 1}]");
        assert_eq!(routes[2].status, 401);
        assert_eq!(routes[2].body, "");
    }

    #[test]
    fn exact_paths_win_over_patterns() {
        let routes = parse(FILE);
        assert_eq!(find(&routes, "GET", "/users/me").unwrap().status, 401);
        assert_eq!(
            find(&routes, "GET", "/users/42?full=1").unwrap().body,
            "{\"id\": 7}"
        );
        assert!(find(&routes, "GET", "/users/1/posts").is_none());
        assert!(find(&routes, "DELETE", "/users/1").is_none());
    }

    #[test]
    fn serves_over_http() {
        let server = MockServer::start(0, parse(FILE)).unwrap();
        let ask = |request: &str| {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port)).unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            let mut answer = String::new();
            stream.read_to_string(&mut answer).unwrap();
            answer
        };
        let answer = ask("GET /users/3 HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(answer.starts_with("HTTP/1.1 200 OK"), "{answer}");
        assert!(answer.contains("Access-Control-Allow-Origin: *"));
        assert!(answer.ends_with("{\"id\": 7}"));
        let missing = ask("GET /nope HTTP/1.1\r\n\r\n");
        assert!(missing.starts_with("HTTP/1.1 404"));
        assert_eq!(server.log().len(), 2);
    }
}
