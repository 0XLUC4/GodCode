//! SQLite database files, read without SQLite: their tables and the rows of each.
//!
//! A file is a run of pages of one size. The first starts with a 100-byte header and holds
//! the schema, itself a table listing every other one. A table is a tree of pages: interior
//! pages point to their children, leaf pages hold the rows in rowid order, and a row too
//! long for its leaf goes on through a chain of overflow pages.
//!
//! Reading only: nothing here writes. `WITHOUT ROWID` tables, text in UTF-16 and encrypted
//! files are refused with an error, and what a write-ahead log still holds is not seen
//! (see [`Database::uses_wal`]).

use std::borrow::Cow;
use std::fmt;

const MAGIC: &[u8] = b"SQLite format 3\0";
const HEADER: usize = 100;
const INTERIOR: u8 = 5;
const LEAF: u8 = 13;
/// No real table is this deep: one that seems to be is a file whose pages loop.
const DEEPEST: usize = 64;

/// What one column of one row holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    /// Bytes that are not text: only how many there are.
    Blob(usize),
}

impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => formatter.write_str("NULL"),
            Self::Integer(whole) => write!(formatter, "{whole}"),
            // As SQLite shows them: `2.0` keeps its point, `1e300` its exponent.
            Self::Real(real) => write!(formatter, "{real:?}"),
            Self::Text(text) => formatter.write_str(text),
            Self::Blob(1) => formatter.write_str("BLOB (1 byte)"),
            Self::Blob(length) => write!(formatter, "BLOB ({length} bytes)"),
        }
    }
}

/// How a column shows what a row stores for it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stored {
    AsIs,
    /// The column is the rowid under another name: rows store nothing for it.
    Rowid,
    /// A column of reals, whose whole numbers are stored as integers to save room.
    Real,
}

/// A table of a database.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub name: String,
    /// The `CREATE TABLE` statement that made it.
    pub sql: String,
    /// The columns a row stores a value for, in order, as far as `sql` could be understood.
    pub columns: Vec<String>,
    /// The page its tree of rows starts at.
    pub root_page: u32,
    stored: Vec<Stored>,
    without_rowid: bool,
}

/// What a `CREATE TABLE` statement says of the table.
struct Declared {
    columns: Vec<String>,
    stored: Vec<Stored>,
    without_rowid: bool,
}

enum Token {
    Word(String),
    /// A name or a string between quotes, without them.
    Quoted(String),
    Symbol(char),
}

fn is(token: &Token, word: &str) -> bool {
    matches!(token, Token::Word(found) if found.eq_ignore_ascii_case(word))
}

fn is_any(token: &Token, words: &[&str]) -> bool {
    words.iter().any(|word| is(token, word))
}

/// What stands before the quote `close`, a doubled one being part of it, and what follows.
fn quoted(rest: &str, close: char) -> (String, &str) {
    let mut inside = String::new();
    let mut chars = rest.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        let doubled = close != ']' && chars.peek().is_some_and(|(_, next)| *next == close);
        if ch != close {
            inside.push(ch);
        } else if doubled {
            inside.push(ch);
            chars.next();
        } else {
            return (inside, &rest[at + ch.len_utf8()..]);
        }
    }
    (inside, "")
}

/// The words, quoted names and symbols of a statement, comments left out.
fn tokens(sql: &str) -> Vec<Token> {
    let in_word = |ch: char| ch.is_alphanumeric() || matches!(ch, '_' | '$') || !ch.is_ascii();
    let mut tokens = Vec::new();
    let mut rest = sql;
    while let Some(first) = rest.chars().next() {
        let after = &rest[first.len_utf8()..];
        let close = match first {
            '"' | '\'' | '`' => Some(first),
            '[' => Some(']'),
            _ => None,
        };
        if first.is_whitespace() {
            rest = after;
        } else if let Some(comment) = rest.strip_prefix("--") {
            rest = comment.split_once('\n').map_or("", |(_, next)| next);
        } else if let Some(comment) = rest.strip_prefix("/*") {
            rest = comment.split_once("*/").map_or("", |(_, next)| next);
        } else if let Some(close) = close {
            let (inside, next) = quoted(after, close);
            tokens.push(Token::Quoted(inside));
            rest = next;
        } else if in_word(first) {
            let end = rest.find(|ch| !in_word(ch)).unwrap_or(rest.len());
            tokens.push(Token::Word(rest[..end].to_owned()));
            rest = &rest[end..];
        } else {
            tokens.push(Token::Symbol(first));
            rest = after;
        }
    }
    tokens
}

/// Whether a column of the declared type `kind` (in capitals) turns its numbers into reals.
fn holds_reals(kind: &str) -> bool {
    let has = |parts: &[&str]| parts.iter().any(|part| kind.contains(part));
    !has(&["INT", "CHAR", "CLOB", "TEXT", "BLOB"]) && has(&["REAL", "FLOA", "DOUB"])
}

/// Read a `CREATE TABLE` statement as well as can be: what is not understood is skipped.
fn declared(sql: &str) -> Declared {
    let tokens = tokens(sql);
    let opens = |token: &Token| matches!(token, Token::Symbol('('));
    let open = tokens.iter().position(opens).unwrap_or(tokens.len());

    // What stands between the commas of the outer brackets: a column or a constraint each.
    let mut definitions: Vec<&[Token]> = Vec::new();
    let (mut depth, mut start, mut end) = (0_usize, open + 1, tokens.len());
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token {
            Token::Symbol('(') => depth += 1,
            Token::Symbol(')') if depth <= 1 => {
                end = index;
                break;
            }
            Token::Symbol(')') => depth -= 1,
            Token::Symbol(',') if depth == 1 => {
                definitions.push(&tokens[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    definitions.extend(tokens.get(start..end));

    let mut columns = Vec::new();
    let mut stored = Vec::new();
    let mut integers = Vec::new();
    let mut sole_key = None;
    for definition in definitions {
        let definition = match definition {
            [first, _, named @ ..] if is(first, "CONSTRAINT") => named,
            whole => whole,
        };
        let Some((first, rest)) = definition.split_first() else {
            continue;
        };
        if is(first, "PRIMARY") {
            let inside = rest.iter().skip_while(|token| !opens(token)).skip(1);
            let mut key = inside.take_while(|token| !matches!(token, Token::Symbol(')')));
            let name = key.next();
            if !key.any(|token| matches!(token, Token::Symbol(','))) {
                sole_key = name;
            }
            continue;
        }
        let (Token::Word(name) | Token::Quoted(name)) = first else {
            continue;
        };
        if is_any(first, &["UNIQUE", "CHECK", "FOREIGN"]) {
            continue;
        }
        // A column computed when read, unless said to be stored, is not in the file.
        let computed = rest
            .windows(2)
            .any(|pair| is(&pair[0], "AS") && opens(&pair[1]));
        if computed && !rest.iter().any(|token| is(token, "STORED")) {
            continue;
        }
        const AFTER_TYPE: [&str; 11] = [
            "CONSTRAINT",
            "PRIMARY",
            "NOT",
            "NULL",
            "UNIQUE",
            "CHECK",
            "DEFAULT",
            "COLLATE",
            "REFERENCES",
            "GENERATED",
            "AS",
        ];
        let kind: Vec<String> = rest
            .iter()
            .map_while(|token| match token {
                Token::Word(word) if !is_any(token, &AFTER_TYPE) => Some(word.to_uppercase()),
                _ => None,
            })
            .collect();
        let integer = kind == ["INTEGER"];
        // Written with `DESC`, an old quirk of SQLite makes it an ordinary column.
        let key = rest.iter().position(|token| is(token, "PRIMARY"));
        let key = key.is_some_and(|at| !rest.get(at + 2).is_some_and(|next| is(next, "DESC")));
        columns.push(name.clone());
        integers.push(integer);
        stored.push(if integer && key {
            Stored::Rowid
        } else if holds_reals(&kind.join(" ")) {
            Stored::Real
        } else {
            Stored::AsIs
        });
    }
    if let Some(Token::Word(key) | Token::Quoted(key)) = sole_key {
        let named = |name: &String| name.eq_ignore_ascii_case(key);
        if let Some(index) = columns
            .iter()
            .position(named)
            .filter(|index| integers[*index])
        {
            stored[index] = Stored::Rowid;
        }
    }
    Declared {
        columns,
        stored,
        without_rowid: tokens[end..].iter().any(|token| is(token, "WITHOUT")),
    }
}

fn damaged(what: impl fmt::Display) -> String {
    format!("The database file is damaged: {what}.")
}

/// The `length` bytes at `at`, when the file has them.
fn slice(bytes: &[u8], at: usize, length: usize) -> Result<&[u8], String> {
    let end = at.checked_add(length);
    end.and_then(|end| bytes.get(at..end))
        .ok_or_else(|| damaged("part of it points past its own end"))
}

/// The whole number the bytes at `at` spell, the first being the heaviest.
fn number(bytes: &[u8], at: usize, length: usize) -> Result<usize, String> {
    let digits = slice(bytes, at, length)?;
    Ok(digits
        .iter()
        .fold(0, |number, byte| number << 8 | usize::from(*byte)))
}

fn page_number(bytes: &[u8], at: usize) -> Result<u32, String> {
    let digits = slice(bytes, at, 4)?;
    Ok(digits
        .iter()
        .fold(0, |number, byte| number << 8 | u32::from(*byte)))
}

/// The number written at `at` in one to nine bytes, and how many it took.
fn varint(bytes: &[u8], at: usize) -> Result<(u64, usize), String> {
    let mut number = 0_u64;
    for length in 1..=8 {
        let byte = slice(bytes, at + length - 1, 1)?[0];
        number = number << 7 | u64::from(byte & 0x7f);
        if byte < 0x80 {
            return Ok((number, length));
        }
    }
    // A ninth byte counts whole.
    let last = slice(bytes, at + 8, 1)?[0];
    Ok((number << 8 | u64::from(last), 9))
}

/// The values of a row, from the bytes it is stored as.
fn record(payload: &[u8]) -> Result<Vec<Value>, String> {
    let (header, mut at) = varint(payload, 0)?;
    let header = usize::try_from(header)
        .ok()
        .filter(|end| *end <= payload.len());
    let header = header.ok_or_else(|| damaged("a row whose list of columns is cut short"))?;
    let mut body = header;
    let mut values = Vec::new();
    while at < header {
        let (kind, taken) = varint(payload, at)?;
        at += taken;
        let length = match kind {
            0 | 8 | 9 => 0,
            1..=4 => kind,
            5 => 6,
            6 | 7 => 8,
            10 | 11 => return Err(damaged("a value of a kind that does not exist")),
            _ => (kind - 12) / 2,
        };
        let length = usize::try_from(length).unwrap_or(usize::MAX);
        let content = slice(payload, body, length)?;
        body += length;
        values.push(match kind {
            0 => Value::Null,
            8 => Value::Integer(0),
            9 => Value::Integer(1),
            1..=6 => {
                let negative = content.first().is_some_and(|byte| *byte >= 0x80);
                let sign = if negative { -1_i64 } else { 0 };
                Value::Integer(
                    content
                        .iter()
                        .fold(sign, |whole, byte| whole << 8 | i64::from(*byte)),
                )
            }
            7 => {
                let bits = content
                    .iter()
                    .fold(0_u64, |bits, byte| bits << 8 | u64::from(*byte));
                Value::Real(f64::from_bits(bits))
            }
            _ if kind % 2 == 0 => Value::Blob(length),
            _ => Value::Text(String::from_utf8_lossy(content).into_owned()),
        });
    }
    Ok(values)
}

/// A page of a table's tree.
struct Page<'a> {
    bytes: &'a [u8],
    leaf: bool,
    /// How many rows a leaf holds, or how many children an interior page has but one.
    cells: usize,
    /// Where the list of where each cell is starts.
    pointers: usize,
    /// The last child of an interior page.
    last_child: u32,
}

impl Page<'_> {
    /// Where the cell `index` starts in the page.
    fn cell(&self, index: usize) -> Result<usize, String> {
        number(self.bytes, self.pointers + 2 * index, 2)
    }
}

/// A SQLite database file held in memory, to read its tables from.
pub struct Database {
    bytes: Vec<u8>,
    page_size: usize,
    /// How much of a page holds data: an extension may keep the end of each for itself.
    usable: usize,
    wal: bool,
    tables: Vec<Table>,
}

impl Database {
    /// Take the bytes of a database file, refusing what is not one or cannot be read.
    pub fn open(bytes: Vec<u8>) -> Result<Self, String> {
        if bytes.is_empty() {
            return Err("This file is empty: it holds no database yet.".to_owned());
        }
        let header = bytes
            .get(..HEADER)
            .filter(|header| header.starts_with(MAGIC));
        let header = header.ok_or("This is not a SQLite database, or it is an encrypted one.")?;
        let page_size = match number(header, 16, 2)? {
            1 => 65536,
            size if size >= 512 && size.is_power_of_two() => size,
            size => return Err(damaged(format!("it says its pages are {size} bytes long"))),
        };
        let usable = page_size - number(header, 20, 1)?;
        if usable < 480 {
            return Err(damaged("its pages leave no room for data"));
        }
        let format = number(header, 19, 1)?;
        if format > 2 {
            return Err("This database is in a newer format than can be read here.".to_owned());
        }
        match number(header, 56, 4)? {
            0 | 1 => {}
            2 | 3 => {
                let refusal =
                    "This database stores its text as UTF-16: only UTF-8 ones can be read.";
                return Err(refusal.to_owned());
            }
            _ => return Err(damaged("its text is in an unknown encoding")),
        }
        let mut database = Self {
            bytes,
            page_size,
            usable,
            wal: format == 2,
            tables: Vec::new(),
        };
        database.tables = database.schema()?;
        Ok(database)
    }

    /// The tables of the database, in the order they were made. Views, indexes, triggers
    /// and the tables SQLite keeps for itself are not among them.
    pub fn tables(&self) -> &[Table] {
        &self.tables
    }

    /// Whether the database writes to a log beside it (the file ending in `-wal`) before
    /// the file itself. What that log holds is not in these bytes: while it is not empty,
    /// the newest changes are missing from what is read here.
    pub fn uses_wal(&self) -> bool {
        self.wal
    }

    /// How many rows `table` has.
    pub fn count(&self, table: &Table) -> Result<usize, String> {
        let mut count = 0_usize;
        self.leaves(self.root(table)?, &mut |page| {
            count = count.saturating_add(page.cells);
            Ok(true)
        })?;
        Ok(count)
    }

    /// Up to `limit` rows of `table` in rowid order, the first `offset` being skipped.
    /// Each has a value for every column of the table at least.
    pub fn rows(
        &self,
        table: &Table,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Vec<Value>>, String> {
        let mut rows = Vec::new();
        for (rowid, mut values) in self.records(self.root(table)?, offset, limit)? {
            // A row written before a column was added has nothing for it.
            if values.len() < table.columns.len() {
                values.resize(table.columns.len(), Value::Null);
            }
            for (value, stored) in values.iter_mut().zip(&table.stored) {
                *value = match (stored, &*value) {
                    (Stored::Rowid, Value::Null) => Value::Integer(rowid),
                    (Stored::Real, Value::Integer(whole)) => Value::Real(*whole as f64),
                    _ => continue,
                };
            }
            rows.push(values);
        }
        Ok(rows)
    }

    fn schema(&self) -> Result<Vec<Table>, String> {
        let mut tables = Vec::new();
        for (_, entry) in self.records(1, 0, usize::MAX)? {
            let [
                Value::Text(kind),
                Value::Text(name),
                _,
                Value::Integer(root),
                Value::Text(sql),
            ] = entry.as_slice()
            else {
                continue;
            };
            if kind != "table" || name.starts_with("sqlite_") {
                continue;
            }
            let root_page = u32::try_from(*root);
            let root_page = root_page.map_err(|_| damaged(format!("no page holds {name}")))?;
            let declared = declared(sql);
            tables.push(Table {
                name: name.clone(),
                sql: sql.clone(),
                columns: declared.columns,
                root_page,
                stored: declared.stored,
                without_rowid: declared.without_rowid,
            });
        }
        Ok(tables)
    }

    /// The page a table's rows start at, when they are stored in a way that can be read.
    fn root(&self, table: &Table) -> Result<u32, String> {
        let name = &table.name;
        if table.without_rowid {
            return Err(format!(
                "{name} is a WITHOUT ROWID table: its rows cannot be read here."
            ));
        }
        if table.root_page == 0 {
            return Err(format!(
                "{name} is a virtual table: its rows are not in the file."
            ));
        }
        Ok(table.root_page)
    }

    /// The page `number` (counted from 1), without what an extension keeps at its end.
    fn page(&self, number: u32) -> Result<&[u8], String> {
        let index = usize::try_from(number)
            .ok()
            .and_then(|number| number.checked_sub(1));
        let start = index.and_then(|index| index.checked_mul(self.page_size));
        let page = start.and_then(|start| self.bytes.get(start..start.checked_add(self.usable)?));
        page.ok_or_else(|| damaged(format!("page {number} is missing")))
    }

    fn tree_page(&self, number: u32) -> Result<Page<'_>, String> {
        let bytes = self.page(number)?;
        // The first page starts with the header of the file.
        let start = if number == 1 { HEADER } else { 0 };
        let leaf = match slice(bytes, start, 1)?[0] {
            LEAF => true,
            INTERIOR => false,
            2 | 10 => {
                return Err(damaged(format!(
                    "page {number} belongs to an index, not a table"
                )));
            }
            _ => return Err(damaged(format!("page {number} is not part of a table"))),
        };
        Ok(Page {
            bytes,
            leaf,
            cells: self::number(bytes, start + 3, 2)?,
            pointers: start + if leaf { 8 } else { 12 },
            last_child: if leaf {
                0
            } else {
                page_number(bytes, start + 8)?
            },
        })
    }

    /// Hand `visit` each leaf of the tree starting at `root`, in order, until it says to stop.
    fn leaves(
        &self,
        root: u32,
        visit: &mut dyn FnMut(&Page) -> Result<bool, String>,
    ) -> Result<(), String> {
        // A tree goes through each of its pages once: more visits than pages is a loop.
        let mut visits_left = self.bytes.len() / self.page_size;
        self.walk(root, 0, &mut visits_left, visit).map(|_| ())
    }

    /// Whether the walk goes on after the leaves under the page `number`.
    fn walk(
        &self,
        number: u32,
        depth: usize,
        visits_left: &mut usize,
        visit: &mut dyn FnMut(&Page) -> Result<bool, String>,
    ) -> Result<bool, String> {
        if depth > DEEPEST || *visits_left == 0 {
            return Err(damaged("the pages of a table lead back to themselves"));
        }
        *visits_left -= 1;
        let page = self.tree_page(number)?;
        if page.leaf {
            return visit(&page);
        }
        for index in 0..page.cells {
            let child = page_number(page.bytes, page.cell(index)?)?;
            if !self.walk(child, depth + 1, visits_left, visit)? {
                return Ok(false);
            }
        }
        self.walk(page.last_child, depth + 1, visits_left, visit)
    }

    /// Up to `limit` rows of the tree at `root` after the first `offset`: the rowid and the
    /// stored values of each.
    fn records(
        &self,
        root: u32,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<(i64, Vec<Value>)>, String> {
        let mut to_skip = offset;
        let mut records = Vec::new();
        self.leaves(root, &mut |page| {
            // A leaf wholly before the window is passed without reading its rows.
            let first = to_skip.min(page.cells);
            to_skip -= first;
            for index in first..page.cells {
                if records.len() >= limit {
                    break;
                }
                let (rowid, payload) = self.row(page, page.cell(index)?)?;
                records.push((rowid, record(&payload)?));
            }
            Ok(records.len() < limit)
        })?;
        Ok(records)
    }

    /// The rowid and the bytes of the row whose cell starts at `at` in a leaf, gathered
    /// from its overflow pages when it has some.
    fn row<'a>(&'a self, leaf: &Page<'a>, at: usize) -> Result<(i64, Cow<'a, [u8]>), String> {
        let (size, size_length) = varint(leaf.bytes, at)?;
        let (rowid, rowid_length) = varint(leaf.bytes, at + size_length)?;
        // The rowid is a signed number written as an unsigned one.
        let rowid = rowid as i64;
        let start = at + size_length + rowid_length;
        let size = usize::try_from(size)
            .ok()
            .filter(|size| *size <= self.bytes.len());
        let size = size.ok_or_else(|| damaged("a row says it is larger than the file"))?;

        // How much of a row stays in its leaf is a rule of the format.
        let most = self.usable - 35;
        if size <= most {
            return Ok((rowid, Cow::Borrowed(slice(leaf.bytes, start, size)?)));
        }
        let least = (self.usable - 12) * 32 / 255 - 23;
        let per_overflow = self.usable - 4;
        let fitted = least + (size - least) % per_overflow;
        let local = if fitted <= most { fitted } else { least };

        let mut payload = slice(leaf.bytes, start, local)?.to_vec();
        let mut next = page_number(leaf.bytes, start + local)?;
        // Each turn adds bytes to a row no larger than the file: this ends.
        while payload.len() < size {
            let overflow = self.page(next)?;
            let taken = (size - payload.len()).min(per_overflow);
            payload.extend_from_slice(slice(overflow, 4, taken)?);
            next = page_number(overflow, 0)?;
        }
        Ok((rowid, Cow::Owned(payload)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = include_bytes!("../tests/fixtures/sample.db");
    const UTF16: &[u8] = include_bytes!("../tests/fixtures/utf16.db");
    const WAL: &[u8] = include_bytes!("../tests/fixtures/wal.db");

    fn sample() -> Database {
        Database::open(SAMPLE.to_vec()).expect("the sample opens")
    }

    fn table<'a>(database: &'a Database, name: &str) -> &'a Table {
        let found = database.tables().iter().find(|table| table.name == name);
        found.unwrap_or_else(|| panic!("no table {name}"))
    }

    fn all_rows(database: &Database, name: &str) -> Vec<Vec<Value>> {
        let rows = database.rows(table(database, name), 0, usize::MAX);
        rows.unwrap_or_else(|error| panic!("{name}: {error}"))
    }

    fn refusal(bytes: &[u8]) -> String {
        match Database::open(bytes.to_vec()) {
            Ok(_) => panic!("it opened"),
            Err(refusal) => refusal,
        }
    }

    /// How many rows the readable tables of a file have, every one being read.
    fn read_everything(bytes: Vec<u8>) -> Result<usize, String> {
        let database = Database::open(bytes)?;
        let mut total = 0;
        for table in database.tables() {
            if table.name == "settings" {
                continue;
            }
            let rows = database.rows(table, 0, usize::MAX)?;
            assert_eq!(rows.len(), database.count(table)?, "{}", table.name);
            total += rows.len();
        }
        Ok(total)
    }

    fn text(text: &str) -> Value {
        Value::Text(text.to_owned())
    }

    fn letters(length: usize) -> String {
        (0..length)
            .map(|index| char::from(b'a' + (index % 26) as u8))
            .collect()
    }

    #[test]
    fn only_the_tables_still_there_are_listed() {
        let database = sample();
        let names: Vec<&str> = database
            .tables()
            .iter()
            .map(|table| table.name.as_str())
            .collect();
        // Not the dropped `scratch`, the view, the trigger, the indexes or `sqlite_` tables.
        let expected = [
            "people",
            "numbers",
            "events",
            "documents",
            "settings",
            "prices",
            "tags",
            "notes",
            "odd \"names\"",
            "empty",
        ];
        assert_eq!(names, expected);
        assert!(
            table(&database, "people")
                .sql
                .starts_with("CREATE TABLE people (")
        );
        assert_eq!(table(&database, "people").root_page, 2);
        assert!(!database.uses_wal());
    }

    #[test]
    fn columns_are_named_after_the_create_table_text() {
        let database = sample();
        let columns = |name: &str| table(&database, name).columns.clone();
        // The comment, the constraints of each column and those of the table are not columns.
        assert_eq!(
            columns("people"),
            ["id", "full name", "age", "score", "photo", "note"]
        );
        assert_eq!(
            columns("odd \"names\""),
            ["first col", "second", "third", "fo,ur"]
        );
        assert_eq!(columns("notes"), ["body", "pinned"]);
        assert_eq!(columns("settings"), ["key", "value"]);
        assert_eq!(columns("empty"), ["a", "b"]);
    }

    #[test]
    fn a_create_table_text_is_read_whatever_its_writing() {
        let read = declared("create table t(a integer primary key desc, b Double Precision, c)");
        assert_eq!(read.columns, ["a", "b", "c"]);
        assert_eq!(read.stored, [Stored::AsIs, Stored::Real, Stored::AsIs]);
        assert!(!read.without_rowid);

        let read = declared(
            "CREATE TABLE t (\"a\" INTEGER /* , x */ CONSTRAINT k PRIMARY KEY AUTOINCREMENT,
             b VARCHAR(10) DEFAULT 'x, y)', c AS (a + 1) STORED,
             CONSTRAINT u UNIQUE (b, c), FOREIGN KEY (b) REFERENCES o (p, q)) STRICT, WITHOUT ROWID",
        );
        assert_eq!(read.columns, ["a", "b", "c"]);
        assert_eq!(read.stored, [Stored::Rowid, Stored::AsIs, Stored::AsIs]);
        assert!(read.without_rowid);

        // A key over two columns names neither as the rowid.
        let read = declared("CREATE TABLE t (a INTEGER, b INTEGER, PRIMARY KEY (a, b))");
        assert_eq!(read.stored, [Stored::AsIs, Stored::AsIs]);

        for unfinished in [
            "",
            "CREATE TABLE t",
            "CREATE TABLE t (",
            "CREATE TABLE t (a, \"b",
        ] {
            declared(unfinished);
        }
    }

    #[test]
    fn nulls_reals_unicode_text_and_blobs_are_read() {
        let rows = all_rows(&sample(), "people");
        let ada = vec![
            Value::Integer(1),
            text("Ada Lovelace"),
            Value::Integer(36),
            Value::Real(9.5),
            Value::Blob(3),
            Value::Null,
        ];
        let zoe = vec![
            Value::Integer(2),
            text("Zoë — 日本語 😀"),
            Value::Null,
            Value::Real(-0.25),
            Value::Null,
            text("héllo"),
        ];
        let bob = vec![
            Value::Integer(40),
            text("Bob"),
            Value::Integer(0),
            Value::Real(1e300),
            Value::Blob(0),
            text(""),
        ];
        assert_eq!(rows, [ada, zoe, bob]);
    }

    #[test]
    fn integers_of_every_stored_size_are_read() {
        let rows = all_rows(&sample(), "numbers");
        let expected = [
            ("zero", Value::Integer(0)),
            ("one", Value::Integer(1)),
            ("minus one", Value::Integer(-1)),
            ("one byte", Value::Integer(127)),
            ("one byte, negative", Value::Integer(-128)),
            ("two bytes", Value::Integer(32767)),
            ("two bytes, negative", Value::Integer(-32768)),
            ("three bytes", Value::Integer(8_388_607)),
            ("three bytes, negative", Value::Integer(-8_388_608)),
            ("four bytes", Value::Integer(2_147_483_647)),
            ("four bytes, negative", Value::Integer(-2_147_483_648)),
            ("six bytes", Value::Integer(140_737_488_355_327)),
            ("six bytes, negative", Value::Integer(-140_737_488_355_328)),
            ("eight bytes", Value::Integer(i64::MAX)),
            ("eight bytes, negative", Value::Integer(i64::MIN)),
            ("real", Value::Real(2.0)),
            ("small real", Value::Real(-1.5e-7)),
        ];
        let expected: Vec<Vec<Value>> = expected
            .into_iter()
            .map(|(label, value)| vec![text(label), value])
            .collect();
        assert_eq!(rows, expected);
    }

    #[test]
    fn a_table_too_big_for_a_page_is_read_through_its_interior_pages() {
        let database = sample();
        let events = table(&database, "events");
        let event = |number: i64| {
            vec![
                Value::Integer(number * 3),
                Value::Text(format!("kind-{}", number % 7)),
                Value::Text(format!("event number {number}")),
            ]
        };
        assert_eq!(database.count(events), Ok(1500));
        let rows = database.rows(events, 0, usize::MAX).expect("rows");
        assert_eq!(rows.len(), 1500);
        assert_eq!(rows.first(), Some(&event(1)));
        assert_eq!(rows.last(), Some(&event(1500)));
        assert!(
            rows.iter()
                .zip(1..)
                .all(|(row, number)| *row == event(number))
        );

        assert_eq!(
            database.rows(events, 700, 3),
            Ok(vec![event(701), event(702), event(703)])
        );
        assert_eq!(
            database.rows(events, 1498, 10),
            Ok(vec![event(1499), event(1500)])
        );
        assert_eq!(database.rows(events, 1500, 10), Ok(Vec::new()));
        assert_eq!(database.rows(events, 5, 0), Ok(Vec::new()));
    }

    #[test]
    fn a_row_too_long_for_its_page_is_read_through_its_overflow_pages() {
        let rows = all_rows(&sample(), "documents");
        // Rows of 475 to 480 bytes straddle the 477 that fit in a page of 512.
        let lengths = [463, 464, 465, 466, 467, 468, 534, 535, 536, 1000, 3000];
        assert_eq!(rows.len(), lengths.len() + 1);
        for (row, length) in rows.iter().zip(lengths) {
            let expected = [
                Value::Text(format!("len-{length}")),
                Value::Text(letters(length)),
                Value::Null,
            ];
            assert_eq!(row[..], expected, "length {length}");
        }
        let unicode = [
            text("unicode"),
            Value::Text("é😀 ".repeat(400)),
            Value::Blob(2000),
        ];
        assert_eq!(rows[lengths.len()][..], unicode);
    }

    #[test]
    fn a_column_standing_for_the_rowid_shows_it() {
        let database = sample();
        let ids = |name: &str| -> Vec<Value> {
            let rows = all_rows(&database, name);
            rows.into_iter().map(|mut row| row.swap_remove(0)).collect()
        };
        assert_eq!(
            ids("people"),
            [Value::Integer(1), Value::Integer(2), Value::Integer(40)]
        );
        // Named as the key at the end of the table rather than on the column.
        assert_eq!(ids("tags"), [Value::Integer(5), Value::Integer(9)]);
    }

    #[test]
    fn a_whole_number_in_a_column_of_reals_is_a_real_and_a_computed_column_is_left_out() {
        let database = sample();
        assert_eq!(table(&database, "prices").columns, ["item", "net", "label"]);
        let tea = vec![text("tea"), Value::Real(3.0), text("hot")];
        assert_eq!(all_rows(&database, "prices"), [tea]);
    }

    #[test]
    fn a_row_older_than_a_column_is_null_in_it() {
        let rows = all_rows(&sample(), "notes");
        let before = vec![text("written before the column came"), Value::Null];
        let after = vec![text("written after"), Value::Integer(1)];
        assert_eq!(rows, [before, after]);
    }

    #[test]
    fn a_table_without_rows_is_empty() {
        let database = sample();
        assert_eq!(database.count(table(&database, "empty")), Ok(0));
        assert!(all_rows(&database, "empty").is_empty());
    }

    #[test]
    fn a_without_rowid_table_is_listed_but_its_rows_are_refused() {
        let database = sample();
        let settings = table(&database, "settings");
        let refusal = "settings is a WITHOUT ROWID table: its rows cannot be read here.";
        assert_eq!(database.rows(settings, 0, 10), Err(refusal.to_owned()));
        assert_eq!(database.count(settings), Err(refusal.to_owned()));
    }

    #[test]
    fn a_database_in_utf16_is_refused_saying_so() {
        let refusal = refusal(UTF16);
        assert!(
            refusal.contains("UTF-16") && refusal.contains("UTF-8"),
            "{refusal}"
        );
    }

    #[test]
    fn a_database_with_a_write_ahead_log_says_so() {
        // The file was copied while a row was still in its log: only the older one is seen.
        let database = Database::open(WAL.to_vec()).expect("it opens");
        assert!(database.uses_wal());
        assert_eq!(all_rows(&database, "early"), [vec![text("checkpointed")]]);
    }

    #[test]
    fn what_is_not_a_database_is_refused() {
        assert!(refusal(b"").contains("empty"));
        assert!(refusal(b"PK\x03\x04 not a database at all").contains("not a SQLite database"));
        assert!(refusal(&[0xA7; 4096]).contains("encrypted"));
        assert!(refusal(&SAMPLE[..50]).contains("not a SQLite database"));

        let mut odd_pages = SAMPLE.to_vec();
        odd_pages[16..18].copy_from_slice(&[0x03, 0x00]);
        assert!(refusal(&odd_pages).contains("damaged"));
        let mut newer = SAMPLE.to_vec();
        newer[19] = 3;
        assert!(refusal(&newer).contains("newer format"));
    }

    #[test]
    fn a_file_cut_short_gives_an_error_wherever_the_cut() {
        for length in (1..SAMPLE.len()).step_by(997) {
            let read = read_everything(SAMPLE[..length].to_vec());
            assert!(read.is_err(), "cut at {length}: {read:?}");
        }
        assert!(refusal(&SAMPLE[..300]).contains("damaged"));
    }

    #[test]
    fn a_table_whose_pages_lead_back_to_themselves_is_refused() {
        let database = sample();
        let root = table(&database, "events").root_page as usize;
        let page = (root - 1) * 512;
        assert_eq!(SAMPLE[page], INTERIOR);
        // Its last child becomes itself.
        let mut looping = SAMPLE.to_vec();
        looping[page + 8..page + 12].copy_from_slice(&(root as u32).to_be_bytes());
        let database = Database::open(looping).expect("the schema is whole");
        let events = table(&database, "events");
        assert!(
            database
                .count(events)
                .is_err_and(|error| error.contains("back to themselves"))
        );
        assert!(database.rows(events, 0, usize::MAX).is_err());
        // The rows before the loop are still there to show.
        assert_eq!(database.rows(events, 0, 5).map(|rows| rows.len()), Ok(5));
    }

    #[test]
    fn a_damaged_file_never_panics_nor_hangs() {
        assert_eq!(
            read_everything(SAMPLE.to_vec()),
            Ok(1500 + 3 + 17 + 12 + 1 + 2 + 2)
        );
        // Always the same damage: a made-up sequence, not chance.
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = |below: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % below as u64) as usize
        };
        let pages = SAMPLE.len() / 512;
        for round in 0..3000 {
            let mut damaged = SAMPLE.to_vec();
            for _ in 0..=next(6) {
                // Half of the rounds aim at the start of pages, where their structure is.
                let at = if round % 2 == 0 {
                    next(SAMPLE.len())
                } else {
                    (next(pages) * 512 + next(40) + if next(8) == 0 { 100 } else { 0 })
                        .min(SAMPLE.len() - 1)
                };
                damaged[at] = next(256) as u8;
            }
            let _ = read_everything(damaged);
        }
    }

    #[test]
    fn values_are_written_for_a_grid() {
        let shown = |value: Value| value.to_string();
        assert_eq!(shown(Value::Null), "NULL");
        assert_eq!(shown(Value::Integer(-42)), "-42");
        assert_eq!(shown(Value::Real(2.0)), "2.0");
        assert_eq!(shown(Value::Real(-0.25)), "-0.25");
        assert_eq!(shown(Value::Real(1e300)), "1e300");
        assert_eq!(shown(text("héllo")), "héllo");
        assert_eq!(shown(Value::Blob(1)), "BLOB (1 byte)");
        assert_eq!(shown(Value::Blob(2000)), "BLOB (2000 bytes)");
    }
}
