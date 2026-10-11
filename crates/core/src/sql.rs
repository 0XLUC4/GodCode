//! Questions asked of a SQLite file: a small part of SQL, read and answered here.
//!
//! One `SELECT` over one table, answered by reading its rows from first to last:
//!
//! ```text
//! SELECT [DISTINCT] * | value [AS name], …
//! FROM table [AS name]
//! [WHERE value] [ORDER BY value [ASC | DESC], …] [LIMIT n [OFFSET m]]
//! ```
//!
//! A value is a column, a number, a `'text'`, `NULL`, `TRUE`, `FALSE`, or values joined by
//! `+ - * / %`, `||`, `= == != <> < <= > >=`, `AND`, `OR`, `NOT`, `IS [NOT]`, `[NOT] LIKE`,
//! `[NOT] IN (…)`, `[NOT] BETWEEN … AND …`, brackets, and the functions `length`, `lower`,
//! `upper`, `abs`, `coalesce`, `ifnull`, `typeof` and `substr`. `count`, `sum`, `avg`,
//! `min`, `max` and `total` answer for all the rows that pass `WHERE` at once. Anything
//! else (`JOIN`, `GROUP BY`, a `SELECT` inside another, whatever writes…) is refused with
//! an error saying what was not understood.
//!
//! Where this is not SQLite:
//! - Values are compared as they are stored, whatever the type of their column: a number
//!   is never equal to a text and comes before every text, so `age = '36'` is false where
//!   SQLite would turn `'36'` into a number first. Arithmetic does read the number a text
//!   starts with: `age = '36' + 0` is the way to say it.
//! - Text is compared byte by byte, whatever the collation its column was declared with.
//! - Of a blob only the length is known. It can be shown, counted, tested with `IS NULL`,
//!   `typeof` and `length`, and it sorts after every text, but blobs compare with each
//!   other by length, `sum`, `avg` and `total` leave them out, and `||`, `LIKE`,
//!   arithmetic, a test of truth and the functions on text give `NULL` for one.
//! - A real turned into text (`||`, `LIKE`, `length`…) is written as the grid shows it:
//!   `1e300`, not `1.0e+300`.
//! - A name between double quotes is always a column or a table, never a text.
//! - Rows have no `rowid` unless a column stands for it, and with no `ORDER BY` they come
//!   in the order of the table, where SQLite may follow an index.
//! - A column cannot stand beside an aggregate, and `abs` of the lowest whole number is a
//!   real rather than an error.
//!
//! `ORDER BY` keeps every row that passes `WHERE` in memory until all are sorted. Without
//! it, reading stops as soon as the rows to keep are found.

use std::cmp::Ordering;
use std::collections::HashSet;

use crate::sqlite::{Database, Table, Value};

/// How many rows are read from the file at a time, at most and at least: each reading
/// finds its place again from the top of the table, so many small ones would be slow.
const PAGE: usize = 1000;
const FEW: usize = 100;
/// How deep brackets, `NOT` and `-` may nest: reading them takes room each time.
const DEEPEST: usize = 40;
/// How many operators a question may join values with: each makes its tree one deeper.
const MOST_OPERATORS: usize = 250;

/// Words that cannot name a table or a column unless between quotes.
const RESERVED: [&str; 41] = [
    "ALL",
    "AND",
    "AS",
    "ASC",
    "BETWEEN",
    "BY",
    "CASE",
    "CAST",
    "COLLATE",
    "CROSS",
    "DESC",
    "DISTINCT",
    "ELSE",
    "ESCAPE",
    "EXCEPT",
    "EXISTS",
    "FROM",
    "GROUP",
    "HAVING",
    "IN",
    "INNER",
    "INTERSECT",
    "IS",
    "JOIN",
    "LEFT",
    "LIKE",
    "LIMIT",
    "NATURAL",
    "NOT",
    "NULL",
    "OFFSET",
    "ON",
    "OR",
    "ORDER",
    "OUTER",
    "SELECT",
    "THEN",
    "UNION",
    "USING",
    "WHEN",
    "WHERE",
];

/// Words a statement that changes something starts with.
const WRITERS: [&str; 18] = [
    "INSERT",
    "UPDATE",
    "DELETE",
    "REPLACE",
    "CREATE",
    "DROP",
    "ALTER",
    "PRAGMA",
    "VACUUM",
    "ATTACH",
    "DETACH",
    "REINDEX",
    "ANALYZE",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "SAVEPOINT",
    "RELEASE",
];

/// The signs a question is written with, the longest first.
const SIGNS: [&str; 19] = [
    "||", "==", "!=", "<>", "<=", ">=", "(", ")", ",", ".", "*", "+", "-", "/", "%", "=", "<", ">",
    ";",
];

/// What a question answered: the names of its columns and its rows.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// More rows fitted the question than were kept.
    pub cut: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Word(String),
    /// A name between `"`, `` ` `` or `[]`, without them.
    Name(String),
    /// A text between `'`, without them.
    Text(String),
    /// A number as written.
    Number(String),
    Sign(&'static str),
}

/// A token and where it stands in the question.
struct Piece {
    token: Token,
    start: usize,
    end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Operator {
    Or,
    And,
    Equal,
    Unequal,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
    Is,
    Like,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Concat,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Function {
    Length,
    Lower,
    Upper,
    Abs,
    Coalesce,
    Typeof,
    Substr,
}

/// The functions that answer for one row: their name and how few and how many values
/// they take.
const FUNCTIONS: [(&str, Function, usize, usize); 9] = [
    ("length", Function::Length, 1, 1),
    ("lower", Function::Lower, 1, 1),
    ("upper", Function::Upper, 1, 1),
    ("abs", Function::Abs, 1, 1),
    ("coalesce", Function::Coalesce, 2, usize::MAX),
    ("ifnull", Function::Coalesce, 2, 2),
    ("typeof", Function::Typeof, 1, 1),
    ("substr", Function::Substr, 2, 3),
    ("substring", Function::Substr, 2, 3),
];

/// What an aggregate works out of all the rows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Total,
}

const AGGREGATES: [(&str, Kind); 6] = [
    ("count", Kind::Count),
    ("sum", Kind::Sum),
    ("avg", Kind::Avg),
    ("min", Kind::Min),
    ("max", Kind::Max),
    ("total", Kind::Total),
];

/// A value worked out of all the rows: `of` is what is taken from each, or nothing for
/// `count(*)`.
#[derive(Clone, Debug)]
struct Aggregate {
    kind: Kind,
    of: Option<Expr>,
}

/// A value still to be worked out for a row.
#[derive(Clone, Debug)]
enum Expr {
    Value(Value),
    /// The column of this place in the table.
    Column(usize),
    /// The aggregate of this place in the question.
    Aggregate(usize),
    Negate(Box<Expr>),
    Not(Box<Expr>),
    Binary(Operator, Box<Expr>, Box<Expr>),
    In(Box<Expr>, Vec<Expr>),
    /// A value, the least and the most it may be.
    Between(Box<[Expr; 3]>),
    Call(Function, Vec<Expr>),
}

/// A question once read.
struct Question<'a> {
    /// No table: the question is answered once, as for a table of one row.
    table: Option<&'a Table>,
    names: Vec<String>,
    items: Vec<Expr>,
    /// When there are some, the answer is a single row.
    aggregates: Vec<Aggregate>,
    distinct: bool,
    filter: Option<Expr>,
    /// What to sort by, and whether from last to first.
    order: Vec<(Expr, bool)>,
    limit: Option<usize>,
    offset: usize,
}

/// A whole number or a real: what arithmetic works with.
#[derive(Clone, Copy)]
enum Number {
    Whole(i64),
    Real(f64),
}

impl Number {
    fn real(self) -> f64 {
        match self {
            Self::Whole(whole) => whole as f64,
            Self::Real(real) => real,
        }
    }

    /// Without what follows the point.
    fn whole(self) -> i64 {
        match self {
            Self::Whole(whole) => whole,
            Self::Real(real) => real as i64,
        }
    }
}

/// What stands before the quote `close`, a doubled one being part of it, and how many
/// bytes that took, the closing quote among them. Nothing when it is never closed.
fn quoted(rest: &str, close: char) -> Option<(String, usize)> {
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
            return Some((inside, at + ch.len_utf8()));
        }
    }
    None
}

/// How many bytes the number `text` starts with takes: digits, a point, an exponent.
fn number_length(text: &str) -> usize {
    let digits = |from: usize| {
        let rest = text.bytes().skip(from);
        rest.take_while(u8::is_ascii_digit).count()
    };
    let byte = |at: usize| text.as_bytes().get(at).copied();
    let mut length = digits(0);
    if byte(length) == Some(b'.') {
        length += 1 + digits(length + 1);
    }
    if matches!(byte(length), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(byte(length + 1), Some(b'+' | b'-')));
        let exponent = digits(length + 1 + sign);
        if exponent > 0 {
            length += 1 + sign + exponent;
        }
    }
    length
}

/// The number `written` spells: a whole one when it fits and has no point nor exponent.
fn spelled(written: &str) -> Option<Number> {
    let whole = written.parse().ok().map(Number::Whole);
    whole.or_else(|| written.parse().ok().map(Number::Real))
}

/// The tokens of a question, comments left out.
fn pieces(query: &str) -> Result<Vec<Piece>, String> {
    let in_word = |ch: char| ch.is_alphanumeric() || matches!(ch, '_' | '$') || !ch.is_ascii();
    let mut pieces = Vec::new();
    let mut at = 0;
    while let Some(rest) = query.get(at..).filter(|rest| !rest.is_empty()) {
        let Some(first) = rest.chars().next() else {
            break;
        };
        let after = &rest[first.len_utf8()..];
        let start = at;
        let close = match first {
            '"' | '\'' | '`' => Some(first),
            '[' => Some(']'),
            _ => None,
        };
        let token = if first.is_whitespace() {
            at += first.len_utf8();
            continue;
        } else if rest.starts_with("--") {
            at += rest.find('\n').unwrap_or(rest.len());
            continue;
        } else if let Some(comment) = rest.strip_prefix("/*") {
            at += comment.find("*/").map_or(rest.len(), |end| end + 4);
            continue;
        } else if let Some(close) = close {
            let Some((inside, length)) = quoted(after, close) else {
                return Err(format!("the {first} opened here is never closed: {rest}"));
            };
            at += first.len_utf8() + length;
            if first == '\'' {
                Token::Text(inside)
            } else {
                Token::Name(inside)
            }
        } else if first.is_ascii_digit()
            || (first == '.' && after.starts_with(|ch: char| ch.is_ascii_digit()))
        {
            at += number_length(rest).max(1);
            Token::Number(query[start..at].to_owned())
        } else if in_word(first) {
            at += rest.find(|ch| !in_word(ch)).unwrap_or(rest.len());
            Token::Word(query[start..at].to_owned())
        } else if let Some(sign) = SIGNS.iter().find(|sign| rest.starts_with(**sign)) {
            at += sign.len();
            Token::Sign(sign)
        } else {
            return Err(format!("near \"{first}\": this sign is not understood"));
        };
        pieces.push(Piece {
            token,
            start,
            end: at,
        });
    }
    Ok(pieces)
}

/// Why `word` is refused, when it belongs to a part of SQL that is not understood here.
fn refusal(word: &str) -> Option<String> {
    let word = word.to_ascii_uppercase();
    let why = match word.as_str() {
        "JOIN" | "INNER" | "LEFT" | "RIGHT" | "FULL" | "CROSS" | "NATURAL" | "OUTER" => {
            return Some("JOIN is not understood here: one table at a time".to_owned());
        }
        "SELECT" => {
            let refusal = "a SELECT inside another is not understood here: one SELECT at a time";
            return Some(refusal.to_owned());
        }
        "GROUP" => {
            let refusal =
                "GROUP BY is not understood here: count, sum and the like answer for all the rows";
            return Some(refusal.to_owned());
        }
        "HAVING" => "there is no GROUP BY for it",
        "UNION" | "INTERSECT" | "EXCEPT" => "one SELECT at a time",
        "OVER" | "WINDOW" | "FILTER" => "window functions are not",
        "WITH" | "VALUES" | "EXPLAIN" => "a question starts with SELECT",
        "CASE" | "CAST" | "EXISTS" | "COLLATE" | "GLOB" | "REGEXP" | "MATCH" | "ESCAPE" => {
            return Some(format!("{word} is not understood here"));
        }
        _ => return None,
    };
    Some(format!("{word} is not understood here: {why}"))
}

/// Names one after the other, as an error lists them.
fn listed<'a>(names: impl Iterator<Item = &'a String>) -> String {
    let names: Vec<&str> = names.map(String::as_str).collect();
    if names.is_empty() {
        "there is none".to_owned()
    } else {
        format!("there are {}", names.join(", "))
    }
}

/// A question being read.
struct Parser<'a> {
    query: &'a str,
    /// The tables of the database.
    tables: &'a [Table],
    pieces: Vec<Piece>,
    /// The piece to read next.
    at: usize,
    table: Option<&'a Table>,
    /// The other name `FROM` gives the table.
    alias: Option<String>,
    aggregates: Vec<Aggregate>,
    /// Whether what is being read is the list after `SELECT`, where aggregates may stand.
    in_list: bool,
    in_aggregate: bool,
    /// The first column of the list after `SELECT` that stands outside an aggregate.
    loose: Option<String>,
    /// What is wrong with the first use of a column while no table is named.
    missing: Option<String>,
    depth: usize,
    operators: usize,
}

impl<'a> Parser<'a> {
    fn token(&self) -> Option<&Token> {
        self.pieces.get(self.at).map(|piece| &piece.token)
    }

    fn is_word(&self, word: &str) -> bool {
        matches!(self.token(), Some(Token::Word(found)) if found.eq_ignore_ascii_case(word))
    }

    fn is_sign(&self, sign: &str) -> bool {
        matches!(self.token(), Some(Token::Sign(found)) if *found == sign)
    }

    fn eat_word(&mut self, word: &str) -> bool {
        let found = self.is_word(word);
        self.at += usize::from(found);
        found
    }

    fn eat_sign(&mut self, sign: &str) -> bool {
        let found = self.is_sign(sign);
        self.at += usize::from(found);
        found
    }

    /// What to say when the next piece is not `wanted`.
    fn unexpected(&self, wanted: &str) -> String {
        let Some(piece) = self.pieces.get(self.at) else {
            return format!("the question stops too soon: {wanted} was expected");
        };
        let refused = match &piece.token {
            Token::Word(word) => refusal(word),
            _ => None,
        };
        let written = &self.query[piece.start..piece.end];
        refused.unwrap_or_else(|| format!("near \"{written}\": {wanted} was expected"))
    }

    fn expect_sign(&mut self, sign: &str) -> Result<(), String> {
        if self.eat_sign(sign) {
            Ok(())
        } else {
            Err(self.unexpected(sign))
        }
    }

    /// The name written next, of a table, a column or a result: `what` says which.
    fn name(&mut self, what: &str) -> Result<String, String> {
        let name = match self.token() {
            Some(Token::Name(name)) => name.clone(),
            Some(Token::Word(word)) if !RESERVED.iter().any(|kept| is_same(kept, word)) => {
                word.clone()
            }
            _ => return Err(self.unexpected(what)),
        };
        self.at += 1;
        Ok(name)
    }

    /// One level further into brackets, `NOT` or `-`, refused past the deepest allowed.
    fn deeper(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > DEEPEST {
            return Err(format!(
                "the question nests too deep: no more than {DEEPEST} brackets, NOT or - \
                 inside one another"
            ));
        }
        Ok(())
    }

    /// The whole question, from `SELECT` to its end.
    fn question(mut self) -> Result<Question<'a>, String> {
        let from = self.from()?;
        let after_table = self.at;

        self.at = 1;
        let distinct = self.eat_word("DISTINCT");
        if !distinct {
            self.eat_word("ALL");
        }
        let mut names = Vec::new();
        let mut items = Vec::new();
        self.in_list = true;
        loop {
            if self.eat_sign("*") {
                self.loose.get_or_insert_with(|| "*".to_owned());
                match self.table {
                    Some(table) => {
                        names.extend(table.columns.iter().cloned());
                        items.extend((0..table.columns.len()).map(Expr::Column));
                    }
                    None => {
                        let wrong = "* stands for the columns of a table: FROM is missing";
                        self.missing.get_or_insert_with(|| wrong.to_owned());
                    }
                }
            } else {
                let start = self.at;
                let item = self.expression()?;
                let written = match (self.pieces.get(start), self.pieces.get(self.at - 1)) {
                    (Some(first), Some(last)) => &self.query[first.start..last.end],
                    _ => "",
                };
                names.push(if self.eat_word("AS") {
                    match self.token() {
                        Some(Token::Text(name)) => {
                            let name = name.clone();
                            self.at += 1;
                            name
                        }
                        _ => self.name("a name after AS")?,
                    }
                } else {
                    match (&item, self.table) {
                        (Expr::Column(place), Some(table)) => table.columns[*place].clone(),
                        _ => written.to_owned(),
                    }
                });
                items.push(item);
            }
            if !self.eat_sign(",") {
                break;
            }
        }
        self.in_list = false;
        if let (Some(loose), false) = (&self.loose, self.aggregates.is_empty()) {
            return Err(format!(
                "{loose} cannot stand beside count, sum and the like: they answer for all \
                 the rows at once, and GROUP BY is not understood here"
            ));
        }

        match from {
            Some(from) if self.at == from => self.at = after_table,
            Some(_) => return Err(self.unexpected("a comma or FROM")),
            None => {
                let goes_on = ["WHERE", "ORDER", "LIMIT"];
                let ends = self.token().is_none() || self.is_sign(";");
                if !ends && !goes_on.iter().any(|word| self.is_word(word)) {
                    return Err(self.unexpected("FROM"));
                }
            }
        }
        if self.table.is_some() && self.is_sign(",") {
            return Err("several tables are not understood here: one table at a time".to_owned());
        }

        let filter = if self.eat_word("WHERE") {
            Some(self.expression()?)
        } else {
            None
        };
        let mut order = Vec::new();
        if self.eat_word("ORDER") {
            if !self.eat_word("BY") {
                return Err(self.unexpected("BY"));
            }
            loop {
                let by = self.order_term(&names, &items)?;
                let descending = self.eat_word("DESC");
                if !descending {
                    self.eat_word("ASC");
                }
                order.push((by, descending));
                if !self.eat_sign(",") {
                    break;
                }
            }
        }
        let (mut limit, mut offset) = (None, 0);
        if self.eat_word("LIMIT") {
            limit = self.count("LIMIT")?;
            if self.eat_word("OFFSET") {
                offset = self.count("OFFSET")?.unwrap_or(0);
            } else if self.eat_sign(",") {
                // The other writing: `LIMIT skipped, kept`.
                offset = limit.unwrap_or(0);
                limit = self.count("LIMIT")?;
            }
        }
        if self.eat_sign(";") && self.token().is_some() {
            return Err("one question at a time: there is more after the ;".to_owned());
        }
        if self.token().is_some() {
            return Err(self.unexpected("the end of the question"));
        }
        if let Some(missing) = self.missing {
            return Err(missing);
        }
        Ok(Question {
            table: self.table,
            names,
            items,
            aggregates: self.aggregates,
            distinct,
            filter,
            order,
            limit,
            offset,
        })
    }

    /// Read the table named after the `FROM` of the question, ahead of the list of what to
    /// show, since that list names its columns. Gives where `FROM` stands and leaves `at`
    /// after the table, or nothing when there is no `FROM`.
    fn from(&mut self) -> Result<Option<usize>, String> {
        let mut depth = 0_usize;
        let mut from = None;
        for (index, piece) in self.pieces.iter().enumerate().skip(1) {
            match &piece.token {
                Token::Sign("(") => depth += 1,
                Token::Sign(")") => depth = depth.saturating_sub(1),
                Token::Word(word) if depth == 0 && is_same(word, "FROM") => {
                    from = Some(index);
                    break;
                }
                _ => {}
            }
        }
        let Some(from) = from else {
            return Ok(None);
        };
        self.at = from + 1;
        if self.is_sign("(") {
            let refusal =
                "a SELECT inside another is not understood here: FROM takes the name of a table";
            return Err(refusal.to_owned());
        }
        let name = self.name("the name of a table")?;
        let table = self.tables.iter().find(|table| is_same(&table.name, &name));
        let Some(table) = table else {
            let tables = self.tables.iter().map(|table| &table.name);
            return Err(format!("there is no table {name}: {}", listed(tables)));
        };
        self.table = Some(table);
        if self.eat_word("AS") {
            self.alias = Some(self.name("a name after AS")?);
        }
        Ok(Some(from))
    }

    /// One of the values `ORDER BY` sorts by: the number or the name of a shown column
    /// stands for it.
    fn order_term(&mut self, names: &[String], items: &[Expr]) -> Result<Expr, String> {
        let ends = match self.pieces.get(self.at + 1).map(|piece| &piece.token) {
            None | Some(Token::Sign("," | ";")) => true,
            Some(Token::Word(word)) => ["ASC", "DESC", "LIMIT"]
                .iter()
                .any(|next| is_same(word, next)),
            Some(_) => false,
        };
        let shown = match self.token() {
            _ if !ends => None,
            Some(Token::Number(digits)) if digits.bytes().all(|digit| digit.is_ascii_digit()) => {
                let place = digits.parse::<usize>().ok();
                let item = place.and_then(|place| items.get(place.checked_sub(1)?));
                Some(item.ok_or_else(|| {
                    format!(
                        "ORDER BY {digits}: the columns shown are numbered from 1 to {}",
                        items.len()
                    )
                })?)
            }
            Some(Token::Word(name) | Token::Name(name)) => {
                let place = names.iter().position(|shown| is_same(shown, name));
                place.and_then(|place| items.get(place))
            }
            _ => None,
        };
        match shown {
            Some(item) => {
                self.at += 1;
                Ok(item.clone())
            }
            None => self.expression(),
        }
    }

    /// The whole number after `LIMIT` or `OFFSET`; a negative one counts for none.
    fn count(&mut self, clause: &str) -> Result<Option<usize>, String> {
        let negative = self.eat_sign("-");
        let count = match self.token() {
            Some(Token::Number(digits)) if digits.bytes().all(|digit| digit.is_ascii_digit()) => {
                digits.parse().unwrap_or(usize::MAX)
            }
            _ => return Err(self.unexpected(&format!("a whole number after {clause}"))),
        };
        self.at += 1;
        Ok((!negative).then_some(count))
    }

    fn expression(&mut self) -> Result<Expr, String> {
        self.binary(1)
    }

    /// The operator written next and how tightly it binds, when it binds at least as
    /// tightly as `floor`. `NOT` before `LIKE`, `IN` or `BETWEEN` is told by the flag.
    fn operator(&self, floor: u8) -> Option<(&'static str, u8, bool)> {
        let word_at = |at: usize| match self.pieces.get(at).map(|piece| &piece.token) {
            Some(Token::Word(word)) => Some(word.to_ascii_uppercase()),
            _ => None,
        };
        let named = |word: &str| {
            Some(match word {
                "OR" => ("OR", 1),
                "AND" => ("AND", 2),
                "IS" => ("IS", 4),
                "LIKE" => ("LIKE", 4),
                "IN" => ("IN", 4),
                "BETWEEN" => ("BETWEEN", 4),
                _ => return None,
            })
        };
        let (found, negated) = match self.token()? {
            Token::Sign(sign) => {
                let level = match *sign {
                    "=" | "==" | "!=" | "<>" => 4,
                    "<" | "<=" | ">" | ">=" => 5,
                    "+" | "-" => 7,
                    "*" | "/" | "%" => 8,
                    "||" => 9,
                    _ => return None,
                };
                ((*sign, level), false)
            }
            Token::Word(word) if is_same(word, "NOT") => {
                let after = named(&word_at(self.at + 1)?)?;
                if after.1 != 4 || after.0 == "IS" {
                    return None;
                }
                (after, true)
            }
            Token::Word(word) => (named(&word.to_ascii_uppercase())?, false),
            _ => return None,
        };
        (found.1 >= floor).then_some((found.0, found.1, negated))
    }

    /// Values joined by operators that bind at least as tightly as `floor`.
    fn binary(&mut self, floor: u8) -> Result<Expr, String> {
        let mut left = if self.eat_word("NOT") {
            // `NOT` binds looser than a comparison: `NOT a = b` denies the comparison.
            self.deeper()?;
            let denied = self.binary(floor.max(3))?;
            self.depth -= 1;
            Expr::Not(Box::new(denied))
        } else {
            self.unary()?
        };
        while let Some((written, level, negated)) = self.operator(floor) {
            self.operators += 1;
            if self.operators > MOST_OPERATORS {
                return Err(format!(
                    "the question is too long: no more than {MOST_OPERATORS} operators"
                ));
            }
            self.at += 1 + usize::from(negated);
            let subject = Box::new(left);
            let found = match written {
                "IN" => {
                    self.expect_sign("(")?;
                    let mut among = Vec::new();
                    while !self.is_sign(")") {
                        among.push(self.nested()?);
                        if !self.eat_sign(",") {
                            break;
                        }
                    }
                    self.expect_sign(")")?;
                    Expr::In(subject, among)
                }
                "BETWEEN" => {
                    let least = self.binary(5)?;
                    if !self.eat_word("AND") {
                        return Err(self.unexpected("AND"));
                    }
                    let most = self.binary(5)?;
                    Expr::Between(Box::new([*subject, least, most]))
                }
                "IS" => {
                    let denied = self.eat_word("NOT");
                    let same = Expr::Binary(Operator::Is, subject, Box::new(self.binary(5)?));
                    if denied {
                        Expr::Not(Box::new(same))
                    } else {
                        same
                    }
                }
                _ => {
                    let operator = match written {
                        "OR" => Operator::Or,
                        "AND" => Operator::And,
                        "=" | "==" => Operator::Equal,
                        "!=" | "<>" => Operator::Unequal,
                        "<" => Operator::Less,
                        "<=" => Operator::LessOrEqual,
                        ">" => Operator::Greater,
                        ">=" => Operator::GreaterOrEqual,
                        "LIKE" => Operator::Like,
                        "+" => Operator::Add,
                        "-" => Operator::Subtract,
                        "*" => Operator::Multiply,
                        "/" => Operator::Divide,
                        "%" => Operator::Remainder,
                        _ => Operator::Concat,
                    };
                    // What follows binds tighter: operators of one level go left to right.
                    let right = self.binary(level + 1)?;
                    Expr::Binary(operator, subject, Box::new(right))
                }
            };
            left = if negated {
                Expr::Not(Box::new(found))
            } else {
                found
            };
        }
        Ok(left)
    }

    /// A whole value one level deeper, as between brackets.
    fn nested(&mut self) -> Result<Expr, String> {
        self.deeper()?;
        let inside = self.expression()?;
        self.depth -= 1;
        Ok(inside)
    }

    /// A value with the `-` or `+` before it.
    fn unary(&mut self) -> Result<Expr, String> {
        let negative = self.eat_sign("-");
        if !negative && !self.eat_sign("+") {
            return self.primary();
        }
        if let (true, Some(Token::Number(digits))) = (negative, self.token()) {
            // Read with its sign, so that the lowest whole number can be written.
            let number = spelled(&format!("-{digits}"));
            let number = number.ok_or_else(|| self.unexpected("a number"))?;
            self.at += 1;
            return Ok(Expr::Value(match number {
                Number::Whole(whole) => Value::Integer(whole),
                Number::Real(real) => Value::Real(real),
            }));
        }
        self.deeper()?;
        let inside = self.unary()?;
        self.depth -= 1;
        Ok(if negative {
            Expr::Negate(Box::new(inside))
        } else {
            inside
        })
    }

    /// A value that no operator splits: a literal, a column, a call, or brackets.
    fn primary(&mut self) -> Result<Expr, String> {
        let wanted = "a value";
        let Some(token) = self.token().cloned() else {
            return Err(self.unexpected(wanted));
        };
        match token {
            Token::Number(digits) => {
                let number = spelled(&digits).ok_or_else(|| self.unexpected("a number"))?;
                self.at += 1;
                Ok(Expr::Value(match number {
                    Number::Whole(whole) => Value::Integer(whole),
                    Number::Real(real) => Value::Real(real),
                }))
            }
            Token::Text(text) => {
                self.at += 1;
                Ok(Expr::Value(Value::Text(text)))
            }
            Token::Sign("(") => {
                self.at += 1;
                let inside = self.nested()?;
                self.expect_sign(")")?;
                Ok(inside)
            }
            Token::Sign(_) => Err(self.unexpected(wanted)),
            Token::Word(word) => {
                let constant = match word.to_ascii_uppercase().as_str() {
                    "NULL" => Some(Value::Null),
                    "TRUE" => Some(Value::Integer(1)),
                    "FALSE" => Some(Value::Integer(0)),
                    _ => None,
                };
                let called = matches!(
                    self.pieces.get(self.at + 1).map(|piece| &piece.token),
                    Some(Token::Sign("("))
                );
                if let Some(constant) = constant {
                    self.at += 1;
                    Ok(Expr::Value(constant))
                } else if RESERVED.iter().any(|kept| is_same(kept, &word)) {
                    Err(self.unexpected(wanted))
                } else if called {
                    self.at += 2;
                    self.call(&word)
                } else {
                    self.column()
                }
            }
            Token::Name(_) => self.column(),
        }
    }

    /// A column, written alone or after the name of the table and a point.
    fn column(&mut self) -> Result<Expr, String> {
        let mut name = self.name("a value")?;
        if self.eat_sign(".") {
            // Once `FROM` gives the table another name, only that one stands for it.
            let table = self.table.map(|table| table.name.as_str());
            let known = self.alias.as_deref().or(table);
            if let Some(known) = known.filter(|known| !is_same(known, &name)) {
                return Err(format!(
                    "{name} is not the table named after FROM, {known}: one table at a time"
                ));
            }
            name = self.name("the name of a column")?;
        }
        let Some(table) = self.table else {
            let wrong = format!("there is no column {name}: no table is named, FROM is missing");
            self.missing.get_or_insert(wrong);
            return Ok(Expr::Value(Value::Null));
        };
        let place = table
            .columns
            .iter()
            .position(|column| is_same(column, &name));
        let Some(place) = place else {
            return Err(format!(
                "there is no column {name} in {}: {}",
                table.name,
                listed(table.columns.iter())
            ));
        };
        if self.in_list && !self.in_aggregate {
            self.loose.get_or_insert(name);
        }
        Ok(Expr::Column(place))
    }

    /// The call of the function `name`, whose opening bracket is read already.
    fn call(&mut self, name: &str) -> Result<Expr, String> {
        if let Some((written, kind)) = AGGREGATES.iter().find(|(known, _)| is_same(known, name)) {
            return self.aggregate(written, *kind);
        }
        let mut given = Vec::new();
        while !self.is_sign(")") {
            given.push(self.nested()?);
            if !self.eat_sign(",") {
                break;
            }
        }
        self.expect_sign(")")?;
        let known = FUNCTIONS.iter().find(|(known, ..)| is_same(known, name));
        let Some((written, function, least, most)) = known else {
            let functions: Vec<&str> = FUNCTIONS.iter().map(|(known, ..)| *known).collect();
            let aggregates: Vec<&str> = AGGREGATES.iter().map(|(known, _)| *known).collect();
            return Err(format!(
                "the function {name} is not known here: there are {}, and {} over all the rows",
                functions.join(", "),
                aggregates.join(", ")
            ));
        };
        if given.len() < *least || given.len() > *most {
            let takes = match (*least, *most) {
                (1, 1) => "1 value".to_owned(),
                (least, most) if least == most => format!("{least} values"),
                (least, usize::MAX) => format!("{least} values or more"),
                (least, most) => format!("{least} to {most} values"),
            };
            return Err(format!("{written} takes {takes}, not {}", given.len()));
        }
        Ok(Expr::Call(*function, given))
    }

    /// The call of an aggregate, whose opening bracket is read already.
    fn aggregate(&mut self, written: &str, kind: Kind) -> Result<Expr, String> {
        if !self.in_list {
            return Err(format!(
                "{written} answers for all the rows at once: it can only stand in the list \
                 after SELECT"
            ));
        }
        if self.in_aggregate {
            return Err(format!("{written} cannot stand inside another aggregate"));
        }
        if self.is_word("DISTINCT") {
            return Err(format!("DISTINCT inside {written} is not understood here"));
        }
        let of = if kind == Kind::Count && self.eat_sign("*") {
            None
        } else {
            self.in_aggregate = true;
            let of = self.nested()?;
            self.in_aggregate = false;
            Some(of)
        };
        if self.is_sign(",") {
            return Err(format!(
                "{written} takes 1 value, over all the rows: several are not understood here"
            ));
        }
        self.expect_sign(")")?;
        self.aggregates.push(Aggregate { kind, of });
        Ok(Expr::Aggregate(self.aggregates.len() - 1))
    }
}

/// Whether two names or keywords are the same, as SQLite tells: whatever the case of
/// their ASCII letters.
fn is_same(one: &str, other: &str) -> bool {
    one.eq_ignore_ascii_case(other)
}

/// The number a value counts for in arithmetic: the one a text starts with, 0 when it
/// starts with none, and nothing for `NULL` and for a blob.
fn number(value: &Value) -> Option<Number> {
    match value {
        Value::Null | Value::Blob(_) => None,
        Value::Integer(whole) => Some(Number::Whole(*whole)),
        Value::Real(real) => Some(Number::Real(*real)),
        Value::Text(text) => {
            let text = text.trim_start();
            let sign = usize::from(text.starts_with(['+', '-']));
            let length = sign + number_length(&text[sign..]);
            Some(spelled(&text[..length]).unwrap_or(Number::Whole(0)))
        }
    }
}

/// A value as text, the way `||`, `LIKE` and `length` read it: nothing for `NULL` and
/// for a blob.
fn text(value: &Value) -> Option<String> {
    match value {
        Value::Null | Value::Blob(_) => None,
        Value::Text(text) => Some(text.clone()),
        number => Some(number.to_string()),
    }
}

/// Whether a value counts as true, when that can be told: not for `NULL` nor for a blob.
fn truth(value: &Value) -> Option<bool> {
    number(value).map(|number| match number {
        Number::Whole(whole) => whole != 0,
        Number::Real(real) => real != 0.0,
    })
}

/// 1 for true, 0 for false, `NULL` for what cannot be told.
fn flag(known: Option<bool>) -> Value {
    known.map_or(Value::Null, |yes| Value::Integer(i64::from(yes)))
}

/// How a whole number stands against a real, without the rounding that turning the whole
/// one into a real would cost.
fn whole_against_real(whole: i64, real: f64) -> Ordering {
    // 2 to the power of 63: one past the largest whole number.
    const PAST: f64 = 9_223_372_036_854_775_808.0;
    if real >= PAST {
        return Ordering::Less;
    }
    if real < -PAST {
        return Ordering::Greater;
    }
    let truncated = real as i64;
    let rest = real - truncated as f64;
    let fraction = 0.0_f64.partial_cmp(&rest).unwrap_or(Ordering::Equal);
    whole.cmp(&truncated).then(fraction)
}

/// The order SQLite sorts in: `NULL`, then numbers by value, then text byte by byte, then
/// blobs. Of blobs only the length is known: the shorter comes first.
fn order(left: &Value, right: &Value) -> Ordering {
    let rank = |value: &Value| match value {
        Value::Null => 0,
        Value::Integer(_) | Value::Real(_) => 1,
        Value::Text(_) => 2,
        Value::Blob(_) => 3,
    };
    match (left, right) {
        (Value::Integer(left), Value::Integer(right)) => left.cmp(right),
        (Value::Real(left), Value::Real(right)) => {
            left.partial_cmp(right).unwrap_or(Ordering::Equal)
        }
        (Value::Integer(left), Value::Real(right)) => whole_against_real(*left, *right),
        (Value::Real(left), Value::Integer(right)) => whole_against_real(*right, *left).reverse(),
        (Value::Text(left), Value::Text(right)) => left.as_bytes().cmp(right.as_bytes()),
        (Value::Blob(left), Value::Blob(right)) => left.cmp(right),
        _ => rank(left).cmp(&rank(right)),
    }
}

/// How two values compare, which cannot be told when one is `NULL`.
fn compared(left: &Value, right: &Value) -> Option<Ordering> {
    let known = *left != Value::Null && *right != Value::Null;
    known.then(|| order(left, right))
}

/// Whether `text` fits `pattern`, where `%` stands for any run of characters and `_` for
/// any one, the case of ASCII letters not counting.
fn like(text: &[char], pattern: &[char]) -> bool {
    let (mut at, mut wanted) = (0, 0);
    // The last `%` met and where in the text it was: where to try again from.
    let mut retry: Option<(usize, usize)> = None;
    while let Some(ch) = text.get(at) {
        match pattern.get(wanted) {
            Some('%') => {
                retry = Some((wanted, at));
                wanted += 1;
            }
            Some(expected) if *expected == '_' || expected.eq_ignore_ascii_case(ch) => {
                at += 1;
                wanted += 1;
            }
            _ => match retry {
                Some((percent, from)) => {
                    wanted = percent + 1;
                    at = from + 1;
                    retry = Some((percent, from + 1));
                }
                None => return false,
            },
        }
    }
    pattern.iter().skip(wanted).all(|ch| *ch == '%')
}

/// What `+ - * / %` give. A whole result that does not fit becomes a real, a division by
/// zero gives `NULL`.
fn arithmetic(operator: Operator, left: &Value, right: &Value) -> Value {
    let (Some(left), Some(right)) = (number(left), number(right)) else {
        return Value::Null;
    };
    if let (Number::Whole(left), Number::Whole(right)) = (left, right) {
        let exact = match operator {
            Operator::Add => left.checked_add(right),
            Operator::Subtract => left.checked_sub(right),
            Operator::Multiply => left.checked_mul(right),
            _ if right == 0 => return Value::Null,
            Operator::Divide => left.checked_div(right),
            // Only the lowest number by -1 has no remainder to give: it is 0.
            _ => Some(left.checked_rem(right).unwrap_or(0)),
        };
        if let Some(exact) = exact {
            return Value::Integer(exact);
        }
    }
    let (real_left, real_right) = (left.real(), right.real());
    let real = match operator {
        Operator::Add => real_left + real_right,
        Operator::Subtract => real_left - real_right,
        Operator::Multiply => real_left * real_right,
        Operator::Divide if real_right == 0.0 => return Value::Null,
        Operator::Divide => real_left / real_right,
        // The remainder is that of the whole parts.
        _ => match left.whole().checked_rem(right.whole()) {
            Some(rest) => rest as f64,
            None if right.whole() == 0 => return Value::Null,
            None => 0.0,
        },
    };
    if real.is_nan() {
        Value::Null
    } else {
        Value::Real(real)
    }
}

/// The part of a text `substr` cuts: characters are counted from 1, from the end when
/// the start is negative, and a negative length takes what stands before the start.
fn substr(given: &[Value]) -> Value {
    let [subject, start, length @ ..] = given else {
        return Value::Null;
    };
    let (Some(subject), Some(start)) = (text(subject), number(start)) else {
        return Value::Null;
    };
    let mut length = match length.first() {
        None => i64::MAX,
        Some(length) => match number(length) {
            Some(length) => length.whole(),
            None => return Value::Null,
        },
    };
    let backwards = length < 0;
    if backwards {
        length = length.saturating_neg();
    }
    let count = i64::try_from(subject.chars().count()).unwrap_or(i64::MAX);
    let mut from = start.whole();
    if from < 0 {
        from = from.saturating_add(count);
        if from < 0 {
            length = length.saturating_add(from).max(0);
            from = 0;
        }
    } else if from > 0 {
        from -= 1;
    } else if length > 0 {
        length -= 1;
    }
    if backwards {
        from = from.saturating_sub(length);
        if from < 0 {
            length = length.saturating_add(from);
            from = 0;
        }
    }
    let from = usize::try_from(from).unwrap_or(0);
    let length = usize::try_from(length).unwrap_or(0);
    Value::Text(subject.chars().skip(from).take(length).collect())
}

/// What a function gives for the values handed to it.
fn call(function: Function, given: &[Value]) -> Value {
    let first = given.first().unwrap_or(&Value::Null);
    let rewritten = |rewrite: fn(&str) -> String| {
        text(first).map_or(Value::Null, |text| Value::Text(rewrite(&text)))
    };
    match function {
        Function::Length => match first {
            Value::Blob(length) => Value::Integer(i64::try_from(*length).unwrap_or(i64::MAX)),
            other => text(other).map_or(Value::Null, |text| {
                Value::Integer(i64::try_from(text.chars().count()).unwrap_or(i64::MAX))
            }),
        },
        Function::Lower => rewritten(str::to_ascii_lowercase),
        Function::Upper => rewritten(str::to_ascii_uppercase),
        // Only a whole number stays whole: a text gives a real, as does the lowest number,
        // which has no whole opposite.
        Function::Abs => match (number(first), first) {
            (None, _) => Value::Null,
            (Some(Number::Whole(whole)), Value::Integer(_)) => match whole.checked_abs() {
                Some(positive) => Value::Integer(positive),
                None => Value::Real((whole as f64).abs()),
            },
            (Some(number), _) => Value::Real(number.real().abs()),
        },
        Function::Coalesce => {
            let found = given.iter().find(|value| **value != Value::Null);
            found.cloned().unwrap_or(Value::Null)
        }
        Function::Typeof => Value::Text(
            match first {
                Value::Null => "null",
                Value::Integer(_) => "integer",
                Value::Real(_) => "real",
                Value::Text(_) => "text",
                Value::Blob(_) => "blob",
            }
            .to_owned(),
        ),
        Function::Substr => substr(given),
    }
}

/// What `expr` is worth for a row, the aggregates being worth `totals`.
fn value(expr: &Expr, row: &[Value], totals: &[Value]) -> Value {
    let of = |inner: &Expr| value(inner, row, totals);
    match expr {
        Expr::Value(value) => value.clone(),
        Expr::Column(place) => row.get(*place).cloned().unwrap_or(Value::Null),
        Expr::Aggregate(place) => totals.get(*place).cloned().unwrap_or(Value::Null),
        Expr::Negate(inner) => arithmetic(Operator::Subtract, &Value::Integer(0), &of(inner)),
        Expr::Not(inner) => flag(truth(&of(inner)).map(|yes| !yes)),
        // A side that settles the answer does so even when the other is not known.
        Expr::Binary(Operator::And, left, right) => match truth(&of(left)) {
            Some(false) => flag(Some(false)),
            left => match (left, truth(&of(right))) {
                (_, Some(false)) => flag(Some(false)),
                (Some(true), Some(true)) => flag(Some(true)),
                _ => Value::Null,
            },
        },
        Expr::Binary(Operator::Or, left, right) => match truth(&of(left)) {
            Some(true) => flag(Some(true)),
            left => match (left, truth(&of(right))) {
                (_, Some(true)) => flag(Some(true)),
                (Some(false), Some(false)) => flag(Some(false)),
                _ => Value::Null,
            },
        },
        Expr::Binary(operator, left, right) => {
            let (left, right) = (of(left), of(right));
            let compare = |fits: fn(Ordering) -> bool| flag(compared(&left, &right).map(fits));
            match operator {
                Operator::Equal => compare(Ordering::is_eq),
                Operator::Unequal => compare(Ordering::is_ne),
                Operator::Less => compare(Ordering::is_lt),
                Operator::LessOrEqual => compare(Ordering::is_le),
                Operator::Greater => compare(Ordering::is_gt),
                Operator::GreaterOrEqual => compare(Ordering::is_ge),
                Operator::Is => flag(Some(order(&left, &right).is_eq())),
                Operator::Like => match (text(&left), text(&right)) {
                    (Some(subject), Some(pattern)) => {
                        let subject: Vec<char> = subject.chars().collect();
                        let pattern: Vec<char> = pattern.chars().collect();
                        flag(Some(like(&subject, &pattern)))
                    }
                    _ => Value::Null,
                },
                Operator::Concat => match (text(&left), text(&right)) {
                    (Some(left), Some(right)) => Value::Text(left + &right),
                    _ => Value::Null,
                },
                _ => arithmetic(*operator, &left, &right),
            }
        }
        Expr::In(subject, among) => {
            let subject = of(subject);
            let mut unknown = subject == Value::Null && !among.is_empty();
            for candidate in among {
                match compared(&subject, &of(candidate)) {
                    Some(Ordering::Equal) => return flag(Some(true)),
                    Some(_) => {}
                    None => unknown = true,
                }
            }
            flag((!unknown).then_some(false))
        }
        Expr::Between(parts) => {
            let [subject, least, most] = parts.as_ref();
            let subject = of(subject);
            let above = compared(&subject, &of(least)).map(Ordering::is_ge);
            let below = compared(&subject, &of(most)).map(Ordering::is_le);
            match (above, below) {
                (Some(false), _) | (_, Some(false)) => flag(Some(false)),
                (Some(true), Some(true)) => flag(Some(true)),
                _ => Value::Null,
            }
        }
        Expr::Call(function, given) => {
            let given: Vec<Value> = given.iter().map(of).collect();
            call(*function, &given)
        }
    }
}

/// Whether a value is a whole number, or a text that spells one and nothing else.
fn is_whole(value: &Value) -> bool {
    match value {
        Value::Integer(_) => true,
        Value::Text(text) => text.trim().parse::<i64>().is_ok(),
        _ => false,
    }
}

/// What an aggregate has gathered from the rows seen so far.
#[derive(Default)]
struct Running {
    /// How many values counted: not the `NULL` ones.
    count: usize,
    /// The sum of the whole numbers, with room for more than fits in one.
    whole: i128,
    /// The sum of the reals.
    real: f64,
    any_real: bool,
    /// The least or the greatest value so far.
    best: Option<Value>,
}

impl Running {
    fn take(&mut self, kind: Kind, value: Value) {
        if value == Value::Null {
            return;
        }
        match kind {
            Kind::Count => self.count += 1,
            Kind::Min | Kind::Max => {
                let wanted = if kind == Kind::Min {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
                let best = self.best.as_ref();
                if best.is_none_or(|best| order(&value, best) == wanted) {
                    self.best = Some(value);
                }
            }
            Kind::Sum | Kind::Avg | Kind::Total => match number(&value) {
                // A text that is not a whole number from end to end counts as a real.
                Some(Number::Whole(whole)) if !is_whole(&value) => {
                    self.count += 1;
                    self.real += whole as f64;
                    self.any_real = true;
                }
                Some(Number::Whole(whole)) => {
                    self.count += 1;
                    self.whole = self.whole.saturating_add(i128::from(whole));
                }
                Some(Number::Real(real)) => {
                    self.count += 1;
                    self.real += real;
                    self.any_real = true;
                }
                // A blob: its bytes are not known, it is left out.
                None => {}
            },
        }
    }

    fn result(self, kind: Kind) -> Result<Value, String> {
        let sum = self.whole as f64 + self.real;
        Ok(match kind {
            Kind::Count => Value::Integer(i64::try_from(self.count).unwrap_or(i64::MAX)),
            Kind::Min | Kind::Max => self.best.unwrap_or(Value::Null),
            Kind::Total => Value::Real(sum),
            _ if self.count == 0 => Value::Null,
            Kind::Avg => Value::Real(sum / self.count as f64),
            _ if self.any_real => Value::Real(sum),
            _ => {
                let whole = i64::try_from(self.whole);
                let too_large = "sum went past the largest whole number: total gives it as a real";
                Value::Integer(whole.map_err(|_| too_large.to_owned())?)
            }
        })
    }
}

/// What tells two rows apart for `DISTINCT`: a real without a fraction is the same as
/// the whole number.
fn mark(row: &[Value]) -> String {
    let mut mark = String::new();
    for value in row {
        let whole = match value {
            Value::Real(real) if real.fract() == 0.0 && real.abs() < 9e15 => {
                Some(Value::Integer(*real as i64))
            }
            _ => None,
        };
        mark.push_str(&format!("{:?},", whole.as_ref().unwrap_or(value)));
    }
    mark
}

/// Read the rows of the table a page at a time and work out the answer.
fn answer(database: &Database, question: &Question, most: usize) -> Result<Answer, String> {
    let aggregated = !question.aggregates.is_empty();
    let room = question.limit.map_or(most, |limit| limit.min(most));
    // One row past what is kept tells that the answer was cut.
    let one_more = question.limit.is_none_or(|limit| limit > most);
    let enough = if aggregated || !question.order.is_empty() {
        usize::MAX
    } else {
        // In no asked order, the first rows that fit are the answer: no need to read on.
        let wanted = question.offset.saturating_add(room);
        wanted.saturating_add(usize::from(one_more))
    };

    let mut running: Vec<Running> = Vec::new();
    running.resize_with(question.aggregates.len(), Running::default);
    // What each row is sorted by, and what is shown of it.
    let mut found: Vec<(Vec<Value>, Vec<Value>)> = Vec::new();
    let mut seen = HashSet::new();
    let mut read = 0;
    'reading: while found.len() < enough {
        // No more than what is still missing, unless rows may be left out on the way.
        let all_kept = question.filter.is_none() && !question.distinct;
        let least = if all_kept { 1 } else { FEW };
        let asked = (enough - found.len()).clamp(least, PAGE);
        let page = match question.table {
            Some(table) => database.rows(table, read, asked)?,
            None => vec![Vec::new()],
        };
        read += page.len();
        for row in &page {
            let kept = question.filter.as_ref();
            if kept.is_some_and(|filter| truth(&value(filter, row, &[])) != Some(true)) {
                continue;
            }
            if aggregated {
                for (running, aggregate) in running.iter_mut().zip(&question.aggregates) {
                    let taken = aggregate.of.as_ref();
                    let taken = taken.map_or(Value::Integer(1), |of| value(of, row, &[]));
                    running.take(aggregate.kind, taken);
                }
                continue;
            }
            let of = |item: &Expr| value(item, row, &[]);
            let shown: Vec<Value> = question.items.iter().map(of).collect();
            if question.distinct && !seen.insert(mark(&shown)) {
                continue;
            }
            found.push((question.order.iter().map(|(by, _)| of(by)).collect(), shown));
            if found.len() >= enough {
                break 'reading;
            }
        }
        if page.len() < asked {
            break;
        }
    }

    if aggregated {
        let totals = running.into_iter().zip(&question.aggregates);
        let totals: Result<Vec<Value>, String> = totals
            .map(|(running, aggregate)| running.result(aggregate.kind))
            .collect();
        let totals = totals?;
        let of = |item: &Expr| value(item, &[], &totals);
        found.push((Vec::new(), question.items.iter().map(of).collect()));
    } else {
        // Rows that sort alike stay in the order of the table.
        found.sort_by(|(left, _), (right, _)| {
            let keys = left.iter().zip(right).zip(&question.order);
            let compared = keys.map(|((left, right), (_, descending))| {
                let order = order(left, right);
                if *descending { order.reverse() } else { order }
            });
            compared
                .into_iter()
                .find(|order| order.is_ne())
                .unwrap_or(Ordering::Equal)
        });
    }
    let rows = found.into_iter().map(|(_, shown)| shown);
    let rows = rows.skip(question.offset);
    let mut rows: Vec<Vec<Value>> = rows.take(question.limit.unwrap_or(usize::MAX)).collect();
    let cut = rows.len() > most;
    rows.truncate(most);
    Ok(Answer {
        columns: question.names.clone(),
        rows,
        cut,
    })
}

/// Answer `query` from `database`, keeping at most `most` rows.
///
/// `query` is one `SELECT` over one table, in the part of SQL the top of this file tells.
/// The error says in plain words what was not understood, or which table or column there
/// is not. `cut` is set when rows were left out because of `most`, not because of `LIMIT`.
pub fn select(database: &Database, query: &str, most: usize) -> Result<Answer, String> {
    let pieces = pieces(query)?;
    match pieces.first().map(|piece| &piece.token) {
        None => return Err("there is no question: write one that starts with SELECT".to_owned()),
        Some(Token::Word(word)) if is_same(word, "SELECT") => {}
        Some(Token::Word(word)) if WRITERS.iter().any(|writer| is_same(writer, word)) => {
            return Err(format!(
                "this only reads: {} is not possible",
                word.to_ascii_uppercase()
            ));
        }
        Some(_) => {}
    }
    let parser = Parser {
        query,
        tables: database.tables(),
        pieces,
        at: 0,
        table: None,
        alias: None,
        aggregates: Vec::new(),
        in_list: false,
        in_aggregate: false,
        loose: None,
        missing: None,
        depth: 0,
        operators: 0,
    };
    if !parser.is_word("SELECT") {
        return Err(parser.unexpected("SELECT"));
    }
    let question = parser.question()?;
    answer(database, &question, most)
}

/// The tests named after what SQLite does hand `same` questions with the columns and the
/// rows that real SQLite (3.50, through Python's `sqlite3`) answered from the very same
/// `sample.db`: what is expected there was not worked out by hand. The other tests state
/// what is particular to this reader: where it is not SQLite, `most` and `cut`, how far
/// it reads, and what it refuses.
#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = include_bytes!("../tests/fixtures/sample.db");

    fn sample() -> Database {
        Database::open(SAMPLE.to_vec()).expect("the sample opens")
    }

    /// The sample with the tree of `events` leading back to itself after its last row:
    /// reading past that row is an error.
    fn endless_events() -> Database {
        let database = sample();
        let events = database
            .tables()
            .iter()
            .find(|table| table.name == "events");
        let root = events.expect("there is events").root_page as usize;
        let page = (root - 1) * 512;
        let mut looping = SAMPLE.to_vec();
        looping[page + 8..page + 12].copy_from_slice(&(root as u32).to_be_bytes());
        Database::open(looping).expect("the schema is whole")
    }

    /// Each row as the grid shows it, `|` between its values.
    fn written(answer: &Answer) -> Vec<String> {
        let row = |row: &Vec<Value>| {
            let values: Vec<String> = row.iter().map(Value::to_string).collect();
            values.join("|")
        };
        answer.rows.iter().map(row).collect()
    }

    fn ask(database: &Database, query: &str, most: usize) -> Answer {
        select(database, query, most).unwrap_or_else(|error| panic!("{query}: {error}"))
    }

    /// The rows the sample answers `query` with, all of them.
    fn rows(query: &str) -> Vec<String> {
        written(&ask(&sample(), query, usize::MAX))
    }

    /// Questions with the names of the columns and the rows real SQLite answered them with.
    fn same(checked: &[(&str, &str, &[&str])]) {
        let database = sample();
        for (query, columns, rows) in checked {
            let answer = ask(&database, query, usize::MAX);
            assert_eq!(answer.columns.join("|"), *columns, "{query}");
            assert_eq!(written(&answer), *rows, "{query}");
            assert!(!answer.cut, "{query}");
        }
    }

    fn refusal(query: &str) -> String {
        match select(&sample(), query, 100) {
            Ok(answer) => panic!("{query} was answered: {answer:?}"),
            Err(refusal) => refusal,
        }
    }

    #[test]
    fn a_star_shows_every_column_of_the_table() {
        same(&[
            (
                "SELECT * FROM people",
                "id|full name|age|score|photo|note",
                &[
                    "1|Ada Lovelace|36|9.5|BLOB (3 bytes)|NULL",
                    "2|Zoë — 日本語 😀|NULL|-0.25|NULL|héllo",
                    "40|Bob|0|1e300|BLOB (0 bytes)|",
                ],
            ),
            ("select * from tags;", "tag_id|label", &["5|five", "9|nine"]),
            (
                "SELECT * FROM notes",
                "body|pinned",
                &["written before the column came|NULL", "written after|1"],
            ),
            ("SELECT * FROM empty", "a|b", &[]),
            (
                "SELECT * FROM numbers WHERE value < 0 AND value > -200",
                "label|value",
                &[
                    "minus one|-1",
                    "one byte, negative|-128",
                    "small real|-1.5e-7",
                ],
            ),
        ]);
    }

    #[test]
    fn chosen_columns_and_values_worked_out_of_them_are_shown_and_named() {
        same(&[
            (
                "SELECT id, \"full name\" FROM people ORDER BY id",
                "id|full name",
                &["1|Ada Lovelace", "2|Zoë — 日本語 😀", "40|Bob"],
            ),
            (
                "SELECT age + 1, score * 2 AS twice, id AS 'key' FROM people",
                "age + 1|twice|key",
                &["37|19.0|1", "NULL|-0.5|2", "1|2e300|40"],
            ),
            (
                "SELECT people.id, PEOPLE.AGE FROM people",
                "id|age",
                &["1|36", "2|NULL", "40|0"],
            ),
            (
                "SELECT p.id, P.note FROM people AS p",
                "id|note",
                &["1|NULL", "2|héllo", "40|"],
            ),
            (
                "SELECT ID, Age FROM PEOPLE WHERE Id = 40",
                "id|age",
                &["40|0"],
            ),
            (
                "SELECT *, tag_id * 2 AS twice FROM tags",
                "tag_id|label|twice",
                &["5|five|10", "9|nine|18"],
            ),
            (
                "SELECT 'it''s', 1.5, .5, 2e3, 10, -3, +4, TRUE, FALSE, NULL FROM tags WHERE \
                 tag_id = 5",
                "'it''s'|1.5|.5|2e3|10|-3|+4|TRUE|FALSE|NULL",
                &["it's|1.5|0.5|2000.0|10|-3|4|1|0|NULL"],
            ),
        ]);
    }

    #[test]
    fn a_question_without_a_table_is_answered_once() {
        same(&[
            ("SELECT 1", "1", &["1"]),
            ("SELECT 1 + 1 AS two, 'a'", "two|'a'", &["2|a"]),
            ("SELECT 1 WHERE 0", "1", &[]),
            ("SELECT 2 WHERE 1 ORDER BY 1 LIMIT 5", "2", &["2"]),
            ("SELECT count(*), max(3)", "count(*)|max(3)", &["1|3"]),
            ("SELECT 1 LIMIT 0", "1", &[]),
        ]);
    }

    #[test]
    fn keywords_are_read_in_any_case_and_comments_and_a_last_semicolon_are_skipped() {
        same(&[
            ("select id from people where id=1 ; ", "id", &["1"]),
            (
                "SELECT id -- the key\nFROM people /* all\n of them */ WHERE id = 2;",
                "id",
                &["2"],
            ),
            (
                "sElEcT Id FrOm PeOpLe wHeRe iD iN (1, 2) oRdEr bY iD dEsC lImIt 1 oFfSeT 0",
                "id",
                &["2"],
            ),
            (
                "SELECT/**/id/**/FROM/**/people/**/ORDER/**/BY/**/id",
                "id",
                &["1", "2", "40"],
            ),
            (
                "SELECT age FROM people -- to the end",
                "age",
                &["36", "NULL", "0"],
            ),
            (
                "SELECT age FROM people /* never closed",
                "age",
                &["36", "NULL", "0"],
            ),
            (
                "SELECT 1 /*/ still open */ + 1 /**/ /***/ AS two",
                "two",
                &["2"],
            ),
            (
                "SELECT '--no' || '/*comment*/' AS kept",
                "kept",
                &["--no/*comment*/"],
            ),
            (
                "SELECT id /* the key */ + 1 FROM people WHERE id = 1",
                "id /* the key */ + 1",
                &["2"],
            ),
        ]);
    }

    #[test]
    fn names_between_quotes_brackets_or_backticks_are_understood() {
        same(&[
            (
                "SELECT \"first col\", [second], `third`, \"fo,ur\" FROM \"odd \"\"names\"\"\"",
                "first col|second|third|fo,ur",
                &[],
            ),
            (
                "SELECT * FROM [odd \"names\"]",
                "first col|second|third|fo,ur",
                &[],
            ),
            (
                "SELECT `odd \"names\"`.`first col` AS a, `ODD \"NAMES\"`.Second FROM `odd \
                 \"names\"` WHERE 0",
                "a|second",
                &[],
            ),
            (
                "SELECT count(*), count(\"fo,ur\"), max([first col]) FROM \"odd \"\"names\"\"\"",
                "count(*)|count(\"fo,ur\")|max([first col])",
                &["0|0|NULL"],
            ),
            (
                "SELECT [full name], `full name`, \"people\".\"full name\", [people].[id] FROM \
                 \"people\" WHERE [id] = 1",
                "full name|full name|full name|id",
                &["Ada Lovelace|Ada Lovelace|Ada Lovelace|1"],
            ),
            (
                "SELECT \"FULL NAME\" FROM people WHERE \"Full Name\" = 'Bob'",
                "full name",
                &["Bob"],
            ),
        ]);
    }

    #[test]
    fn where_keeps_the_rows_for_which_it_is_true() {
        same(&[
            ("SELECT id FROM people WHERE age > 10", "id", &["1"]),
            ("SELECT id FROM people WHERE age", "id", &["1"]),
            ("SELECT id FROM people WHERE NOT age", "id", &["40"]),
            ("SELECT id FROM people WHERE 'abc'", "id", &[]),
            (
                "SELECT id FROM people WHERE '1' ORDER BY id",
                "id",
                &["1", "2", "40"],
            ),
            (
                "SELECT id FROM people WHERE 0.5 ORDER BY id",
                "id",
                &["1", "2", "40"],
            ),
            (
                "SELECT id FROM events WHERE payload = 'event number 777'",
                "id",
                &["2331"],
            ),
            (
                "SELECT count(*) FROM events WHERE id % 2 = 0 AND kind = 'kind-3'",
                "count(*)",
                &["107"],
            ),
            (
                "SELECT id FROM events WHERE id > 4490 AND NOT kind = 'kind-0'",
                "id",
                &["4491", "4497", "4500"],
            ),
        ]);
    }

    #[test]
    fn values_are_compared_with_each_of_the_comparison_signs() {
        same(&[
            (
                "SELECT label FROM numbers WHERE value = 2",
                "label",
                &["real"],
            ),
            (
                "SELECT label FROM numbers WHERE value == 127",
                "label",
                &["one byte"],
            ),
            (
                "SELECT label FROM numbers WHERE value != 0 AND value <> 1 AND value < 2 AND \
                 value > -2",
                "label",
                &["minus one", "small real"],
            ),
            (
                "SELECT label FROM numbers WHERE value <= -8388608 AND value >= -2147483648",
                "label",
                &["three bytes, negative", "four bytes, negative"],
            ),
            (
                "SELECT label FROM numbers WHERE value < 1.5 AND value > -1",
                "label",
                &["zero", "one", "small real"],
            ),
            (
                "SELECT label FROM numbers WHERE value > 9223372036854775806",
                "label",
                &["eight bytes"],
            ),
            (
                "SELECT label FROM numbers WHERE value >= 9.3e18 OR value <= -9.3e18",
                "label",
                &[],
            ),
            (
                "SELECT 1 = 1, 1 == 2, 1 != 1, 1 <> 2, 1 < 2, 2 <= 2, 1 > 2, 2 >= 3",
                "1 = 1|1 == 2|1 != 1|1 <> 2|1 < 2|2 <= 2|1 > 2|2 >= 3",
                &["1|0|0|1|1|1|0|0"],
            ),
        ]);
    }

    #[test]
    fn whole_numbers_and_reals_compare_by_value_however_large() {
        same(&[
            (
                "SELECT 2 = 2.0, 2 < 2.5, 3 > 2.5, -2 > -2.5, 1 < 1e300, -1e300 < 1",
                "2 = 2.0|2 < 2.5|3 > 2.5|-2 > -2.5|1 < 1e300|-1e300 < 1",
                &["1|1|1|1|1|1"],
            ),
            (
                "SELECT 9007199254740993 > 9007199254740992.0, 9007199254740993 = \
                 9007199254740993.0",
                "9007199254740993 > 9007199254740992.0|9007199254740993 = 9007199254740993.0",
                &["1|0"],
            ),
            (
                "SELECT 9223372036854775807 < 9223372036854775808.0, 9223372036854775807 = \
                 9223372036854775807.0",
                "9223372036854775807 < 9223372036854775808.0|9223372036854775807 = \
                 9223372036854775807.0",
                &["1|0"],
            ),
            (
                "SELECT -9223372036854775808 = -9223372036854775808.0, -9223372036854775808 > \
                 -1e19",
                "-9223372036854775808 = -9223372036854775808.0|-9223372036854775808 > -1e19",
                &["1|1"],
            ),
        ]);
    }

    #[test]
    fn null_then_numbers_then_text_then_blobs_is_the_order_across_types() {
        same(&[
            (
                "SELECT 1 < 'a', 99999 < '1', 1e300 < '', '9' > 10, 'a' < photo, photo > 5, '' < \
                 photo FROM people WHERE id = 1",
                "1 < 'a'|99999 < '1'|1e300 < ''|'9' > 10|'a' < photo|photo > 5|'' < photo",
                &["1|1|1|1|1|1|1"],
            ),
            (
                "SELECT '5' = 5, 5 = '5', '5' < 5, 'abc' = 0, '' = 0",
                "'5' = 5|5 = '5'|'5' < 5|'abc' = 0|'' = 0",
                &["0|0|0|0|0"],
            ),
            (
                "SELECT id FROM people ORDER BY coalesce(note, age)",
                "id",
                &["1", "40", "2"],
            ),
            (
                "SELECT id FROM people ORDER BY coalesce(photo, note) DESC",
                "id",
                &["1", "40", "2"],
            ),
            (
                "SELECT id, typeof(coalesce(photo, note, age)) FROM people ORDER BY \
                 coalesce(photo, note, age), id",
                "id|typeof(coalesce(photo, note, age))",
                &["2|text", "40|blob", "1|blob"],
            ),
        ]);
    }

    #[test]
    fn text_is_compared_byte_by_byte() {
        same(&[
            (
                "SELECT 'B' < 'a', 'a' < 'B', 'é' > 'z', 'Z' < 'a', 'abc' < 'abd', 'ab' < 'abc', \
                 '' < 'a', 'a' = 'A'",
                "'B' < 'a'|'a' < 'B'|'é' > 'z'|'Z' < 'a'|'abc' < 'abd'|'ab' < 'abc'|'' < 'a'|'a' \
                 = 'A'",
                &["1|0|1|1|1|1|1|0"],
            ),
            (
                "SELECT \"full name\" FROM people ORDER BY \"full name\"",
                "full name",
                &["Ada Lovelace", "Bob", "Zoë — 日本語 😀"],
            ),
            (
                "SELECT label FROM numbers ORDER BY label DESC LIMIT 3",
                "label",
                &["zero", "two bytes, negative", "two bytes"],
            ),
            (
                "SELECT id FROM people ORDER BY note",
                "id",
                &["1", "40", "2"],
            ),
        ]);
    }

    #[test]
    fn arithmetic_keeps_whole_numbers_whole_and_division_by_zero_is_null() {
        same(&[
            (
                "SELECT 7 / 2, -7 / 2, 7 % 3, -7 % 3, 7 % -3, 7.0 / 2, 7 / 2.0, 1 / 0, 1 % 0, \
                 1.5 / 0, 0 / 0.0",
                "7 / 2|-7 / 2|7 % 3|-7 % 3|7 % -3|7.0 / 2|7 / 2.0|1 / 0|1 % 0|1.5 / 0|0 / 0.0",
                &["3|-3|1|-1|1|3.5|3.5|NULL|NULL|NULL|NULL"],
            ),
            (
                "SELECT 5.5 % 2, 7 % 2.5, 7 % 0.5, 2 * 3 + 4, 2 + 3 * 4, (2 + 3) * 4, - 3 + 1, \
                 -(3 + 1), 10 - 4 - 3, 2 * 3 % 4, 100 / 10 / 5",
                "5.5 % 2|7 % 2.5|7 % 0.5|2 * 3 + 4|2 + 3 * 4|(2 + 3) * 4|- 3 + 1|-(3 + 1)|10 - 4 \
                 - 3|2 * 3 % 4|100 / 10 / 5",
                &["1.0|1.0|NULL|10|14|20|-2|-4|3|2|2"],
            ),
            (
                "SELECT 9223372036854775807 + 1, -9223372036854775808, -9223372036854775807 - 2",
                "9223372036854775807 + 1|-9223372036854775808|-9223372036854775807 - 2",
                &["9.223372036854776e18|-9223372036854775808|-9.223372036854776e18"],
            ),
            (
                "SELECT 9223372036854775807 * 2, 9223372036854775808, 3037000500 * 3037000500",
                "9223372036854775807 * 2|9223372036854775808|3037000500 * 3037000500",
                &["1.8446744073709552e19|9.223372036854776e18|9.22337203700025e18"],
            ),
            (
                "SELECT -9223372036854775808 / -1, -9223372036854775808 % -1, \
                 9223372036854775807 - -1, - -9223372036854775807",
                "-9223372036854775808 / -1|-9223372036854775808 % -1|9223372036854775807 - -1|- \
                 -9223372036854775807",
                &["9.223372036854776e18|0|9.223372036854776e18|9223372036854775807"],
            ),
            (
                "SELECT age + 1, age * score, age / 0, -age, id % 3, score - 1 FROM people",
                "age + 1|age * score|age / 0|-age|id % 3|score - 1",
                &[
                    "37|342.0|NULL|-36|1|8.5",
                    "NULL|NULL|NULL|NULL|2|-1.25",
                    "1|0.0|NULL|0|1|1e300",
                ],
            ),
            (
                "SELECT 1 + 2.5, 0.1 + 0.2, 3 * 0.5, 1 - 1.0, 2.0 * 2, 1e300 * 1e300, 1.5 + 1.5",
                "1 + 2.5|0.1 + 0.2|3 * 0.5|1 - 1.0|2.0 * 2|1e300 * 1e300|1.5 + 1.5",
                &["3.5|0.30000000000000004|1.5|0.0|4.0|inf|3.0"],
            ),
        ]);
    }

    #[test]
    fn arithmetic_reads_the_number_a_text_starts_with() {
        same(&[
            (
                "SELECT '12abc' + 1, 'abc' + 1, '1.5' * 2, ' 3' + 1, '1e2' + 0, '.5' + 0, '5.' + \
                 0, '1e' + 0, '+7' + 0, '-' + 0, - '4', '-2.5x' * 2, '' + 0",
                "'12abc' + 1|'abc' + 1|'1.5' * 2|' 3' + 1|'1e2' + 0|'.5' + 0|'5.' + 0|'1e' + \
                 0|'+7' + 0|'-' + 0|- '4'|'-2.5x' * 2|'' + 0",
                &["13|1|3.0|4|100.0|0.5|5.0|1|7|0|-4|-5.0|0"],
            ),
            (
                "SELECT NULL + 1, 1 - NULL, NULL * NULL, -NULL, NULL / 0, NULL % 2",
                "NULL + 1|1 - NULL|NULL * NULL|-NULL|NULL / 0|NULL % 2",
                &["NULL|NULL|NULL|NULL|NULL|NULL"],
            ),
            (
                "SELECT kind + 1, payload * 2, substr(payload, 14) + 1 FROM events WHERE id = 30",
                "kind + 1|payload * 2|substr(payload, 14) + 1",
                &["1|0|11"],
            ),
        ]);
    }

    #[test]
    fn two_bars_join_texts_and_give_null_for_null() {
        same(&[
            (
                "SELECT 'a' || 'b' || 'c', 1 || 2, 'x' || NULL, NULL || 'x', 'n' || 3, 1 + 2 || \
                 3, 'a' || 1 + 2, '' || ''",
                "'a' || 'b' || 'c'|1 || 2|'x' || NULL|NULL || 'x'|'n' || 3|1 + 2 || 3|'a' || 1 + \
                 2|'' || ''",
                &["abc|12|NULL|NULL|n3|24|2|"],
            ),
            (
                "SELECT \"full name\" || ' (' || id || ')', note || '!', age || '' FROM people",
                "\"full name\" || ' (' || id || ')'|note || '!'|age || ''",
                &[
                    "Ada Lovelace (1)|NULL|36",
                    "Zoë — 日本語 😀 (2)|héllo!|NULL",
                    "Bob (40)|!|0",
                ],
            ),
            (
                "SELECT 1.5 || '', 3.0 || 'x', -0.25 || 1",
                "1.5 || ''|3.0 || 'x'|-0.25 || 1",
                &["1.5|3.0x|-0.251"],
            ),
        ]);
    }

    #[test]
    fn null_makes_a_comparison_null_and_where_keeps_only_what_is_true() {
        same(&[
            (
                "SELECT NULL = NULL, NULL != 1, NULL < 1, NULL <> NULL, 1 >= NULL",
                "NULL = NULL|NULL != 1|NULL < 1|NULL <> NULL|1 >= NULL",
                &["NULL|NULL|NULL|NULL|NULL"],
            ),
            (
                "SELECT NULL AND 0, NULL AND 1, 0 AND NULL, 1 AND NULL, NULL AND NULL",
                "NULL AND 0|NULL AND 1|0 AND NULL|1 AND NULL|NULL AND NULL",
                &["0|NULL|0|NULL|NULL"],
            ),
            (
                "SELECT NULL OR 1, NULL OR 0, 1 OR NULL, 0 OR NULL, NULL OR NULL",
                "NULL OR 1|NULL OR 0|1 OR NULL|0 OR NULL|NULL OR NULL",
                &["1|NULL|1|NULL|NULL"],
            ),
            (
                "SELECT NOT NULL, NOT 1, NOT 0, NOT 5, NOT 'a', NOT '2'",
                "NOT NULL|NOT 1|NOT 0|NOT 5|NOT 'a'|NOT '2'",
                &["NULL|0|1|0|1|0"],
            ),
            (
                "SELECT 1 AND 1, 1 AND 0, 0 AND 0, 1 OR 0, 0 OR 0, 2 AND 3, 0.0 OR 0, 0.5 AND 1",
                "1 AND 1|1 AND 0|0 AND 0|1 OR 0|0 OR 0|2 AND 3|0.0 OR 0|0.5 AND 1",
                &["1|0|0|1|0|1|0|1"],
            ),
            ("SELECT id FROM people WHERE age = NULL", "id", &[]),
            ("SELECT id FROM people WHERE age <> 36", "id", &["40"]),
            ("SELECT id FROM people WHERE NOT (age = 36)", "id", &["40"]),
            (
                "SELECT id FROM people WHERE age = 36 OR age IS NULL",
                "id",
                &["1", "2"],
            ),
            ("SELECT id FROM people WHERE NULL", "id", &[]),
            ("SELECT id FROM people WHERE NOT NULL", "id", &[]),
            (
                "SELECT count(*) FROM people WHERE age > 10 OR age <= 10",
                "count(*)",
                &["2"],
            ),
            (
                "SELECT id FROM people WHERE age > 10 OR id = 2",
                "id",
                &["1", "2"],
            ),
            ("SELECT id FROM people WHERE age < 10 AND id = 2", "id", &[]),
        ]);
    }

    #[test]
    fn is_tells_null_from_the_rest_and_never_gives_null() {
        same(&[
            (
                "SELECT NULL IS NULL, NULL IS NOT NULL, 1 IS NULL, 1 IS NOT NULL, NULL IS 1, 1 \
                 IS 1, 1 IS NOT 2, 'a' IS 'a', 1 IS 1.0, 'a' IS NOT 'A'",
                "NULL IS NULL|NULL IS NOT NULL|1 IS NULL|1 IS NOT NULL|NULL IS 1|1 IS 1|1 IS NOT \
                 2|'a' IS 'a'|1 IS 1.0|'a' IS NOT 'A'",
                &["1|0|0|1|0|1|1|1|1|1"],
            ),
            ("SELECT id FROM people WHERE age IS NULL", "id", &["2"]),
            (
                "SELECT id FROM people WHERE age IS NOT NULL",
                "id",
                &["1", "40"],
            ),
            (
                "SELECT id FROM people WHERE note IS NULL OR photo IS NULL",
                "id",
                &["1", "2"],
            ),
            ("SELECT id FROM people WHERE age IS 0", "id", &["40"]),
            (
                "SELECT id FROM people WHERE age IS NOT 36",
                "id",
                &["2", "40"],
            ),
            (
                "SELECT body FROM notes WHERE pinned IS NULL",
                "body",
                &["written before the column came"],
            ),
        ]);
    }

    #[test]
    fn like_matches_percent_and_underscore_whatever_the_case_of_ascii_letters() {
        same(&[
            (
                "SELECT \"full name\" FROM people WHERE \"full name\" LIKE 'a%'",
                "full name",
                &["Ada Lovelace"],
            ),
            (
                "SELECT \"full name\" FROM people WHERE \"full name\" LIKE '%LOVE%'",
                "full name",
                &["Ada Lovelace"],
            ),
            (
                "SELECT \"full name\" FROM people WHERE \"full name\" LIKE 'b_b'",
                "full name",
                &["Bob"],
            ),
            (
                "SELECT \"full name\" FROM people WHERE \"full name\" NOT LIKE '%o%'",
                "full name",
                &[],
            ),
            (
                "SELECT id FROM people WHERE \"full name\" LIKE 'zoë%'",
                "id",
                &["2"],
            ),
            (
                "SELECT id FROM people WHERE \"full name\" LIKE 'ZOË%'",
                "id",
                &[],
            ),
            (
                "SELECT id FROM people WHERE \"full name\" LIKE '%日本_ 😀'",
                "id",
                &["2"],
            ),
            (
                "SELECT id FROM people WHERE note LIKE '%'",
                "id",
                &["2", "40"],
            ),
            (
                "SELECT id FROM people WHERE note NOT LIKE 'x'",
                "id",
                &["2", "40"],
            ),
            (
                "SELECT count(*) FROM events WHERE payload LIKE 'event number 1_'",
                "count(*)",
                &["10"],
            ),
            (
                "SELECT count(*) FROM events WHERE payload LIKE '%7' AND kind LIKE 'KIND-_'",
                "count(*)",
                &["150"],
            ),
            (
                "SELECT 'abc' LIKE 'ABC', 'abc' LIKE 'a%c%', 'abc' LIKE '', '' LIKE '', '' LIKE \
                 '%', 'a%' LIKE 'a%', 'abc' LIKE 'ab', 'abc' LIKE '_bc_'",
                "'abc' LIKE 'ABC'|'abc' LIKE 'a%c%'|'abc' LIKE ''|'' LIKE ''|'' LIKE '%'|'a%' \
                 LIKE 'a%'|'abc' LIKE 'ab'|'abc' LIKE '_bc_'",
                &["1|1|0|1|1|1|0|0"],
            ),
            (
                "SELECT 'abc' LIKE NULL, NULL LIKE 'a', NULL NOT LIKE 'a', 12 LIKE '1%', 'aXbXc' \
                 LIKE '%X%X%', 'ab' LIKE 'a_%', 'a' LIKE 'a_%', 'mississippi' LIKE '%ss%pp_', \
                 'mississippi' LIKE '%ss%pp'",
                "'abc' LIKE NULL|NULL LIKE 'a'|NULL NOT LIKE 'a'|12 LIKE '1%'|'aXbXc' LIKE \
                 '%X%X%'|'ab' LIKE 'a_%'|'a' LIKE 'a_%'|'mississippi' LIKE \
                 '%ss%pp_'|'mississippi' LIKE '%ss%pp'",
                &["NULL|NULL|NULL|1|1|1|0|1|0"],
            ),
        ]);
    }

    #[test]
    fn in_looks_for_a_value_in_a_list() {
        same(&[
            (
                "SELECT id FROM people WHERE id IN (1, 40, 99)",
                "id",
                &["1", "40"],
            ),
            (
                "SELECT id FROM people WHERE id NOT IN (1, 40)",
                "id",
                &["2"],
            ),
            (
                "SELECT id FROM people WHERE age IN (36, NULL)",
                "id",
                &["1"],
            ),
            (
                "SELECT id FROM people WHERE age NOT IN (36, NULL)",
                "id",
                &[],
            ),
            ("SELECT id FROM people WHERE age NOT IN (36)", "id", &["40"]),
            (
                "SELECT id FROM people WHERE \"full name\" IN ('Bob', 'ada lovelace')",
                "id",
                &["40"],
            ),
            (
                "SELECT 1 IN (), NULL IN (), 1 NOT IN (), 1 IN (1.0), 'a' IN ('A'), NULL IN (1), \
                 2 IN (1, NULL), 2 IN (1 + 1), 2 NOT IN (1, NULL), 3 IN (1, 2, 3)",
                "1 IN ()|NULL IN ()|1 NOT IN ()|1 IN (1.0)|'a' IN ('A')|NULL IN (1)|2 IN (1, \
                 NULL)|2 IN (1 + 1)|2 NOT IN (1, NULL)|3 IN (1, 2, 3)",
                &["0|0|1|1|0|NULL|NULL|1|NULL|1"],
            ),
            (
                "SELECT count(*) FROM events WHERE kind IN ('kind-1', 'kind-2') AND id IN (3, 6, \
                 9, 12)",
                "count(*)",
                &["2"],
            ),
        ]);
    }

    #[test]
    fn between_takes_both_ends() {
        same(&[
            (
                "SELECT id FROM events WHERE id BETWEEN 10 AND 20",
                "id",
                &["12", "15", "18"],
            ),
            (
                "SELECT id FROM events WHERE id NOT BETWEEN 4 AND 4497",
                "id",
                &["3", "4500"],
            ),
            (
                "SELECT label FROM numbers WHERE value BETWEEN -1 AND 1",
                "label",
                &["zero", "one", "minus one", "small real"],
            ),
            (
                "SELECT 2 BETWEEN 1 AND 3, 2 BETWEEN 3 AND 1, NULL BETWEEN 1 AND 2, 2 BETWEEN \
                 NULL AND 3, 5 BETWEEN NULL AND 3, 2 NOT BETWEEN 1 AND 3, 1 BETWEEN 1 AND 2 AND \
                 0, 'b' BETWEEN 'a' AND 'c', 2 BETWEEN 1 AND 1 + 1, 0 BETWEEN 1 AND NULL",
                "2 BETWEEN 1 AND 3|2 BETWEEN 3 AND 1|NULL BETWEEN 1 AND 2|2 BETWEEN NULL AND 3|5 \
                 BETWEEN NULL AND 3|2 NOT BETWEEN 1 AND 3|1 BETWEEN 1 AND 2 AND 0|'b' BETWEEN \
                 'a' AND 'c'|2 BETWEEN 1 AND 1 + 1|0 BETWEEN 1 AND NULL",
                &["1|0|NULL|NULL|0|0|0|1|1|0"],
            ),
        ]);
    }

    #[test]
    fn operators_bind_as_tightly_as_sqlite_has_them() {
        same(&[
            (
                "SELECT 1 OR 0 AND 0, NOT 1 = 2, NOT 0 AND 0, 1 + 2 * 3 - 4 / 2, 2 * 3 || 4, 1 < \
                 2 = 1, 1 = 2 < 3, -2 || 3",
                "1 OR 0 AND 0|NOT 1 = 2|NOT 0 AND 0|1 + 2 * 3 - 4 / 2|2 * 3 || 4|1 < 2 = 1|1 = 2 \
                 < 3|-2 || 3",
                &["1|1|0|5|68|1|1|-23"],
            ),
            (
                "SELECT NOT 1 IS NULL, 1 + 1 IN (2), 5 - 2 - 1, 2 = 1 + 1, NOT NOT 3, 1 OR 1 AND \
                 0 OR 0, (1 OR 1) AND 0, 2 > 1 AND 3 > 2, 1 || 2 * 3",
                "NOT 1 IS NULL|1 + 1 IN (2)|5 - 2 - 1|2 = 1 + 1|NOT NOT 3|1 OR 1 AND 0 OR 0|(1 \
                 OR 1) AND 0|2 > 1 AND 3 > 2|1 || 2 * 3",
                &["1|1|2|1|1|1|0|1|36"],
            ),
            (
                "SELECT 1 + NOT 2, 1 = NOT 2, - - 3, -(-3), - 'x', NOT 'abc', NOT '1x', 'abc' \
                 AND 1, '0.5' AND 1",
                "1 + NOT 2|1 = NOT 2|- - 3|-(-3)|- 'x'|NOT 'abc'|NOT '1x'|'abc' AND 1|'0.5' AND \
                 1",
                &["1|0|3|3|0|1|0|0|1"],
            ),
            (
                "SELECT 1 = 1 IS NOT NULL, NULL IS NULL = 1, 3 - -2, 2 - - - 2, NOT 1 + 1, 6 / 3 \
                 * 2, 2 < 3 < 1, 'a' || 'b' LIKE 'AB', 1 + 1 BETWEEN 1 + 1 AND 4 / 2",
                "1 = 1 IS NOT NULL|NULL IS NULL = 1|3 - -2|2 - - - 2|NOT 1 + 1|6 / 3 * 2|2 < 3 < \
                 1|'a' || 'b' LIKE 'AB'|1 + 1 BETWEEN 1 + 1 AND 4 / 2",
                &["1|1|5|0|0|4|0|1|1"],
            ),
        ]);
    }

    #[test]
    fn length_lower_upper_and_abs_work_on_one_value() {
        same(&[
            (
                "SELECT length('héllo'), length(''), length(NULL), length(12345)",
                "length('héllo')|length('')|length(NULL)|length(12345)",
                &["5|0|NULL|5"],
            ),
            (
                "SELECT length(-1.5), LENGTH(Lower('ABC')), length(' a '), length('日本語 😀')",
                "length(-1.5)|LENGTH(Lower('ABC'))|length(' a ')|length('日本語 😀')",
                &["4|3|3|5"],
            ),
            (
                "SELECT length(\"full name\"), length(photo), length(note), length(age) FROM \
                 people",
                "length(\"full name\")|length(photo)|length(note)|length(age)",
                &["12|3|NULL|2", "11|NULL|5|NULL", "3|0|0|1"],
            ),
            (
                "SELECT lower('ÀBC Def'), upper('àbc def'), lower(NULL), upper(12), \
                 upper(lower('MiXed'))",
                "lower('ÀBC Def')|upper('àbc def')|lower(NULL)|upper(12)|upper(lower('MiXed'))",
                &["Àbc def|àBC DEF|NULL|12|MIXED"],
            ),
            (
                "SELECT abs(-5), abs(5.5), abs(-0.25), abs(NULL), abs(-9223372036854775807), \
                 abs(0), abs(2 - 7)",
                "abs(-5)|abs(5.5)|abs(-0.25)|abs(NULL)|abs(-9223372036854775807)|abs(0)|abs(2 - \
                 7)",
                &["5|5.5|0.25|NULL|9223372036854775807|0|5"],
            ),
            (
                "SELECT abs('5'), abs('abc'), abs('-2.5')",
                "abs('5')|abs('abc')|abs('-2.5')",
                &["5.0|0.0|2.5"],
            ),
            (
                "SELECT length(data), length(body), length(title) FROM documents WHERE title IN \
                 ('unicode', 'len-3000')",
                "length(data)|length(body)|length(title)",
                &["NULL|3000|8", "2000|1200|7"],
            ),
        ]);
    }

    #[test]
    fn coalesce_and_ifnull_give_the_first_value_that_is_not_null() {
        same(&[
            (
                "SELECT coalesce(NULL, NULL, 3), coalesce(NULL, 'a', 3), ifnull(NULL, 1), \
                 ifnull(2, 1), coalesce(NULL, NULL), IFNULL(NULL, NULL)",
                "coalesce(NULL, NULL, 3)|coalesce(NULL, 'a', 3)|ifnull(NULL, 1)|ifnull(2, \
                 1)|coalesce(NULL, NULL)|IFNULL(NULL, NULL)",
                &["3|a|1|2|NULL|NULL"],
            ),
            (
                "SELECT id, coalesce(age, -1), ifnull(note, 'none'), coalesce(note, age, id) \
                 FROM people",
                "id|coalesce(age, -1)|ifnull(note, 'none')|coalesce(note, age, id)",
                &["1|36|none|36", "2|-1|héllo|héllo", "40|0||"],
            ),
        ]);
    }

    #[test]
    fn typeof_names_the_type_a_value_is_stored_as() {
        same(&[
            (
                "SELECT typeof(1), typeof(1.0), typeof('a'), typeof(NULL), typeof(1 + 1.0), \
                 typeof(7 / 2), typeof('1' + 1), typeof(1 || 1), typeof(1 = 1)",
                "typeof(1)|typeof(1.0)|typeof('a')|typeof(NULL)|typeof(1 + 1.0)|typeof(7 / \
                 2)|typeof('1' + 1)|typeof(1 || 1)|typeof(1 = 1)",
                &["integer|real|text|null|real|integer|integer|text|integer"],
            ),
            (
                "SELECT typeof(id), typeof(age), typeof(score), typeof(photo), typeof(note) FROM \
                 people",
                "typeof(id)|typeof(age)|typeof(score)|typeof(photo)|typeof(note)",
                &[
                    "integer|integer|real|blob|null",
                    "integer|null|real|null|text",
                    "integer|integer|real|blob|text",
                ],
            ),
            (
                "SELECT label, typeof(value) FROM numbers WHERE label LIKE '%real' OR label = \
                 'zero'",
                "label|typeof(value)",
                &["zero|integer", "real|real", "small real|real"],
            ),
            (
                "SELECT typeof(net), net FROM prices",
                "typeof(net)|net",
                &["real|3.0"],
            ),
        ]);
    }

    #[test]
    fn substr_cuts_a_text_counting_characters_from_one_or_from_the_end() {
        same(&[
            (
                "SELECT substr('hello', 2), substr('hello', 2, 3), substr('hello', 0), \
                 substr('hello', 0, 2), substr('hello', -3), substr('hello', -3, 2), \
                 substr('hello', 2, -1), substr('hello', 10)",
                "substr('hello', 2)|substr('hello', 2, 3)|substr('hello', 0)|substr('hello', 0, \
                 2)|substr('hello', -3)|substr('hello', -3, 2)|substr('hello', 2, \
                 -1)|substr('hello', 10)",
                &["ello|ell|hello|h|llo|ll|h|"],
            ),
            (
                "SELECT substr('hello', -10, 7), substr('hello', 4, -10), substr('héllo', 2, 2), \
                 substr('hello', 1, 0), substr('hello', -1, -2), substring('hello', 2, 2), \
                 substr('hello', 2.9), substr('hello', -10)",
                "substr('hello', -10, 7)|substr('hello', 4, -10)|substr('héllo', 2, \
                 2)|substr('hello', 1, 0)|substr('hello', -1, -2)|substring('hello', 2, \
                 2)|substr('hello', 2.9)|substr('hello', -10)",
                &["he|hel|él||ll|el|ello|hello"],
            ),
            (
                "SELECT substr(NULL, 1), substr('hello', NULL), substr('hello', 1, NULL), \
                 substr(12345, 2, 2), substr('hello', 5, 5), substr('hello', 6), substr('hello', \
                 0, -2), substr('hello', -2, -2)",
                "substr(NULL, 1)|substr('hello', NULL)|substr('hello', 1, NULL)|substr(12345, 2, \
                 2)|substr('hello', 5, 5)|substr('hello', 6)|substr('hello', 0, \
                 -2)|substr('hello', -2, -2)",
                &["NULL|NULL|NULL|23|o|||el"],
            ),
            (
                "SELECT substr('hello', '2x', 2), substr('hello', 2, '2'), substr('hello', \
                 -1.5), substr('', 1), substr(1.5, 2)",
                "substr('hello', '2x', 2)|substr('hello', 2, '2')|substr('hello', \
                 -1.5)|substr('', 1)|substr(1.5, 2)",
                &["el|el|o||.5"],
            ),
            (
                "SELECT substr(\"full name\", 1, 3), upper(substr(\"full name\", -3)) FROM \
                 people ORDER BY id",
                "substr(\"full name\", 1, 3)|upper(substr(\"full name\", -3))",
                &["Ada|ACE", "Zoë|語 😀", "Bob|BOB"],
            ),
        ]);
    }

    #[test]
    fn order_by_sorts_nulls_first_and_last_when_descending() {
        same(&[
            (
                "SELECT id, age FROM people ORDER BY age",
                "id|age",
                &["2|NULL", "40|0", "1|36"],
            ),
            (
                "SELECT id, age FROM people ORDER BY age DESC",
                "id|age",
                &["1|36", "40|0", "2|NULL"],
            ),
            (
                "SELECT id, age FROM people ORDER BY age ASC, id DESC",
                "id|age",
                &["2|NULL", "40|0", "1|36"],
            ),
            (
                "SELECT body FROM notes ORDER BY pinned",
                "body",
                &["written before the column came", "written after"],
            ),
            (
                "SELECT body FROM notes ORDER BY pinned DESC",
                "body",
                &["written after", "written before the column came"],
            ),
            (
                "SELECT id FROM people ORDER BY photo",
                "id",
                &["2", "40", "1"],
            ),
            (
                "SELECT id FROM people ORDER BY photo DESC",
                "id",
                &["1", "40", "2"],
            ),
        ]);
    }

    #[test]
    fn order_by_sorts_by_values_by_shown_names_and_by_shown_places() {
        same(&[
            (
                "SELECT label FROM numbers ORDER BY value",
                "label",
                &[
                    "eight bytes, negative",
                    "six bytes, negative",
                    "four bytes, negative",
                    "three bytes, negative",
                    "two bytes, negative",
                    "one byte, negative",
                    "minus one",
                    "small real",
                    "zero",
                    "one",
                    "real",
                    "one byte",
                    "two bytes",
                    "three bytes",
                    "four bytes",
                    "six bytes",
                    "eight bytes",
                ],
            ),
            (
                "SELECT label FROM numbers ORDER BY value DESC LIMIT 4",
                "label",
                &["eight bytes", "six bytes", "four bytes", "three bytes"],
            ),
            (
                "SELECT kind, id FROM events ORDER BY kind DESC, id LIMIT 4",
                "kind|id",
                &["kind-6|18", "kind-6|39", "kind-6|60", "kind-6|81"],
            ),
            (
                "SELECT kind, id FROM events ORDER BY kind, id DESC LIMIT 3",
                "kind|id",
                &["kind-0|4494", "kind-0|4473", "kind-0|4452"],
            ),
            (
                "SELECT id FROM events ORDER BY 1 DESC LIMIT 2",
                "id",
                &["4500", "4497"],
            ),
            (
                "SELECT id AS n FROM events ORDER BY n DESC LIMIT 2",
                "n",
                &["4500", "4497"],
            ),
            (
                "SELECT id, id % 5 AS r FROM events ORDER BY r DESC, 1 LIMIT 3",
                "id|r",
                &["9|4", "24|4", "39|4"],
            ),
            (
                "SELECT label FROM numbers ORDER BY length(label), label LIMIT 5",
                "label",
                &["one", "real", "zero", "one byte", "minus one"],
            ),
            (
                "SELECT title FROM documents ORDER BY length(body) DESC LIMIT 2",
                "title",
                &["len-3000", "unicode"],
            ),
            (
                "SELECT id FROM events ORDER BY -id LIMIT 2",
                "id",
                &["4500", "4497"],
            ),
            (
                "SELECT id FROM events WHERE kind = 'kind-0' ORDER BY payload DESC LIMIT 3",
                "id",
                &["2982", "2961", "2940"],
            ),
            (
                "SELECT id FROM events ORDER BY events.id % 500, id DESC LIMIT 4;",
                "id",
                &["4500", "3000", "1500", "3501"],
            ),
            (
                "SELECT id FROM events ORDER BY 1 + 1, id DESC LIMIT 1",
                "id",
                &["4500"],
            ),
        ]);
    }

    #[test]
    fn distinct_keeps_one_of_the_rows_that_show_the_same() {
        same(&[
            (
                "SELECT DISTINCT kind FROM events",
                "kind",
                &[
                    "kind-1", "kind-2", "kind-3", "kind-4", "kind-5", "kind-6", "kind-0",
                ],
            ),
            (
                "SELECT DISTINCT kind FROM events ORDER BY kind DESC LIMIT 3",
                "kind",
                &["kind-6", "kind-5", "kind-4"],
            ),
            (
                "SELECT DISTINCT id % 2, kind FROM events ORDER BY 1, 2 LIMIT 3",
                "id % 2|kind",
                &["0|kind-0", "0|kind-1", "0|kind-2"],
            ),
            (
                "SELECT DISTINCT typeof(value) FROM numbers ORDER BY 1",
                "typeof(value)",
                &["integer", "real"],
            ),
            (
                "SELECT DISTINCT abs(value) FROM numbers WHERE value BETWEEN -1 AND 2 ORDER BY 1",
                "abs(value)",
                &["0", "1.5e-7", "1", "2.0"],
            ),
            (
                "SELECT DISTINCT coalesce(pinned, 1.0) FROM notes",
                "coalesce(pinned, 1.0)",
                &["1.0"],
            ),
            (
                "SELECT DISTINCT kind FROM events LIMIT 3 OFFSET 2",
                "kind",
                &["kind-3", "kind-4", "kind-5"],
            ),
            (
                "SELECT DISTINCT age IS NULL, note IS NULL FROM people",
                "age IS NULL|note IS NULL",
                &["0|1", "1|0", "0|0"],
            ),
            (
                "SELECT ALL kind FROM events LIMIT 2",
                "kind",
                &["kind-1", "kind-2"],
            ),
            ("SELECT DISTINCT 1 FROM events", "1", &["1"]),
            (
                "SELECT DISTINCT * FROM tags",
                "tag_id|label",
                &["5|five", "9|nine"],
            ),
        ]);
    }

    #[test]
    fn limit_and_offset_cut_the_answer_after_it_is_sorted() {
        same(&[
            ("SELECT id FROM events LIMIT 3", "id", &["3", "6", "9"]),
            (
                "SELECT id FROM events LIMIT 3 OFFSET 2",
                "id",
                &["9", "12", "15"],
            ),
            ("SELECT id FROM events LIMIT 2, 3", "id", &["9", "12", "15"]),
            ("SELECT id FROM events LIMIT 0", "id", &[]),
            (
                "SELECT id FROM events LIMIT 5 OFFSET 1498",
                "id",
                &["4497", "4500"],
            ),
            (
                "SELECT id FROM events LIMIT -1 OFFSET 1498",
                "id",
                &["4497", "4500"],
            ),
            ("SELECT id FROM events LIMIT 2 OFFSET -5", "id", &["3", "6"]),
            (
                "SELECT id FROM events ORDER BY id DESC LIMIT 2 OFFSET 1",
                "id",
                &["4497", "4494"],
            ),
            (
                "SELECT age FROM people LIMIT 10",
                "age",
                &["36", "NULL", "0"],
            ),
            ("SELECT age FROM people LIMIT 10 OFFSET 5", "age", &[]),
            (
                "SELECT id FROM events WHERE id > 4490 LIMIT 2",
                "id",
                &["4491", "4494"],
            ),
            (
                "SELECT id FROM events WHERE kind = 'kind-6' LIMIT 2 OFFSET 212",
                "id",
                &["4470", "4491"],
            ),
            ("SELECT count(*) FROM events LIMIT 0", "count(*)", &[]),
            (
                "SELECT count(*) FROM events LIMIT 1 OFFSET 1",
                "count(*)",
                &[],
            ),
            ("SELECT count(*) FROM events LIMIT 5", "count(*)", &["1500"]),
        ]);
    }

    #[test]
    fn aggregates_answer_in_one_row_for_the_rows_that_pass_where() {
        same(&[
            ("SELECT count(*) FROM events", "count(*)", &["1500"]),
            (
                "SELECT count(*), count(age), count(note), count(photo) FROM people",
                "count(*)|count(age)|count(note)|count(photo)",
                &["3|2|2|2"],
            ),
            (
                "SELECT sum(age), avg(age), min(age), max(age), total(age) FROM people",
                "sum(age)|avg(age)|min(age)|max(age)|total(age)",
                &["36|18.0|0|36|36.0"],
            ),
            (
                "SELECT sum(id), avg(id), min(id), max(id), total(id) FROM events",
                "sum(id)|avg(id)|min(id)|max(id)|total(id)",
                &["3377250|2251.5|3|4500|3377250.0"],
            ),
            (
                "SELECT count(*), sum(id) FROM events WHERE kind = 'kind-3'",
                "count(*)|sum(id)",
                &["214|480537"],
            ),
            (
                "SELECT min(kind), max(kind), min(payload), max(payload) FROM events",
                "min(kind)|max(kind)|min(payload)|max(payload)",
                &["kind-0|kind-6|event number 1|event number 999"],
            ),
            (
                "SELECT sum(score), min(score), max(score) FROM people",
                "sum(score)|min(score)|max(score)",
                &["1e300|-0.25|1e300"],
            ),
            (
                "SELECT avg(score) FROM people WHERE id < 40",
                "avg(score)",
                &["4.625"],
            ),
            (
                "SELECT min(value), max(value) FROM numbers",
                "min(value)|max(value)",
                &["-9223372036854775808|9223372036854775807"],
            ),
            (
                "SELECT sum(value) FROM numbers WHERE label LIKE 'one%'",
                "sum(value)",
                &["0"],
            ),
            (
                "SELECT sum(value), typeof(sum(value)) FROM numbers WHERE value BETWEEN 0 AND 2",
                "sum(value)|typeof(sum(value))",
                &["3.0|real"],
            ),
            (
                "SELECT sum(value), total(value) FROM numbers WHERE label LIKE 'eight%'",
                "sum(value)|total(value)",
                &["-1|-1.0"],
            ),
            (
                "SELECT avg(value) FROM numbers WHERE value BETWEEN -128 AND 127",
                "avg(value)",
                &["0.14285712142857143"],
            ),
            (
                "SELECT COUNT(*) AS n, Max(label) FROM tags",
                "n|Max(label)",
                &["2|nine"],
            ),
            ("SELECT count(*) FROM people ORDER BY 1", "count(*)", &["3"]),
            (
                "SELECT count(*) FROM events WHERE kind LIKE '%-0'",
                "count(*)",
                &["214"],
            ),
            (
                "SELECT max(photo), min(photo), count(photo), max(note), min(note) FROM people",
                "max(photo)|min(photo)|count(photo)|max(note)|min(note)",
                &["BLOB (3 bytes)|BLOB (0 bytes)|2|héllo|"],
            ),
            (
                "SELECT min(coalesce(note, age)), max(coalesce(note, age)) FROM people",
                "min(coalesce(note, age))|max(coalesce(note, age))",
                &["36|héllo"],
            ),
        ]);
    }

    #[test]
    fn aggregates_of_no_row_are_null_but_count_and_total() {
        same(&[
            (
                "SELECT sum(a), avg(a), min(a), max(a), total(a), count(a), count(*) FROM empty",
                "sum(a)|avg(a)|min(a)|max(a)|total(a)|count(a)|count(*)",
                &["NULL|NULL|NULL|NULL|0.0|0|0"],
            ),
            (
                "SELECT count(*), sum(id), avg(id), min(id), max(id), total(id) FROM events \
                 WHERE id < 0",
                "count(*)|sum(id)|avg(id)|min(id)|max(id)|total(id)",
                &["0|NULL|NULL|NULL|NULL|0.0"],
            ),
            (
                "SELECT sum(note), count(note), total(note) FROM people WHERE id = 1",
                "sum(note)|count(note)|total(note)",
                &["NULL|0|0.0"],
            ),
        ]);
    }

    #[test]
    fn aggregates_take_worked_out_values_and_are_worked_with_in_turn() {
        same(&[
            (
                "SELECT max(id) - min(id) + 1, count(*) * 2 AS twice, 'n=' || count(*) FROM \
                 events",
                "max(id) - min(id) + 1|twice|'n=' || count(*)",
                &["4498|3000|n=1500"],
            ),
            (
                "SELECT sum(id * 2), count(id % 2 = 0), sum(id % 2 = 0), max(length(payload)) \
                 FROM events",
                "sum(id * 2)|count(id % 2 = 0)|sum(id % 2 = 0)|max(length(payload))",
                &["6754500|1500|750|17"],
            ),
            (
                "SELECT sum(age + 1), sum(coalesce(age, 100)), avg(age * 1.0), abs(min(score)) \
                 FROM people",
                "sum(age + 1)|sum(coalesce(age, 100))|avg(age * 1.0)|abs(min(score))",
                &["38|136|18.0|0.25"],
            ),
            (
                "SELECT count(*) > 1000, typeof(avg(id)), typeof(sum(id)), typeof(total(id)), 1 \
                 FROM events",
                "count(*) > 1000|typeof(avg(id))|typeof(sum(id))|typeof(total(id))|1",
                &["1|real|integer|real|1"],
            ),
            (
                "SELECT sum('3 apples'), sum(kind), max(substr(kind, 6) + 0) FROM events",
                "sum('3 apples')|sum(kind)|max(substr(kind, 6) + 0)",
                &["4500.0|0.0|6"],
            ),
            (
                "SELECT count(NULL), count(1), sum(1), max(1), min(NULL) FROM people",
                "count(NULL)|count(1)|sum(1)|max(1)|min(NULL)",
                &["0|3|3|1|NULL"],
            ),
            (
                "SELECT sum(' 5 '), typeof(sum(' 5 ')), typeof(sum('5.0')), typeof(sum('5x'))",
                "sum(' 5 ')|typeof(sum(' 5 '))|typeof(sum('5.0'))|typeof(sum('5x'))",
                &["5|integer|real|real"],
            ),
        ]);
    }

    #[test]
    fn a_star_over_a_table_is_what_the_reader_of_the_file_gives() {
        let database = sample();
        for table in database.tables() {
            if table.name == "settings" {
                continue;
            }
            let query = format!("SELECT * FROM \"{}\"", table.name.replace('"', "\"\""));
            let answer = ask(&database, &query, usize::MAX);
            assert_eq!(answer.columns, table.columns, "{query}");
            assert_eq!(
                Ok(answer.rows),
                database.rows(table, 0, usize::MAX),
                "{query}"
            );
        }
    }

    #[test]
    fn a_number_is_never_equal_to_a_text_whatever_the_type_of_the_column() {
        // SQLite turns '36' into a number for a column of numbers, and answers 1, 1 and 0.
        assert_eq!(rows("SELECT count(*) FROM people WHERE age = '36'"), ["0"]);
        assert_eq!(rows("SELECT count(*) FROM people WHERE id = '1'"), ["0"]);
        assert_eq!(rows("SELECT count(*) FROM people WHERE age < '0'"), ["2"]);
        // As SQLite does when neither side is a column.
        assert_eq!(rows("SELECT '36' = 36, 36 < '36', 'a' > 1e300"), ["0|1|1"]);
        // Arithmetic does read the number of a text: this is how to compare them.
        let read = "SELECT count(*) FROM people WHERE age = '36' + 0";
        assert_eq!(rows(read), ["1"]);
    }

    #[test]
    fn of_a_blob_only_the_length_is_known() {
        let shown = "SELECT photo, typeof(photo), length(photo), photo IS NULL FROM people";
        assert_eq!(
            rows(shown),
            [
                "BLOB (3 bytes)|blob|3|0",
                "NULL|null|NULL|1",
                "BLOB (0 bytes)|blob|0|0"
            ]
        );
        // Where SQLite would work on its bytes, the answer is NULL.
        let worked = "SELECT photo || 'x', photo LIKE '%', photo + 1, NOT photo, upper(photo), \
                      substr(photo, 1), abs(photo) FROM people WHERE id = 1";
        assert_eq!(rows(worked), ["NULL|NULL|NULL|NULL|NULL|NULL|NULL"]);
        assert_eq!(rows("SELECT count(*) FROM people WHERE photo"), ["0"]);
        let summed = "SELECT sum(photo), total(photo), avg(photo) FROM people";
        assert_eq!(rows(summed), ["NULL|0.0|NULL"]);
        // Blobs stand against each other by length, and after everything else.
        let longer = "SELECT id FROM people WHERE photo > photo - photo OR photo > 'zzz'";
        assert_eq!(rows(longer), ["1", "40"]);
        assert_eq!(
            rows("SELECT photo = photo FROM people WHERE id = 40"),
            ["1"]
        );
    }

    #[test]
    fn a_real_turned_into_text_is_written_as_the_grid_shows_it() {
        // SQLite writes the last one 1.0e+300, eight characters long.
        assert_eq!(
            rows("SELECT score || '', length(score) FROM people"),
            ["9.5|3", "-0.25|5", "1e300|5"]
        );
    }

    #[test]
    fn a_name_between_double_quotes_is_never_a_text() {
        // SQLite falls back on the text 'Bob' when no column has the name.
        let refusal = refusal("SELECT id FROM people WHERE \"full name\" = \"Bob\"");
        assert!(
            refusal.starts_with("there is no column Bob in people"),
            "{refusal}"
        );
    }

    #[test]
    fn most_caps_the_rows_kept_and_cut_tells_when_it_left_some_out() {
        let database = sample();
        let kept = |query: &str, most: usize| {
            let answer = ask(&database, query, most);
            (written(&answer).join(" "), answer.cut)
        };
        assert_eq!(
            kept("SELECT id FROM people", 3),
            ("1 2 40".to_owned(), false)
        );
        assert_eq!(kept("SELECT id FROM people", 2), ("1 2".to_owned(), true));
        assert_eq!(kept("SELECT id FROM people", 0), (String::new(), true));
        assert_eq!(kept("SELECT a FROM empty", 0), (String::new(), false));
        assert_eq!(
            kept("SELECT id FROM events", 4),
            ("3 6 9 12".to_owned(), true)
        );

        // What LIMIT leaves out is not cut: it was not asked for.
        assert_eq!(
            kept("SELECT id FROM events LIMIT 2", 2),
            ("3 6".to_owned(), false)
        );
        assert_eq!(
            kept("SELECT id FROM events LIMIT 2", 9),
            ("3 6".to_owned(), false)
        );
        assert_eq!(
            kept("SELECT id FROM events LIMIT 3", 2),
            ("3 6".to_owned(), true)
        );
        assert_eq!(
            kept("SELECT id FROM events LIMIT 0", 0),
            (String::new(), false)
        );
        let last = "SELECT id FROM events LIMIT 100 OFFSET 1497";
        assert_eq!(kept(last, 3), ("4494 4497 4500".to_owned(), false));
        assert_eq!(kept(last, 2), ("4494 4497".to_owned(), true));

        let sorted = "SELECT id FROM events ORDER BY id DESC";
        assert_eq!(kept(sorted, 2), ("4500 4497".to_owned(), true));
        assert_eq!(
            kept(&format!("{sorted} LIMIT 2"), 2),
            ("4500 4497".to_owned(), false)
        );
        assert_eq!(
            kept(&format!("{sorted} LIMIT 3 OFFSET 1"), 2),
            ("4497 4494".to_owned(), true)
        );

        let kinds = "SELECT DISTINCT substr(kind, 6) FROM events";
        assert_eq!(kept(kinds, 7), ("1 2 3 4 5 6 0".to_owned(), false));
        assert_eq!(kept(kinds, 6), ("1 2 3 4 5 6".to_owned(), true));
        assert_eq!(
            kept("SELECT count(*) FROM events", 1),
            ("1500".to_owned(), false)
        );
        assert_eq!(
            kept("SELECT count(*) FROM events", 0),
            (String::new(), true)
        );
    }

    #[test]
    fn a_question_in_no_order_reads_no_further_than_the_rows_it_keeps() {
        // The rows of this table come round again and again until reading it is refused:
        // an answer proves that no more than `readable` rows were read.
        let database = endless_events();
        let events = database
            .tables()
            .iter()
            .find(|table| table.name == "events");
        let events = events.expect("there is events");
        let (mut readable, mut refused) = (1500, 1 << 24);
        while refused - readable > 1 {
            let middle = readable + (refused - readable) / 2;
            match database.rows(events, middle - 1, 1) {
                Ok(_) => readable = middle,
                Err(_) => refused = middle,
            }
        }
        let ids = |query: &str, most: usize| {
            select(&database, query, most).map(|answer| written(&answer).join(" "))
        };
        let whole = ids("SELECT count(*) FROM events", 10);
        assert!(whole.is_err_and(|error| error.contains("back to themselves")));
        assert!(ids("SELECT id FROM events", usize::MAX).is_err());
        // Sorting needs every row.
        assert!(ids("SELECT id FROM events ORDER BY id LIMIT 3", 10).is_err());

        assert_eq!(
            ids("SELECT id FROM events LIMIT 3", 10),
            Ok("3 6 9".to_owned())
        );
        assert_eq!(ids("SELECT id FROM events", 3), Ok("3 6 9".to_owned()));
        let picked = "SELECT id FROM events WHERE kind = 'kind-0' LIMIT 3";
        assert_eq!(ids(picked, 10), Ok("21 42 63".to_owned()));
        let kinds = ids("SELECT DISTINCT kind FROM events LIMIT 7", 10);
        assert!(kinds.is_ok_and(|kinds| kinds.ends_with("kind-0")));

        // To the very last row that can be read, and not one further.
        let last = |limit: usize, back: usize| {
            format!(
                "SELECT id FROM events LIMIT {limit} OFFSET {}",
                readable - back
            )
        };
        assert!(ids(&last(2, 2), 10).is_ok());
        assert!(ids(&last(3, 2), 10).is_err());
        // One row past `most` is read to tell that the answer is cut, when LIMIT lets it be.
        assert!(ids(&last(2, 2), 1).is_ok());
        assert!(ids(&last(2, 1), 1).is_err());
        assert!(ids(&last(1, 1), 1).is_ok());
    }

    #[test]
    fn a_table_whose_rows_cannot_be_read_says_so() {
        let unreadable = "settings is a WITHOUT ROWID table: its rows cannot be read here.";
        assert_eq!(refusal("SELECT * FROM settings"), unreadable);
        assert_eq!(refusal("SELECT count(*) FROM settings"), unreadable);
        // Its columns are known all the same.
        let answer = ask(&sample(), "SELECT * FROM settings LIMIT 0", 10);
        assert_eq!(answer.columns, ["key", "value"]);
    }

    #[test]
    fn a_sum_too_large_for_a_whole_number_is_refused_as_sqlite_refuses_it() {
        let large = "FROM numbers WHERE typeof(value) = 'integer' AND value > 0";
        assert_eq!(
            refusal(&format!("SELECT sum(value) {large}")),
            "sum went past the largest whole number: total gives it as a real"
        );
        assert_eq!(
            rows(&format!("SELECT total(value), avg(value) > 1e18 {large}")),
            ["9.223512776499036e18|1"]
        );
    }

    #[test]
    fn what_writes_is_refused_as_not_possible() {
        for (query, word) in [
            ("INSERT INTO people VALUES (1)", "INSERT"),
            ("update people set age = 1", "UPDATE"),
            ("Delete FROM people", "DELETE"),
            ("REPLACE INTO tags VALUES (1, 'x')", "REPLACE"),
            ("CREATE TABLE t (a)", "CREATE"),
            ("DROP TABLE people", "DROP"),
            ("ALTER TABLE people ADD COLUMN x", "ALTER"),
            ("PRAGMA table_info(people)", "PRAGMA"),
            ("VACUUM", "VACUUM"),
            ("BEGIN; DELETE FROM people; COMMIT", "BEGIN"),
        ] {
            let expected = format!("this only reads: {word} is not possible");
            assert_eq!(refusal(query), expected);
        }
    }

    #[test]
    fn what_does_not_start_with_select_is_refused() {
        let empty = "there is no question: write one that starts with SELECT";
        assert_eq!(refusal(""), empty);
        assert_eq!(refusal("  \n -- nothing\n /* at all */ "), empty);
        assert_eq!(refusal("people"), "near \"people\": SELECT was expected");
        assert_eq!(refusal("(SELECT 1)"), "near \"(\": SELECT was expected");
        assert_eq!(refusal("; SELECT 1"), "near \";\": SELECT was expected");
        let starts = "is not understood here: a question starts with SELECT";
        assert_eq!(
            refusal("WITH x AS (SELECT 1) SELECT * FROM x"),
            format!("WITH {starts}")
        );
        assert_eq!(refusal("EXPLAIN SELECT 1"), format!("EXPLAIN {starts}"));
        assert_eq!(refusal("values (1)"), format!("VALUES {starts}"));
    }

    #[test]
    fn a_misspelt_question_says_near_what_and_what_was_expected() {
        for (query, expected) in [
            ("SELECT * FORM people", "near \"FORM\": FROM was expected"),
            ("SELECT id FORM people", "near \"FORM\": FROM was expected"),
            (
                "SELECT id name FROM people",
                "near \"name\": a comma or FROM was expected",
            ),
            ("SELECT FROM people", "near \"FROM\": a value was expected"),
            (
                "SELECT id, FROM people",
                "near \"FROM\": a value was expected",
            ),
            (
                "SELECT id AS FROM people",
                "near \"FROM\": a name after AS was expected",
            ),
            ("SELECT (1 + 2 FROM people", "near \"FROM\": ) was expected"),
            (
                "SELECT * FROM people WERE id = 1",
                "near \"WERE\": the end of the question was expected",
            ),
            (
                "SELECT * FROM people WHERE id = 1 2",
                "near \"2\": the end of the question was expected",
            ),
            (
                "SELECT * FROM people WHERE id = = 1",
                "near \"=\": a value was expected",
            ),
            (
                "SELECT * FROM people WHERE id IN 1",
                "near \"1\": ( was expected",
            ),
            (
                "SELECT * FROM people WHERE id IN (1, 2",
                "the question stops too soon: ) was expected",
            ),
            (
                "SELECT * FROM people WHERE id BETWEEN 1 OR 2",
                "near \"OR\": AND was expected",
            ),
            (
                "SELECT * FROM people WHERE id IS DISTINCT FROM 1",
                "near \"DISTINCT\": a value was expected",
            ),
            (
                "SELECT * FROM people ORDER id",
                "near \"id\": BY was expected",
            ),
            (
                "SELECT * FROM people ORDER BY id NULLS LAST",
                "near \"NULLS\": the end of the question was expected",
            ),
            (
                "SELECT * FROM people LIMIT x",
                "near \"x\": a whole number after LIMIT was expected",
            ),
            (
                "SELECT * FROM people LIMIT 1.5",
                "near \"1.5\": a whole number after LIMIT was expected",
            ),
            (
                "SELECT * FROM people LIMIT 1 OFFSET",
                "the question stops too soon: a whole number after OFFSET was expected",
            ),
            (
                "SELECT * FROM",
                "the question stops too soon: the name of a table was expected",
            ),
            (
                "SELECT * FROM WHERE id = 1",
                "near \"WHERE\": the name of a table was expected",
            ),
            (
                "SELECT * FROM 'people'",
                "near \"'people'\": the name of a table was expected",
            ),
            (
                "SELECT",
                "the question stops too soon: a value was expected",
            ),
            (
                "SELECT 1 +",
                "the question stops too soon: a value was expected",
            ),
            (
                "SELECT * FROM people WHERE",
                "the question stops too soon: a value was expected",
            ),
            (
                "SELECT * FROM people; SELECT 1",
                "one question at a time: there is more after the ;",
            ),
            (
                "SELECT * FROM people;;",
                "one question at a time: there is more after the ;",
            ),
        ] {
            assert_eq!(refusal(query), expected, "{query}");
        }
    }

    #[test]
    fn a_quote_never_closed_and_a_sign_not_known_are_refused() {
        assert_eq!(
            refusal("SELECT 'abc FROM people"),
            "the ' opened here is never closed: 'abc FROM people"
        );
        assert_eq!(
            refusal("SELECT \"id FROM people"),
            "the \" opened here is never closed: \"id FROM people"
        );
        assert_eq!(
            refusal("SELECT [id FROM people"),
            "the [ opened here is never closed: [id FROM people"
        );
        assert_eq!(
            refusal("SELECT `id"),
            "the ` opened here is never closed: `id"
        );
        assert_eq!(
            refusal("SELECT id FROM people WHERE id = ?"),
            "near \"?\": this sign is not understood"
        );
        assert_eq!(
            refusal("SELECT 1 & 2"),
            "near \"&\": this sign is not understood"
        );
        assert_eq!(
            refusal("SELECT {id}"),
            "near \"{\": this sign is not understood"
        );
    }

    #[test]
    fn a_table_that_is_not_there_is_named_with_the_ones_there_are() {
        let there_are = "there are people, numbers, events, documents, settings, prices, tags, \
                         notes, odd \"names\", empty";
        assert_eq!(
            refusal("SELECT * FROM peple"),
            format!("there is no table peple: {there_are}")
        );
        // Neither a view nor what SQLite keeps for itself is a table to read here.
        assert_eq!(
            refusal("SELECT count(*) FROM adults WHERE 1"),
            format!("there is no table adults: {there_are}")
        );
        assert_eq!(
            refusal("SELECT * FROM sqlite_master"),
            format!("there is no table sqlite_master: {there_are}")
        );
        assert_eq!(
            refusal("SELECT * FROM \"odd names\""),
            format!("there is no table odd names: {there_are}")
        );
    }

    #[test]
    fn a_column_that_is_not_there_is_named_with_the_ones_there_are() {
        let there_are = "in people: there are id, full name, age, score, photo, note";
        for (query, name) in [
            ("SELECT nme FROM people", "nme"),
            ("SELECT id FROM people WHERE aeg > 1", "aeg"),
            ("SELECT id FROM people ORDER BY nam", "nam"),
            ("SELECT length(full_name) FROM people", "full_name"),
            ("SELECT count(agee) FROM people", "agee"),
            ("SELECT people.label FROM people", "label"),
            ("SELECT rowid FROM people", "rowid"),
        ] {
            assert_eq!(
                refusal(query),
                format!("there is no column {name} {there_are}"),
                "{query}"
            );
        }
        assert_eq!(
            refusal("SELECT c FROM empty"),
            "there is no column c in empty: there are a, b"
        );
        assert_eq!(
            refusal("SELECT tags.label FROM people"),
            "tags is not the table named after FROM, people: one table at a time"
        );
        assert_eq!(
            refusal("SELECT people.id FROM people AS p"),
            "people is not the table named after FROM, p: one table at a time"
        );
        assert_eq!(
            refusal("SELECT id"),
            "there is no column id: no table is named, FROM is missing"
        );
        assert_eq!(
            refusal("SELECT 1 WHERE id = 1"),
            "there is no column id: no table is named, FROM is missing"
        );
        assert_eq!(
            refusal("SELECT *"),
            "* stands for the columns of a table: FROM is missing"
        );
        assert_eq!(
            refusal("SELECT id FROM people ORDER BY 2"),
            "ORDER BY 2: the columns shown are numbered from 1 to 1"
        );
        assert_eq!(
            refusal("SELECT * FROM tags ORDER BY 0"),
            "ORDER BY 0: the columns shown are numbered from 1 to 2"
        );
    }

    #[test]
    fn several_tables_are_refused_one_table_at_a_time() {
        let join = "JOIN is not understood here: one table at a time";
        for query in [
            "SELECT * FROM people JOIN tags ON people.id = tags.tag_id",
            "SELECT * FROM people LEFT JOIN tags ON 1",
            "SELECT * FROM people left outer join tags USING (id)",
            "SELECT * FROM people INNER JOIN tags",
            "SELECT * FROM people CROSS JOIN tags",
            "SELECT * FROM people NATURAL JOIN tags",
            "SELECT * FROM people AS p JOIN tags AS t ON p.id = t.tag_id WHERE p.id = 1",
        ] {
            assert_eq!(refusal(query), join, "{query}");
        }
        assert_eq!(
            refusal("SELECT * FROM people, tags"),
            "several tables are not understood here: one table at a time"
        );
    }

    #[test]
    fn a_select_inside_another_and_selects_joined_together_are_refused() {
        let inside = "a SELECT inside another is not understood here: one SELECT at a time";
        assert_eq!(refusal("SELECT (SELECT 1) FROM people"), inside);
        assert_eq!(
            refusal("SELECT id FROM people WHERE id IN (SELECT tag_id FROM tags)"),
            inside
        );
        assert_eq!(
            refusal("SELECT id FROM people WHERE id = (select max(id) from people)"),
            inside
        );
        assert_eq!(
            refusal("SELECT * FROM (SELECT 1)"),
            "a SELECT inside another is not understood here: FROM takes the name of a table"
        );
        assert_eq!(
            refusal("SELECT id FROM people WHERE EXISTS (SELECT 1)"),
            "EXISTS is not understood here"
        );
        let one = "is not understood here: one SELECT at a time";
        assert_eq!(refusal("SELECT 1 UNION SELECT 2"), format!("UNION {one}"));
        assert_eq!(
            refusal("SELECT id FROM people UNION ALL SELECT tag_id FROM tags"),
            format!("UNION {one}")
        );
        assert_eq!(
            refusal("SELECT id FROM people except SELECT 1"),
            format!("EXCEPT {one}")
        );
        assert_eq!(
            refusal("SELECT id FROM people INTERSECT SELECT 1"),
            format!("INTERSECT {one}")
        );
    }

    #[test]
    fn group_by_and_what_goes_with_it_are_refused() {
        assert_eq!(
            refusal("SELECT count(*) FROM events GROUP BY kind"),
            "GROUP BY is not understood here: count, sum and the like answer for all the rows"
        );
        assert_eq!(
            refusal("SELECT count(*) FROM events WHERE id > 5 group by kind ORDER BY 1"),
            "GROUP BY is not understood here: count, sum and the like answer for all the rows"
        );
        assert_eq!(
            refusal("SELECT count(*) FROM events HAVING count(*) > 1"),
            "HAVING is not understood here: there is no GROUP BY for it"
        );
        let windows = "is not understood here: window functions are not";
        assert_eq!(
            refusal("SELECT count(*) OVER () FROM people"),
            format!("OVER {windows}")
        );
        assert_eq!(
            refusal("SELECT count(*) FILTER (WHERE id > 1) FROM people"),
            format!("FILTER {windows}")
        );
    }

    #[test]
    fn a_column_beside_an_aggregate_is_refused() {
        let beside = "cannot stand beside count, sum and the like: they answer for all the \
                      rows at once, and GROUP BY is not understood here";
        for (query, loose) in [
            ("SELECT id, count(*) FROM people", "id"),
            ("SELECT kind, count(*) FROM events GROUP BY kind", "kind"),
            ("SELECT count(*), * FROM people", "*"),
            ("SELECT max(age) + age FROM people", "age"),
            ("SELECT max(age), upper(note) FROM people", "note"),
        ] {
            assert_eq!(refusal(query), format!("{loose} {beside}"), "{query}");
        }
        let only = "answers for all the rows at once: it can only stand in the list after SELECT";
        assert_eq!(
            refusal("SELECT id FROM people WHERE count(*) > 1"),
            format!("count {only}")
        );
        assert_eq!(
            refusal("SELECT id FROM people ORDER BY max(id)"),
            format!("max {only}")
        );
        assert_eq!(
            refusal("SELECT max(count(*)) FROM people"),
            "count cannot stand inside another aggregate"
        );
        assert_eq!(
            refusal("SELECT count(DISTINCT kind) FROM events"),
            "DISTINCT inside count is not understood here"
        );
        assert_eq!(
            refusal("SELECT max(id, 5) FROM people"),
            "max takes 1 value, over all the rows: several are not understood here"
        );
        assert_eq!(
            refusal("SELECT sum(*) FROM people"),
            "near \"*\": a value was expected"
        );
    }

    #[test]
    fn parts_of_sql_that_are_not_understood_are_named() {
        for (query, expected) in [
            (
                "SELECT CASE WHEN age > 1 THEN 1 ELSE 0 END FROM people",
                "CASE is not understood here",
            ),
            (
                "SELECT CAST(age AS TEXT) FROM people",
                "CAST is not understood here",
            ),
            (
                "SELECT id FROM people WHERE note LIKE 'a!%' ESCAPE '!'",
                "ESCAPE is not understood here",
            ),
            (
                "SELECT id FROM people WHERE note GLOB 'a*'",
                "GLOB is not understood here",
            ),
            (
                "SELECT id FROM people WHERE note REGEXP 'a'",
                "REGEXP is not understood here",
            ),
            (
                "SELECT id FROM people ORDER BY note COLLATE NOCASE",
                "COLLATE is not understood here",
            ),
        ] {
            assert_eq!(refusal(query), expected, "{query}");
        }
    }

    #[test]
    fn a_function_not_known_or_handed_too_few_or_too_many_values_is_refused() {
        assert_eq!(
            refusal("SELECT round(score) FROM people"),
            "the function round is not known here: there are length, lower, upper, abs, \
             coalesce, ifnull, typeof, substr, substring, and count, sum, avg, min, max, total \
             over all the rows"
        );
        for (query, expected) in [
            ("SELECT length() FROM people", "length takes 1 value, not 0"),
            (
                "SELECT upper(note, 1) FROM people",
                "upper takes 1 value, not 2",
            ),
            ("SELECT substr('a')", "substr takes 2 to 3 values, not 1"),
            (
                "SELECT SUBSTR('a', 1, 2, 3)",
                "substr takes 2 to 3 values, not 4",
            ),
            (
                "SELECT coalesce(1)",
                "coalesce takes 2 values or more, not 1",
            ),
            ("SELECT ifnull(1, 2, 3)", "ifnull takes 2 values, not 3"),
            (
                "SELECT typeof(",
                "the question stops too soon: a value was expected",
            ),
            (
                "SELECT typeof(1",
                "the question stops too soon: ) was expected",
            ),
        ] {
            assert_eq!(refusal(query), expected, "{query}");
        }
    }

    #[test]
    fn a_question_nested_too_deep_or_too_long_is_refused_without_harm() {
        let too_deep =
            "the question nests too deep: no more than 40 brackets, NOT or - inside one another";
        let brackets = |depth: usize| format!("SELECT {}1{}", "(".repeat(depth), ")".repeat(depth));
        assert_eq!(rows(&brackets(40)), ["1"]);
        assert_eq!(refusal(&brackets(41)), too_deep);
        assert_eq!(refusal(&brackets(100_000)), too_deep);
        assert_eq!(
            refusal(&format!("SELECT {}", "(".repeat(100_000))),
            too_deep
        );
        assert_eq!(
            refusal(&format!("SELECT {}1", "NOT ".repeat(100_000))),
            too_deep
        );
        assert_eq!(
            refusal(&format!("SELECT {}1", "- ".repeat(100_000))),
            too_deep
        );
        assert_eq!(
            refusal(&format!("SELECT {}x", "-+".repeat(100_000))),
            too_deep
        );
        let calls = format!("SELECT {}1{}", "abs(".repeat(100_000), ")".repeat(100_000));
        assert_eq!(refusal(&calls), too_deep);
        assert_eq!(rows(&format!("SELECT {}1", "NOT ".repeat(40))), ["1"]);

        let too_long = "the question is too long: no more than 250 operators";
        assert_eq!(rows(&format!("SELECT 1{}", " + 1".repeat(250))), ["251"]);
        assert_eq!(
            refusal(&format!("SELECT 1{}", " + 1".repeat(251))),
            too_long
        );
        assert_eq!(
            refusal(&format!("SELECT 1{}", " OR 1".repeat(100_000))),
            too_long
        );
        assert_eq!(
            refusal(&format!("SELECT 1{}", " BETWEEN 0 AND 1".repeat(100_000))),
            too_long
        );

        // A long list is not deep.
        let among: Vec<String> = (0..5000).step_by(2).map(|even| even.to_string()).collect();
        let listed = format!(
            "SELECT count(*) FROM events WHERE id IN ({})",
            among.join(", ")
        );
        assert_eq!(rows(&listed), ["750"]);
    }

    #[test]
    fn a_cut_or_garbled_question_is_answered_or_refused_and_never_panics() {
        let database = sample();
        let questions = [
            "SELECT * FROM people WHERE age >= 18 AND \"full name\" LIKE 'A%' \
             ORDER BY id DESC LIMIT 2 OFFSET 1;",
            "SELECT DISTINCT kind, length(payload) AS n FROM events \
             WHERE id BETWEEN 10 AND 90 ORDER BY 2, kind",
            "SELECT count(*), sum(value), avg(value), min(label), max(value), total(value) \
             FROM numbers",
            "SELECT [first col], `second`, \"fo,ur\" FROM \"odd \"\"names\"\"\" \
             WHERE [second] IS NOT NULL",
            "SELECT id, -age * (score + 1.5e3) / 7 % 3 || 'x''y' FROM people \
             WHERE note IS NULL OR NOT id IN (1, 2.5, 'é')",
            "select upper(substr(title, 1, 3)), coalesce(data, body, 0) from documents -- end\n \
             where typeof(data) <> 'blob' /* or */ limit 3",
            "SELECT p.id, ifnull(p.note, '日本語 😀') FROM people AS p \
             WHERE abs(p.score) < 1e2 ORDER BY p.note DESC, 1 ASC",
            "SELECT tag_id, label FROM tags \
             WHERE label NOT LIKE '%i_e' AND tag_id NOT BETWEEN 1 AND 6 OR NULL",
        ];
        let (mut answered, mut refused) = (0, 0);
        let mut put = |query: &str, most: usize| match select(&database, query, most) {
            Ok(answer) => {
                assert!(answer.rows.len() <= most, "{query}");
                answered += 1;
            }
            Err(refusal) => {
                assert!(!refusal.is_empty(), "{query}");
                refused += 1;
            }
        };
        for question in questions {
            put(question, 1000);
            for (cut, _) in question.char_indices() {
                put(&question[..cut], 5);
                put(&question[cut..], 5);
            }
        }

        // Always the same garbling: a made-up sequence, not chance.
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = |below: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % below as u64) as usize
        };
        let scraps = [
            "SELECT",
            "FROM",
            "WHERE",
            "ORDER",
            "BY",
            "LIMIT",
            "OFFSET",
            "DISTINCT",
            "AS",
            "AND",
            "OR",
            "NOT",
            "IS",
            "NULL",
            "IN",
            "LIKE",
            "BETWEEN",
            "DESC",
            "GROUP",
            "JOIN",
            "UNION",
            "CASE",
            "count",
            "sum",
            "max",
            "substr",
            "length",
            "people",
            "events",
            "numbers",
            "tags",
            "id",
            "age",
            "kind",
            "value",
            "label",
            "\"full name\"",
            "*",
            "(",
            ")",
            ",",
            ".",
            ";",
            "+",
            "-",
            "/",
            "%",
            "||",
            "=",
            "==",
            "!=",
            "<>",
            "<",
            "<=",
            ">",
            ">=",
            "'",
            "\"",
            "`",
            "[",
            "]",
            "--",
            "/*",
            "*/",
            "\n",
            " ",
            "0",
            "1",
            "9223372036854775808",
            "1e999",
            ".5",
            "1.",
            "'a%'",
            "'_'",
            "''",
            "é",
            "😀",
            "\0",
            "?",
            "\\",
            "{",
            "!",
            "|",
            "&",
            "~",
            "x'00'",
        ];
        for _ in 0..10_000 {
            let mut garbled = questions[next(questions.len())].to_owned();
            for _ in 0..=next(4) {
                let mut at = next(garbled.len() + 1);
                while !garbled.is_char_boundary(at) {
                    at -= 1;
                }
                let mut end = (at + next(8)).min(garbled.len());
                while !garbled.is_char_boundary(end) {
                    end -= 1;
                }
                let scrap = scraps[next(scraps.len())];
                match next(3) {
                    0 => garbled.replace_range(at..end, ""),
                    1 => garbled.insert_str(at, scrap),
                    _ => garbled.replace_range(at..end, scrap),
                }
            }
            put(&garbled, next(4));
        }
        for _ in 0..10_000 {
            let mut soup = String::from(if next(4) == 0 { "" } else { "SELECT " });
            for _ in 0..next(14) {
                soup.push_str(scraps[next(scraps.len())]);
                if next(3) > 0 {
                    soup.push(' ');
                }
            }
            put(&soup, next(4));
        }
        // Both ways out were taken: the garbling neither spares nor kills every question.
        assert!(
            answered > 100 && refused > 1000,
            "{answered} answered, {refused} refused"
        );
    }
}
