//! Live preview: the open folder served to the browser of this computer, pages reloading
//! by themselves when a file is saved.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Asked for by the pages to know when to reload.
const VERSION_PATH: &str = "/__godcode_live";
/// Added to every page: it reloads when the version it is told changes.
const RELOADER: &str = "<script>(()=>{let seen;setInterval(async()=>{try{const now=await(await \
                        fetch('/__godcode_live')).text();if(seen===undefined)seen=now;else \
                        if(now!==seen)location.reload()}catch(e){}},500)})()</script>";

/// A folder being served.
pub struct Live {
    pub port: u16,
    pub root: PathBuf,
    /// Bumped at every save: pages reload when it moves.
    version: Arc<AtomicU64>,
}

impl Live {
    /// Serve `root` on a free port of this computer only.
    pub fn start(root: PathBuf) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let version = Arc::new(AtomicU64::new(0));
        let (served, counter) = (root.clone(), Arc::clone(&version));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (root, version) = (served.clone(), Arc::clone(&counter));
                std::thread::spawn(move || answer(stream, &root, &version));
            }
        });
        Ok(Self {
            port,
            root,
            version,
        })
    }

    /// A file was saved: the pages on show reload.
    pub fn changed(&self) {
        self.version.fetch_add(1, Ordering::Relaxed);
    }

    /// The address of `file`, when it is in the folder served.
    pub fn url_of(&self, file: &Path) -> Option<String> {
        let inside = file.strip_prefix(&self.root).ok()?;
        let path = inside.to_string_lossy().replace('\\', "/");
        Some(format!("http://127.0.0.1:{}/{path}", self.port))
    }
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" | "webmanifest" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        "txt" | "md" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// The file of `root` a request path names; `None` for a path that would leave the folder.
fn resolve(root: &Path, requested: &str) -> Option<PathBuf> {
    let path = requested.split(['?', '#']).next().unwrap_or("");
    let relative = Path::new(path.trim_start_matches('/'));
    let inside = relative
        .components()
        .all(|part| matches!(part, Component::Normal(_)));
    let file = inside.then(|| root.join(relative))?;
    Some(if file.is_dir() {
        file.join("index.html")
    } else {
        file
    })
}

fn answer(mut stream: TcpStream, root: &Path, version: &AtomicU64) {
    let Ok(mut reader) = stream.try_clone().map(BufReader::new) else {
        return;
    };
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    // The headers are read to their end: closing on unread bytes resets the connection,
    // and the browser loses the answer.
    let mut header = String::new();
    while matches!(reader.read_line(&mut header), Ok(read) if read > 0) && header.trim() != "" {
        header.clear();
    }
    let requested = first.split_whitespace().nth(1).unwrap_or("/");
    let (status, kind, body) = if requested == VERSION_PATH {
        let now = version.load(Ordering::Relaxed).to_string();
        ("200 OK", "text/plain", now.into_bytes())
    } else {
        let file = resolve(root, &requested.replace("%20", " "));
        match file
            .as_deref()
            .and_then(|file| Some((file, std::fs::read(file).ok()?)))
        {
            Some((file, mut bytes)) => {
                let kind = content_type(file);
                if kind.starts_with("text/html") {
                    bytes.extend_from_slice(RELOADER.as_bytes());
                }
                ("200 OK", kind, bytes)
            }
            None => ("404 Not Found", "text/plain", b"Not found".to_vec()),
        }
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: \
         no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    // A browser that went away is no error worth telling.
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
    // Said out loud: a shell started meanwhile holds a copy of the socket, and the answer
    // would end only when that shell does.
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_served_and_pages_are_told_to_reload() {
        use std::io::Read as _;
        let root = std::env::temp_dir().join(format!("godcode-live-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("folder");
        std::fs::write(root.join("index.html"), "<h1>hi</h1>").expect("page");
        let live = Live::start(root.clone()).expect("server");
        let fetch = |path: &str| {
            let mut stream = TcpStream::connect(("127.0.0.1", live.port)).expect("connect");
            write!(stream, "GET {path} HTTP/1.1\r\n\r\n").expect("ask");
            let mut answer = String::new();
            stream.read_to_string(&mut answer).expect("answer");
            answer
        };
        let page = fetch("/");
        assert!(page.starts_with("HTTP/1.1 200 OK"), "{page}");
        assert!(page.contains("<h1>hi</h1>") && page.contains("__godcode_live"));
        assert!(fetch("/missing.css").starts_with("HTTP/1.1 404"));
        assert!(fetch("/../x").starts_with("HTTP/1.1 404"));
        assert!(fetch(VERSION_PATH).ends_with("\r\n\r\n0"));
        live.changed();
        assert!(fetch(VERSION_PATH).ends_with("\r\n\r\n1"));
        assert_eq!(
            live.url_of(&root.join("index.html")),
            Some(format!("http://127.0.0.1:{}/index.html", live.port))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn requests_stay_inside_the_folder() {
        let root = Path::new("site");
        assert_eq!(
            resolve(root, "/a/b.css?v=2"),
            Some(root.join("a").join("b.css"))
        );
        assert_eq!(resolve(root, "/../secret.txt"), None);
        assert_eq!(resolve(root, "/a/../../secret.txt"), None);
        assert_eq!(
            content_type(Path::new("x.HTML")),
            "application/octet-stream"
        );
        assert_eq!(
            content_type(Path::new("x.html")),
            "text/html; charset=utf-8"
        );
    }
}
