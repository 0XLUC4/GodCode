//! A PostgreSQL server asked questions over its own protocol, written by hand.
//!
//! Version 3 of the protocol, simple queries, values as text. There is no TLS here: what
//! is said travels in the clear, so this is for a server on this machine, in a container,
//! or on a network one trusts. One call at a time, each waiting for its answer: it is
//! meant to be driven from a thread.

use std::fmt;
use std::hash::{BuildHasher as _, Hasher as _};
use std::io::{ErrorKind, Read, Write as _};
use std::net::{TcpStream, ToSocketAddrs as _};
use std::time::Duration;

/// How long a server may take to accept the connection.
const CONNECT: Duration = Duration::from_secs(10);
/// How long a server may stay silent while an answer is awaited.
const SILENCE: Duration = Duration::from_secs(30);
/// The largest message taken from a server, or sent to one.
const LARGEST: usize = 256 << 20;
/// The most rounds of hashing a server may ask of a password: more is a way to keep us busy.
const MOST_ROUNDS: u32 = 10_000_000;

const SHORT: &str = "The server sent a message that ends too soon";
const CLOSED: &str = "The connection to the server is closed";
const NO_COPY: &str = "COPY is not possible here: only rows can be read";
const UNREADABLE: &str = "The server answered the password exchange with something unreadable";

/// Where a server is and who asks: read from `postgres://user:password@host:port/database`.
#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
}

/// Everything but the password: it must not end in a log or in a message.
impl fmt::Debug for Target {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Target")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &"***")
            .field("database", &self.database)
            .finish()
    }
}

impl Target {
    /// Read an address: `postgres://` or `postgresql://`, then `user:password@host:port/database`.
    /// The port is 5432 and the database is named like the user when they are not given.
    /// A `/`, `?` or `%` in a name or a password must be written `%2F`, `%3F`, `%25`.
    /// An address asking for encryption is refused; what else follows `?` is ignored.
    /// No error quotes the address: it holds the password.
    pub fn parse(url: &str) -> Result<Target, String> {
        let url = url.trim();
        let rest = url
            .strip_prefix("postgres://")
            .or_else(|| url.strip_prefix("postgresql://"))
            .ok_or("The address must start with postgres://")?;
        let (place, after) = rest.split_at(rest.find(['/', '?']).unwrap_or(rest.len()));
        let (path, options) = after.split_once('?').unwrap_or((after, ""));
        for (name, value) in options.split('&').filter_map(|it| it.split_once('=')) {
            let encrypted = match name {
                "sslmode" => matches!(value, "require" | "verify-ca" | "verify-full"),
                "ssl" | "requiressl" => matches!(value, "true" | "1"),
                "gssencmode" | "channel_binding" => value == "require",
                "sslnegotiation" => value == "direct",
                _ => false,
            };
            if encrypted {
                return Err(format!(
                    "{name}={value} asks for an encrypted connection, which cannot be made here: \
                     take it out of the address, for a server you trust"
                ));
            }
        }
        let (who, host) = place
            .rsplit_once('@')
            .ok_or("The address names no user: postgres://user:password@host/database")?;
        let (user, password) = who.split_once(':').unwrap_or((who, ""));
        let user = unescaped(user)?;
        if user.is_empty() {
            return Err("The address names no user: postgres://user:password@host/database".into());
        }
        let (host, port) = match host.strip_prefix('[') {
            // An IPv6 address stands between brackets, its colons are not the port's.
            Some(inside) => {
                let (host, port) = inside
                    .split_once(']')
                    .ok_or("The address opens a bracket and does not close it")?;
                (host, port.strip_prefix(':'))
            }
            None => match host.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (host, None),
            },
        };
        let port = match port {
            Some(port) => port
                .parse()
                .ok()
                .filter(|port| *port != 0)
                .ok_or("The port of the address is not a number from 1 to 65535")?,
            None => 5432,
        };
        let database = unescaped(path.strip_prefix('/').unwrap_or(path))?;
        Ok(Target {
            host: if host.is_empty() { "localhost" } else { host }.to_owned(),
            port,
            password: unescaped(password)?,
            database: if database.is_empty() {
                user.clone()
            } else {
                database
            },
            user,
        })
    }
}

/// `text` with its `%41` read as the bytes they stand for.
fn unescaped(text: &str) -> Result<String, String> {
    let mut plain = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'%' {
            plain.push(byte);
            continue;
        }
        let digit = |byte: Option<u8>| byte.and_then(|it| char::from(it).to_digit(16));
        match (digit(bytes.next()), digit(bytes.next())) {
            (Some(high), Some(low)) => plain.push((high * 16 + low) as u8),
            _ => return Err("A % in the address is not followed by two hex digits".into()),
        }
    }
    String::from_utf8(plain).map_err(|_| "The address does not hold UTF-8 text".into())
}

/// What a question answered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    pub columns: Vec<String>,
    /// One value per column; `None` is NULL.
    pub rows: Vec<Vec<Option<String>>>,
    /// What the server said it did: `SELECT 3`, `UPDATE 1`.
    pub done: String,
    /// More rows came than were kept.
    pub cut: bool,
}

/// A session with a server, logged in.
#[derive(Debug)]
pub struct Connection {
    stream: TcpStream,
    /// The exchange went wrong in a way that leaves no telling where a message starts:
    /// nothing more is asked.
    lost: bool,
}

impl Connection {
    /// Reach the server of `target` and log in. It may take 10 seconds to accept the
    /// connection, then 30 to say each thing.
    pub fn open(target: &Target) -> Result<Connection, String> {
        if [&target.user, &target.password, &target.database]
            .iter()
            .any(|it| it.contains('\0'))
        {
            return Err("A name or a password cannot hold a zero byte".into());
        }
        let place = format!("{}:{}", target.host, target.port);
        let addresses = (target.host.as_str(), target.port)
            .to_socket_addrs()
            .map_err(|err| format!("Cannot find {place}: {err}"))?;
        let mut why = String::from("it has no address");
        let mut stream = None;
        for address in addresses {
            match TcpStream::connect_timeout(&address, CONNECT) {
                Ok(reached) => {
                    stream = Some(reached);
                    break;
                }
                Err(err) => why = err.to_string(),
            }
        }
        let stream = stream.ok_or_else(|| format!("Cannot reach PostgreSQL at {place}: {why}"))?;
        for set in [
            stream.set_read_timeout(Some(SILENCE)),
            stream.set_write_timeout(Some(SILENCE)),
        ] {
            set.map_err(|err| format!("Cannot talk to {place}: {err}"))?;
        }
        // Small messages go at once; a system that will not do it is only slower.
        let _ = stream.set_nodelay(true);
        let mut connection = Connection {
            stream,
            lost: false,
        };
        connection
            .stream
            .write_all(&startup(target))
            .map_err(broken)?;
        connection.log_in(target)?;
        Ok(connection)
    }

    /// Ask `sql` (simple query protocol, text format); at most `most` rows are kept, the
    /// rest read and dropped. Of several statements, the last that gave columns is the
    /// one kept. When the server refuses, the connection can still be asked something
    /// else; when the exchange itself breaks, it is closed.
    pub fn query(&mut self, sql: &str, most: usize) -> Result<Answer, String> {
        if self.lost {
            return Err(CLOSED.into());
        }
        if sql.contains('\0') || sql.len() >= LARGEST {
            return Err("A query cannot hold a zero byte, nor be that long".into());
        }
        let told = self
            .send(b'Q', &ended(sql))
            .and_then(|()| self.answer(most));
        told.unwrap_or_else(|err| {
            self.lost = true;
            Err(err)
        })
    }

    /// The tables and views one can read, as `schema.name`, system schemas left out.
    /// A name that needs quotes to stand in a query already has them.
    pub fn tables(&mut self) -> Result<Vec<String>, String> {
        let answer = self.query(LIST_TABLES, usize::MAX)?;
        let names = answer.rows.into_iter();
        Ok(names
            .filter_map(|row| row.into_iter().next().flatten())
            .collect())
    }

    /// Answer what the server asks to know who we are, until it is ready for a query.
    fn log_in(&mut self, target: &Target) -> Result<(), String> {
        let mut scram: Option<Scram> = None;
        let mut welcomed = false;
        loop {
            let (kind, body) = read_message(&mut self.stream)?;
            let mut body = Body(&body);
            match kind {
                b'R' => match body.int()? {
                    0 if scram.as_ref().is_some_and(|it| !it.proven) => {
                        return Err(
                            "The server let us in without proving it knows the password".into()
                        );
                    }
                    0 => welcomed = true,
                    3 => self.send(b'p', &ended(&target.password))?,
                    5 => {
                        let hashed = md5_password(&target.user, &target.password, body.take(4)?);
                        self.send(b'p', &ended(&hashed))?;
                    }
                    10 => {
                        let mut offered = Vec::new();
                        loop {
                            let mechanism = body.text()?;
                            if mechanism.is_empty() {
                                break;
                            }
                            offered.push(mechanism);
                        }
                        // SCRAM-SHA-256-PLUS binds the exchange to a TLS channel: not here.
                        if !offered.iter().any(|it| it == "SCRAM-SHA-256") {
                            return Err(format!(
                                "The server asks for a way to log in that is not known here: {}",
                                offered.join(", ")
                            ));
                        }
                        // Like libpq, no name: the server takes the one of the startup message.
                        let begun = Scram::start("", nonce());
                        let first = begun.first();
                        let mut said = ended("SCRAM-SHA-256");
                        said.extend((first.len() as u32).to_be_bytes());
                        said.extend(first.bytes());
                        self.send(b'p', &said)?;
                        scram = Some(begun);
                    }
                    11 => {
                        let scram = scram.as_mut().ok_or(UNREADABLE)?;
                        let first = std::str::from_utf8(body.0).map_err(|_| UNREADABLE)?;
                        let last = scram.prove(&target.password, first)?;
                        self.send(b'p', last.as_bytes())?;
                    }
                    12 => {
                        let scram = scram.as_mut().ok_or(UNREADABLE)?;
                        scram.check(std::str::from_utf8(body.0).map_err(|_| UNREADABLE)?)?;
                    }
                    other => {
                        let method = match other {
                            2 => "Kerberos V5".to_owned(),
                            6 => "SCM credentials".to_owned(),
                            7 | 8 => "GSSAPI".to_owned(),
                            9 => "SSPI".to_owned(),
                            _ => format!("method {other}"),
                        };
                        return Err(format!(
                            "The server asks for a way to log in that is not known here: {method}"
                        ));
                    }
                },
                b'E' => return Err(refusal(&mut body)),
                b'Z' if welcomed => return Ok(()),
                b'Z' => return Err("The server is ready before having let us in".into()),
                // Its settings, the key to cancel a query, a notice: nothing to do with them.
                _ => {}
            }
        }
    }

    /// Read what the server answers to a query, to the point where it is ready for the
    /// next. The outer error is an exchange that broke, the inner one a server saying no.
    fn answer(&mut self, most: usize) -> Result<Result<Answer, String>, String> {
        let mut answer = Answer::default();
        let mut refused: Option<String> = None;
        // Whether the rows coming belong to a result worth keeping.
        let mut keeping = false;
        loop {
            let (kind, body) = read_message(&mut self.stream)?;
            let mut body = Body(&body);
            match kind {
                b'T' => {
                    let columns = columns(&mut body)?;
                    keeping = !columns.is_empty();
                    if keeping {
                        answer.columns = columns;
                        answer.rows.clear();
                        answer.cut = false;
                    }
                }
                b'D' if keeping && answer.rows.len() < most => answer.rows.push(row(&mut body)?),
                b'D' if keeping => answer.cut = true,
                b'C' => answer.done = body.text()?,
                b'E' => {
                    refused.get_or_insert(refusal(&mut body));
                }
                // It waits for rows from us: told that none will come, it gives the query up.
                b'G' => {
                    refused.get_or_insert(NO_COPY.into());
                    self.send(b'f', &ended("COPY is not possible from this program"))?;
                }
                // It sends rows its own way: they are read as they come and dropped.
                b'H' => {
                    refused.get_or_insert(NO_COPY.into());
                }
                // Both ways at once is for replication: there is no clean way out of it.
                b'W' => return Err(NO_COPY.into()),
                b'Z' => return Ok(refused.map_or(Ok(answer), Err)),
                // Notices, settings that changed, notifications, the rows of a COPY.
                _ => {}
            }
        }
    }

    fn send(&mut self, kind: u8, body: &[u8]) -> Result<(), String> {
        self.stream.write_all(&message(kind, body)).map_err(broken)
    }
}

/// The server is told that we leave, so that it does not log a lost client.
impl Drop for Connection {
    fn drop(&mut self) {
        if !self.lost {
            let _ = self.send(b'X', &[]);
        }
    }
}

/// The SQL `tables` runs: one column, `schema.name`, quoted where a query would need it.
pub const LIST_TABLES: &str = "\
SELECT pg_catalog.quote_ident(n.nspname) || '.' || pg_catalog.quote_ident(c.relname)
FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f')
  AND n.nspname <> 'information_schema'
  AND n.nspname !~ '^pg_'
  AND pg_catalog.has_schema_privilege(n.oid, 'USAGE')
  AND pg_catalog.has_table_privilege(c.oid, 'SELECT')
ORDER BY 1";

/// `name` written so that it can stand in a query: `"my table"`, inner quotes doubled.
#[allow(dead_code)]
pub fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Why reading or writing failed, in words.
fn broken(err: std::io::Error) -> String {
    match err.kind() {
        ErrorKind::UnexpectedEof => "The server closed the connection".into(),
        ErrorKind::TimedOut | ErrorKind::WouldBlock => "The server did not answer in time".into(),
        _ => format!("The connection to the server broke: {err}"),
    }
}

/// `text` followed by the zero byte that ends a string on the wire.
fn ended(text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    bytes
}

/// A message as it travels: its kind, its length (which counts itself), its body.
/// Bodies are small or checked before: the length fits.
fn message(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut bytes = vec![kind];
    bytes.extend((body.len() as u32 + 4).to_be_bytes());
    bytes.extend(body);
    bytes
}

/// The first message, the only one without a kind: the version of the protocol, then
/// who asks and for which database.
fn startup(target: &Target) -> Vec<u8> {
    let mut body = 196_608_u32.to_be_bytes().to_vec();
    for text in [
        "user",
        &target.user,
        "database",
        &target.database,
        "client_encoding",
        "UTF8",
        "application_name",
        "GodCode",
    ] {
        body.extend(ended(text));
    }
    body.push(0);
    let mut bytes = (body.len() as u32 + 4).to_be_bytes().to_vec();
    bytes.extend(body);
    bytes
}

/// The next message of the server: its kind and its body. The body grows as bytes
/// come, so a length that lies costs no memory.
fn read_message(from: &mut impl Read) -> Result<(u8, Vec<u8>), String> {
    let mut head = [0; 5];
    from.read_exact(&mut head).map_err(broken)?;
    let length = i32::from_be_bytes([head[1], head[2], head[3], head[4]]);
    let size = usize::try_from(length)
        .ok()
        .and_then(|it| it.checked_sub(4))
        .filter(|it| *it <= LARGEST)
        .ok_or_else(|| format!("The server sent a message of an impossible length: {length}"))?;
    let mut body = Vec::new();
    from.take(size as u64)
        .read_to_end(&mut body)
        .map_err(broken)?;
    if body.len() < size {
        return Err("The server closed the connection".into());
    }
    Ok((head[0], body))
}

/// What is left to read of the body of a message.
struct Body<'a>(&'a [u8]);

impl<'a> Body<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let (taken, rest) = self.0.split_at_checked(count).ok_or(SHORT)?;
        self.0 = rest;
        Ok(taken)
    }

    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    /// A count: of columns, of values.
    fn short(&mut self) -> Result<u16, String> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn int(&mut self) -> Result<i32, String> {
        let bytes = self.take(4)?;
        Ok(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// A string, up to the zero byte that ends it.
    fn text(&mut self) -> Result<String, String> {
        let end = self.0.iter().position(|it| *it == 0).ok_or(SHORT)?;
        let text = String::from_utf8_lossy(self.take(end)?).into_owned();
        self.take(1)?;
        Ok(text)
    }
}

/// The names of the columns of a result. What follows each name says its type and its
/// table: nothing needed to show text.
fn columns(body: &mut Body) -> Result<Vec<String>, String> {
    (0..body.short()?)
        .map(|_| {
            let name = body.text()?;
            body.take(18)?;
            Ok(name)
        })
        .collect()
}

/// The values of a row, as the text the server wrote.
fn row(body: &mut Body) -> Result<Vec<Option<String>>, String> {
    (0..body.short()?)
        .map(|_| match body.int()? {
            -1 => Ok(None),
            length => {
                let length = usize::try_from(length).map_err(|_| SHORT)?;
                let value = body.take(length)?;
                Ok(Some(String::from_utf8_lossy(value).into_owned()))
            }
        })
        .collect()
}

/// What a server says when it refuses: `ERROR: message (at character 8)`, then its
/// detail and its hint on a second line when it gave any.
fn refusal(body: &mut Body) -> String {
    let (mut severity, mut message, mut more) = (String::new(), String::new(), Vec::new());
    let mut position = String::new();
    while let Ok(field) = body.byte() {
        let Ok(value) = body.text() else { break };
        match field {
            b'S' => severity = value,
            b'M' => message = value,
            b'P' => position = format!(" (at character {value})"),
            b'D' => more.push(format!("Detail: {value}")),
            b'H' => more.push(format!("Hint: {value}")),
            _ => {}
        }
    }
    if message.is_empty() {
        return "The server refused, without saying why".into();
    }
    let first = match severity.is_empty() {
        true => format!("{message}{position}"),
        false => format!("{severity}: {message}{position}"),
    };
    match more.is_empty() {
        true => first,
        false => format!("{first}\n{}", more.join(" ")),
    }
}

/// What is sent in place of a password when the server asks for its MD5: the hash of
/// (the hash of the password and the name) and the salt of the server.
fn md5_password(user: &str, password: &str, salt: &[u8]) -> String {
    let mut inner = hex(&md5(format!("{password}{user}").as_bytes())).into_bytes();
    inner.extend(salt);
    format!("md5{}", hex(&md5(&inner)))
}

/// The client side of a SCRAM-SHA-256 exchange (RFC 5802, RFC 7677), without channel
/// binding. The password is used as its bytes are: the preparation the RFC asks for
/// (SASLprep) changes nothing to ASCII, and is not done for the rest.
struct Scram {
    /// What we said first, without its header: it is part of what gets signed.
    bare: String,
    nonce: String,
    /// The signature the server must show at the end: only one that knows the password can.
    expected: Option<[u8; 32]>,
    proven: bool,
}

impl Scram {
    /// `nonce` must be new for each exchange: it is what keeps an old one from being replayed.
    fn start(user: &str, nonce: String) -> Scram {
        Scram {
            bare: format!("n={user},r={nonce}"),
            nonce,
            expected: None,
            proven: false,
        }
    }

    /// The message that opens the exchange.
    fn first(&self) -> String {
        format!("n,,{}", self.bare)
    }

    /// Our last message, answering the `first` of the server: the proof that we know
    /// the password, which does not give it away.
    fn prove(&mut self, password: &str, first: &str) -> Result<String, String> {
        let field = |name: &str| first.split(',').find_map(|it| it.strip_prefix(name));
        let nonce = field("r=")
            .filter(|it| it.len() > self.nonce.len() && it.starts_with(&self.nonce))
            .ok_or("The server answered another password exchange than ours")?;
        let salt = field("s=").and_then(unbase64).ok_or(UNREADABLE)?;
        let rounds = field("i=")
            .and_then(|it| it.parse().ok())
            .filter(|it| (1..=MOST_ROUNDS).contains(it))
            .ok_or(UNREADABLE)?;
        let salted = pbkdf2(password.as_bytes(), &salt, rounds);
        let client_key = hmac(&salted, b"Client Key");
        // `biws` is "n,," in base64: no channel binding.
        let unproven = format!("c=biws,r={nonce}");
        let said = format!("{},{first},{unproven}", self.bare);
        let signature = hmac(&sha256(&client_key), said.as_bytes());
        let proof: Vec<u8> = client_key
            .iter()
            .zip(signature)
            .map(|(key, signed)| key ^ signed)
            .collect();
        self.expected = Some(hmac(&hmac(&salted, b"Server Key"), said.as_bytes()));
        Ok(format!("{unproven},p={}", base64(&proof)))
    }

    /// Check the `last` message of the server: its signature shows it knows the password too.
    fn check(&mut self, last: &str) -> Result<(), String> {
        if let Some(why) = last.strip_prefix("e=") {
            return Err(format!("The server refused the password exchange: {why}"));
        }
        let shown = last.split(',').find_map(|it| it.strip_prefix("v="));
        let shown = shown.and_then(unbase64).ok_or(UNREADABLE)?;
        if self
            .expected
            .is_none_or(|expected| expected[..] != shown[..])
        {
            return Err("The server did not prove it knows the password: it may be another".into());
        }
        self.proven = true;
        Ok(())
    }
}

/// 24 characters nobody can guess. The standard library draws no random numbers, but
/// it asks the system for some to key its hash maps: those keys are what is used.
fn nonce() -> String {
    let mut seed = Vec::new();
    for _ in 0..4 {
        let keyed = std::collections::hash_map::RandomState::new();
        seed.extend(keyed.build_hasher().finish().to_be_bytes());
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    seed.extend(now.map_or(0, |it| it.as_nanos()).to_be_bytes());
    seed.extend(std::process::id().to_be_bytes());
    base64(&sha256(&seed)[..18])
}

/// PBKDF2 with HMAC-SHA-256, for one block of output: the `Hi` of SCRAM (RFC 5802).
fn pbkdf2(password: &[u8], salt: &[u8], rounds: u32) -> [u8; 32] {
    let mut first = salt.to_vec();
    first.extend(1_u32.to_be_bytes());
    let mut last = hmac(password, &first);
    let mut mixed = last;
    for _ in 1..rounds {
        last = hmac(password, &last);
        for (kept, new) in mixed.iter_mut().zip(last) {
            *kept ^= new;
        }
    }
    mixed
}

/// HMAC-SHA-256 (RFC 2104).
fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0; 64];
    if key.len() > block.len() {
        block[..32].copy_from_slice(&sha256(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = block.iter().map(|it| it ^ 0x36).collect();
    inner.extend(message);
    let mut outer: Vec<u8> = block.iter().map(|it| it ^ 0x5c).collect();
    outer.extend(sha256(&inner));
    sha256(&outer)
}

/// `message` made a whole number of 64-byte blocks, the way MD5 and SHA-256 both want
/// it: a 1 bit, zeros, and the length in bits, which they write in opposite orders.
fn padded(message: &[u8], big_endian: bool) -> Vec<u8> {
    let bits = (message.len() as u64).wrapping_mul(8);
    let mut whole = message.to_vec();
    whole.push(0x80);
    while whole.len() % 64 != 56 {
        whole.push(0);
    }
    whole.extend(match big_endian {
        true => bits.to_be_bytes(),
        false => bits.to_le_bytes(),
    });
    whole
}

const SHA_START: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

const SHA_ROUNDS: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 (FIPS 180-4).
fn sha256(message: &[u8]) -> [u8; 32] {
    let mut state = SHA_START;
    for block in padded(message, true).chunks_exact(64) {
        let mut words = [0_u32; 64];
        for (word, bytes) in words.iter_mut().zip(block.chunks_exact(4)) {
            *word = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for at in 16..64 {
            let (near, far) = (words[at - 15], words[at - 2]);
            let low = near.rotate_right(7) ^ near.rotate_right(18) ^ (near >> 3);
            let high = far.rotate_right(17) ^ far.rotate_right(19) ^ (far >> 10);
            words[at] = words[at - 16]
                .wrapping_add(low)
                .wrapping_add(words[at - 7])
                .wrapping_add(high);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for (constant, word) in SHA_ROUNDS.iter().zip(words) {
            let first = h
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(*constant)
                .wrapping_add(word);
            let second = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
                .wrapping_add((a & b) ^ (a & c) ^ (b & c));
            (h, g, f, e) = (g, f, e, d.wrapping_add(first));
            (d, c, b, a) = (c, b, a, first.wrapping_add(second));
        }
        for (kept, new) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *kept = kept.wrapping_add(new);
        }
    }
    let mut digest = [0; 32];
    for (bytes, word) in digest.chunks_exact_mut(4).zip(state) {
        bytes.copy_from_slice(&word.to_be_bytes());
    }
    digest
}

const MD5_ROUNDS: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

/// How far each step of MD5 turns its bits: four amounts per round of sixteen steps.
const MD5_TURNS: [u32; 16] = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];

/// MD5 (RFC 1321): broken for anything that matters, but what old servers ask of a password.
fn md5(message: &[u8]) -> [u8; 16] {
    let mut state: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    for block in padded(message, false).chunks_exact(64) {
        let mut words = [0_u32; 16];
        for (word, bytes) in words.iter_mut().zip(block.chunks_exact(4)) {
            *word = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        let [mut a, mut b, mut c, mut d] = state;
        for step in 0..64 {
            let (mixed, word) = match step / 16 {
                0 => ((b & c) | (!b & d), step),
                1 => ((d & b) | (!d & c), (5 * step + 1) % 16),
                2 => (b ^ c ^ d, (3 * step + 5) % 16),
                _ => (c ^ (b | !d), (7 * step) % 16),
            };
            let turned = a
                .wrapping_add(mixed)
                .wrapping_add(MD5_ROUNDS[step])
                .wrapping_add(words[word])
                .rotate_left(MD5_TURNS[step / 16 * 4 + step % 4]);
            (a, d, c, b) = (d, c, b, b.wrapping_add(turned));
        }
        for (kept, new) in state.iter_mut().zip([a, b, c, d]) {
            *kept = kept.wrapping_add(new);
        }
    }
    let mut digest = [0; 16];
    for (bytes, word) in digest.chunks_exact_mut(4).zip(state) {
        bytes.copy_from_slice(&word.to_le_bytes());
    }
    digest
}

/// `bytes` as lowercase hex digits.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|it| format!("{it:02x}")).collect()
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `bytes` in base64 (RFC 4648), padded.
fn base64(bytes: &[u8]) -> String {
    let mut text = String::new();
    for chunk in bytes.chunks(3) {
        let mut group = [0; 4];
        group[1..=chunk.len()].copy_from_slice(chunk);
        let group = u32::from_be_bytes(group);
        for place in 0..4 {
            let digit = (group >> (18 - 6 * place)) & 63;
            text.push(match place <= chunk.len() {
                true => char::from(BASE64[digit as usize]),
                false => '=',
            });
        }
    }
    text
}

/// The bytes that `text` stands for in base64, when it is base64.
fn unbase64(text: &str) -> Option<Vec<u8>> {
    let groups = text.as_bytes().chunks_exact(4);
    if !groups.remainder().is_empty() {
        return None;
    }
    let mut bytes = Vec::new();
    for group in groups {
        let filled = group.iter().take_while(|it| **it != b'=').count();
        if filled < 2 || group[filled..].iter().any(|it| *it != b'=') {
            return None;
        }
        let mut bits = 0_u32;
        for digit in &group[..filled] {
            bits = (bits << 6) | BASE64.iter().position(|it| it == digit)? as u32;
        }
        bits <<= 6 * (4 - filled);
        bytes.extend(&bits.to_be_bytes()[1..filled]);
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};

    const PASSWORD: &str = "s3cret-word";

    fn text(bytes: &[u8]) -> String {
        hex(bytes)
    }

    /// A PostgreSQL server of pretend: it waits on a free port for one client and plays
    /// `script` with it. The target is the way to it.
    fn server(script: impl FnOnce(&mut TcpStream) + Send + 'static) -> (Target, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = Target {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            user: "luca".into(),
            password: PASSWORD.into(),
            database: "shop".into(),
        };
        let played = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // A client that says nothing fails the test instead of hanging it.
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            script(&mut stream);
        });
        (target, played)
    }

    /// A server that lets anyone in, then plays `script`.
    fn open_server(
        script: impl FnOnce(&mut TcpStream) + Send + 'static,
    ) -> (Connection, JoinHandle<()>) {
        let (target, played) = server(|stream| {
            greeted(stream);
            tell(stream, b'R', &0_i32.to_be_bytes());
            tell(stream, b'Z', b"I");
            script(stream);
        });
        (Connection::open(&target).unwrap(), played)
    }

    /// Read the startup message: the strings it holds, names and values in turn.
    fn greeted(stream: &mut TcpStream) -> Vec<String> {
        let mut length = [0; 4];
        stream.read_exact(&mut length).unwrap();
        let mut body = vec![0; u32::from_be_bytes(length) as usize - 4];
        stream.read_exact(&mut body).unwrap();
        assert_eq!(body[..4], 196_608_u32.to_be_bytes());
        assert_eq!(body[body.len() - 2..], [0, 0]);
        body[4..body.len() - 2]
            .split(|it| *it == 0)
            .map(|it| String::from_utf8(it.to_vec()).unwrap())
            .collect()
    }

    fn tell(stream: &mut TcpStream, kind: u8, body: &[u8]) {
        stream.write_all(&message(kind, body)).unwrap();
    }

    fn hear(stream: &mut TcpStream) -> (u8, Vec<u8>) {
        read_message(stream).unwrap()
    }

    /// The query the client asks next.
    fn asked(stream: &mut TcpStream) -> String {
        let (kind, body) = hear(stream);
        assert_eq!(kind, b'Q');
        Body(&body).text().unwrap()
    }

    /// What the client sent before it left.
    fn left(stream: &mut TcpStream) -> Vec<u8> {
        let mut rest = Vec::new();
        // A client that hangs up without reading everything ends this with an error.
        let _ = stream.read_to_end(&mut rest);
        rest
    }

    fn described(names: &[&str]) -> Vec<u8> {
        let mut body = (names.len() as u16).to_be_bytes().to_vec();
        for name in names {
            body.extend(ended(name));
            body.extend([0; 18]);
        }
        body
    }

    fn values(values: &[Option<&str>]) -> Vec<u8> {
        let mut body = (values.len() as u16).to_be_bytes().to_vec();
        for value in values {
            match value {
                Some(value) => {
                    body.extend((value.len() as i32).to_be_bytes());
                    body.extend(value.bytes());
                }
                None => body.extend((-1_i32).to_be_bytes()),
            }
        }
        body
    }

    fn fields(fields: &[(u8, &str)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (field, value) in fields {
            body.push(*field);
            body.extend(ended(value));
        }
        body.push(0);
        body
    }

    /// Send a whole result: its columns, its rows, what was done.
    fn result(stream: &mut TcpStream, names: &[&str], rows: &[&[Option<&str>]], done: &str) {
        tell(stream, b'T', &described(names));
        for row in rows {
            tell(stream, b'D', &values(row));
        }
        tell(stream, b'C', &ended(done));
    }

    fn request(code: i32, rest: &[u8]) -> Vec<u8> {
        let mut body = code.to_be_bytes().to_vec();
        body.extend(rest);
        body
    }

    /// How the fake server ends a SCRAM exchange.
    #[derive(Clone, Copy, PartialEq)]
    enum Ending {
        Honest,
        /// A signature that one who does not know the password would make up.
        Forged,
        /// No signature at all.
        Silent,
    }

    /// The server side of SCRAM-SHA-256: it checks the proof of the client.
    fn scram_server(stream: &mut TcpStream, ending: Ending) {
        greeted(stream);
        tell(
            stream,
            b'R',
            &request(10, b"SCRAM-SHA-256-PLUS\0SCRAM-SHA-256\0\0"),
        );
        let (kind, first) = hear(stream);
        assert_eq!(kind, b'p');
        let mut first = Body(&first);
        assert_eq!(first.text().unwrap(), "SCRAM-SHA-256");
        assert_eq!(first.int().unwrap() as usize, first.0.len());
        let first = String::from_utf8(first.0.to_vec()).unwrap();
        let bare = first.strip_prefix("n,,").unwrap();
        let client_nonce = bare.strip_prefix("n=,r=").unwrap();
        assert_eq!(client_nonce.len(), 24);
        let (salt, rounds) = (b"salt of the fake", 4096);
        let nonce = format!("{client_nonce}3rfcNHYJY1ZVvWVs7j");
        let server_first = format!("r={nonce},s={},i={rounds}", base64(salt));
        tell(stream, b'R', &request(11, server_first.as_bytes()));

        let (kind, last) = hear(stream);
        assert_eq!(kind, b'p');
        let last = String::from_utf8(last).unwrap();
        let (unproven, proof) = last.split_once(",p=").unwrap();
        assert_eq!(unproven, format!("c=biws,r={nonce}"));
        let said = format!("{bare},{server_first},{unproven}");
        let salted = pbkdf2(PASSWORD.as_bytes(), salt, rounds);
        let stored = sha256(&hmac(&salted, b"Client Key"));
        let key: Vec<u8> = unbase64(proof)
            .unwrap()
            .iter()
            .zip(hmac(&stored, said.as_bytes()))
            .map(|(proved, signed)| proved ^ signed)
            .collect();
        assert_eq!(sha256(&key), stored, "the proof is the one of the password");

        let mut signature = hmac(&hmac(&salted, b"Server Key"), said.as_bytes());
        if ending == Ending::Forged {
            signature[0] ^= 1;
        }
        if ending != Ending::Silent {
            let last = format!("v={}", base64(&signature));
            tell(stream, b'R', &request(12, last.as_bytes()));
        }
        tell(stream, b'R', &request(0, &[]));
        tell(stream, b'Z', b"I");
        left(stream);
    }

    #[test]
    fn sha256_gives_the_digests_nist_published() {
        assert_eq!(
            text(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            text(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            text(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            text(&sha256(&vec![b'a'; 1_000_000])),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn md5_gives_the_digests_of_rfc_1321() {
        let published = [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (message, digest) in published {
            assert_eq!(text(&md5(message.as_bytes())), digest, "{message:?}");
        }
    }

    #[test]
    fn hmac_gives_the_codes_of_rfc_4231() {
        assert_eq!(
            text(&hmac(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            text(&hmac(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            text(&hmac(&[0xaa; 20], &[0xdd; 50])),
            "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
        );
        let counting: Vec<u8> = (1..=25).collect();
        assert_eq!(
            text(&hmac(&counting, &[0xcd; 50])),
            "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b"
        );
        // A key longer than a block is hashed first.
        assert_eq!(
            text(&hmac(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
        assert_eq!(
            text(&hmac(
                &[0xaa; 131],
                b"This is a test using a larger than block-size key and a larger than \
                  block-size data. The key needs to be hashed before being used by the \
                  HMAC algorithm."
            )),
            "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2"
        );
    }

    #[test]
    fn pbkdf2_gives_the_keys_published_for_password_and_salt() {
        assert_eq!(
            text(&pbkdf2(b"password", b"salt", 1)),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
        assert_eq!(
            text(&pbkdf2(b"password", b"salt", 2)),
            "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"
        );
        assert_eq!(
            text(&pbkdf2(b"password", b"salt", 4096)),
            "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"
        );
    }

    #[test]
    fn base64_goes_both_ways_on_the_words_of_rfc_4648() {
        let published = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, coded) in published {
            assert_eq!(base64(plain.as_bytes()), coded);
            assert_eq!(unbase64(coded).unwrap(), plain.as_bytes());
        }
        assert_eq!(base64(&[0xfb, 0xff]), "+/8=");
        for broken in ["Zg", "Zg=", "Z===", "Zm9*", "=Zg=", "Zg=v"] {
            assert_eq!(unbase64(broken), None, "{broken}");
        }
    }

    #[test]
    fn the_scram_exchange_of_rfc_7677_is_said_byte_for_byte() {
        let mut scram = Scram::start("user", "rOprNGfwEbeRWgbNEkqO".into());
        assert_eq!(scram.first(), "n,,n=user,r=rOprNGfwEbeRWgbNEkqO");
        let first = "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,\
                     s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096";
        assert_eq!(
            scram.prove("pencil", first).unwrap(),
            "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,\
             p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ="
        );
        assert!(!scram.proven);
        scram
            .check("v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=")
            .unwrap();
        assert!(scram.proven);
    }

    #[test]
    fn a_scram_answer_that_is_not_ours_or_asks_too_much_is_refused() {
        let start = || Scram::start("", "abcdef".into());
        // Another nonce than the one we sent, or ours with nothing added.
        assert!(start().prove("x", "r=zzzzzzYY,s=c2FsdA==,i=4096").is_err());
        assert!(start().prove("x", "r=abcdef,s=c2FsdA==,i=4096").is_err());
        assert!(start().prove("x", "r=abcdefYY,s=c2FsdA==,i=0").is_err());
        assert!(
            start()
                .prove("x", "r=abcdefYY,s=c2FsdA==,i=4000000000")
                .is_err()
        );
        assert!(start().prove("x", "r=abcdefYY,s=???,i=4096").is_err());
        // A signature shown before any proof was made proves nothing.
        assert!(start().check("v=c2FsdA==").is_err());
        let refused = start().check("e=invalid-proof").unwrap_err();
        assert!(refused.contains("invalid-proof"), "{refused}");
    }

    #[test]
    fn two_nonces_are_never_the_same() {
        let (one, other) = (nonce(), nonce());
        assert_eq!(one.len(), 24);
        assert_ne!(one, other);
    }

    #[test]
    fn the_md5_of_a_password_is_the_one_postgresql_documents() {
        // md5(md5(password + user) + salt), as hex, after "md5".
        let inner = text(&md5(b"s3cret-wordluca"));
        let mut salted = inner.into_bytes();
        salted.extend([1, 2, 3, 4]);
        assert_eq!(
            md5_password("luca", "s3cret-word", &[1, 2, 3, 4]),
            format!("md5{}", text(&md5(&salted)))
        );
        assert_eq!(md5_password("luca", "s3cret-word", &[1, 2, 3, 4]).len(), 35);
    }

    #[test]
    fn an_address_is_read_with_its_defaults() {
        let full = Target::parse("postgres://luca:pass@db.local:6543/shop").unwrap();
        assert_eq!(
            (full.host.as_str(), full.port, full.user.as_str()),
            ("db.local", 6543, "luca")
        );
        assert_eq!(
            (full.password.as_str(), full.database.as_str()),
            ("pass", "shop")
        );

        let bare = Target::parse(" postgresql://luca@localhost ").unwrap();
        assert_eq!((bare.port, bare.database.as_str()), (5432, "luca"));
        assert_eq!(bare.password, "");

        let odd = Target::parse("postgres://l%40c:p%3A%2F%20%C3%A9@[::1]:5433/my%20db").unwrap();
        assert_eq!((odd.user.as_str(), odd.password.as_str()), ("l@c", "p:/ é"));
        assert_eq!(
            (odd.host.as_str(), odd.port, odd.database.as_str()),
            ("::1", 5433, "my db")
        );

        // An @ left as it is in a password: the last one is the one before the host.
        let at = Target::parse("postgres://luca:p@ss@host/").unwrap();
        assert_eq!(
            (at.password.as_str(), at.host.as_str(), at.database.as_str()),
            ("p@ss", "host", "luca")
        );
        let nowhere = Target::parse("postgres://luca:x@:5433?application_name=x").unwrap();
        assert_eq!((nowhere.host.as_str(), nowhere.port), ("localhost", 5433));
    }

    #[test]
    fn an_address_that_asks_for_encryption_is_refused_and_other_options_ignored() {
        for secured in [
            "sslmode=require",
            "sslmode=verify-full",
            "ssl=true",
            "connect_timeout=3&channel_binding=require",
            "gssencmode=require",
        ] {
            let refused = Target::parse(&format!("postgres://u:hidden@h/d?{secured}")).unwrap_err();
            assert!(refused.contains("encrypted"), "{refused}");
            assert!(!refused.contains("hidden"), "{refused}");
        }
        for plain in ["sslmode=disable", "sslmode=prefer", "connect_timeout=3", ""] {
            let read = Target::parse(&format!("postgres://u:p@h/d?{plain}")).unwrap();
            assert_eq!(read.database, "d");
        }
    }

    #[test]
    fn an_address_that_cannot_be_read_says_why_without_the_password() {
        for wrong in [
            "mysql://u:hidden@h/d",
            "u:hidden@h/d",
            "postgres://h/d",
            "postgres://:hidden@h/d",
            "postgres://u:hidden@h:port/d",
            "postgres://u:hidden@h:0/d",
            "postgres://u:hidden@h:70000/d",
            "postgres://u:hidden%zz@h/d",
            "postgres://u:hidden%4@h/d",
            "postgres://u:hidden%ff@h/d",
            "postgres://u:hidden@[::1/d",
            "postgres://u:hidden",
        ] {
            let why = Target::parse(wrong).unwrap_err();
            assert!(!why.is_empty() && !why.contains("hidden"), "{wrong}: {why}");
        }
    }

    #[test]
    fn a_target_shown_for_debugging_hides_its_password() {
        let target = Target::parse("postgres://luca:s3cret-word@host/shop").unwrap();
        let shown = format!("{target:?}");
        assert!(!shown.contains("s3cret-word"), "{shown}");
        assert!(shown.contains("luca") && shown.contains("shop"), "{shown}");
    }

    #[test]
    fn a_name_is_quoted_to_stand_in_a_query() {
        assert_eq!(quoted("users"), "\"users\"");
        assert_eq!(quoted("my table"), "\"my table\"");
        assert_eq!(quoted("a\"b"), "\"a\"\"b\"");
        assert_eq!(quoted(""), "\"\"");
    }

    #[test]
    fn messages_are_written_with_their_kind_and_length() {
        assert_eq!(message(b'Q', &ended("select 1")), b"Q\0\0\0\x0dselect 1\0");
        assert_eq!(message(b'X', &[]), b"X\0\0\0\x04");
        let target = Target::parse("postgres://luca:pw@host/shop").unwrap();
        let expected: &[u8] = b"\0\0\0\x4f\0\x03\0\0user\0luca\0database\0shop\0\
            client_encoding\0UTF8\0application_name\0GodCode\0\0";
        assert_eq!(startup(&target), expected);
    }

    #[test]
    fn messages_are_read_and_impossible_ones_refused() {
        let mut two: &[u8] = b"C\0\0\0\x0dSELECT 1\0Z\0\0\0\x05I";
        assert_eq!(
            read_message(&mut two).unwrap(),
            (b'C', b"SELECT 1\0".to_vec())
        );
        assert_eq!(read_message(&mut two).unwrap(), (b'Z', b"I".to_vec()));
        assert!(read_message(&mut two).unwrap_err().contains("closed"));

        // A length that does not count itself, a negative one, one of 2 GB.
        for head in [
            b"D\0\0\0\x03",
            b"D\xff\xff\xff\xff",
            b"D\x7f\xff\xff\xff",
            b"D\x10\0\0\x05",
        ] {
            let mut bytes: &[u8] = head;
            let refused = read_message(&mut bytes).unwrap_err();
            assert!(refused.contains("impossible length"), "{refused}");
        }
        // 256 MB announced and three bytes sent: an error, and no such allocation.
        let mut cut: &[u8] = b"D\x10\0\0\x04abc";
        assert!(read_message(&mut cut).unwrap_err().contains("closed"));
        let mut half: &[u8] = b"D\0\0";
        assert!(read_message(&mut half).unwrap_err().contains("closed"));
    }

    #[test]
    fn a_body_that_ends_too_soon_is_an_error_and_never_a_panic() {
        assert!(row(&mut Body(&[0, 2, 0, 0, 0, 1, b'a'])).is_err());
        assert!(row(&mut Body(&[0, 1, 0, 0, 0, 9, b'a'])).is_err());
        // A length below zero that is not the -1 of NULL.
        assert!(row(&mut Body(&[0, 1, 0xff, 0xff, 0xff, 0xfe])).is_err());
        assert!(row(&mut Body(&[0xff, 0xff])).is_err());
        assert!(columns(&mut Body(b"\0\x01name\0\0\0")).is_err());
        assert!(columns(&mut Body(b"\0\x01name")).is_err());
        assert!(Body(&[]).int().is_err());
        assert_eq!(row(&mut Body(&[0, 0])).unwrap(), Vec::new());
    }

    #[test]
    fn a_refusal_is_told_in_a_line_or_two() {
        let whole = fields(&[
            (b'S', "ERROR"),
            (b'V', "ERROR"),
            (b'C', "42P01"),
            (b'M', "relation \"nope\" does not exist"),
            (b'P', "15"),
        ]);
        assert_eq!(
            refusal(&mut Body(&whole)),
            "ERROR: relation \"nope\" does not exist (at character 15)"
        );
        let more = fields(&[
            (b'S', "ERROR"),
            (b'M', "duplicate key"),
            (b'D', "Key (id)=(1) already exists."),
            (b'H', "Use another."),
        ]);
        assert_eq!(
            refusal(&mut Body(&more)),
            "ERROR: duplicate key\nDetail: Key (id)=(1) already exists. Hint: Use another."
        );
        assert!(refusal(&mut Body(&[])).contains("without saying why"));
        // Cut in the middle of a field: what was read is told.
        assert_eq!(refusal(&mut Body(b"Mno\0Dhalf")), "no");
    }

    #[test]
    fn a_server_that_trusts_lets_in_and_is_told_goodbye() {
        let (target, played) = server(|stream| {
            assert_eq!(
                greeted(stream),
                [
                    "user",
                    "luca",
                    "database",
                    "shop",
                    "client_encoding",
                    "UTF8",
                    "application_name",
                    "GodCode"
                ]
            );
            tell(stream, b'R', &request(0, &[]));
            tell(stream, b'S', b"server_version\x0017.2\0");
            tell(stream, b'N', &fields(&[(b'S', "NOTICE"), (b'M', "hello")]));
            tell(stream, b'K', &[0, 0, 0, 7, 1, 2, 3, 4]);
            tell(stream, b'Z', b"I");
            assert_eq!(left(stream), b"X\0\0\0\x04");
        });
        let connection = Connection::open(&target).unwrap();
        assert!(!format!("{connection:?}").contains(PASSWORD));
        drop(connection);
        played.join().unwrap();
    }

    #[test]
    fn a_server_that_asks_for_the_password_gets_it() {
        let (target, played) = server(|stream| {
            greeted(stream);
            tell(stream, b'R', &request(3, &[]));
            assert_eq!(hear(stream), (b'p', ended(PASSWORD)));
            tell(stream, b'R', &request(0, &[]));
            tell(stream, b'Z', b"I");
            left(stream);
        });
        drop(Connection::open(&target).unwrap());
        played.join().unwrap();
    }

    #[test]
    fn a_server_that_asks_for_md5_gets_the_hash_of_its_salt() {
        let (target, played) = server(|stream| {
            greeted(stream);
            tell(stream, b'R', &request(5, &[9, 8, 7, 6]));
            let inner = text(&md5(format!("{PASSWORD}luca").as_bytes()));
            let mut salted = inner.into_bytes();
            salted.extend([9, 8, 7, 6]);
            let expected = format!("md5{}", text(&md5(&salted)));
            assert_eq!(hear(stream), (b'p', ended(&expected)));
            tell(stream, b'R', &request(0, &[]));
            tell(stream, b'Z', b"I");
            left(stream);
        });
        drop(Connection::open(&target).unwrap());
        played.join().unwrap();
    }

    #[test]
    fn a_server_that_asks_for_scram_gets_a_proof_and_gives_its_own() {
        let (target, played) = server(|stream| scram_server(stream, Ending::Honest));
        drop(Connection::open(&target).unwrap());
        played.join().unwrap();
    }

    #[test]
    fn a_server_that_forges_its_scram_signature_is_not_believed() {
        let (target, played) = server(|stream| scram_server(stream, Ending::Forged));
        let refused = Connection::open(&target).unwrap_err();
        assert!(refused.contains("did not prove"), "{refused}");
        played.join().unwrap();
    }

    #[test]
    fn a_server_that_skips_its_scram_signature_is_not_believed() {
        let (target, played) = server(|stream| scram_server(stream, Ending::Silent));
        let refused = Connection::open(&target).unwrap_err();
        assert!(refused.contains("without proving"), "{refused}");
        played.join().unwrap();
    }

    #[test]
    fn a_wrong_password_is_told_as_the_server_tells_it_and_never_quoted() {
        let (target, played) = server(|stream| {
            greeted(stream);
            tell(stream, b'R', &request(3, &[]));
            hear(stream);
            let said = "password authentication failed for user \"luca\"";
            tell(stream, b'E', &fields(&[(b'S', "FATAL"), (b'M', said)]));
        });
        let refused = Connection::open(&target).unwrap_err();
        assert_eq!(
            refused,
            "FATAL: password authentication failed for user \"luca\""
        );
        assert!(!refused.contains(PASSWORD));
        played.join().unwrap();
    }

    #[test]
    fn a_way_to_log_in_that_is_not_known_is_named() {
        for (code, rest, name) in [
            (7, &b""[..], "GSSAPI"),
            (9, b"", "SSPI"),
            (2, b"", "Kerberos V5"),
            (42, b"", "method 42"),
            (10, b"SCRAM-SHA-256-PLUS\0\0", "SCRAM-SHA-256-PLUS"),
        ] {
            let (target, played) = server(move |stream| {
                greeted(stream);
                tell(stream, b'R', &request(code, rest));
                left(stream);
            });
            let refused = Connection::open(&target).unwrap_err();
            assert!(refused.contains(name), "{refused}");
            assert!(!refused.contains(PASSWORD));
            played.join().unwrap();
        }
    }

    #[test]
    fn a_password_with_a_zero_byte_is_refused_before_anyone_is_called() {
        let target = Target::parse("postgres://luca:hid%00den@127.0.0.1:1/shop").unwrap();
        let refused = Connection::open(&target).unwrap_err();
        assert!(refused.contains("zero byte"), "{refused}");
        assert!(!refused.contains("hid"), "{refused}");
    }

    #[test]
    fn a_select_gives_its_columns_and_rows_with_nulls_and_any_text() {
        let (mut connection, played) = open_server(|stream| {
            assert_eq!(asked(stream), "select * from people");
            tell(stream, b'T', &described(&["id", "name", "note"]));
            tell(
                stream,
                b'D',
                &values(&[Some("1"), Some("Zoë — 日本語"), None]),
            );
            tell(stream, b'D', &values(&[Some("2"), Some(""), Some("x")]));
            // A value that is not UTF-8: read as well as can be.
            tell(stream, b'D', b"\0\x01\0\0\0\x02f\xff");
            tell(stream, b'C', &ended("SELECT 3"));
            tell(stream, b'Z', b"I");
            left(stream);
        });
        let answer = connection.query("select * from people", 100).unwrap();
        assert_eq!(answer.columns, ["id", "name", "note"]);
        let text = |text: &str| Some(text.to_owned());
        assert_eq!(
            answer.rows,
            [
                vec![text("1"), text("Zoë — 日本語"), None],
                vec![text("2"), text(""), text("x")],
                vec![text("f\u{fffd}")],
            ]
        );
        assert_eq!((answer.done.as_str(), answer.cut), ("SELECT 3", false));
        drop(connection);
        played.join().unwrap();
    }

    #[test]
    fn only_the_first_rows_are_kept_and_the_cut_is_told() {
        let (mut connection, played) = open_server(|stream| {
            for _ in 0..3 {
                asked(stream);
                let numbers: Vec<String> = (0..500).map(|it| it.to_string()).collect();
                tell(stream, b'T', &described(&["n"]));
                for number in &numbers {
                    tell(stream, b'D', &values(&[Some(number)]));
                }
                tell(stream, b'C', &ended("SELECT 500"));
                tell(stream, b'Z', b"I");
            }
            left(stream);
        });
        let cut = connection.query("select n", 3).unwrap();
        let kept: Vec<_> = cut.rows.iter().map(|row| row[0].as_deref()).collect();
        assert_eq!(kept, [Some("0"), Some("1"), Some("2")]);
        assert_eq!((cut.done.as_str(), cut.cut), ("SELECT 500", true));

        let whole = connection.query("select n", 500).unwrap();
        assert_eq!((whole.rows.len(), whole.cut), (500, false));
        let none = connection.query("select n", 0).unwrap();
        assert_eq!((none.rows.len(), none.cut), (0, true));
        assert_eq!(none.columns, ["n"]);
        drop(connection);
        played.join().unwrap();
    }

    #[test]
    fn of_several_statements_the_last_rows_and_the_last_word_are_kept() {
        let (mut connection, played) = open_server(|stream| {
            asked(stream);
            result(
                stream,
                &["a"],
                &[&[Some("1")], &[Some("2")], &[Some("3")]],
                "SELECT 3",
            );
            result(stream, &["b", "c"], &[&[Some("x"), None]], "SELECT 1");
            tell(stream, b'C', &ended("UPDATE 4"));
            // A select of no column at all: its rows are nothing to show.
            result(stream, &[], &[&[], &[]], "SELECT 2");
            tell(stream, b'Z', b"I");

            asked(stream);
            tell(stream, b'I', &[]);
            tell(stream, b'Z', b"I");
            left(stream);
        });
        let answer = connection.query("select 1; select 2; update; select", 2);
        assert_eq!(
            answer.unwrap(),
            Answer {
                columns: vec!["b".into(), "c".into()],
                rows: vec![vec![Some("x".into()), None]],
                done: "SELECT 2".into(),
                // The first result was cut, the one kept was not.
                cut: false,
            }
        );
        assert_eq!(connection.query("", 2).unwrap(), Answer::default());
        drop(connection);
        played.join().unwrap();
    }

    #[test]
    fn a_refused_query_is_an_error_and_the_next_one_is_answered() {
        let (mut connection, played) = open_server(|stream| {
            asked(stream);
            result(stream, &["a"], &[&[Some("1")]], "SELECT 1");
            let said = "relation \"nope\" does not exist";
            let refused = fields(&[(b'S', "ERROR"), (b'M', said), (b'P', "15")]);
            tell(stream, b'E', &refused);
            tell(stream, b'Z', b"I");

            assert_eq!(asked(stream), "select 2");
            result(stream, &["b"], &[&[Some("2")]], "SELECT 1");
            tell(stream, b'Z', b"I");
            assert_eq!(left(stream), b"X\0\0\0\x04");
        });
        let refused = connection.query("select 1; select * from nope", 9);
        assert_eq!(
            refused.unwrap_err(),
            "ERROR: relation \"nope\" does not exist (at character 15)"
        );
        let answer = connection.query("select 2", 9).unwrap();
        assert_eq!(answer.columns, ["b"]);
        assert_eq!(answer.rows, [[Some("2".to_owned())]]);
        drop(connection);
        played.join().unwrap();
    }

    #[test]
    fn what_a_server_says_aside_does_not_disturb_an_answer() {
        let (mut connection, played) = open_server(|stream| {
            asked(stream);
            let notice = fields(&[(b'S', "NOTICE"), (b'M', "table is new")]);
            tell(stream, b'N', &notice);
            tell(stream, b'T', &described(&["a"]));
            tell(stream, b'D', &values(&[Some("1")]));
            tell(stream, b'N', &notice);
            tell(stream, b'S', b"TimeZone\0UTC\0");
            tell(stream, b'A', b"\0\0\0\x07channel\0payload\0");
            tell(stream, b'D', &values(&[Some("2")]));
            tell(stream, b'C', &ended("SELECT 2"));
            tell(stream, b'Z', b"T");
            left(stream);
        });
        let answer = connection.query("select a", 9).unwrap();
        assert_eq!(answer.rows.len(), 2);
        assert_eq!(answer.done, "SELECT 2");
        drop(connection);
        played.join().unwrap();
    }

    #[test]
    fn a_server_that_leaves_in_the_middle_of_an_answer_is_an_error() {
        let (mut connection, played) = open_server(|stream| {
            asked(stream);
            tell(stream, b'T', &described(&["a"]));
            tell(stream, b'D', &values(&[Some("1")]));
            // Half a row, then nothing.
            stream.write_all(b"D\0\0\0\x20\0\x01").unwrap();
        });
        let refused = connection.query("select a", 9).unwrap_err();
        played.join().unwrap();
        assert!(
            refused.contains("closed") || refused.contains("broke"),
            "{refused}"
        );
        // Nothing more is asked of a connection that broke.
        assert_eq!(connection.query("select 1", 9).unwrap_err(), CLOSED);
        assert_eq!(connection.tables().unwrap_err(), CLOSED);
    }

    #[test]
    fn a_message_of_an_absurd_length_is_an_error_and_closes_the_connection() {
        for head in [b"D\x7f\xff\xff\xf0", b"D\xff\xff\xff\xf0", b"D\0\0\0\x01"] {
            let (mut connection, played) = open_server(move |stream| {
                asked(stream);
                tell(stream, b'T', &described(&["a"]));
                stream.write_all(head).unwrap();
                // No goodbye comes: the client no longer knows where a message starts.
                assert_eq!(left(stream), b"");
            });
            let refused = connection.query("select a", 9).unwrap_err();
            assert!(refused.contains("impossible length"), "{refused}");
            assert_eq!(connection.query("select 1", 9).unwrap_err(), CLOSED);
            drop(connection);
            played.join().unwrap();
        }
    }

    #[test]
    fn a_row_that_lies_about_its_size_is_an_error_and_closes_the_connection() {
        let (mut connection, played) = open_server(|stream| {
            asked(stream);
            tell(stream, b'T', &described(&["a"]));
            // One value said to be 100 bytes long, in a message that holds one.
            tell(stream, b'D', b"\0\x01\0\0\0\x64a");
            left(stream);
        });
        assert_eq!(connection.query("select a", 9).unwrap_err(), SHORT);
        assert_eq!(connection.query("select 1", 9).unwrap_err(), CLOSED);
        drop(connection);
        played.join().unwrap();
    }

    #[test]
    fn a_copy_is_refused_either_way_and_the_connection_goes_on() {
        let (mut connection, played) = open_server(|stream| {
            assert_eq!(asked(stream), "copy people from stdin");
            tell(stream, b'G', &[0, 0, 0]);
            let (kind, _) = hear(stream);
            assert_eq!(kind, b'f', "the client says no rows will come");
            let said = "COPY from stdin failed";
            tell(stream, b'E', &fields(&[(b'S', "ERROR"), (b'M', said)]));
            tell(stream, b'Z', b"I");

            assert_eq!(asked(stream), "copy people to stdout");
            tell(stream, b'H', &[0, 0, 0]);
            tell(stream, b'd', b"1\tZoe\n");
            tell(stream, b'd', b"2\tLuca\n");
            tell(stream, b'c', &[]);
            tell(stream, b'C', &ended("COPY 2"));
            tell(stream, b'Z', b"I");

            asked(stream);
            result(stream, &["a"], &[&[Some("1")]], "SELECT 1");
            tell(stream, b'Z', b"I");
            assert_eq!(left(stream), b"X\0\0\0\x04");
        });
        let coming = connection.query("copy people from stdin", 9).unwrap_err();
        assert_eq!(coming, NO_COPY);
        let going = connection.query("copy people to stdout", 9).unwrap_err();
        assert_eq!(going, NO_COPY);
        assert_eq!(connection.query("select 1", 9).unwrap().rows.len(), 1);
        drop(connection);
        played.join().unwrap();
    }

    #[test]
    fn a_query_with_a_zero_byte_is_not_sent() {
        let (mut connection, played) = open_server(|stream| {
            assert_eq!(asked(stream), "select 1");
            tell(stream, b'I', &[]);
            tell(stream, b'Z', b"I");
            left(stream);
        });
        assert!(connection.query("select 1\0; drop", 9).is_err());
        // It was refused here: the connection is as good as before.
        connection.query("select 1", 9).unwrap();
        drop(connection);
        played.join().unwrap();
    }

    #[test]
    fn the_tables_are_asked_for_and_listed() {
        let (mut connection, played) = open_server(|stream| {
            assert_eq!(asked(stream), LIST_TABLES);
            result(
                stream,
                &["?column?"],
                &[
                    &[Some("public.orders")],
                    &[Some("public.\"my table\"")],
                    &[None],
                    &[Some("sales.totals")],
                ],
                "SELECT 4",
            );
            tell(stream, b'Z', b"I");
            left(stream);
        });
        assert_eq!(
            connection.tables().unwrap(),
            ["public.orders", "public.\"my table\"", "sales.totals"]
        );
        drop(connection);
        played.join().unwrap();
    }
}
