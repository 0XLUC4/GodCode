//! A WebSocket held open with a server: what is sent, what comes back.
//!
//! The protocol (RFC 6455) is written by hand on a plain TCP connection: there is no TLS
//! here, so only `ws://` addresses work. A thread of its own connects and reads; the
//! window sends, and polls what was heard.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher as _, Hasher as _};
use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write as _};
use std::net::{Shutdown, TcpStream, ToSocketAddrs as _};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long the server has to take the connection, and then to answer the handshake.
const PATIENCE: Duration = Duration::from_secs(10);
/// The longest message taken from a server, in bytes.
const MAX_MESSAGE: usize = 16 * 1024 * 1024;
/// The longest answer to the handshake, in bytes.
const MAX_HEAD: usize = 16 * 1024;
/// What the server adds to the key before hashing it, to prove it speaks WebSocket.
const MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

const CONTINUATION: u8 = 0x0;
const TEXT: u8 = 0x1;
const BINARY: u8 = 0x2;
const CLOSE: u8 = 0x8;
const PING: u8 = 0x9;
const PONG: u8 = 0xA;

/// What a socket says.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The server took the connection: messages can be sent.
    Opened,
    /// A text message; bytes that are not UTF-8 show as `�`.
    Text(String),
    /// A binary message, of that many bytes.
    Binary(usize),
    /// The conversation ended, and why, in words. Nothing follows.
    Closed(String),
    /// The conversation could not start or broke, and why. Nothing follows.
    Failed(String),
}

/// A WebSocket and the thread that listens to it.
#[allow(dead_code)]
pub struct Socket {
    incoming: Receiver<Event>,
    link: Arc<Mutex<Link>>,
}

/// What the window and the listening thread share of the connection.
#[derive(Default)]
struct Link {
    /// The connection, once made: the thread reads it, frames are written here.
    stream: Option<Arc<TcpStream>>,
    /// The handshake is done and nobody closed yet.
    open: bool,
    /// Closed from this side: the thread has nothing more to do.
    let_go: bool,
}

impl Link {
    fn write(&mut self, opcode: u8, payload: &[u8]) -> io::Result<()> {
        let Some(mut stream) = self.stream.as_deref() else {
            return Err(ErrorKind::NotConnected.into());
        };
        stream.write_all(&frame(true, opcode, payload, Some(mask())))
    }

    /// End the connection: the thread reading it sees it end too.
    fn hang_up(&mut self) {
        self.open = false;
        if let Some(stream) = &self.stream {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

/// A thread that panicked while holding the link left it in a state still worth reading.
fn lock(link: &Mutex<Link>) -> MutexGuard<'_, Link> {
    link.lock().unwrap_or_else(PoisonError::into_inner)
}

#[allow(dead_code)]
impl Socket {
    /// Open `url` (`ws://host[:port]/path?query`), with the extra `headers` written
    /// `Name: value`. Returns at once; `Event::Opened` or `Event::Failed` follows. `wake`
    /// is called whenever there is something to poll.
    pub fn open(
        url: &str,
        headers: &[String],
        wake: impl Fn() + Send + 'static,
    ) -> Result<Self, String> {
        let address = parse_url(url)?;
        let key = base64(&[random().to_be_bytes(), random().to_be_bytes()].concat());
        let request = request(&address, &key, headers)?;
        let link = Arc::new(Mutex::new(Link::default()));
        let (sender, incoming) = channel();
        let shared = Arc::clone(&link);
        std::thread::Builder::new()
            .name("websocket".to_owned())
            .spawn(move || {
                let tell = |event| {
                    let _ = sender.send(event);
                    wake();
                };
                let last = converse(&address, &request, &key, &shared, &tell)
                    .unwrap_or_else(Event::Failed);
                let mut link = lock(&shared);
                let last = if link.let_go {
                    Event::Closed("closed here".to_owned())
                } else {
                    last
                };
                link.hang_up();
                drop(link);
                tell(last);
            })
            .map_err(|err| format!("Cannot start listening to the socket: {err}"))?;
        Ok(Self { incoming, link })
    }

    /// Send a text message.
    pub fn send(&mut self, text: &str) -> Result<(), String> {
        let mut link = lock(&self.link);
        if !link.open {
            return Err("The socket is not open".to_owned());
        }
        link.write(TEXT, text.as_bytes()).map_err(|err| {
            // Half a frame may be gone: nothing that follows would be understood.
            link.hang_up();
            format!("Cannot send: {err}")
        })
    }

    /// What was heard since the last call, in order.
    pub fn poll(&mut self) -> Vec<Event> {
        self.incoming.try_iter().collect()
    }

    /// Whether messages can be sent now.
    pub fn is_open(&self) -> bool {
        lock(&self.link).open
    }

    /// Send a close frame, then let go; also done when the socket is dropped.
    /// `Event::Closed` follows.
    pub fn close(&mut self) {
        let mut link = lock(&self.link);
        link.let_go = true;
        if link.open {
            // A server already gone cannot be told.
            let _ = link.write(CLOSE, &1000_u16.to_be_bytes());
        }
        link.hang_up();
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.close();
    }
}

/// The whole life of a connection, on its thread: how it ended, or why it failed.
fn converse(
    address: &Address,
    request: &str,
    key: &str,
    link: &Mutex<Link>,
    tell: &dyn Fn(Event),
) -> Result<Event, String> {
    let lost = || Event::Closed("the connection was lost".to_owned());
    let stream = Arc::new(connect(address)?);
    let broken = |err: io::Error| format!("Cannot use the connection: {err}");
    stream.set_read_timeout(Some(PATIENCE)).map_err(broken)?;
    stream.set_write_timeout(Some(PATIENCE)).map_err(broken)?;
    // Small messages leave at once instead of waiting for more to send.
    let _ = stream.set_nodelay(true);
    {
        let mut link = lock(link);
        if link.let_go {
            return Ok(lost());
        }
        link.stream = Some(Arc::clone(&stream));
    }
    (&*stream)
        .write_all(request.as_bytes())
        .map_err(|err| format!("Cannot send the handshake: {err}"))?;
    let mut reader = BufReader::new(&*stream);
    check_response(&read_head(&mut reader)?, key)?;
    // From here on a silent server is a quiet one, not a broken one.
    stream.set_read_timeout(None).map_err(broken)?;
    {
        let mut link = lock(link);
        link.open = !link.let_go;
    }
    tell(Event::Opened);

    let mut partial = None;
    loop {
        let frame = match read_frame(&mut reader) {
            Ok(frame) => frame,
            Err(err) if err.kind() == ErrorKind::InvalidData => return Err(err.to_string()),
            Err(_) => return Ok(lost()),
        };
        match frame.opcode {
            CONTINUATION | TEXT | BINARY => {
                if let Some(message) = assemble(&mut partial, frame)? {
                    tell(message);
                }
            }
            PING => {
                if lock(link).write(PONG, &frame.payload).is_err() {
                    return Ok(lost());
                }
            }
            PONG => {}
            CLOSE => {
                // The close is answered with its own code, as the protocol asks.
                let code = frame.payload.get(..2).unwrap_or_default();
                let _ = lock(link).write(CLOSE, code);
                return Ok(Event::Closed(close_reason(&frame.payload)));
            }
            unknown => return Err(format!("The server sent a frame of unknown kind {unknown}")),
        }
    }
}

/// Where a `ws://` address leads.
#[derive(Debug, PartialEq, Eq)]
struct Address {
    host: String,
    port: u16,
    /// The path and the query, as asked from the server.
    path: String,
}

impl Address {
    /// The `Host` header: the port is left out when it is the usual one.
    fn host_header(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == 80 {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

fn parse_url(url: &str) -> Result<Address, String> {
    let malformed = || format!("Not a WebSocket address (ws://host:port/path): {url}");
    let url = url.trim();
    let (scheme, rest) = url.split_once("://").ok_or_else(malformed)?;
    if scheme.eq_ignore_ascii_case("wss") {
        return Err(format!(
            "No TLS here, only ws:// works: use a local server or a ws:// address instead of {url}"
        ));
    }
    if !scheme.eq_ignore_ascii_case("ws") || rest.contains(char::is_whitespace) {
        return Err(malformed());
    }
    // What follows a `#` is never sent to a server.
    let rest = rest.split('#').next().unwrap_or_default();
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(split);
    let (host, port) = match authority.strip_prefix('[') {
        // An IPv6 address, between brackets to tell its colons from the one of the port.
        Some(bracketed) => {
            let (host, after) = bracketed.split_once(']').ok_or_else(malformed)?;
            (host, after.strip_prefix(':'))
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    if host.is_empty() || host.contains('@') {
        return Err(malformed());
    }
    let port = match port {
        Some(port) => port.parse().map_err(|_| malformed())?,
        None => 80,
    };
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    Ok(Address {
        host: host.to_owned(),
        port,
        path,
    })
}

/// The request that asks a server to switch to WebSocket.
fn request(address: &Address, key: &str, headers: &[String]) -> Result<String, String> {
    let mut text = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n",
        address.path,
        address.host_header()
    );
    for header in headers {
        if !header.contains(':') || header.contains(['\r', '\n']) {
            return Err(format!("Not a header (Name: value): {header}"));
        }
        text.push_str(header.trim());
        text.push_str("\r\n");
    }
    text.push_str("\r\n");
    Ok(text)
}

/// Connect to the first address of the host that answers. They are all tried at once:
/// `localhost` is two addresses, and a server often listens on one only, the other taking
/// seconds to refuse.
fn connect(address: &Address) -> Result<TcpStream, String> {
    let found = (address.host.as_str(), address.port)
        .to_socket_addrs()
        .map_err(|err| format!("Cannot find {}: {err}", address.host))?;
    let (sender, answers) = channel();
    for candidate in found {
        let sender = sender.clone();
        // An address that cannot be tried is one that did not answer.
        let _ = std::thread::Builder::new().spawn(move || {
            let answer = TcpStream::connect_timeout(&candidate, PATIENCE)
                .map_err(|err| format!("Cannot reach {candidate}: {err}"));
            let _ = sender.send(answer);
        });
    }
    drop(sender);
    let mut failure = format!("Cannot find {}", address.host);
    for answer in answers {
        match answer {
            Ok(stream) => return Ok(stream),
            Err(why) => failure = why,
        }
    }
    Err(failure)
}

/// The answer to the handshake, up to the empty line that ends it.
fn read_head(reader: &mut impl BufRead) -> Result<String, String> {
    let mut head = Vec::new();
    for byte in reader.bytes().take(MAX_HEAD) {
        head.push(byte.map_err(|err| format!("The server did not answer the handshake: {err}"))?);
        if head.ends_with(b"\r\n\r\n") {
            return Ok(String::from_utf8_lossy(&head).into_owned());
        }
    }
    Err("The server did not answer as a WebSocket server".to_owned())
}

/// Whether the server agreed to switch, and proved it read the key.
fn check_response(head: &str, key: &str) -> Result<(), String> {
    let mut lines = head.lines();
    let status = lines.next().unwrap_or_default().trim();
    if status.split_whitespace().nth(1) != Some("101") {
        return Err(format!("The server did not open a WebSocket: {status}"));
    }
    let accepted = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("Sec-WebSocket-Accept"))
        .map(|(_, value)| value.trim());
    if accepted != Some(accept_for(key).as_str()) {
        return Err("The server answered the handshake with the wrong key".to_owned());
    }
    Ok(())
}

/// What a server must answer to `key` in `Sec-WebSocket-Accept`.
fn accept_for(key: &str) -> String {
    base64(&sha1(format!("{key}{MAGIC}").as_bytes()))
}

/// One frame of the protocol: a message, a piece of one, or a control frame.
#[derive(Debug, PartialEq, Eq)]
struct Frame {
    /// The last piece of its message.
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
}

/// The bytes of a frame. A client masks what it sends, a server does not.
fn frame(fin: bool, opcode: u8, payload: &[u8], mask: Option<[u8; 4]>) -> Vec<u8> {
    let masked = if mask.is_some() { 0x80 } else { 0 };
    let mut bytes = vec![u8::from(fin) << 7 | opcode];
    match payload.len() {
        length @ 0..=125 => bytes.push(masked | length as u8),
        length @ 126..=0xFFFF => {
            bytes.push(masked | 126);
            bytes.extend((length as u16).to_be_bytes());
        }
        length => {
            bytes.push(masked | 127);
            bytes.extend((length as u64).to_be_bytes());
        }
    }
    match mask {
        Some(mask) => {
            bytes.extend(mask);
            bytes.extend(
                payload
                    .iter()
                    .zip(mask.iter().cycle())
                    .map(|(byte, key)| byte ^ key),
            );
        }
        None => bytes.extend(payload),
    }
    bytes
}

fn invalid(why: &str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, why)
}

fn too_big() -> String {
    format!(
        "The server sent a message of more than {} MB",
        MAX_MESSAGE / (1024 * 1024)
    )
}

/// Read one frame, unmasked if it was masked. A frame against the protocol is an error of
/// the kind `InvalidData`; any other error is the connection ending.
fn read_frame(reader: &mut impl Read) -> io::Result<Frame> {
    let mut head = [0_u8; 2];
    reader.read_exact(&mut head)?;
    if head[0] & 0x70 != 0 {
        return Err(invalid("The server used an extension nobody asked for"));
    }
    let fin = head[0] & 0x80 != 0;
    let opcode = head[0] & 0x0F;
    let length = match head[1] & 0x7F {
        126 => {
            let mut bytes = [0_u8; 2];
            reader.read_exact(&mut bytes)?;
            u64::from(u16::from_be_bytes(bytes))
        }
        127 => {
            let mut bytes = [0_u8; 8];
            reader.read_exact(&mut bytes)?;
            u64::from_be_bytes(bytes)
        }
        short => u64::from(short),
    };
    if opcode >= CLOSE && (length > 125 || !fin) {
        return Err(invalid(
            "The server sent a control frame too long or in pieces",
        ));
    }
    if length > MAX_MESSAGE as u64 {
        return Err(invalid(&too_big()));
    }
    let mut mask = [0_u8; 4];
    if head[1] & 0x80 != 0 {
        reader.read_exact(&mut mask)?;
    }
    // Grown as the bytes come: a frame that lies about its length costs no memory.
    let mut payload = Vec::new();
    reader.take(length).read_to_end(&mut payload)?;
    if payload.len() as u64 != length {
        return Err(ErrorKind::UnexpectedEof.into());
    }
    for (byte, key) in payload.iter_mut().zip(mask.iter().cycle()) {
        *byte ^= key;
    }
    Ok(Frame {
        fin,
        opcode,
        payload,
    })
}

/// Add a data frame to the message being put together (its kind and its bytes so far).
/// Gives the message once its last piece is there.
fn assemble(partial: &mut Option<(u8, Vec<u8>)>, frame: Frame) -> Result<Option<Event>, String> {
    let (kind, mut bytes) = match (frame.opcode, partial.take()) {
        (CONTINUATION, Some(started)) => started,
        (CONTINUATION, None) => {
            return Err("The server continued a message it never started".to_owned());
        }
        (_, Some(_)) => {
            return Err("The server started a message before ending the last one".to_owned());
        }
        (kind, None) => (kind, Vec::new()),
    };
    if bytes.len() + frame.payload.len() > MAX_MESSAGE {
        return Err(too_big());
    }
    if bytes.is_empty() {
        bytes = frame.payload;
    } else {
        bytes.extend(frame.payload);
    }
    if !frame.fin {
        *partial = Some((kind, bytes));
        return Ok(None);
    }
    Ok(Some(if kind == TEXT {
        let text = String::from_utf8(bytes)
            .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned());
        Event::Text(text)
    } else {
        Event::Binary(bytes.len())
    }))
}

/// Why a server closed, in words: its code, what the code means, what it added.
fn close_reason(payload: &[u8]) -> String {
    let Some((code, reason)) = payload.split_first_chunk::<2>() else {
        return "no reason given".to_owned();
    };
    let code = u16::from_be_bytes(*code);
    let meaning = match code {
        1000 => " normal closure",
        1001 => " going away",
        1002 => " protocol error",
        1003 => " unsupported data",
        1007 => " invalid payload data",
        1008 => " policy violation",
        1009 => " message too big",
        1010 => " mandatory extension",
        1011 => " internal error",
        1012 => " service restart",
        1013 => " try again later",
        _ => "",
    };
    let reason = String::from_utf8_lossy(reason);
    if reason.is_empty() {
        format!("{code}{meaning}")
    } else {
        format!("{code}{meaning}: {reason}")
    }
}

/// Bits nobody can guess from outside: the hasher of the standard library is seeded by the
/// system for each one made. Enough for keys and masks, which keep nothing secret.
fn random() -> u64 {
    let mut hasher = RandomState::new().build_hasher();
    let now = SystemTime::now().duration_since(UNIX_EPOCH);
    hasher.write_u128(now.map_or(0, |since| since.as_nanos()));
    hasher.finish()
}

fn mask() -> [u8; 4] {
    (random() as u32).to_be_bytes()
}

/// The SHA-1 digest of `message`, as the handshake wants it; no longer safe for secrets.
fn sha1(message: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    // The message, a 1 bit, zeros, then its length in bits: a whole number of blocks.
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend((message.len() as u64 * 8).to_be_bytes());
    for block in padded.chunks_exact(64) {
        let mut words = [0_u32; 80];
        for (word, bytes) in words.iter_mut().zip(block.chunks_exact(4)) {
            *word = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for index in 16..80 {
            let mixed = words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16];
            words[index] = mixed.rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = state;
        for (index, word) in words.iter().enumerate() {
            let (mixed, constant) = match index / 20 {
                0 => ((b & c) | (!b & d), 0x5A82_7999),
                1 => (b ^ c ^ d, 0x6ED9_EBA1),
                2 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6_u32),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(mixed)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        for (sum, value) in state.iter_mut().zip([a, b, c, d, e]) {
            *sum = sum.wrapping_add(value);
        }
    }
    let mut digest = [0_u8; 20];
    for (bytes, value) in digest.chunks_exact_mut(4).zip(state) {
        bytes.copy_from_slice(&value.to_be_bytes());
    }
    digest
}

/// `bytes` in base64, padded with `=`.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::new();
    for chunk in bytes.chunks(3) {
        let group = chunk
            .iter()
            .fold(0_u32, |group, byte| group << 8 | u32::from(*byte))
            << (8 * (3 - chunk.len()));
        for place in 0..4 {
            if place <= chunk.len() {
                text.push(char::from(
                    ALPHABET[(group >> (18 - 6 * place)) as usize & 63],
                ));
            } else {
                text.push('=');
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread::JoinHandle;
    use std::time::Instant;

    use super::*;

    /// Long enough for a slow machine; a test that works never waits that long.
    const DEADLINE: Duration = Duration::from_secs(5);

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn address(host: &str, port: u16, path: &str) -> Address {
        Address {
            host: host.to_owned(),
            port,
            path: path.to_owned(),
        }
    }

    fn data(fin: bool, opcode: u8, payload: &[u8]) -> Frame {
        Frame {
            fin,
            opcode,
            payload: payload.to_vec(),
        }
    }

    /// Wait until `done`, checking often.
    fn wait(what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < DEADLINE, "still waiting for {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Everything the socket says, up to the event that ends it.
    fn listen_to_the_end(socket: &mut Socket) -> Vec<Event> {
        let mut heard = Vec::new();
        wait("the end of the conversation", || {
            heard.extend(socket.poll());
            matches!(heard.last(), Some(Event::Closed(_) | Event::Failed(_)))
        });
        heard
    }

    fn wait_until_open(socket: &mut Socket) {
        let mut heard = Vec::new();
        wait("the socket to open", || {
            heard.extend(socket.poll());
            !heard.is_empty()
        });
        assert_eq!(heard, [Event::Opened]);
        assert!(socket.is_open());
    }

    /// Held by a test while it talks to a server, so that they do it one at a time. On
    /// Windows, with several local connections made at once while another is being
    /// refused, every socket of the process was seen frozen for ten seconds now and then.
    fn alone() -> MutexGuard<'static, ()> {
        static TURN: Mutex<()> = Mutex::new(());
        TURN.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn say(reader: &BufReader<TcpStream>, bytes: &[u8]) {
        let mut stream = reader.get_ref();
        stream.write_all(bytes).unwrap();
    }

    /// A WebSocket server for one client on a free local port: it shakes hands, then does
    /// what `script` says. Gives its address and the thread it runs on.
    fn serve<T: Send + 'static>(
        script: impl FnOnce(BufReader<TcpStream>) -> T + Send + 'static,
    ) -> (String, JoinHandle<T>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/chat?room=1", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(DEADLINE)).unwrap();
            let mut reader = BufReader::new(stream);
            let head = read_head(&mut reader).unwrap();
            assert!(head.starts_with("GET /chat?room=1 HTTP/1.1\r\n"), "{head}");
            let key = head
                .lines()
                .find_map(|line| line.strip_prefix("Sec-WebSocket-Key: "))
                .unwrap();
            let answer = format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
                 Connection: Upgrade\r\nsec-websocket-accept: {}\r\n\r\n",
                accept_for(key)
            );
            say(&reader, answer.as_bytes());
            script(reader)
        });
        (url, server)
    }

    #[test]
    fn sha1_gives_the_known_digests() {
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        // 112 bytes: more than one block.
        let long = b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmno\
                     ijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu";
        assert_eq!(hex(&sha1(long)), "a49b2446a02c645bf419f995b67091253a04a259");
        // 55 and 56 bytes: the padding fits in the block, then no longer does.
        assert_eq!(
            hex(&sha1(&[b'a'; 55])),
            "c1c8bbdc22796e28c0e15163d20899b65621d65a"
        );
        assert_eq!(
            hex(&sha1(&[b'a'; 56])),
            "c2db330f6083854c99d4b5bfb6e8f29f201be699"
        );
    }

    #[test]
    fn base64_pads_what_does_not_fill_three_bytes() {
        let expected = [
            "", "Zg==", "Zm8=", "Zm9v", "Zm9vYg==", "Zm9vYmE=", "Zm9vYmFy",
        ];
        for (length, text) in expected.iter().enumerate() {
            assert_eq!(base64(&b"foobar"[..length]), *text);
        }
        assert_eq!(base64(&[0xFB, 0xFF, 0xFE]), "+//+");
    }

    #[test]
    fn the_accept_value_of_the_example_key_is_the_one_of_the_rfc() {
        assert_eq!(
            accept_for("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn random_keys_differ_and_are_sixteen_bytes_in_base64() {
        let key = base64(&[random().to_be_bytes(), random().to_be_bytes()].concat());
        assert_eq!(key.len(), 24);
        assert!(key.ends_with("=="));
        assert_ne!(random(), random());
    }

    #[test]
    fn an_address_is_split_into_host_port_and_path() {
        assert_eq!(
            parse_url("ws://localhost:8080/chat"),
            Ok(address("localhost", 8080, "/chat"))
        );
        assert_eq!(
            parse_url("ws://example.com"),
            Ok(address("example.com", 80, "/"))
        );
        assert_eq!(
            parse_url(" WS://example.com/a/b?x=1&y=2#top "),
            Ok(address("example.com", 80, "/a/b?x=1&y=2"))
        );
        assert_eq!(
            parse_url("ws://host:9?x=1"),
            Ok(address("host", 9, "/?x=1"))
        );
        assert_eq!(
            parse_url("ws://[::1]:9001/s"),
            Ok(address("::1", 9001, "/s"))
        );
        assert_eq!(parse_url("ws://[::1]"), Ok(address("::1", 80, "/")));
    }

    #[test]
    fn a_malformed_address_is_refused_in_words() {
        let malformed = [
            "",
            "localhost:8080",
            "http://localhost/",
            "ws://",
            "ws:///chat",
            "ws://host:port/",
            "ws://host:99999/",
            "ws://[::1/",
            "ws://user@host/",
            "ws://host/a b",
        ];
        for url in malformed {
            let refusal = parse_url(url).unwrap_err();
            assert!(
                refusal.starts_with("Not a WebSocket address"),
                "{url}: {refusal}"
            );
        }
    }

    #[test]
    fn a_wss_address_is_refused_because_there_is_no_tls() {
        let refusal = Socket::open("wss://example.com/chat", &[], || {})
            .err()
            .unwrap();
        assert!(refusal.contains("No TLS"), "{refusal}");
        assert!(refusal.contains("local server"), "{refusal}");
        assert!(refusal.contains("ws://"), "{refusal}");
    }

    #[test]
    fn the_handshake_asks_for_an_upgrade_with_the_extra_headers() {
        let headers = [
            "Authorization: Bearer abc".to_owned(),
            " X-Room: 1 ".to_owned(),
        ];
        let text = request(&address("localhost", 8080, "/chat?x=1"), "KEY", &headers).unwrap();
        assert_eq!(
            text,
            "GET /chat?x=1 HTTP/1.1\r\nHost: localhost:8080\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Key: KEY\r\nSec-WebSocket-Version: 13\r\n\
             Authorization: Bearer abc\r\nX-Room: 1\r\n\r\n"
        );
        assert_eq!(address("example.com", 80, "/").host_header(), "example.com");
        assert_eq!(address("::1", 9001, "/").host_header(), "[::1]:9001");
    }

    #[test]
    fn a_header_that_is_not_one_or_spans_lines_is_refused() {
        let place = address("localhost", 80, "/");
        assert!(request(&place, "KEY", &["no colon".to_owned()]).is_err());
        assert!(request(&place, "KEY", &["A: b\r\nEvil: yes".to_owned()]).is_err());
    }

    #[test]
    fn the_answer_to_the_handshake_must_be_101_with_the_right_key() {
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let good = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
                    SEC-WEBSOCKET-ACCEPT:  s3pPLMBiTxaQ9kYGzzhZRbK+xOo= \r\n\r\n";
        assert_eq!(check_response(good, key), Ok(()));
        assert_eq!(
            check_response("HTTP/1.1 200 OK\r\n\r\n", key),
            Err("The server did not open a WebSocket: HTTP/1.1 200 OK".to_owned())
        );
        let wrong = "HTTP/1.1 101 Switching Protocols\r\nSec-WebSocket-Accept: nope\r\n\r\n";
        assert!(
            check_response(wrong, key)
                .unwrap_err()
                .contains("wrong key")
        );
        let missing = "HTTP/1.1 101 Switching Protocols\r\n\r\n";
        assert!(check_response(missing, key).is_err());
        assert!(check_response("", key).is_err());
    }

    #[test]
    fn an_endless_answer_to_the_handshake_is_given_up() {
        let mut endless = Cursor::new(vec![b'a'; MAX_HEAD + 100]);
        assert!(read_head(&mut endless).is_err());
        assert!(read_head(&mut Cursor::new(b"HTTP/1.1 101".to_vec())).is_err());
    }

    #[test]
    fn a_frame_has_a_header_as_long_as_its_length_needs() {
        let sizes = [
            (0, 2),
            (125, 2),
            (126, 4),
            (65535, 4),
            (65536, 10),
            (70_000, 10),
        ];
        for (length, header) in sizes {
            let payload = vec![7_u8; length];
            let bytes = frame(true, BINARY, &payload, None);
            assert_eq!(bytes.len(), header + length, "{length} bytes");
            assert_eq!(bytes[0], 0x82);
            let expected = match header {
                2 => length as u8,
                4 => 126,
                _ => 127,
            };
            assert_eq!(bytes[1], expected);
            let read = read_frame(&mut Cursor::new(bytes)).unwrap();
            assert_eq!(read, data(true, BINARY, &payload), "{length} bytes");
        }
        assert_eq!(frame(true, TEXT, b"Hello", None), b"\x81\x05Hello");
        assert_eq!(frame(false, TEXT, b"Hel", None), b"\x01\x03Hel");
        assert_eq!(
            &frame(true, BINARY, &[0; 256], None)[..4],
            [0x82, 126, 1, 0]
        );
    }

    #[test]
    fn a_masked_frame_hides_its_payload_and_reads_back_the_same() {
        // The example of the RFC: "Hello" masked with 37 fa 21 3d.
        let bytes = frame(true, TEXT, b"Hello", Some([0x37, 0xFA, 0x21, 0x3D]));
        assert_eq!(
            bytes,
            [
                0x81, 0x85, 0x37, 0xFA, 0x21, 0x3D, 0x7F, 0x9F, 0x4D, 0x51, 0x58
            ]
        );
        let read = read_frame(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(read, data(true, TEXT, b"Hello"));
        for length in [126, 70_000] {
            let payload: Vec<u8> = (0..length).map(|index| index as u8).collect();
            let bytes = frame(true, BINARY, &payload, Some(mask()));
            assert_eq!(bytes[1] & 0x80, 0x80);
            assert_eq!(
                read_frame(&mut Cursor::new(bytes)).unwrap().payload,
                payload
            );
        }
    }

    #[test]
    fn a_frame_against_the_protocol_is_invalid_and_a_cut_one_is_a_lost_connection() {
        let kind = |bytes: &[u8]| {
            read_frame(&mut Cursor::new(bytes.to_vec()))
                .unwrap_err()
                .kind()
        };
        // A reserved bit, a ping in pieces, a ping too long.
        assert_eq!(kind(b"\xC1\x00"), ErrorKind::InvalidData);
        assert_eq!(kind(b"\x09\x00"), ErrorKind::InvalidData);
        assert_eq!(
            kind(&frame(true, PING, &[0; 126], None)),
            ErrorKind::InvalidData
        );
        // Cut in the header, in the length, in the payload.
        assert_eq!(kind(b"\x81"), ErrorKind::UnexpectedEof);
        assert_eq!(kind(b"\x81\x7E\x01"), ErrorKind::UnexpectedEof);
        assert_eq!(kind(b"\x81\x05Hel"), ErrorKind::UnexpectedEof);
    }

    #[test]
    fn a_frame_claiming_to_be_huge_fails_without_being_read() {
        let mut bytes = vec![0x82, 127];
        bytes.extend(u64::MAX.to_be_bytes());
        let err = read_frame(&mut Cursor::new(bytes)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        assert_eq!(
            err.to_string(),
            "The server sent a message of more than 16 MB"
        );
        let mut bytes = vec![0x82, 127];
        bytes.extend((MAX_MESSAGE as u64 + 1).to_be_bytes());
        assert_eq!(
            read_frame(&mut Cursor::new(bytes)).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
    }

    #[test]
    fn the_pieces_of_a_message_are_put_back_together() {
        let mut partial = None;
        assert_eq!(assemble(&mut partial, data(false, TEXT, b"Hel")), Ok(None));
        assert_eq!(
            assemble(&mut partial, data(false, CONTINUATION, b"lo ")),
            Ok(None)
        );
        assert_eq!(
            assemble(&mut partial, data(true, CONTINUATION, b"you")),
            Ok(Some(Event::Text("Hello you".to_owned())))
        );
        assert_eq!(partial, None);
        assert_eq!(
            assemble(&mut partial, data(false, BINARY, &[1, 2])),
            Ok(None)
        );
        assert_eq!(
            assemble(&mut partial, data(true, CONTINUATION, &[3])),
            Ok(Some(Event::Binary(3)))
        );
        assert_eq!(
            assemble(&mut partial, data(true, TEXT, b"whole")),
            Ok(Some(Event::Text("whole".to_owned())))
        );
        // A character cut between two pieces is whole again.
        let euro = "€".as_bytes();
        assert_eq!(
            assemble(&mut partial, data(false, TEXT, &euro[..1])),
            Ok(None)
        );
        assert_eq!(
            assemble(&mut partial, data(true, CONTINUATION, &euro[1..])),
            Ok(Some(Event::Text("€".to_owned())))
        );
    }

    #[test]
    fn pieces_out_of_order_or_too_many_fail() {
        let mut partial = None;
        assert!(assemble(&mut partial, data(true, CONTINUATION, b"x")).is_err());
        assert_eq!(assemble(&mut partial, data(false, TEXT, b"a")), Ok(None));
        assert!(assemble(&mut partial, data(true, TEXT, b"b")).is_err());
        let mut partial = Some((BINARY, vec![0; MAX_MESSAGE]));
        assert_eq!(
            assemble(&mut partial, data(true, CONTINUATION, b"x")),
            Err("The server sent a message of more than 16 MB".to_owned())
        );
    }

    #[test]
    fn text_that_is_not_utf8_is_shown_with_replacement_characters() {
        let mut partial = None;
        assert_eq!(
            assemble(&mut partial, data(true, TEXT, b"a\xFFb")),
            Ok(Some(Event::Text("a\u{FFFD}b".to_owned())))
        );
    }

    #[test]
    fn a_close_code_is_told_in_words() {
        assert_eq!(close_reason(&[0x03, 0xE8]), "1000 normal closure");
        assert_eq!(close_reason(b"\x03\xE8bye"), "1000 normal closure: bye");
        assert_eq!(close_reason(&[0x03, 0xE9]), "1001 going away");
        assert_eq!(
            close_reason(b"\x03\xF1too much"),
            "1009 message too big: too much"
        );
        assert_eq!(close_reason(&[0x03, 0xF3]), "1011 internal error");
        assert_eq!(close_reason(b"\x0F\xA0custom"), "4000: custom");
        assert_eq!(close_reason(&[]), "no reason given");
        assert_eq!(close_reason(&[0x03]), "no reason given");
    }

    #[test]
    fn a_conversation_with_a_server_goes_from_opened_to_closed() {
        let _alone = alone();
        let (url, server) = serve(|mut reader| {
            let hello = read_frame(&mut reader).unwrap();
            say(&reader, &frame(true, TEXT, &hello.payload, None));
            say(&reader, &frame(true, PING, b"are you there", None));
            let pong = read_frame(&mut reader).unwrap();
            // A message in three pieces, with a ping in the middle of it.
            say(&reader, &frame(false, TEXT, b"in ", None));
            say(&reader, &frame(false, CONTINUATION, b"three ", None));
            say(&reader, &frame(true, PING, b"again", None));
            say(&reader, &frame(true, CONTINUATION, b"pieces", None));
            let second_pong = read_frame(&mut reader).unwrap();
            say(&reader, &frame(true, TEXT, &[b'x'; 70_000], None));
            say(&reader, &frame(true, BINARY, &[0, 159, 146, 150], None));
            say(&reader, &frame(true, CLOSE, b"\x03\xE8bye", None));
            let close = read_frame(&mut reader).unwrap();
            (hello, pong, second_pong, close)
        });
        let wakes = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&wakes);
        let mut socket = Socket::open(&url, &["X-Test: yes".to_owned()], move || {
            counted.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();
        wait_until_open(&mut socket);
        socket.send("héllo").unwrap();
        let heard = listen_to_the_end(&mut socket);
        assert_eq!(
            heard,
            [
                Event::Text("héllo".to_owned()),
                Event::Text("in three pieces".to_owned()),
                Event::Text("x".repeat(70_000)),
                Event::Binary(4),
                Event::Closed("1000 normal closure: bye".to_owned()),
            ]
        );
        assert!(!socket.is_open());
        assert!(socket.send("too late").is_err());
        assert_eq!(wakes.load(Ordering::Relaxed), 6);

        let (hello, pong, second_pong, close) = server.join().unwrap();
        assert_eq!(hello, data(true, TEXT, "héllo".as_bytes()));
        assert_eq!(pong, data(true, PONG, b"are you there"));
        assert_eq!(second_pong, data(true, PONG, b"again"));
        assert_eq!(close, data(true, CLOSE, &[0x03, 0xE8]));
    }

    #[test]
    fn what_the_client_sends_is_masked_at_every_length() {
        let _alone = alone();
        let (url, server) = serve(|mut reader| {
            let mut seen = Vec::new();
            for _ in 0..3 {
                let mut head = [0_u8; 2];
                reader.read_exact(&mut head).unwrap();
                // Put the header back in front of the rest to read the frame whole.
                let frame = read_frame(&mut Cursor::new(head).chain(&mut reader)).unwrap();
                seen.push((head[1] & 0x80 != 0, frame.payload.len()));
            }
            seen
        });
        let mut socket = Socket::open(&url, &[], || {}).unwrap();
        wait_until_open(&mut socket);
        for length in [5, 300, 70_000] {
            socket.send(&"a".repeat(length)).unwrap();
        }
        assert_eq!(
            server.join().unwrap(),
            [(true, 5), (true, 300), (true, 70_000)]
        );
    }

    #[test]
    fn dropping_the_socket_says_goodbye_and_ends_its_thread() {
        let _alone = alone();
        let (url, server) = serve(|mut reader| {
            let close = read_frame(&mut reader).unwrap();
            // Then nothing more: the connection is over.
            (close, read_frame(&mut reader).is_err())
        });
        // The thread owns `wake`, so it holds this until it ends.
        let held = Arc::new(());
        let holder = Arc::clone(&held);
        let mut socket = Socket::open(&url, &[], move || drop(Arc::clone(&holder))).unwrap();
        wait_until_open(&mut socket);
        drop(socket);
        wait("the thread to end", || Arc::strong_count(&held) == 1);
        let (close, over) = server.join().unwrap();
        assert_eq!(close, data(true, CLOSE, &[0x03, 0xE8]));
        assert!(over);
    }

    #[test]
    fn closing_from_here_tells_the_server_and_ends_with_closed() {
        let _alone = alone();
        let (url, server) = serve(|mut reader| read_frame(&mut reader).unwrap());
        let mut socket = Socket::open(&url, &[], || {}).unwrap();
        wait_until_open(&mut socket);
        socket.close();
        assert!(!socket.is_open());
        assert_eq!(
            socket.send("too late"),
            Err("The socket is not open".to_owned())
        );
        assert_eq!(
            listen_to_the_end(&mut socket),
            [Event::Closed("closed here".to_owned())]
        );
        assert_eq!(server.join().unwrap(), data(true, CLOSE, &[0x03, 0xE8]));
        socket.close();
    }

    #[test]
    fn a_server_that_goes_away_gives_a_lost_connection() {
        let _alone = alone();
        let (url, server) = serve(drop);
        let mut socket = Socket::open(&url, &[], || {}).unwrap();
        let heard = listen_to_the_end(&mut socket);
        assert_eq!(
            heard,
            [
                Event::Opened,
                Event::Closed("the connection was lost".to_owned())
            ]
        );
        assert!(!socket.is_open());
        server.join().unwrap();
    }

    #[test]
    fn a_server_breaking_the_protocol_gives_failed() {
        let _alone = alone();
        let (url, server) = serve(|reader| {
            let mut huge = vec![0x82, 127];
            huge.extend(u64::MAX.to_be_bytes());
            say(&reader, &huge);
            // Stay until the client hangs up.
            let mut rest = Vec::new();
            let _ = reader.into_inner().read_to_end(&mut rest);
        });
        let mut socket = Socket::open(&url, &[], || {}).unwrap();
        let heard = listen_to_the_end(&mut socket);
        assert_eq!(
            heard,
            [
                Event::Opened,
                Event::Failed("The server sent a message of more than 16 MB".to_owned())
            ]
        );
        server.join().unwrap();
    }

    #[test]
    fn a_server_answering_plain_http_gives_failed() {
        let _alone = alone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            read_head(&mut reader).unwrap();
            say(&reader, b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi");
        });
        let mut socket = Socket::open(&url, &[], || {}).unwrap();
        let heard = listen_to_the_end(&mut socket);
        assert_eq!(
            heard,
            [Event::Failed(
                "The server did not open a WebSocket: HTTP/1.1 200 OK".to_owned()
            )]
        );
        assert!(!socket.is_open());
        server.join().unwrap();
    }

    #[test]
    fn a_refused_connection_gives_failed() {
        let _alone = alone();
        // A port that was free a moment ago, with nobody listening on it now.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut socket = Socket::open(&format!("ws://127.0.0.1:{port}/"), &[], || {}).unwrap();
        assert!(!socket.is_open());
        assert!(socket.send("too early").is_err());
        let heard = listen_to_the_end(&mut socket);
        assert!(
            matches!(heard.as_slice(), [Event::Failed(why)] if why.starts_with("Cannot reach 127.0.0.1")),
            "{heard:?}"
        );
    }
}
