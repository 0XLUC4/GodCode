//! The Vim and Emacs ways of editing: what a key does to a text and its caret. Places are
//! counted in chars.

use std::collections::HashMap;
use std::ops::Range;

/// A key, as these ways of editing name them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stroke {
    Char(char),
    Ctrl(char),
    Alt(char),
    Escape,
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
}

/// Something a key asks of the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    Undo,
    Redo,
    Find,
    Save,
    Open,
    /// Close the file.
    Close,
    /// Save the file, then close it.
    SaveClose,
}

/// What a key did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Done {
    /// The selection to show; empty for a plain caret.
    pub selection: Range<usize>,
    pub changed: bool,
    /// Text put aside: for the clipboard too.
    pub copied: Option<String>,
    pub ask: Option<Ask>,
}

fn line_start(chars: &[char], at: usize) -> usize {
    let before = chars[..at].iter().rposition(|ch| *ch == '\n');
    before.map_or(0, |found| found + 1)
}

fn line_end(chars: &[char], at: usize) -> usize {
    let after = chars[at..].iter().position(|ch| *ch == '\n');
    after.map_or(chars.len(), |found| at + found)
}

/// The first char of the line of `at` that is not white space.
fn first_text(chars: &[char], at: usize) -> usize {
    let (start, end) = (line_start(chars, at), line_end(chars, at));
    let blank = chars[start..end].iter().take_while(|ch| ch.is_whitespace());
    start + blank.count()
}

/// One line up or down, in the same column when the line is long enough.
fn vertical(chars: &[char], at: usize, down: bool) -> usize {
    let (start, end) = (line_start(chars, at), line_end(chars, at));
    let column = at - start;
    if down {
        if end == chars.len() {
            return at;
        }
        (end + 1 + column).min(line_end(chars, end + 1))
    } else {
        if start == 0 {
            return at;
        }
        (line_start(chars, start - 1) + column).min(start - 1)
    }
}

/// The start of line `number`, counted from 1; the last line when there are fewer.
fn line_numbered(chars: &[char], number: usize) -> usize {
    let mut start = 0;
    for _ in 1..number {
        let end = line_end(chars, start);
        if end == chars.len() {
            break;
        }
        start = end + 1;
    }
    start
}

/// What a char is, for a move word by word: part of a name, white space, or a sign.
fn kind(ch: char) -> u8 {
    if ch.is_alphanumeric() || ch == '_' {
        0
    } else if ch.is_whitespace() {
        1
    } else {
        2
    }
}

/// The start of the next word.
fn word_forward(chars: &[char], at: usize) -> usize {
    let mut at = at;
    if let Some(first) = chars.get(at).map(|ch| kind(*ch)).filter(|kind| *kind != 1) {
        while at < chars.len() && kind(chars[at]) == first {
            at += 1;
        }
    }
    while at < chars.len() && kind(chars[at]) == 1 {
        at += 1;
    }
    at
}

/// The start of the word, or of the one before.
fn word_back(chars: &[char], at: usize) -> usize {
    let mut at = at;
    while at > 0 && kind(chars[at - 1]) == 1 {
        at -= 1;
    }
    if at > 0 {
        let word = kind(chars[at - 1]);
        while at > 0 && kind(chars[at - 1]) == word {
            at -= 1;
        }
    }
    at
}

/// The end of the word, or of the next one: where Emacs stops going forward by words.
fn word_after(chars: &[char], at: usize) -> usize {
    let mut at = at;
    while at < chars.len() && kind(chars[at]) != 0 {
        at += 1;
    }
    while at < chars.len() && kind(chars[at]) == 0 {
        at += 1;
    }
    at
}

/// The last char of the word, or of the next one.
fn word_end(chars: &[char], at: usize) -> usize {
    let mut at = at + 1;
    while at < chars.len() && kind(chars[at]) == 1 {
        at += 1;
    }
    if at >= chars.len() {
        return chars.len().saturating_sub(1);
    }
    let word = kind(chars[at]);
    while at + 1 < chars.len() && kind(chars[at + 1]) == word {
        at += 1;
    }
    at
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    /// Keys are commands.
    #[default]
    Normal,
    /// Keys are typed, as anywhere else.
    Insert,
    Visual,
    /// Whole lines are selected.
    VisualLine,
}

/// What a move covers when an operator follows it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Span {
    /// Up to the char reached.
    Upto,
    /// The char reached too.
    With,
    /// The whole lines between.
    Lines,
}

/// Vim: its mode and what it remembers between keys.
#[derive(Default)]
pub struct Vim {
    pub mode: Mode,
    /// How many times the next command is done; 0 when no number was typed.
    count: usize,
    /// An operator (`d`, `c`, `y`) or a first key (`g`, `r`) that waits for what follows.
    pending: Option<char>,
    /// Where the selection started and where it goes to, in the visual modes.
    anchor: usize,
    head: usize,
    /// The selection last shown: the visual mode ends when something else moved it.
    shown: Range<usize>,
    /// What was last deleted or copied, and whether it is whole lines.
    register: (String, bool),
    /// The registers that have a name: `a` to `z`, `0` for what was last copied, `1` to
    /// `9` for the lines last deleted, the newest first, and `-` for less than a line
    /// deleted. Each holds a text and whether it is whole lines; a macro is kept there as
    /// text too.
    registers: HashMap<char, (String, bool)>,
    /// The register named with `"` for the command that follows.
    named: Option<char>,
    /// `"`, `q` or `@` was typed: the next key names a register.
    prefix: Option<char>,
    /// The register a macro is being recorded into, and the keys typed so far.
    macro_into: Option<char>,
    macro_keys: String,
    /// The register whose macro was last played, for `@@`.
    macro_last: Option<char>,
    /// How many macros are being played, one inside the other, and how many keys they may
    /// still type.
    running: usize,
    keys_left: usize,
    /// Where insert mode was entered and how long the text was then, for a macro to keep
    /// what is typed there.
    typed_from: Option<(usize, usize)>,
    /// A key that waits for a char: `f`, `F`, `t`, `T` for the one to go to, `i` or `a` for
    /// the text object to take, `m`, `'` or a backtick for the name of a mark.
    waiting: Option<char>,
    /// The last `f`, `F`, `t` or `T` and its char, for `;` and `,` to do again.
    found: Option<(char, char)>,
    /// The keys of the command being typed; those of the last one that changed the text,
    /// with what was typed in insert mode after it: what `.` does again.
    recording: Vec<Stroke>,
    last: Vec<Stroke>,
    last_typed: String,
    /// Where insert mode was entered, and how long the text was then.
    insert_from: Option<(usize, usize)>,
    /// `.` is doing the last command again: nothing is recorded meanwhile.
    replaying: bool,
    /// The command being typed after a `:`.
    line: Option<String>,
    /// The places that have a name: `a` to `z` set with `m`, `'` where the caret was
    /// before the last jump, `<` and `>` the two ends of the last selection.
    marks: HashMap<char, usize>,
    /// The pattern and the replacement of the last `:s`, for `&` and a bare `:s`.
    last_substitute: Option<(String, String)>,
    /// What one level of indent is, for `>>` and `<<`: four spaces unless told.
    pub unit: String,
}

impl Vim {
    /// What `key` does, the caret being at the start of `selection`. `None` when the key is
    /// not for Vim: in insert mode, the text field takes it.
    pub fn key(
        &mut self,
        text: &mut String,
        selection: &Range<usize>,
        key: Stroke,
    ) -> Option<Done> {
        let recorded = self.recorded();
        let done = self.press(text, selection, key);
        // Neither the keys that start a recording nor the `q` that ends it are part of it.
        if let (true, true, Some(_), Some(ch)) = (recorded, self.recorded(), &done, key_char(key)) {
            self.macro_keys.push(ch);
        }
        done
    }

    /// The register a macro is being recorded into, to show while it is.
    pub fn recording(&self) -> Option<char> {
        self.macro_into
    }

    /// The keys typed now go into a macro: one is being recorded, and it is the keyboard
    /// that types, not `.` or another macro.
    fn recorded(&self) -> bool {
        self.macro_into.is_some() && self.running == 0 && !self.replaying
    }

    /// What `key` does, whoever types it.
    fn press(&mut self, text: &mut String, selection: &Range<usize>, key: Stroke) -> Option<Done> {
        let mut chars: Vec<char> = text.chars().collect();
        let mut done = Done::default();
        if self.mode == Mode::Insert {
            if key != Stroke::Escape {
                return None;
            }
            let at = selection.start.min(chars.len());
            let since = self.typed_from.take();
            // What was typed or erased there moved the marks after it.
            if let Some((from, length)) = since {
                let low = from.min(at);
                let added = at - low;
                let removed = (added + length).saturating_sub(chars.len());
                edited(&mut self.marks, low, removed, added);
            }
            // What was typed since insert mode was entered: for the macro being recorded,
            // and for `.` to type again.
            if let (Some(since), true) = (since, self.recorded()) {
                self.macro_keys.push_str(&typed(&chars, at, since));
            }
            if let (Some(since), false) = (self.insert_from.take(), self.replaying) {
                self.last_typed = typed(&chars, at, since);
            }
            // Out of insert mode the caret is on a char: the one just typed.
            let at = at.max(line_start(&chars, at) + 1) - 1;
            self.mode = Mode::Normal;
            done.selection = at..at;
            self.shown = done.selection.clone();
            return Some(done);
        }
        // After a `:`, keys write a command, which Enter carries out.
        if let Some(mut typed) = self.line.take() {
            let mut at = selection.start.min(chars.len());
            match key {
                Stroke::Char(ch) => {
                    typed.push(ch);
                    self.line = Some(typed);
                }
                Stroke::Backspace => self.line = typed.pop().map(|_| typed),
                Stroke::Enter => match typed.trim() {
                    "w" => done.ask = Some(Ask::Save),
                    "q" | "q!" => done.ask = Some(Ask::Close),
                    "wq" | "x" => done.ask = Some(Ask::SaveClose),
                    // The spaces at its end may be what `:s` is to write.
                    _ => at = self.ex(&mut chars, at, typed.trim_start(), &mut done),
                },
                _ => {}
            }
            if done.changed {
                *text = chars.iter().collect();
            }
            let at = at.min(chars.len());
            done.selection = at..at;
            self.shown = done.selection.clone();
            return Some(done);
        }
        let mut visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        if visual && *selection != self.shown {
            self.mode = Mode::Normal;
            visual = false;
        }
        // The key after `"`, `q` or `@` names a register.
        if let Some(prefix) = self.prefix.take() {
            let done = self.name(text, selection, prefix, key);
            self.shown = done.selection.clone();
            return Some(done);
        }
        let idle = self.pending.is_none() && self.waiting.is_none();
        if key == Stroke::Char('.') && idle && !visual {
            // A number typed before it takes the place of the one the command had, and
            // stays with it.
            let count = std::mem::take(&mut self.count);
            self.recording.clear();
            if count > 0 && !self.last.is_empty() {
                self.last = counted(&self.last, count);
            }
            return Some(self.repeat(text, selection));
        }
        if let (Stroke::Char(prefix @ ('"' | 'q' | '@')), true) = (key, idle) {
            // While a macro is recorded, `q` ends it: its keys go to its register.
            match self.macro_into.take_if(|_| prefix == 'q') {
                Some(name) => {
                    let keys = std::mem::take(&mut self.macro_keys);
                    self.keep(name, keys, false);
                }
                None => self.prefix = Some(prefix),
            }
            done.selection = selection.clone();
            self.shown = done.selection.clone();
            return Some(done);
        }
        if !self.replaying {
            self.recording.push(key);
        }
        let mut at = if visual {
            self.head.min(chars.len())
        } else {
            selection.start.min(chars.len())
        };
        let before = chars.len();
        let pending = self.pending.take();
        let waiting = self.waiting.take();
        let operator = pending.filter(|op| "dcy".contains(*op));
        let counted = (self.count > 0).then_some(self.count);
        let times = counted.unwrap_or(1);
        // A command is done: the number typed before it is used up, unless it goes on.
        let mut keep_count = false;

        let mut target = None;
        // The move is a jump: where the caret was is kept, for `''` to come back to.
        let mut jump = false;
        match (pending, key) {
            (_, Stroke::Escape) => self.mode = Mode::Normal,
            (_, Stroke::Char(name)) if waiting == Some('m') => {
                if name.is_ascii_lowercase() {
                    self.marks.insert(name, at);
                } else if "'`".contains(name) {
                    self.marks.insert('\'', at);
                }
            }
            // To a mark: the first letter of its line, or with a backtick its very place.
            (_, Stroke::Char(name)) if matches!(waiting, Some('\'' | '`')) => {
                let name = if name == '`' { '\'' } else { name };
                let mark = self.marks.get(&name).map(|mark| (*mark).min(chars.len()));
                target = mark.map(|mark| match waiting {
                    Some('`') => (mark, Span::Upto),
                    _ => (first_text(&chars, mark), Span::Lines),
                });
                jump = true;
            }
            // The char a key was waiting for: where to go on the line, or what to take.
            (_, Stroke::Char(named)) if matches!(waiting, Some('i' | 'a')) => {
                let range = object(&chars, at, named, waiting == Some('a'));
                match (range, operator) {
                    (Some(range), _) if visual && !range.is_empty() => {
                        self.anchor = range.start;
                        at = range.end - 1;
                    }
                    (Some(range), Some(op)) if !visual => {
                        at = self.operate(&mut chars, (op, at), range, false, &mut done);
                    }
                    _ => {}
                }
            }
            (_, Stroke::Char(wanted)) if waiting.is_some() => {
                let kind = waiting.unwrap_or('f');
                target = find(&chars, at, kind, wanted, times);
                self.found = Some((kind, wanted));
            }
            (_, _) if waiting.is_some() => {}
            (Some('r'), Stroke::Char(with)) => {
                if chars.get(at).is_some_and(|ch| *ch != '\n') {
                    chars[at] = with;
                    done.changed = true;
                }
            }
            (Some('g'), Stroke::Char('g')) => {
                target = Some((line_numbered(&chars, times), Span::Lines));
                jump = true;
            }
            (_, Stroke::Char(digit @ '0'..='9')) if digit != '0' || self.count > 0 => {
                self.count = self.count * 10 + digit as usize - '0' as usize;
                self.pending = pending;
                keep_count = true;
            }
            // `>>` and `<<` push lines in and out by one level.
            (Some(op @ ('>' | '<')), Stroke::Char(again)) if op == again => {
                let last = (1..times).fold(at, |at, _| vertical(&chars, at, true));
                let unit = self.indent().to_owned();
                at = shift(&mut chars, &mut self.marks, at, last, op == '>', &unit);
                done.changed = true;
            }
            // An operator typed twice is done on whole lines.
            (Some(op), Stroke::Char(again)) if op == again && "dcy".contains(op) => {
                let last = (1..times).fold(at, |at, _| vertical(&chars, at, true));
                let range = lines(&chars, at, last, op == 'c');
                at = self.operate(&mut chars, (op, at), range, true, &mut done);
            }
            (_, Stroke::Char(prefix @ ('f' | 'F' | 't' | 'T' | '\'' | '`'))) => {
                self.waiting = Some(prefix);
                self.pending = pending;
                keep_count = true;
            }
            (None, Stroke::Char('m')) => {
                self.waiting = Some('m');
                keep_count = true;
            }
            (_, Stroke::Char(prefix @ ('i' | 'a'))) if visual || operator.is_some() => {
                self.waiting = Some(prefix);
                self.pending = pending;
                keep_count = true;
            }
            // The last search on the line again, the same way or the other.
            (_, Stroke::Char(again @ (';' | ','))) => {
                if let Some((kind, wanted)) = self.found {
                    let kind = match (again, kind.is_lowercase()) {
                        (';', _) => kind,
                        (_, true) => kind.to_ascii_uppercase(),
                        (_, false) => kind.to_ascii_lowercase(),
                    };
                    target = find(&chars, at, kind, wanted, times);
                }
            }
            _ => {
                target = motion(&chars, at, key, counted);
                jump = matches!(key, Stroke::Char('G' | '%' | '*' | '#'));
            }
        }
        if let Some((mut to, mut span)) = target {
            if jump {
                self.marks.insert('\'', at);
            }
            match operator {
                Some(op) if !visual => {
                    // `cw` changes the word, not the white space after it.
                    let on_word = chars.get(at).is_some_and(|ch| kind(*ch) != 1);
                    if op == 'c' && key == Stroke::Char('w') && on_word {
                        (to, span) = (word_end(&chars, at), Span::With);
                    }
                    let (low, high) = (at.min(to), at.max(to));
                    let range = match span {
                        Span::Upto => low..high,
                        Span::With => low..(high + 1).min(chars.len()),
                        Span::Lines => lines(&chars, low, high, op == 'c'),
                    };
                    at = self.operate(&mut chars, (op, at), range, span == Span::Lines, &mut done);
                }
                _ => at = to,
            }
        } else if !keep_count
            && key != Stroke::Escape
            && pending.is_none()
            && waiting.is_none()
            && !matches!(key, Stroke::Char(';' | ','))
        {
            at = self.command(&mut chars, at, key, times, visual, &mut done);
            keep_count = self.pending.is_some();
        }
        if !keep_count {
            self.count = 0;
        }
        done.changed |= chars.len() != before;
        if done.changed {
            *text = chars.iter().collect();
        }
        let at = at.min(chars.len());
        done.selection = match self.mode {
            Mode::Visual | Mode::VisualLine => {
                // Entered just now: the selection starts where the caret is.
                if !visual {
                    self.anchor = at;
                }
                self.head = at;
                let (low, high) = (self.anchor.min(at), self.anchor.max(at));
                self.marks.extend([('<', low), ('>', high)]);
                if self.mode == Mode::Visual {
                    low..(high + 1).min(chars.len())
                } else {
                    line_start(&chars, low)..line_end(&chars, high.min(chars.len()))
                }
            }
            Mode::Insert => at..at,
            Mode::Normal => {
                // The caret is on a char, never after the last one of a line.
                let (start, end) = (line_start(&chars, at), line_end(&chars, at));
                let at = if at >= end && end > start {
                    end - 1
                } else {
                    at
                };
                at..at
            }
        };
        let complete = self.pending.is_none() && self.waiting.is_none() && !keep_count;
        if complete {
            // The register named is for one command only.
            self.named = None;
        }
        if self.mode == Mode::Insert {
            self.typed_from = Some((at, chars.len()));
        }
        if !self.replaying {
            // A command that changed the text, or went to insert mode, is what `.` repeats.
            let changes = (done.changed || self.mode == Mode::Insert) && !visual;
            if complete && changes {
                self.last = std::mem::take(&mut self.recording);
                self.last_typed.clear();
                self.insert_from = (self.mode == Mode::Insert).then_some((at, chars.len()));
            } else if complete {
                self.recording.clear();
            }
        }
        self.shown = done.selection.clone();
        Some(done)
    }

    /// The command being typed after a `:`, to show while it is.
    pub fn typing(&self) -> Option<&str> {
        self.line.as_deref()
    }

    /// Forget every mark. They are places in one text, kept right through what Vim itself
    /// does to it: for the application to call when the text becomes another one.
    pub fn forget_marks(&mut self) {
        self.marks.clear();
    }

    fn indent(&self) -> &str {
        match self.unit.as_str() {
            "" => "    ",
            unit => unit,
        }
    }

    /// The line an address names, counted from 1, and what is written after it: a number,
    /// `.` for the line of the caret `at`, `$` for the last one, `'a` for the line of a mark.
    fn address<'a>(&self, chars: &[char], at: usize, written: &'a str) -> Option<(usize, &'a str)> {
        let mut signs = written.chars();
        let line = match signs.next()? {
            '.' => line_number(chars, at),
            '$' => line_number(chars, chars.len()),
            '\'' => {
                let mark = *self.marks.get(&signs.next()?)?;
                line_number(chars, mark.min(chars.len()))
            }
            '0'..='9' => {
                let rest = written.trim_start_matches(|sign: char| sign.is_ascii_digit());
                let digits = written.len() - rest.len();
                return Some((written[..digits].parse().ok()?, rest));
            }
            _ => return None,
        };
        Some((line, signs.as_str()))
    }

    /// Carry out `command`, written after a `:`, when it is not one for the application:
    /// lines alone to go to the last of them, or lines and `d` to take them out, or `s` to
    /// substitute in them. The lines are one address or two with a comma, or `%` for all;
    /// without any, the line of the caret `at`. Returns where the caret goes.
    fn ex(&mut self, chars: &mut Vec<char>, at: usize, command: &str, done: &mut Done) -> usize {
        let last = line_number(chars, chars.len());
        let whole = command.strip_prefix('%');
        let (range, rest) = match (whole, self.address(chars, at, command)) {
            (Some(rest), _) => (Some((1, last)), rest),
            (None, Some((first, rest))) => {
                let after = rest.strip_prefix(',');
                match after.and_then(|after| self.address(chars, at, after)) {
                    Some((second, rest)) => (Some((first.min(second), first.max(second))), rest),
                    None => (Some((first, first)), rest),
                }
            }
            (None, None) => (None, command),
        };
        let rest = rest.trim_start();
        if rest.is_empty() {
            let Some((_, to)) = range else {
                return at;
            };
            self.marks.insert('\'', at);
            return line_numbered(chars, to);
        }
        let here = line_number(chars, at);
        let (first, to) = range.unwrap_or((here, here));
        // Lines the text does not have: nothing is done, as in Vim.
        if first == 0 || to > last {
            return at;
        }
        if let Some(written) = rest.strip_prefix('s') {
            return self.substitute(chars, at, (first, to), written, done);
        }
        if matches!(rest.trim_end(), "d" | "delete") {
            let (low, high) = (line_numbered(chars, first), line_numbered(chars, to));
            let range = lines(chars, low, high, false);
            return self.operate(chars, ('d', at), range, true, done);
        }
        at
    }

    /// `:s`: in the lines `first` to `to`, counted from 1, write a replacement where a
    /// pattern is found. `written` is what follows the `s`: the pattern, the replacement and
    /// the flags, with a sign of any kind between them; nothing, for the last substitution
    /// again without its flags. The pattern is a regular expression as the `regex` crate
    /// reads them; `g` is for every time on a line and not the first only, `i` for capitals
    /// and small letters alike. Returns where the caret goes: the last line that changed.
    fn substitute(
        &mut self,
        chars: &mut Vec<char>,
        at: usize,
        (first, to): (usize, usize),
        written: &str,
        done: &mut Done,
    ) -> usize {
        let mut signs = written.chars();
        let (pattern, replacement, flags) = match signs.next() {
            None => match self.last_substitute.clone() {
                Some((pattern, replacement)) => (pattern, replacement, String::new()),
                None => return at,
            },
            Some(between) => {
                if between.is_alphanumeric() || between.is_whitespace() || "\\\"|".contains(between)
                {
                    return at;
                }
                let mut parts = split(signs.as_str(), between).into_iter();
                let pattern = parts.next().unwrap_or_default();
                // No pattern: the one of the last substitution.
                let before = self.last_substitute.as_ref().map(|(before, _)| before);
                let pattern = match (pattern.is_empty(), before) {
                    (false, _) => pattern,
                    (true, Some(before)) => before.clone(),
                    (true, None) => return at,
                };
                let replacement = parts.next().unwrap_or_default();
                (pattern, replacement, parts.next().unwrap_or_default())
            }
        };
        let flags = flags.trim_end();
        if flags.chars().any(|flag| !"gi".contains(flag)) {
            return at;
        }
        let mut reader = regex::RegexBuilder::new(&pattern);
        let Ok(wanted) = reader.case_insensitive(flags.contains('i')).build() else {
            return at;
        };
        let top = line_numbered(chars, first);
        let mut start = line_numbered(chars, to);
        let mut reached = None;
        // From the last line up, so that the lines still to read stay where they are.
        loop {
            let end = line_end(chars, start);
            let line: String = chars[start..end].iter().collect();
            if let Some(line) = substituted(&line, &wanted, &replacement, flags.contains('g')) {
                let (removed, added) = (end - start, line.chars().count());
                edited(&mut self.marks, start, removed, added);
                chars.splice(start..end, line.chars());
                // The last line that changed, which a change above it moves.
                reached = Some(reached.map_or(start, |below: usize| below + added - removed));
            }
            if start <= top {
                break;
            }
            start = line_start(chars, start - 1);
        }
        self.last_substitute = Some((pattern, replacement));
        let Some(reached) = reached else {
            return at;
        };
        done.changed = true;
        first_text(chars, reached)
    }

    /// `.`: the last command that changed the text, done again where the caret is now,
    /// with what was typed after it in insert mode.
    fn repeat(&mut self, text: &mut String, selection: &Range<usize>) -> Done {
        let (keys, typed) = (self.last.clone(), self.last_typed.clone());
        let mut done = Done {
            selection: selection.clone(),
            ..Done::default()
        };
        self.replaying = true;
        for key in keys {
            self.retype(text, &mut done, key);
        }
        if self.mode == Mode::Insert {
            let at = done.selection.start;
            text.insert_str(crate::byte_index(text, at), &typed);
            let after = at + typed.chars().count();
            done.selection = after..after;
            done.changed = true;
            self.retype(text, &mut done, Stroke::Escape);
        }
        self.replaying = false;
        self.shown = done.selection.clone();
        done
    }

    /// Type `key` where `done` left the caret, and add what it does to `done`. False when
    /// the key is not for Vim.
    fn retype(&mut self, text: &mut String, done: &mut Done, key: Stroke) -> bool {
        let Some(did) = self.key(text, &done.selection, key) else {
            return false;
        };
        done.selection = did.selection;
        done.changed |= did.changed;
        done.copied = did.copied.or(done.copied.take());
        done.ask = did.ask.or(done.ask);
        true
    }

    /// The key after `"`, `q` or `@` (`prefix`): the register to use for the next command,
    /// to record a macro into, or whose macro to play. Any other key gives the command up.
    fn name(
        &mut self,
        text: &mut String,
        selection: &Range<usize>,
        prefix: char,
        key: Stroke,
    ) -> Done {
        let done = Done {
            selection: selection.clone(),
            ..Done::default()
        };
        let Stroke::Char(name) = key else {
            return done;
        };
        match prefix {
            '"' if name.is_ascii_alphanumeric() || "_+*-".contains(name) => {
                // The clipboard is where the register without a name goes already.
                self.named = Some(name).filter(|name| !"+*".contains(*name));
                if !self.replaying {
                    self.recording.extend([Stroke::Char('"'), key]);
                }
            }
            'q' if name.is_ascii_alphabetic() => {
                self.macro_into = Some(name);
                self.macro_keys.clear();
            }
            '@' if name.is_ascii_alphanumeric() || name == '@' => {
                let times = self.count.max(1);
                self.count = 0;
                self.named = None;
                self.recording.clear();
                return self.play(text, selection, name, times);
            }
            _ => {}
        }
        done
    }

    /// Keep `taken` in the register `name`; a capital letter adds it to what the small
    /// letter holds. Returns what the register holds now.
    fn keep(&mut self, name: char, taken: String, whole_lines: bool) -> (String, bool) {
        let held = self.registers.entry(name.to_ascii_lowercase()).or_default();
        if name.is_ascii_uppercase() {
            // Lines added after a few chars start on a line of their own.
            if whole_lines && !held.1 && !held.0.is_empty() {
                held.0.push('\n');
            }
            held.0.push_str(&taken);
            held.1 |= whole_lines;
            if held.1 && !held.0.ends_with('\n') {
                held.0.push('\n');
            }
        } else {
            *held = (taken, whole_lines);
        }
        held.clone()
    }

    /// `@`: the keys the register `name` holds, typed `times` times; `@` is the register
    /// last played. A key that asks something of the application ends it, and so does a
    /// macro that goes on calling itself.
    fn play(
        &mut self,
        text: &mut String,
        selection: &Range<usize>,
        name: char,
        times: usize,
    ) -> Done {
        let mut done = Done {
            selection: selection.clone(),
            ..Done::default()
        };
        let name = match name {
            '@' => self.macro_last,
            name => Some(name.to_ascii_lowercase()),
        };
        let held = name.and_then(|name| Some((name, self.registers.get(&name)?.0.clone())));
        let Some((name, keys)) = held.filter(|(_, keys)| !keys.is_empty()) else {
            return done;
        };
        self.macro_last = Some(name);
        if self.running == 0 {
            self.keys_left = MACRO_KEYS;
        }
        if self.running == MACRO_DEPTH {
            return done;
        }
        self.running += 1;
        'played: for _ in 0..times {
            for ch in keys.chars() {
                if self.keys_left == 0 || done.ask.is_some() {
                    break 'played;
                }
                self.keys_left -= 1;
                if !self.retype(text, &mut done, char_key(ch)) {
                    // Insert mode: the char is typed.
                    let at = done.selection.start;
                    text.insert(crate::byte_index(text, at), ch);
                    done.selection = at + 1..at + 1;
                    done.changed = true;
                }
            }
        }
        self.running -= 1;
        done
    }

    /// Do the operator `op` on `range`, the caret being at `at`. Returns where it goes.
    fn operate(
        &mut self,
        chars: &mut Vec<char>,
        (op, at): (char, usize),
        range: Range<usize>,
        whole_lines: bool,
        done: &mut Done,
    ) -> usize {
        let mut taken: String = chars[range.clone()].iter().collect();
        if whole_lines {
            // Kept as lines whatever end the text has: without the line ends around it,
            // with one after it.
            taken = taken.trim_matches('\n').to_owned() + "\n";
        }
        let named = self.named.take();
        // `"_` is the black hole: nothing is kept, and what was kept before stays.
        if named != Some('_') {
            self.register = match named {
                Some(name) => self.keep(name, taken, whole_lines),
                None => {
                    done.copied = Some(taken.clone());
                    let held = (taken.clone(), whole_lines);
                    if op == 'y' {
                        self.registers.insert('0', held);
                    } else if whole_lines || taken.contains('\n') {
                        // A line or more taken out goes to `1`, and what `1` to `8` held
                        // goes one register down.
                        for number in (b'1'..b'9').rev() {
                            if let Some(older) = self.registers.remove(&char::from(number)) {
                                self.registers.insert(char::from(number + 1), older);
                            }
                        }
                        self.registers.insert('1', held);
                    } else {
                        self.registers.insert('-', held);
                    }
                    (taken, whole_lines)
                }
            };
        }
        self.mode = Mode::Normal;
        if op == 'y' {
            // Copying lines leaves the caret where it is.
            return if whole_lines { at } else { range.start };
        }
        chars.drain(range.clone());
        edited(&mut self.marks, range.start, range.len(), 0);
        done.changed = true;
        if op == 'c' {
            self.mode = Mode::Insert;
            return range.start;
        }
        let at = range.start.min(chars.len());
        if whole_lines {
            first_text(chars, at)
        } else {
            at
        }
    }

    /// A key that is neither a move nor the end of something started. Returns where the
    /// caret goes.
    fn command(
        &mut self,
        chars: &mut Vec<char>,
        at: usize,
        key: Stroke,
        times: usize,
        visual: bool,
        done: &mut Done,
    ) -> usize {
        let (start, end) = (line_start(chars, at), line_end(chars, at));
        let Stroke::Char(named) = key else {
            match key {
                Stroke::Ctrl('r') => done.ask = Some(Ask::Redo),
                Stroke::Delete if !visual => {
                    let range = at..(at + times).min(end);
                    return self.operate(chars, ('d', at), range, false, done);
                }
                _ => {}
            }
            return at;
        };
        if visual {
            let (low, high) = (self.anchor.min(at), self.anchor.max(at));
            let by_lines = self.mode == Mode::VisualLine;
            let op = match named {
                '>' | '<' => {
                    let unit = self.indent().to_owned();
                    self.mode = Mode::Normal;
                    done.changed = true;
                    return shift(chars, &mut self.marks, low, high, named == '>', &unit);
                }
                // The command is for the lines of the selection, unless told otherwise.
                ':' => {
                    self.mode = Mode::Normal;
                    self.line = Some("'<,'>".to_owned());
                    return at;
                }
                'd' | 'x' => 'd',
                'c' | 'y' => named,
                'v' | 'V' => {
                    self.mode = Mode::Normal;
                    return at;
                }
                _ => return at,
            };
            let range = if by_lines {
                lines(chars, low, high, op == 'c')
            } else {
                low..(high + 1).min(chars.len())
            };
            return self.operate(chars, (op, at), range, by_lines, done);
        }
        match named {
            'd' | 'c' | 'y' | 'g' | 'r' | '>' | '<' => self.pending = Some(named),
            ':' => self.line = Some(String::new()),
            // The last substitution again, on this line.
            '&' => {
                let here = line_number(chars, at);
                return self.substitute(chars, at, (here, here), "", done);
            }
            'x' => {
                let range = at..(at + times).min(end);
                return self.operate(chars, ('d', at), range, false, done);
            }
            '~' => {
                // The case of the letters under the caret, which then goes on past them.
                let to = (at + times).min(end);
                for ch in &mut chars[at..to] {
                    let other: String = match ch.is_uppercase() {
                        true => ch.to_lowercase().collect(),
                        false => ch.to_uppercase().collect(),
                    };
                    let mut other = other.chars();
                    // A letter whose other case is several letters is left as it is.
                    if let (Some(one), None) = (other.next(), other.next()) {
                        *ch = one;
                    }
                }
                done.changed = to > at;
                return to;
            }
            'D' => return self.operate(chars, ('d', at), at..end, false, done),
            'C' => return self.operate(chars, ('c', at), at..end, false, done),
            'i' => self.mode = Mode::Insert,
            'a' => {
                self.mode = Mode::Insert;
                return (at + 1).min(end);
            }
            'I' => {
                self.mode = Mode::Insert;
                return first_text(chars, at);
            }
            'A' => {
                self.mode = Mode::Insert;
                return end;
            }
            'o' | 'O' => {
                // The new line starts under the first letter of this one.
                let indent: String = chars[start..first_text(chars, at)].iter().collect();
                self.mode = Mode::Insert;
                if named == 'o' {
                    put(chars, &mut self.marks, end, &format!("\n{indent}"));
                    return end + 1 + indent.chars().count();
                }
                put(chars, &mut self.marks, start, &format!("{indent}\n"));
                return start + indent.chars().count();
            }
            'p' | 'P' => {
                let (written, whole_lines) = match self.named.take() {
                    Some(name) => {
                        let held = self.registers.get(&name.to_ascii_lowercase());
                        held.cloned().unwrap_or_default()
                    }
                    None => self.register.clone(),
                };
                let length = written.chars().count();
                if length == 0 {
                    return at;
                }
                let after = named == 'p';
                return match (whole_lines, after) {
                    (true, true) => {
                        let lines = format!("\n{}", written.trim_end_matches('\n'));
                        put(chars, &mut self.marks, end, &lines);
                        end + 1
                    }
                    (true, false) => {
                        put(chars, &mut self.marks, start, &written);
                        start
                    }
                    (false, _) => {
                        let to = if after { (at + 1).min(end) } else { at };
                        put(chars, &mut self.marks, to, &written);
                        to + length - 1
                    }
                };
            }
            'J' if end < chars.len() => {
                // The next line comes up, its indent becoming one space.
                let next = first_text(chars, end + 1);
                chars.splice(end..next, [' ']);
                edited(&mut self.marks, end, next - end, 1);
                return end;
            }
            'u' => done.ask = Some(Ask::Undo),
            '/' => done.ask = Some(Ask::Find),
            'v' => self.mode = Mode::Visual,
            'V' => self.mode = Mode::VisualLine,
            _ => {}
        }
        at
    }
}

/// How many macros may play one inside the other, and how many keys one `@` may type in
/// all: where a macro that calls itself stops.
const MACRO_DEPTH: usize = 32;
const MACRO_KEYS: usize = 100_000;

/// The number of the line of `at`, counted from 1.
fn line_number(chars: &[char], at: usize) -> usize {
    chars[..at].iter().filter(|ch| **ch == '\n').count() + 1
}

/// Move the marks after a change of the text: `removed` chars went from `at` on, and
/// `added` came in their place. A mark that was in what went stays as far into what came
/// as it can.
fn edited(marks: &mut HashMap<char, usize>, at: usize, removed: usize, added: usize) {
    for mark in marks.values_mut() {
        if *mark >= at + removed {
            *mark = *mark + added - removed;
        } else if *mark > at {
            *mark = at + (*mark - at).min(added);
        }
    }
}

/// Write `written` at `at`, the marks after it moving with their text.
fn put(chars: &mut Vec<char>, marks: &mut HashMap<char, usize>, at: usize, written: &str) {
    let before = chars.len();
    chars.splice(at..at, written.chars());
    edited(marks, at, 0, chars.len() - before);
}

/// The keys of a command with `count` as its number, in place of any typed with it.
fn counted(keys: &[Stroke], count: usize) -> Vec<Stroke> {
    // A number does not start with a zero: that one is the move to the start of the line.
    let number = |keys: &[Stroke]| match keys.first() {
        Some(Stroke::Char('1'..='9')) => keys
            .iter()
            .take_while(|key| matches!(key, Stroke::Char('0'..='9')))
            .count(),
        _ => 0,
    };
    let mut rest = &keys[number(keys)..];
    let mut counted = Vec::new();
    // The register named may come before the number or after it.
    if let [Stroke::Char('"'), name, after @ ..] = rest {
        counted.extend([Stroke::Char('"'), *name]);
        rest = &after[number(after)..];
    }
    counted.splice(0..0, count.to_string().chars().map(Stroke::Char));
    // An operator may have a number of its own after it.
    if let [
        operator @ Stroke::Char('d' | 'c' | 'y' | '<' | '>'),
        after @ ..,
    ] = rest
    {
        counted.push(*operator);
        rest = &after[number(after)..];
    }
    // After a number a zero would be part of it: that command is left as it was.
    if let Some(Stroke::Char('0')) = rest.first() {
        return keys.to_vec();
    }
    counted.extend(rest);
    counted
}

/// What is written between the signs `between`, which a `\` before it makes part of the
/// text.
fn split(written: &str, between: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut part = String::new();
    let mut signs = written.chars();
    while let Some(sign) = signs.next() {
        match (sign, signs.clone().next()) {
            ('\\', Some(next)) => {
                // Any other `\` stays with its sign, for the pattern to read.
                if next != between {
                    part.push(sign);
                }
                part.push(next);
                signs.next();
            }
            _ if sign == between => parts.push(std::mem::take(&mut part)),
            _ => part.push(sign),
        }
    }
    parts.push(part);
    parts
}

/// `line` with `replacement` where `wanted` is found: the first time, or every time
/// (`all`). `None` when it is not found. In the replacement `&` is what was found, `\1` to
/// `\9` its groups, `\r` and `\n` a line end, `\t` a tab; after a `\` any other sign is
/// itself.
fn substituted(line: &str, wanted: &regex::Regex, replacement: &str, all: bool) -> Option<String> {
    let mut written = String::new();
    let mut from = None;
    let times = if all { usize::MAX } else { 1 };
    for found in wanted.captures_iter(line).take(times) {
        let group = |number: usize| found.get(number).map_or("", |group| group.as_str());
        let whole = found.get(0)?;
        written.push_str(&line[from.unwrap_or(0)..whole.start()]);
        from = Some(whole.end());
        let mut signs = replacement.chars();
        while let Some(sign) = signs.next() {
            match (sign, signs.clone().next()) {
                ('&', _) => written.push_str(group(0)),
                ('\\', Some(next)) => {
                    match next {
                        '0'..='9' => written.push_str(group(next as usize - '0' as usize)),
                        'r' | 'n' => written.push('\n'),
                        't' => written.push('\t'),
                        _ => written.push(next),
                    }
                    signs.next();
                }
                _ => written.push(sign),
            }
        }
    }
    written.push_str(&line[from?..]);
    Some(written)
}

/// What was typed up to the caret `at` since it was at `from`, the text being `length`
/// chars long then; nothing when more than typing was done.
fn typed(chars: &[char], at: usize, (from, length): (usize, usize)) -> String {
    let typed = at
        .checked_sub(from)
        .filter(|typed| length + typed == chars.len());
    typed.map_or_else(String::new, |typed| {
        chars[from..from + typed].iter().collect()
    })
}

/// The char that stands for a key in a register, where a macro is kept as text. Keys
/// that Vim has no use for have none.
fn key_char(key: Stroke) -> Option<char> {
    Some(match key {
        Stroke::Char(ch) => ch,
        Stroke::Ctrl(ch @ 'a'..='z') => char::from(ch as u8 - b'a' + 1),
        Stroke::Ctrl(_) | Stroke::Alt(_) => return None,
        Stroke::Escape => '\u{1b}',
        Stroke::Enter => '\n',
        Stroke::Backspace => '\u{8}',
        Stroke::Delete => '\u{7f}',
        Stroke::Left => '←',
        Stroke::Right => '→',
        Stroke::Up => '↑',
        Stroke::Down => '↓',
    })
}

/// The key a char of a register stands for, when the register is played as a macro.
fn char_key(ch: char) -> Stroke {
    match ch {
        '\u{1b}' => Stroke::Escape,
        '\n' | '\r' => Stroke::Enter,
        '\u{8}' => Stroke::Backspace,
        '\u{7f}' => Stroke::Delete,
        '←' => Stroke::Left,
        '→' => Stroke::Right,
        '↑' => Stroke::Up,
        '↓' => Stroke::Down,
        '\t' => Stroke::Char(ch),
        '\u{1}'..='\u{1a}' => Stroke::Ctrl(char::from(ch as u8 - 1 + b'a')),
        ch => Stroke::Char(ch),
    }
}

/// The whole lines from the one of `low` to the one of `high`. To change them, their text
/// alone; to take them out, a line end with them: the one after, or else the one before.
fn lines(chars: &[char], low: usize, high: usize, text_only: bool) -> Range<usize> {
    let (start, end) = (line_start(chars, low), line_end(chars, high));
    if text_only {
        start..end
    } else if end < chars.len() {
        start..end + 1
    } else {
        start.saturating_sub(1)..end
    }
}

/// Where `f`, `F`, `t` or `T` (`kind`) takes the caret on its line: to the `times`-th
/// `wanted` after or before it, or next to it.
fn find(
    chars: &[char],
    at: usize,
    kind: char,
    wanted: char,
    times: usize,
) -> Option<(usize, Span)> {
    let (start, end) = (line_start(chars, at), line_end(chars, at));
    let mut found = at;
    for _ in 0..times {
        found = if kind.is_lowercase() {
            (found + 1..end).find(|index| chars[*index] == wanted)?
        } else {
            (start..found).rev().find(|index| chars[*index] == wanted)?
        };
    }
    Some(match kind {
        'f' => (found, Span::With),
        't' => (found - 1, Span::With),
        'F' => (found, Span::Upto),
        _ => (found + 1, Span::Upto),
    })
}

/// The chars the text object `named` covers at `at`: a word (`w`), or what a pair of
/// brackets or of quotes holds. `around`: with the white space after the word, or with
/// the pair itself.
fn object(chars: &[char], at: usize, named: char, around: bool) -> Option<Range<usize>> {
    let (open, close) = match named {
        'w' => {
            let here = kind(*chars.get(at).filter(|ch| **ch != '\n')?);
            let same = |ch: &char| *ch != '\n' && kind(*ch) == here;
            let start = at - chars[..at].iter().rev().take_while(|ch| same(ch)).count();
            let mut end = at + chars[at..].iter().take_while(|ch| same(ch)).count();
            if around && here != 1 {
                end += chars[end..].iter().take_while(|ch| **ch == ' ').count();
            }
            return Some(start..end);
        }
        '(' | ')' | 'b' => ('(', ')'),
        '{' | '}' | 'B' => ('{', '}'),
        '[' | ']' => ('[', ']'),
        '<' | '>' => ('<', '>'),
        '"' | '\'' | '`' => {
            // Quotes pair up along the line: the pair the caret is in, or on.
            let (start, end) = (line_start(chars, at), line_end(chars, at));
            let quotes: Vec<usize> = (start..end)
                .filter(|index| chars[*index] == named)
                .collect();
            let pair = quotes
                .chunks_exact(2)
                .find(|pair| pair[0] <= at && at <= pair[1])?;
            return Some(if around {
                pair[0]..pair[1] + 1
            } else {
                pair[0] + 1..pair[1]
            });
        }
        _ => return None,
    };
    // Back to the bracket that is still open here, then on to the one that closes it.
    let mut depth = 0;
    let mut opened = None;
    for index in (0..=at.min(chars.len().checked_sub(1)?)).rev() {
        let ch = chars[index];
        if ch == close && index != at {
            depth += 1;
        } else if ch == open {
            if depth == 0 {
                opened = Some(index);
                break;
            }
            depth -= 1;
        }
    }
    let opened = opened?;
    let mut depth = 0;
    let closed = (opened + 1..chars.len()).find(|index| {
        let ch = chars[*index];
        if ch == open {
            depth += 1;
        } else if ch == close {
            if depth == 0 {
                return true;
            }
            depth -= 1;
        }
        false
    })?;
    Some(if around {
        opened..closed + 1
    } else {
        opened + 1..closed
    })
}

/// Push the lines from the one of `first` to the one of `last` one `unit` in (`deeper`) or
/// out. Returns the first letter of the first of them, where the caret goes.
fn shift(
    chars: &mut Vec<char>,
    marks: &mut HashMap<char, usize>,
    first: usize,
    last: usize,
    deeper: bool,
    unit: &str,
) -> usize {
    let top = line_start(chars, first.min(last));
    let mut start = line_start(chars, first.max(last));
    // From the last line up, so that the lines still to move stay where they are.
    loop {
        let end = line_end(chars, start);
        if deeper && end > start {
            put(chars, marks, start, unit);
        } else if !deeper {
            let blank = chars[start..end].iter().take_while(|ch| **ch == ' ');
            let spaces = blank.count().min(unit.chars().count());
            let taken = match chars.get(start) {
                Some('\t') => 1,
                _ => spaces,
            };
            chars.drain(start..start + taken);
            edited(marks, start, taken, 0);
        }
        if start <= top {
            break;
        }
        start = line_start(chars, start - 1);
    }
    first_text(chars, top)
}

/// The bracket that goes with the first one at or after `at` on its line.
fn partner(chars: &[char], at: usize) -> Option<usize> {
    const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];
    let bracket = |ch: &char| PAIRS.iter().any(|(open, close)| ch == open || ch == close);
    let from = at + chars[at..line_end(chars, at)].iter().position(bracket)?;
    let (open, close) = *PAIRS
        .iter()
        .find(|(open, close)| chars[from] == *open || chars[from] == *close)?;
    let forward = chars[from] == open;
    let mut depth = 0_usize;
    // Out from it, past the pairs that open and close on the way.
    let mut pairs = |index: &usize| {
        let ch = chars[*index];
        if ch == chars[from] {
            depth += 1;
        } else if ch == open || ch == close {
            if depth == 0 {
                return true;
            }
            depth -= 1;
        }
        false
    };
    match forward {
        true => (from + 1..chars.len()).find(&mut pairs),
        false => (0..from).rev().find(&mut pairs),
    }
}

/// Where the word under the caret is written next (`forward`) or before, as a whole word;
/// the search goes round the ends of the text.
fn occurrence(chars: &[char], at: usize, forward: bool) -> Option<usize> {
    let wordy = |index: usize| chars.get(index).is_some_and(|ch| kind(*ch) == 0);
    if !wordy(at) {
        return None;
    }
    let start = (0..=at).rev().take_while(|index| wordy(*index)).last()?;
    let end = (at..chars.len()).take_while(|index| wordy(*index)).last()? + 1;
    let word = &chars[start..end];
    let whole = |from: &usize| {
        let before = from.checked_sub(1).is_some_and(wordy);
        chars[*from..].starts_with(word) && !before && !wordy(from + word.len())
    };
    let starts = 0..chars.len().saturating_sub(word.len() - 1);
    match forward {
        true => starts
            .clone()
            .skip(start + 1)
            .chain(starts.take(start))
            .find(whole),
        false => starts
            .clone()
            .take(start)
            .rev()
            .chain(starts.skip(start + 1).rev())
            .find(whole),
    }
}

/// Where a key that moves the caret takes it, `count` times, and what the move covers.
fn motion(chars: &[char], at: usize, key: Stroke, count: Option<usize>) -> Option<(usize, Span)> {
    let repeat = |step: &dyn Fn(usize) -> usize| (0..count.unwrap_or(1)).fold(at, |at, _| step(at));
    Some(match key {
        Stroke::Char('h') | Stroke::Left | Stroke::Backspace => (
            repeat(&|at| at.max(line_start(chars, at) + 1) - 1),
            Span::Upto,
        ),
        Stroke::Char('l' | ' ') | Stroke::Right => {
            (repeat(&|at| (at + 1).min(line_end(chars, at))), Span::Upto)
        }
        Stroke::Char('j') | Stroke::Down | Stroke::Enter => {
            (repeat(&|at| vertical(chars, at, true)), Span::Lines)
        }
        Stroke::Char('k') | Stroke::Up => (repeat(&|at| vertical(chars, at, false)), Span::Lines),
        Stroke::Char('w') => (repeat(&|at| word_forward(chars, at)), Span::Upto),
        Stroke::Char('b') => (repeat(&|at| word_back(chars, at)), Span::Upto),
        Stroke::Char('e') => (repeat(&|at| word_end(chars, at)), Span::With),
        Stroke::Char('0') => (line_start(chars, at), Span::Upto),
        Stroke::Char('^') => (first_text(chars, at), Span::Upto),
        Stroke::Char('$') => (line_end(chars, at), Span::Upto),
        Stroke::Char('%') => (partner(chars, at)?, Span::With),
        Stroke::Char('*') => (occurrence(chars, at, true)?, Span::Upto),
        Stroke::Char('#') => (occurrence(chars, at, false)?, Span::Upto),
        Stroke::Char('G') => (
            line_numbered(chars, count.unwrap_or(usize::MAX)),
            Span::Lines,
        ),
        _ => return None,
    })
}

/// Emacs: what it remembers between keys.
#[derive(Default)]
pub struct Emacs {
    /// Where the selection starts, once set with Ctrl+Space.
    mark: Option<usize>,
    /// What was last killed, for Ctrl+Y to put back.
    killed: String,
    /// Ctrl+X was pressed: the next key ends the command.
    prefix: bool,
}

impl Emacs {
    /// What `key` does; `None` when the key is not one of Emacs's.
    pub fn key(
        &mut self,
        text: &mut String,
        selection: &Range<usize>,
        key: Stroke,
    ) -> Option<Done> {
        let mut chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        // The mark holds as long as the selection still starts or ends at it.
        self.mark = self
            .mark
            .filter(|mark| *mark == selection.start || *mark == selection.end);
        let mut at = match self.mark {
            Some(mark) if mark == selection.end => selection.start,
            _ => selection.end,
        }
        .min(len);
        let mut done = Done::default();
        if std::mem::take(&mut self.prefix) {
            match key {
                Stroke::Ctrl('s') => done.ask = Some(Ask::Save),
                Stroke::Ctrl('f') => done.ask = Some(Ask::Open),
                Stroke::Char('h') => {
                    self.mark = Some(0);
                    at = len;
                }
                Stroke::Char('u') => done.ask = Some(Ask::Undo),
                _ => {}
            }
            done.selection = self.selected(at);
            return Some(done);
        }
        let end = line_end(&chars, at);
        let region = self.selected(at);
        let kill = |chars: &mut Vec<char>, range: Range<usize>, killed: &mut String| {
            *killed = chars.drain(range.clone()).collect();
            range.start
        };
        match key {
            Stroke::Ctrl('x') => self.prefix = true,
            Stroke::Ctrl('f') => at = (at + 1).min(len),
            Stroke::Ctrl('b') => at = at.saturating_sub(1),
            Stroke::Ctrl('n') => at = vertical(&chars, at, true),
            Stroke::Ctrl('p') => at = vertical(&chars, at, false),
            Stroke::Ctrl('a') => at = line_start(&chars, at),
            Stroke::Ctrl('e') => at = end,
            Stroke::Alt('f') => at = word_after(&chars, at),
            Stroke::Alt('b') => at = word_back(&chars, at),
            Stroke::Ctrl(' ') => self.mark = Some(at),
            Stroke::Ctrl('g') => self.mark = None,
            Stroke::Ctrl('d') => {
                if at < len {
                    chars.remove(at);
                }
            }
            // The rest of the line; at its end, the line end itself.
            Stroke::Ctrl('k') => {
                let to = if end == at { (at + 1).min(len) } else { end };
                at = kill(&mut chars, at..to, &mut self.killed);
                done.copied = Some(self.killed.clone());
            }
            Stroke::Alt('d') => {
                let to = word_after(&chars, at);
                at = kill(&mut chars, at..to, &mut self.killed);
                done.copied = Some(self.killed.clone());
            }
            Stroke::Ctrl('w') | Stroke::Alt('w') => {
                if key == Stroke::Ctrl('w') {
                    at = kill(&mut chars, region, &mut self.killed);
                } else {
                    self.killed = chars[region].iter().collect();
                }
                done.copied = Some(self.killed.clone());
                self.mark = None;
            }
            Stroke::Ctrl('y') => {
                chars.splice(at..at, self.killed.chars());
                at += self.killed.chars().count();
                self.mark = None;
            }
            Stroke::Ctrl('/' | '_') => done.ask = Some(Ask::Undo),
            Stroke::Ctrl('s' | 'r') => done.ask = Some(Ask::Find),
            _ => return None,
        }
        done.changed = chars.len() != len || matches!(key, Stroke::Ctrl('y'));
        if done.changed {
            *text = chars.iter().collect();
            self.mark = self.mark.map(|mark| mark.min(chars.len()));
        }
        done.selection = self.selected(at);
        Some(done)
    }

    /// From the mark to `at`, when there is a mark.
    fn selected(&self, at: usize) -> Range<usize> {
        let mark = self.mark.unwrap_or(at);
        mark.min(at)..mark.max(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Type `keys` in Vim, from the char `at`. Returns the text and the caret.
    fn vim(text: &str, at: usize, keys: &str) -> (String, usize) {
        let mut text = text.to_owned();
        let done = type_in(&mut Vim::default(), &mut text, at, keys);
        (text, done.selection.start)
    }

    /// Type `keys` in `vim`, from the char `at`. Returns what the last of them did.
    fn type_in(vim: &mut Vim, text: &mut String, at: usize, keys: &str) -> Done {
        let mut last = Done {
            selection: at..at,
            ..Done::default()
        };
        for key in keys.chars() {
            let stroke = match key {
                '\u{1b}' => Stroke::Escape,
                '\r' => Stroke::Enter,
                key => Stroke::Char(key),
            };
            match vim.key(text, &last.selection, stroke) {
                Some(done) => last = done,
                // Insert mode: the text field would write it.
                None => {
                    let at = last.selection.start;
                    text.insert(crate::byte_index(text, at), key);
                    last.selection = at + 1..at + 1;
                }
            }
        }
        last
    }

    #[test]
    fn vim_moves_by_chars_words_and_lines() {
        let text = "let alpha = 1;\n  beta(two)\n\nend";
        assert_eq!(vim(text, 0, "l").1, 1);
        assert_eq!(vim(text, 0, "3l").1, 3);
        assert_eq!(vim(text, 0, "h").1, 0);
        assert_eq!((vim(text, 0, "w").1, vim(text, 0, "2w").1), (4, 10));
        assert_eq!((vim(text, 4, "e").1, vim(text, 6, "b").1), (8, 4));
        // The caret stays on the last char of a line, never after it.
        assert_eq!((vim(text, 0, "$").1, vim(text, 5, "0").1), (13, 0));
        assert_eq!((vim(text, 20, "^").1, vim(text, 5, "j").1), (17, 20));
        assert_eq!((vim(text, 5, "jj").1, vim(text, 27, "k").1), (27, 15));
        assert_eq!((vim(text, 0, "G").1, vim(text, 27, "gg").1), (28, 0));
        assert_eq!(vim(text, 0, "2G").1, 15);
    }

    #[test]
    fn vim_deletes_changes_and_pastes() {
        assert_eq!(vim("one two three", 4, "dw"), ("one three".to_owned(), 4));
        assert_eq!(
            vim("one two three", 4, "cwX\u{1b}"),
            ("one X three".to_owned(), 4)
        );
        assert_eq!(vim("one two", 0, "x"), ("ne two".to_owned(), 0));
        assert_eq!(vim("one two", 4, "D"), ("one ".to_owned(), 3));
        assert_eq!(vim("a\nb\nc", 2, "dd"), ("a\nc".to_owned(), 2));
        assert_eq!(vim("a\nb\nc", 4, "dd"), ("a\nb".to_owned(), 2));
        assert_eq!(vim("a\nb\nc\nd", 0, "2dd"), ("c\nd".to_owned(), 0));
        // Lines taken out go back whole, under the line of the caret or above it.
        assert_eq!(vim("a\nb\nc", 0, "ddp"), ("b\na\nc".to_owned(), 2));
        assert_eq!(vim("a\nb", 2, "yyP"), ("a\nb\nb".to_owned(), 2));
        assert_eq!(vim("a\nb", 2, "yyp"), ("a\nb\nb".to_owned(), 4));
        assert_eq!(vim("ab", 0, "xp"), ("ba".to_owned(), 1));
        assert_eq!(vim("one two", 0, "ywP"), ("one one two".to_owned(), 3));
        assert_eq!(vim("  one\ntwo", 2, "cc"), ("\ntwo".to_owned(), 0));
        assert_eq!(vim("abc", 1, "rX"), ("aXc".to_owned(), 1));
        assert_eq!(vim("a\n  b", 0, "J"), ("a b".to_owned(), 1));
    }

    #[test]
    fn vim_inserts_and_selects() {
        assert_eq!(vim("ac", 0, "ab\u{1b}"), ("abc".to_owned(), 1));
        assert_eq!(vim("bc", 1, "Ia\u{1b}"), ("abc".to_owned(), 0));
        assert_eq!(vim("ab", 0, "Ac\u{1b}"), ("abc".to_owned(), 2));
        assert_eq!(vim("  a\nb", 2, "ox\u{1b}"), ("  a\n  x\nb".to_owned(), 6));
        assert_eq!(vim("  a", 2, "Ox\u{1b}"), ("  x\n  a".to_owned(), 2));
        assert_eq!(vim("one two three", 0, "vwd"), ("wo three".to_owned(), 0));
        assert_eq!(vim("a\nb\nc", 0, "Vjd"), ("c".to_owned(), 0));
        assert_eq!(vim("one two", 0, "vey$p"), ("one twoone".to_owned(), 9));

        let mut vim = Vim::default();
        let mut text = "one two".to_owned();
        let done = vim.key(&mut text, &(0..0), Stroke::Char('v')).expect("vim");
        assert_eq!((vim.mode, done.selection.clone()), (Mode::Visual, 0..1));
        let done = vim
            .key(&mut text, &done.selection, Stroke::Char('e'))
            .expect("vim");
        assert_eq!(done.selection, 0..3);
        // A click moved the caret: the selection is over.
        let done = vim.key(&mut text, &(5..5), Stroke::Char('l')).expect("vim");
        assert_eq!((vim.mode, done.selection), (Mode::Normal, 6..6));
        let done = vim.key(&mut text, &(6..6), Stroke::Char('u')).expect("vim");
        assert_eq!(done.ask, Some(Ask::Undo));
        // In insert mode only Escape is Vim's.
        vim.key(&mut text, &(6..6), Stroke::Char('i'));
        assert_eq!(vim.key(&mut text, &(6..6), Stroke::Char('x')), None);
    }

    #[test]
    fn vim_finds_on_the_line_and_takes_text_objects() {
        let text = "one two three";
        assert_eq!((vim(text, 0, "fw").1, vim(text, 0, "tw").1), (5, 4));
        assert_eq!((vim(text, 0, "fe;").1, vim(text, 0, "fe;;,").1), (11, 11));
        assert_eq!((vim(text, 12, "Fo").1, vim(text, 12, "To").1), (6, 7));
        assert_eq!(vim(text, 0, "2fe").1, 11);
        // Nothing to find: the caret stays, and nothing is taken out.
        assert_eq!(vim(text, 0, "dfz"), (text.to_owned(), 0));
        assert_eq!(vim(text, 0, "dfw"), ("o three".to_owned(), 0));
        assert_eq!(vim(text, 0, "dtw"), ("wo three".to_owned(), 0));
        assert_eq!(vim(text, 12, "dFo"), ("one twe".to_owned(), 6));
        assert_eq!(vim(text, 5, "diw"), ("one  three".to_owned(), 4));
        assert_eq!(vim(text, 5, "daw"), ("one three".to_owned(), 4));
        assert_eq!(vim(text, 5, "viwd"), ("one  three".to_owned(), 4));
        assert_eq!(vim("f(a, (b)) + c", 3, "di("), ("f() + c".to_owned(), 2));
        assert_eq!(vim("f(a, (b)) + c", 3, "da("), ("f + c".to_owned(), 1));
        assert_eq!(
            vim("f(a, (b)) + c", 6, "dib"),
            ("f(a, ()) + c".to_owned(), 6)
        );
        assert_eq!(vim("a {\n  b\n}", 5, "di{"), ("a {}".to_owned(), 3));
        assert_eq!(
            vim("say \"hi\" now", 6, "ci\"X\u{1b}"),
            ("say \"X\" now".to_owned(), 5)
        );
        assert_eq!(vim("a [1] b", 0, "di["), ("a [1] b".to_owned(), 0));
    }

    #[test]
    fn vim_goes_to_partners_and_to_the_same_word() {
        let text = "if (a[1] + f(b)) { one(); }\nlet one = one + done;";
        // From before a bracket to it, then to its partner; and back.
        assert_eq!((vim(text, 0, "%").1, vim(text, 15, "%").1), (15, 3));
        assert_eq!((vim(text, 5, "%").1, vim(text, 17, "%").1), (7, 26));
        assert_eq!(vim("a (b", 0, "%").1, 0);
        // The word under the caret, whole: `done` is not `one`.
        assert_eq!(vim(text, 19, "*").1, 32);
        assert_eq!(vim(text, 32, "*").1, 38);
        assert_eq!(vim(text, 38, "*").1, 19);
        assert_eq!(vim(text, 19, "#").1, 38);
        assert_eq!(vim("alone here", 2, "*").1, 2);
        // With an operator, to the partner and with it.
        assert_eq!(vim("f(a, b) + c", 1, "d%"), ("f + c".to_owned(), 1));
        assert_eq!(vim("aBc é", 0, "~~~"), ("AbC é".to_owned(), 3));
        assert_eq!(vim("ab", 0, "3~"), ("AB".to_owned(), 1));
    }

    #[test]
    fn vim_shifts_lines_and_takes_commands() {
        assert_eq!(vim("a\nb", 0, ">>"), ("    a\nb".to_owned(), 4));
        assert_eq!(vim("a\n\nb", 0, "3>>"), ("    a\n\n    b".to_owned(), 4));
        assert_eq!(
            vim("      a\n  b\n\tc", 0, "3<<"),
            ("  a\nb\nc".to_owned(), 2)
        );
        assert_eq!(vim("a\nb\nc", 0, "Vj>"), ("    a\n    b\nc".to_owned(), 4));
        assert_eq!(vim("    a", 4, "<<."), ("a".to_owned(), 0));
        // `:` and a number go to that line; a command is asked of the application.
        assert_eq!(vim("a\nb\nc", 0, ":3\r").1, 4);
        let mut vim = Vim::default();
        let mut text = "one".to_owned();
        let mut ask = |keys: &str| {
            let mut asked = None;
            for key in keys.chars() {
                let stroke = match key {
                    '\r' => Stroke::Enter,
                    '\u{8}' => Stroke::Backspace,
                    key => Stroke::Char(key),
                };
                asked = vim
                    .key(&mut text, &(1..1), stroke)
                    .and_then(|done| done.ask);
            }
            (asked, vim.typing().map(str::to_owned))
        };
        assert_eq!(ask(":w"), (None, Some("w".to_owned())));
        assert_eq!(ask("\r"), (Some(Ask::Save), None));
        assert_eq!(ask(":wq\r").0, Some(Ask::SaveClose));
        assert_eq!(ask(":q!\r").0, Some(Ask::Close));
        // Erased back past its start, the command is given up.
        assert_eq!(ask(":x\u{8}\u{8}"), (None, None));
        assert_eq!(text, "one");
    }

    #[test]
    fn vim_does_the_last_change_again() {
        assert_eq!(
            vim("one two three four", 0, "dw."),
            ("three four".to_owned(), 0)
        );
        assert_eq!(vim("a b c", 0, "x.."), (" c".to_owned(), 0));
        assert_eq!(vim("one\ntwo\nthree", 0, "dd."), ("three".to_owned(), 0));
        // What was typed in insert mode is typed again.
        assert_eq!(vim("ab", 0, "iX\u{1b}."), ("XXab".to_owned(), 0));
        assert_eq!(
            vim("one two one", 0, "cwX\u{1b}ww."),
            ("X two X".to_owned(), 6)
        );
        assert_eq!(vim("a\nb", 0, "A;\u{1b}j."), ("a;\nb;".to_owned(), 4));
        // A move is not a change: `.` still does the change before it.
        assert_eq!(vim("abcd", 0, "xl."), ("bd".to_owned(), 1));
        // Nothing done yet: nothing to do again.
        assert_eq!(vim("abc", 1, "."), ("abc".to_owned(), 1));
    }

    #[test]
    fn a_named_register_keeps_what_is_yanked_into_it() {
        // Something else is deleted meanwhile: the register still holds the word.
        assert_eq!(
            vim("one two three", 0, "\"aywwdw\"aP"),
            ("one one three".to_owned(), 7)
        );
        // From a selection too.
        assert_eq!(
            vim("one two", 0, "ve\"ay$\"ap"),
            ("one twoone".to_owned(), 9)
        );
        // A register nothing was put in pastes nothing.
        assert_eq!(vim("one", 0, "yw\"bp"), ("one".to_owned(), 0));
    }

    #[test]
    fn a_named_register_keeps_what_is_deleted_or_changed_into_it() {
        assert_eq!(vim("ab", 0, "\"bx\"bp"), ("ba".to_owned(), 1));
        assert_eq!(
            vim("one two", 0, "\"ccwX\u{1b}\"cp"),
            ("Xone two".to_owned(), 3)
        );
        assert_eq!(vim("one two", 4, "\"dDyl\"dp"), ("one two".to_owned(), 6));
    }

    #[test]
    fn lines_kept_in_a_named_register_are_pasted_as_lines() {
        // The line in `a` goes under the caret's; the one copied since, with a plain `p`.
        assert_eq!(vim("a\nb\nc", 0, "\"addyy\"ap"), ("b\na\nc".to_owned(), 2));
        assert_eq!(
            vim("a\nb\nc", 0, "\"addyy\"app"),
            ("b\na\nb\nc".to_owned(), 4)
        );
        assert_eq!(vim("a\nb", 2, "\"ayyk\"aP"), ("b\na\nb".to_owned(), 0));
    }

    #[test]
    fn a_count_goes_before_or_after_the_name_of_the_register() {
        assert_eq!(
            vim("a\nb\nc\nd", 0, "\"a2ddG\"ap"),
            ("c\nd\na\nb".to_owned(), 4)
        );
        assert_eq!(
            vim("a\nb\nc\nd", 0, "2\"addG\"ap"),
            ("c\nd\na\nb".to_owned(), 4)
        );
    }

    #[test]
    fn a_capital_letter_adds_to_the_register_of_its_small_letter() {
        assert_eq!(
            vim("one two three", 0, "\"ayww\"Ayw\"aP"),
            ("one one two two three".to_owned(), 11)
        );
        assert_eq!(
            vim("a\nb\nc", 0, "\"ayyj\"Ayy\"ap"),
            ("a\nb\na\nb\nc".to_owned(), 4)
        );
        // Lines added to a few chars: the whole is lines.
        assert_eq!(
            vim("one x\ntwo", 0, "\"aywj\"Ayy\"ap"),
            ("one x\ntwo\none \ntwo".to_owned(), 10)
        );
        // A capital letter reads the same register as its small one.
        assert_eq!(vim("ab", 0, "\"ayl\"Ap"), ("aab".to_owned(), 1));
    }

    #[test]
    fn the_black_hole_keeps_nothing_and_leaves_what_was_kept() {
        assert_eq!(vim("a\nb\nc", 0, "yyj\"_ddP"), ("a\na\nc".to_owned(), 2));
        assert_eq!(vim("ab", 0, "yl\"_x\"_p"), ("b".to_owned(), 0));
        // Nothing goes to the clipboard either.
        let (mut vim, mut text) = (Vim::default(), "one two".to_owned());
        let done = type_in(&mut vim, &mut text, 0, "\"_dw");
        assert_eq!((text.as_str(), done.copied), ("two", None));
    }

    #[test]
    fn register_zero_holds_the_last_yank_whatever_is_deleted_after() {
        assert_eq!(vim("a\nb\nc", 0, "yyjdd\"0p"), ("a\nc\na".to_owned(), 4));
        assert_eq!(vim("one two", 0, "ywx\"0P"), ("one ne two".to_owned(), 3));
        // A yank into a register with a name is not the one it holds.
        assert_eq!(vim("a\nb", 0, "yyj\"byy\"0P"), ("a\na\nb".to_owned(), 2));
    }

    #[test]
    fn the_register_without_a_name_gets_what_goes_into_a_named_one() {
        assert_eq!(vim("one two", 0, "\"aywP"), ("one one two".to_owned(), 3));
        assert_eq!(vim("a\nb", 0, "\"addp"), ("b\na".to_owned(), 2));
    }

    #[test]
    fn only_the_register_without_a_name_and_the_clipboard_ones_reach_the_clipboard() {
        let (mut vim, mut text) = (Vim::default(), "one two".to_owned());
        let done = type_in(&mut vim, &mut text, 0, "yw");
        assert_eq!(done.copied.as_deref(), Some("one "));
        let done = type_in(&mut vim, &mut text, 4, "\"ayw");
        assert_eq!(done.copied, None);
        let done = type_in(&mut vim, &mut text, 4, "\"+yw");
        assert_eq!(done.copied.as_deref(), Some("two"));
        let done = type_in(&mut vim, &mut text, 0, "\"*yw");
        assert_eq!(done.copied.as_deref(), Some("one "));
        // They paste what the register without a name holds.
        type_in(&mut vim, &mut text, 0, "\"+P");
        type_in(&mut vim, &mut text, 0, "\"*P");
        assert_eq!(text, "one one one two");
    }

    #[test]
    fn a_key_that_names_no_register_gives_the_command_up() {
        assert_eq!(vim("a\nb", 0, "\"!dd"), ("b".to_owned(), 0));
        assert_eq!(vim("ab", 0, "\"\u{1b}x"), ("b".to_owned(), 0));
        // The register is for one command: a move uses it up.
        assert_eq!(vim("ab cd", 0, "yl\"awdw\"ap"), ("ab ".to_owned(), 2));
        // Quotes are still what `f` goes to and what a text object takes.
        assert_eq!(vim("a \"b\" c", 0, "f\"").1, 2);
        assert_eq!(vim("a \"b\" c", 3, "di\""), ("a \"\" c".to_owned(), 3));
        assert_eq!(vim("a q b @", 0, "fq").1, 2);
        assert_eq!(vim("abc", 1, "rq"), ("aqc".to_owned(), 1));
    }

    #[test]
    fn the_last_change_is_done_again_with_its_register() {
        assert_eq!(
            vim("a\nb\nc\nd", 0, "\"Add.\"aP"),
            ("a\nb\nc\nd".to_owned(), 0)
        );
        assert_eq!(vim("abcd", 0, "\"ax.\"ap"), ("cbd".to_owned(), 1));
    }

    #[test]
    fn a_recording_is_known_from_its_start_to_its_end() {
        let (mut vim, mut text) = (Vim::default(), "a q b".to_owned());
        assert_eq!(vim.recording(), None);
        type_in(&mut vim, &mut text, 0, "q");
        assert_eq!(vim.recording(), None);
        type_in(&mut vim, &mut text, 0, "a");
        assert_eq!(vim.recording(), Some('a'));
        // A `q` that another key was waiting for does not end it.
        let done = type_in(&mut vim, &mut text, 0, "fq");
        assert_eq!((vim.recording(), done.selection), (Some('a'), 2..2));
        type_in(&mut vim, &mut text, 2, "q");
        assert_eq!(vim.recording(), None);
        assert_eq!(type_in(&mut vim, &mut text, 0, "@a").selection, 2..2);
        // `q` then a key that names no register starts nothing.
        type_in(&mut vim, &mut text, 0, "q\u{1b}");
        assert_eq!((vim.recording(), text.as_str()), (None, "a q b"));
    }

    #[test]
    fn a_macro_does_again_the_keys_that_were_recorded() {
        assert_eq!(
            vim("one two three four", 0, "qadwq@a"),
            ("three four".to_owned(), 0)
        );
        // With what was typed in insert mode.
        assert_eq!(
            vim("a\nb\nc", 0, "qaA;\u{1b}jq@a@a"),
            ("a;\nb;\nc;".to_owned(), 7)
        );
        assert_eq!(vim("one two", 0, "qavecX\u{1b}wq@a"), ("X X".to_owned(), 2));
        // With a register named inside it, and a command after a `:`.
        assert_eq!(vim("a\nb\nc", 0, "qa\"bddq@a\"bP"), ("b\nc".to_owned(), 0));
        assert_eq!(
            vim("a\nb\nc\nd", 0, "qa:3\rxq1G@a"),
            ("a\nb\n\nd".to_owned(), 4)
        );
    }

    #[test]
    fn a_count_plays_a_macro_that_many_times() {
        assert_eq!(vim("abcdef", 0, "qaxq3@a"), ("ef".to_owned(), 0));
        assert_eq!(vim("abcdefgh", 0, "qaxlq2@a"), ("bdfgh".to_owned(), 3));
        // The count is used up: the next command is done once.
        assert_eq!(vim("abcdef", 0, "qaxq2@ax"), ("ef".to_owned(), 0));
    }

    #[test]
    fn two_at_signs_play_the_macro_last_played() {
        assert_eq!(vim("abcdef", 0, "qaxq@a@@"), ("def".to_owned(), 0));
        assert_eq!(vim("abcdefgh", 0, "qaxqqbxxq@a2@@"), ("gh".to_owned(), 0));
        // None was played yet: nothing to play.
        assert_eq!(vim("abc", 0, "qaxq@@"), ("bc".to_owned(), 0));
        assert_eq!(vim("abc", 0, "@z"), ("abc".to_owned(), 0));
    }

    #[test]
    fn a_macro_is_kept_as_text_in_its_register() {
        // Its keys are pasted; a text copied into a register is played as keys.
        assert_eq!(vim("one two", 0, "qadwq\"ap"), ("tdwwo".to_owned(), 2));
        assert_eq!(vim("dw\none two", 0, "\"ayyj@a"), ("dw\ntwo".to_owned(), 3));
        assert_eq!(
            vim("ab", 0, "qaiX\u{1b}q\"ap"),
            ("XiX\u{1b}ab".to_owned(), 3)
        );
        // A capital letter adds keys to the macro.
        assert_eq!(vim("abcdefgh", 0, "qaxqqAxq@a"), ("efgh".to_owned(), 0));
        // Recorded again, it is replaced; recorded empty, it does nothing.
        assert_eq!(vim("abcdef", 0, "qaxxqqaxq@a"), ("ef".to_owned(), 0));
        assert_eq!(vim("abcdef", 0, "qaxqqaq@a"), ("bcdef".to_owned(), 0));
    }

    #[test]
    fn every_key_has_a_char_that_stands_for_it_in_a_register() {
        let keys = [
            Stroke::Char('x'),
            Stroke::Char('\t'),
            Stroke::Ctrl('r'),
            Stroke::Escape,
            Stroke::Enter,
            Stroke::Backspace,
            Stroke::Delete,
            Stroke::Left,
            Stroke::Right,
            Stroke::Up,
            Stroke::Down,
        ];
        for key in keys {
            assert_eq!(key_char(key).map(char_key), Some(key));
        }
        assert_eq!(key_char(Stroke::Alt('f')), None);
    }

    #[test]
    fn a_macro_plays_other_macros_and_is_recorded_as_one_key_in_another() {
        assert_eq!(vim("abcdefgh", 0, "qaxqqb@a@aq@b"), ("fgh".to_owned(), 0));
        // `.` inside a recording is one key, and after a macro it does its last change.
        assert_eq!(vim("abcdefgh", 0, "qax.q@a"), ("efgh".to_owned(), 0));
        assert_eq!(vim("abcdef", 0, "qaxq@a."), ("def".to_owned(), 0));
    }

    #[test]
    fn a_macro_ends_at_a_key_that_asks_something_of_the_application() {
        let (mut vim, mut text) = (Vim::default(), "abcd".to_owned());
        type_in(&mut vim, &mut text, 0, "qaxuxq");
        assert_eq!(text, "cd");
        let done = type_in(&mut vim, &mut text, 0, "@a");
        assert_eq!((text.as_str(), done.ask), ("d", Some(Ask::Undo)));
        assert!(done.changed);
    }

    #[test]
    fn a_macro_that_calls_itself_stops() {
        // Once while it is recorded, then once at each depth: on a small stack too.
        let small = std::thread::Builder::new().stack_size(256 * 1024);
        let played = small.spawn(|| vim(&"a".repeat(200), 0, "qax@aq@a"));
        let (text, _) = played.expect("a thread").join().expect("no overflow");
        assert_eq!(text.len(), 200 - 1 - MACRO_DEPTH);
        // Calling itself twice, it would go on for ever: the keys it may type run out.
        let (text, _) = vim(&"a".repeat(200), 0, "qa@a@axq@a");
        assert!(text.len() < 199);
        // Two macros that call each other stop as well.
        let (text, _) = vim(&"a".repeat(200), 0, "qbq qax@bq qbx@aq @a");
        assert!(text.len() < 198);
    }

    #[test]
    fn a_mark_brings_the_caret_back_to_its_line() {
        // To the first letter of the line, wherever on it the mark was set.
        assert_eq!(vim("one\n  two\nthree", 7, "magg'a").1, 6);
        assert_eq!(vim("one\n  two\nthree", 7, "maG'a").1, 6);
    }

    #[test]
    fn a_backtick_brings_the_caret_back_to_the_very_place_of_a_mark() {
        assert_eq!(vim("one\n  two\nthree", 7, "magg`a").1, 7);
    }

    #[test]
    fn each_letter_is_a_mark_of_its_own() {
        assert_eq!(vim("a\nb\nc", 0, "majmbG`a").1, 0);
        assert_eq!(vim("a\nb\nc", 0, "majmbG`b").1, 2);
        // Set again, a mark is where it was last set.
        assert_eq!(vim("a\nb\nc", 0, "majmagg`a").1, 2);
    }

    #[test]
    fn deleting_to_a_mark_after_a_quote_takes_whole_lines() {
        assert_eq!(vim("a\nb\nc\nd", 4, "makd'a"), ("a\nd".to_owned(), 2));
        assert_eq!(vim("a\nb\nc\nd", 2, "majd'a"), ("a\nd".to_owned(), 2));
    }

    #[test]
    fn deleting_to_a_mark_after_a_backtick_takes_the_chars_up_to_it() {
        assert_eq!(
            vim("one two three", 4, "mawd`a"),
            ("one three".to_owned(), 4)
        );
        assert_eq!(vim("one two three", 8, "ma0d`a"), ("three".to_owned(), 0));
    }

    #[test]
    fn copying_to_a_mark_takes_the_lines_up_to_it() {
        assert_eq!(
            vim("a\nb\nc", 0, "majy'aP"),
            ("a\na\nb\nb\nc".to_owned(), 2)
        );
    }

    #[test]
    fn changing_to_a_mark_types_in_place_of_the_lines_up_to_it() {
        assert_eq!(
            vim("a\nb\nc\nd", 2, "majc'aX\u{1b}"),
            ("a\nX\nd".to_owned(), 2)
        );
    }

    #[test]
    fn a_selection_goes_on_to_a_mark() {
        assert_eq!(vim("one two three", 0, "mawwv`ad"), ("hree".to_owned(), 0));
        assert_eq!(vim("a\nb\nc\nd", 0, "majjV'ad"), ("d".to_owned(), 0));
    }

    #[test]
    fn two_quotes_go_back_to_where_the_caret_was_before_a_jump() {
        let text = "  one\ntwo\nthree";
        // To the first letter of that line, or with backticks to the very place.
        assert_eq!(vim(text, 3, "G''").1, 2);
        assert_eq!(vim(text, 3, "G``").1, 3);
        assert_eq!(vim(text, 12, "gg``").1, 12);
        // Going back is a jump too: again, and the caret is where it came from.
        assert_eq!(vim(text, 3, "G''''").1, 10);
        assert_eq!(vim(text, 3, "G````").1, 10);
    }

    #[test]
    fn going_to_a_mark_is_a_jump_to_come_back_from() {
        assert_eq!(vim("  one\ntwo\nthree", 3, "maG'a``").1, 10);
    }

    #[test]
    fn going_to_a_partner_or_to_the_same_word_is_a_jump_to_come_back_from() {
        assert_eq!(vim("f(a) x", 1, "%``").1, 1);
        assert_eq!(vim("one two one", 0, "*``").1, 0);
        assert_eq!(vim("one two one", 8, "#``").1, 8);
    }

    #[test]
    fn going_to_a_line_by_its_number_is_a_jump_to_come_back_from() {
        assert_eq!(vim("a\nb\nc", 2, ":3\r``").1, 2);
    }

    #[test]
    fn a_plain_move_is_not_a_jump() {
        assert_eq!(vim("a\nb\nc", 0, "Gggjj``").1, 4);
        // No jump yet: nowhere to go back to.
        assert_eq!(vim("a\nb\nc", 2, "j''").1, 4);
    }

    #[test]
    fn a_mark_follows_its_text_when_lines_above_it_are_deleted() {
        assert_eq!(vim("a\nb\nc", 4, "maggdd`ax"), ("b\n".to_owned(), 2));
    }

    #[test]
    fn a_mark_follows_its_text_when_something_is_typed_above_it() {
        assert_eq!(
            vim("a\nb", 2, "maggOnew\u{1b}`ax"),
            ("new\na\n".to_owned(), 6)
        );
        assert_eq!(
            vim("ab\ncd", 0, "jmakcwlonger\u{1b}`ax"),
            ("longer\nd".to_owned(), 7)
        );
    }

    #[test]
    fn a_mark_follows_its_text_when_lines_are_pasted_or_pushed_in_above_it() {
        assert_eq!(vim("a\nb", 0, "yyjmakP`ax"), ("a\na\n".to_owned(), 4));
        assert_eq!(vim("a\nb", 2, "magg>>`ax"), ("    a\n".to_owned(), 6));
        assert_eq!(vim("a\n b\nc", 5, "maggJ`ax"), ("a b\n".to_owned(), 4));
    }

    #[test]
    fn a_mark_follows_its_text_when_a_substitution_changes_lines_above_it() {
        assert_eq!(
            vim("a\nb", 2, "ma:1s/a/long/\r`ax"),
            ("long\n".to_owned(), 5)
        );
    }

    #[test]
    fn a_mark_in_text_that_is_deleted_stays_where_that_text_was() {
        assert_eq!(vim("a\nb\nc", 2, "maddgg`a").1, 2);
    }

    #[test]
    fn a_mark_that_was_not_set_does_nothing() {
        assert_eq!(vim("a\nb", 2, "'z"), ("a\nb".to_owned(), 2));
        assert_eq!(vim("a\nb", 2, "`z"), ("a\nb".to_owned(), 2));
        // The operator that waited for it is given up.
        assert_eq!(vim("a\nb", 2, "d'zx"), ("a\n".to_owned(), 2));
        // Only small letters name a mark.
        assert_eq!(vim("a\nb", 2, "m1gg'1").1, 0);
    }

    #[test]
    fn a_mark_past_the_end_of_a_text_changed_elsewhere_is_its_end() {
        let (mut vim, mut text) = (Vim::default(), "a\nbcdef".to_owned());
        type_in(&mut vim, &mut text, 5, "ma");
        text = "a".to_owned();
        assert_eq!(type_in(&mut vim, &mut text, 0, "`a").selection, 0..0);
        assert_eq!(type_in(&mut vim, &mut text, 0, "d'a").selection, 0..0);
        assert_eq!(text, "");
    }

    #[test]
    fn marks_are_forgotten_when_the_application_says_so() {
        let (mut vim, mut text) = (Vim::default(), "a\nb".to_owned());
        type_in(&mut vim, &mut text, 2, "ma");
        vim.forget_marks();
        assert_eq!(type_in(&mut vim, &mut text, 0, "'a").selection, 0..0);
    }

    #[test]
    fn the_ends_of_the_last_selection_are_marks() {
        assert_eq!(vim("one two", 0, "ve\u{1b}$`<").1, 0);
        assert_eq!(vim("one two", 0, "ve\u{1b}$`>").1, 2);
    }

    #[test]
    fn a_substitution_changes_the_first_match_on_the_line_of_the_caret() {
        assert_eq!(vim("aa\naa", 3, ":s/a/b/\r"), ("aa\nba".to_owned(), 3));
        // The sign that ends it may be left out, and so may the replacement.
        assert_eq!(vim("abc", 0, ":s/b/X\r").0, "aXc");
        assert_eq!(vim("abc", 0, ":s/b\r").0, "ac");
        assert_eq!(vim("abc", 0, ":s/b/ \r").0, "a c");
    }

    #[test]
    fn the_flag_g_changes_every_match_on_the_line() {
        assert_eq!(vim("aa\naa", 3, ":s/a/b/g\r"), ("aa\nbb".to_owned(), 3));
    }

    #[test]
    fn a_percent_sign_is_for_every_line() {
        assert_eq!(vim("aa\naa", 0, ":%s/a/b/g\r"), ("bb\nbb".to_owned(), 3));
        assert_eq!(vim("aa\naa", 0, ":%s/a/b/\r"), ("ba\nba".to_owned(), 3));
    }

    #[test]
    fn two_numbers_are_the_first_and_the_last_line_to_substitute_in() {
        let text = "a\na\na\na";
        assert_eq!(vim(text, 0, ":2,3s/a/b/\r"), ("a\nb\nb\na".to_owned(), 4));
        assert_eq!(vim(text, 0, ":3,2s/a/b/\r").0, "a\nb\nb\na");
        assert_eq!(vim(text, 0, ":3s/a/b/\r"), ("a\na\nb\na".to_owned(), 4));
        // A dot is the line of the caret, a dollar the last one.
        assert_eq!(vim(text, 2, ":.,$s/a/b/\r").0, "a\nb\nb\nb");
    }

    #[test]
    fn a_colon_typed_on_a_selection_is_for_its_lines() {
        let (mut vim, mut text) = (Vim::default(), "a\na\na".to_owned());
        let done = type_in(&mut vim, &mut text, 2, "Vj:");
        assert_eq!((vim.mode, vim.typing()), (Mode::Normal, Some("'<,'>")));
        type_in(&mut vim, &mut text, done.selection.start, "s/a/b/\r");
        assert_eq!(text, "a\nb\nb");
    }

    #[test]
    fn the_lines_of_the_last_selection_can_be_typed_as_a_range() {
        assert_eq!(
            vim("a\na\na", 2, "Vj\u{1b}gg:'<,'>s/a/b/\r"),
            ("a\nb\nb".to_owned(), 4)
        );
        assert_eq!(vim("a\na\na", 0, "vj\u{1b}G:'<,'>d\r").0, "a");
    }

    #[test]
    fn the_lines_of_two_marks_can_be_typed_as_a_range() {
        assert_eq!(
            vim("a\nb\nc\nd", 2, "majmbgg:'a,'bd\r"),
            ("a\nd".to_owned(), 2)
        );
        // A mark that was not set makes no range: nothing is done.
        assert_eq!(vim("a\nb", 0, ":'a,'bd\r").0, "a\nb");
    }

    #[test]
    fn the_flag_i_finds_capitals_and_small_letters_alike() {
        assert_eq!(vim("A a", 0, ":s/a/b/\r").0, "A b");
        assert_eq!(vim("A a", 0, ":s/a/b/i\r").0, "b a");
        assert_eq!(vim("A a", 0, ":s/a/b/gi\r").0, "b b");
    }

    #[test]
    fn the_pattern_of_a_substitution_is_a_regular_expression() {
        assert_eq!(vim("a1 b22", 0, ":s/\\d+/N/g\r").0, "aN bN");
        assert_eq!(vim("a\nb", 0, ":%s/^/> /\r"), ("> a\n> b".to_owned(), 4));
        assert_eq!(vim("a  \nb ", 0, ":%s/\\s+$//\r").0, "a\nb");
    }

    #[test]
    fn the_groups_of_the_pattern_are_written_by_their_number() {
        assert_eq!(
            vim("one two", 0, ":s/(\\w+) (\\w+)/\\2 \\1/\r").0,
            "two one"
        );
        // A group the pattern does not have writes nothing.
        assert_eq!(vim("one", 0, ":s/(o)/\\1\\2!/\r").0, "o!ne");
    }

    #[test]
    fn an_ampersand_in_the_replacement_is_what_was_found() {
        assert_eq!(vim("foo", 0, ":s/o/[&]/g\r").0, "f[o][o]");
        // After a backslash it is itself, and so is a backslash.
        assert_eq!(vim("foo", 0, ":s/o/\\&/\r").0, "f&o");
        assert_eq!(vim("foo", 0, ":s/o/\\\\/\r").0, "f\\o");
    }

    #[test]
    fn a_line_end_in_the_replacement_cuts_the_line() {
        assert_eq!(vim("a, b", 0, ":s/, /\\r/\r").0, "a\nb");
    }

    #[test]
    fn another_sign_than_a_slash_goes_between_the_parts() {
        assert_eq!(vim("a/b", 0, ":s#a/b#c#\r").0, "c");
        assert_eq!(vim("a b", 0, ":s,a,c,\r").0, "c b");
        // After a backslash the sign is part of the text.
        assert_eq!(vim("a/b", 0, ":s/a\\/b/c\\/d/\r").0, "c/d");
        // A letter goes between nothing: that is another command.
        assert_eq!(vim("set", 0, ":set\r").0, "set");
    }

    #[test]
    fn a_pattern_that_cannot_be_read_changes_nothing() {
        let (mut vim, mut text) = (Vim::default(), "a(b".to_owned());
        let done = type_in(&mut vim, &mut text, 1, ":s/(/x/\r");
        assert_eq!((text.as_str(), done.changed), ("a(b", false));
        assert_eq!(done.selection, 1..1);
        // Neither does a flag that is not known.
        let done = type_in(&mut vim, &mut text, 1, ":s/a/x/c\r");
        assert_eq!((text.as_str(), done.changed), ("a(b", false));
    }

    #[test]
    fn a_substitution_in_lines_the_text_does_not_have_changes_nothing() {
        assert_eq!(vim("a\na", 0, ":2,9s/a/b/\r"), ("a\na".to_owned(), 0));
        assert_eq!(vim("a\na", 0, ":9s/a/b/\r").0, "a\na");
        assert_eq!(vim("a\na", 0, ":0,1s/a/b/\r").0, "a\na");
    }

    #[test]
    fn a_pattern_that_is_not_found_changes_nothing() {
        let (mut vim, mut text) = (Vim::default(), "abc".to_owned());
        let done = type_in(&mut vim, &mut text, 1, ":%s/z/y/g\r");
        assert_eq!((text.as_str(), done.changed), ("abc", false));
        assert_eq!(done.selection, 1..1);
    }

    #[test]
    fn a_substitution_is_one_change_made_when_enter_is_pressed() {
        let (mut vim, mut text) = (Vim::default(), "aa\naa".to_owned());
        let done = type_in(&mut vim, &mut text, 0, ":%s/a/b/g");
        assert_eq!((text.as_str(), done.changed), ("aa\naa", false));
        let done = type_in(&mut vim, &mut text, 0, "\r");
        assert_eq!((text.as_str(), done.changed), ("bb\nbb", true));
        assert_eq!((done.copied, done.ask, vim.typing()), (None, None, None));
    }

    #[test]
    fn the_caret_goes_to_the_first_letter_of_the_last_line_substituted_in() {
        assert_eq!(vim("  a\n  a\nz", 0, ":%s/a/b/\r").1, 6);
    }

    #[test]
    fn the_last_substitution_is_done_again_without_its_flags() {
        assert_eq!(
            vim("a a\na a", 0, ":s/a/b/g\rj&"),
            ("b b\nb a".to_owned(), 4)
        );
        assert_eq!(vim("a a\na a", 0, ":s/a/b/g\rj:s\r").0, "b b\nb a");
        // Without a pattern, the one of the last substitution.
        assert_eq!(vim("a a", 0, ":s/a/b/\r:s//c/\r").0, "b c");
        // None was done yet: nothing to do again.
        assert_eq!(vim("a", 0, "&"), ("a".to_owned(), 0));
        assert_eq!(vim("a", 0, ":s\r:s//b/\r").0, "a");
    }

    #[test]
    fn a_dollar_after_a_colon_goes_to_the_last_line() {
        assert_eq!(vim("a\nb\nc", 0, ":$\r").1, 4);
        // Of two lines, the caret goes to the second.
        assert_eq!(vim("a\nb\nc", 0, ":2,3\r").1, 4);
    }

    #[test]
    fn lines_are_deleted_by_their_numbers() {
        assert_eq!(vim("a\nb\nc\nd\ne", 0, ":2,4d\r"), ("a\ne".to_owned(), 2));
        assert_eq!(vim("a\nb\nc\nd\ne", 0, ":4,2d\r").0, "a\ne");
        assert_eq!(vim("a\nb\nc", 0, ":3d\r"), ("a\nb".to_owned(), 2));
        assert_eq!(vim("a\nb\nc", 0, ":%d\r"), (String::new(), 0));
        // Without a number, the line of the caret.
        assert_eq!(vim("a\nb\nc", 2, ":d\r"), ("a\nc".to_owned(), 2));
        assert_eq!(vim("a\nb\nc", 2, ":delete\r").0, "a\nc");
    }

    #[test]
    fn lines_deleted_by_their_numbers_are_kept_to_paste() {
        let (mut vim, mut text) = (Vim::default(), "a\nb\nc\nd\ne".to_owned());
        let done = type_in(&mut vim, &mut text, 0, ":2,4d\r");
        assert_eq!(done.copied.as_deref(), Some("b\nc\nd\n"));
        assert!(done.changed);
        type_in(&mut vim, &mut text, 2, "p");
        assert_eq!(text, "a\ne\nb\nc\nd");
    }

    #[test]
    fn deleting_lines_the_text_does_not_have_changes_nothing() {
        assert_eq!(vim("a\nb", 0, ":9d\r"), ("a\nb".to_owned(), 0));
        assert_eq!(vim("a\nb", 0, ":2,9d\r").0, "a\nb");
    }

    #[test]
    fn a_command_that_is_not_known_does_nothing() {
        let (mut vim, mut text) = (Vim::default(), "a\nb".to_owned());
        let done = type_in(&mut vim, &mut text, 2, ":noh\r");
        assert_eq!((text.as_str(), done.selection), ("a\nb", 2..2));
        assert_eq!((done.changed, done.ask), (false, None));
        assert!(!type_in(&mut vim, &mut text, 2, ":2dog\r").changed);
        assert_eq!(type_in(&mut vim, &mut text, 2, ":\r").selection, 2..2);
    }

    #[test]
    fn a_count_before_the_dot_takes_the_place_of_the_one_the_change_had() {
        assert_eq!(vim("a\nb\nc\nd\ne\nf", 0, "2dd3."), ("f".to_owned(), 0));
        assert_eq!(vim("abcdef", 0, "x3."), ("ef".to_owned(), 0));
        assert_eq!(vim("a b c d e f g", 0, "d2w3."), ("f g".to_owned(), 0));
        assert_eq!(vim("a b c d e f g", 0, "2dw3."), ("f g".to_owned(), 0));
    }

    #[test]
    fn the_count_given_to_the_dot_stays_for_the_next_dot() {
        assert_eq!(vim("abcdefgh", 0, "x3.."), ("h".to_owned(), 0));
        assert_eq!(vim("abcdefgh", 0, "3x.2."), ("".to_owned(), 0));
    }

    #[test]
    fn the_count_given_to_the_dot_is_not_for_the_command_after_it() {
        assert_eq!(vim("abcdef", 0, "x2.x"), ("ef".to_owned(), 0));
        // Nothing to do again: the count is used up all the same.
        assert_eq!(vim("abc", 0, "3.x"), ("bc".to_owned(), 0));
        // And it is not part of the change made after it.
        assert_eq!(vim("abcdefgh", 0, "x2.lx."), ("dgh".to_owned(), 1));
    }

    #[test]
    fn a_count_before_the_dot_keeps_the_register_of_the_change() {
        assert_eq!(
            vim("a\nb\nc\nd\ne\nf", 0, "\"a2dd3.\"ap"),
            ("f\nc\nd\ne".to_owned(), 2)
        );
        assert_eq!(
            vim("a\nb\nc\nd\ne\nf", 0, "2\"add3.\"ap"),
            ("f\nc\nd\ne".to_owned(), 2)
        );
    }

    #[test]
    fn a_change_up_to_the_start_of_the_line_is_done_again_without_the_count() {
        let keys = |keys: &str| keys.chars().map(Stroke::Char).collect::<Vec<_>>();
        assert_eq!(counted(&keys("d0"), 3), keys("d0"));
        assert_eq!(counted(&keys("10dd"), 3), keys("3dd"));
        assert_eq!(counted(&keys("r5"), 12), keys("12r5"));
        assert_eq!(vim("abcdef ghi", 3, "d0$2."), ("i".to_owned(), 0));
    }

    #[test]
    fn deleted_lines_go_to_register_one_and_the_older_ones_go_down() {
        assert_eq!(vim("a\nb\nc", 0, "dddd\"1P\"2P"), ("a\nb\nc".to_owned(), 0));
        assert_eq!(vim("a\nb\nc", 0, "dddd\"2p"), ("c\na".to_owned(), 2));
    }

    #[test]
    fn what_register_nine_held_is_lost_at_the_next_delete() {
        let lines: Vec<String> = (0..10).map(|number| number.to_string()).collect();
        let text = lines.join("\n") + "\nend";
        let deletes = "dd".repeat(10);
        assert_eq!(vim(&text, 0, &format!("{deletes}\"9p")).0, "end\n1");
        assert_eq!(vim(&text, 0, &format!("{deletes}\"1p")).0, "end\n9");
        assert_eq!(vim(&text, 0, &format!("{deletes}\"5p")).0, "end\n5");
    }

    #[test]
    fn less_than_a_line_deleted_goes_to_the_small_delete_register() {
        // The lines deleted before are still in register one.
        assert_eq!(vim("ab\ncd", 0, "ddx\"-p\"1p"), ("dc\nab".to_owned(), 3));
        assert_eq!(vim("one two", 0, "dwdw\"-P").0, "two");
        // Nothing went to register one.
        assert_eq!(vim("ab", 0, "x\"1p").0, "b");
    }

    #[test]
    fn chars_deleted_over_a_line_end_go_to_register_one() {
        assert_eq!(vim("ab\ncd", 1, "vjd\"1p"), ("ab\ncd".to_owned(), 4));
        assert_eq!(vim("ab\ncd", 1, "vjd\"-p").0, "a");
    }

    #[test]
    fn changed_lines_go_to_register_one() {
        assert_eq!(vim("a\nb", 0, "ccX\u{1b}\"1p"), ("X\na\nb".to_owned(), 2));
    }

    #[test]
    fn copying_leaves_the_numbered_registers_as_they_are() {
        assert_eq!(vim("a\nb", 0, "ddyy\"1p").0, "b\na");
        assert_eq!(vim("a\nb", 0, "yy\"1p").0, "a\nb");
    }

    #[test]
    fn a_delete_into_a_named_register_or_the_black_hole_shifts_nothing() {
        assert_eq!(vim("a\nb\nc", 0, "dd\"add\"1p").0, "c\na");
        assert_eq!(vim("a\nb\nc", 0, "dd\"_dd\"1p").0, "c\na");
        assert_eq!(vim("a\nb\nc", 0, "dd\"_dd\"2p").0, "c");
    }

    #[test]
    fn emacs_moves_kills_and_yanks() {
        let mut emacs = Emacs::default();
        let mut text = "one two\nthree".to_owned();
        let mut key = |text: &mut String, selection: Range<usize>, key| {
            emacs.key(text, &selection, key).expect("an Emacs key")
        };
        assert_eq!(key(&mut text, 0..0, Stroke::Ctrl('e')).selection, 7..7);
        assert_eq!(key(&mut text, 7..7, Stroke::Ctrl('n')).selection, 13..13);
        assert_eq!(key(&mut text, 13..13, Stroke::Ctrl('a')).selection, 8..8);
        assert_eq!(key(&mut text, 0..0, Stroke::Alt('f')).selection, 3..3);
        assert_eq!(key(&mut text, 3..3, Stroke::Alt('f')).selection, 7..7);
        assert_eq!(key(&mut text, 7..7, Stroke::Alt('b')).selection, 4..4);
        // The rest of the line is killed, then put back twice.
        let done = key(&mut text, 4..4, Stroke::Ctrl('k'));
        assert_eq!(
            (text.as_str(), done.copied.as_deref()),
            ("one \nthree", Some("two"))
        );
        key(&mut text, 4..4, Stroke::Ctrl('y'));
        let done = key(&mut text, 7..7, Stroke::Ctrl('y'));
        assert_eq!(
            (text.as_str(), done.selection),
            ("one twotwo\nthree", 10..10)
        );
        // From the mark to the caret is the region.
        key(&mut text, 0..0, Stroke::Ctrl(' '));
        let done = key(&mut text, 0..0, Stroke::Ctrl('f'));
        assert_eq!(done.selection, 0..1);
        let done = key(&mut text, 0..1, Stroke::Alt('f'));
        assert_eq!(done.selection, 0..3);
        let done = key(&mut text, 0..3, Stroke::Ctrl('w'));
        assert_eq!((text.as_str(), done.selection), (" twotwo\nthree", 0..0));
        key(&mut text, 0..0, Stroke::Ctrl('x'));
        assert_eq!(key(&mut text, 0..0, Stroke::Ctrl('s')).ask, Some(Ask::Save));
        assert_eq!(emacs.key(&mut text, &(0..0), Stroke::Char('a')), None);
    }
}
