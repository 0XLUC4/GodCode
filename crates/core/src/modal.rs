//! The Vim and Emacs ways of editing: what a key does to a text and its caret. Places are
//! counted in chars.

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
    /// A key that waits for a char: `f`, `F`, `t`, `T` for the one to go to, `i` or `a` for
    /// the text object to take.
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
        let mut chars: Vec<char> = text.chars().collect();
        let mut done = Done::default();
        if self.mode == Mode::Insert {
            if key != Stroke::Escape {
                return None;
            }
            let at = selection.start.min(chars.len());
            // What was typed since insert mode was entered, for `.` to type again.
            if let (Some((from, length)), false) = (self.insert_from.take(), self.replaying) {
                let typed = at
                    .checked_sub(from)
                    .filter(|typed| length + typed == chars.len());
                self.last_typed = match typed {
                    Some(typed) => chars[from..from + typed].iter().collect(),
                    None => String::new(),
                };
            }
            // Out of insert mode the caret is on a char: the one just typed.
            let at = at.max(line_start(&chars, at) + 1) - 1;
            self.mode = Mode::Normal;
            done.selection = at..at;
            self.shown = done.selection.clone();
            return Some(done);
        }
        let mut visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        if visual && *selection != self.shown {
            self.mode = Mode::Normal;
            visual = false;
        }
        let idle = self.pending.is_none() && self.waiting.is_none();
        if key == Stroke::Char('.') && idle && !visual {
            return Some(self.repeat(text, selection));
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
        match (pending, key) {
            (_, Stroke::Escape) => self.mode = Mode::Normal,
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
            }
            (_, Stroke::Char(digit @ '0'..='9')) if digit != '0' || self.count > 0 => {
                self.count = self.count * 10 + digit as usize - '0' as usize;
                self.pending = pending;
                keep_count = true;
            }
            // An operator typed twice is done on whole lines.
            (Some(op), Stroke::Char(again)) if op == again && "dcy".contains(op) => {
                let last = (1..times).fold(at, |at, _| vertical(&chars, at, true));
                let range = lines(&chars, at, last, op == 'c');
                at = self.operate(&mut chars, (op, at), range, true, &mut done);
            }
            (_, Stroke::Char(prefix @ ('f' | 'F' | 't' | 'T'))) => {
                self.waiting = Some(prefix);
                self.pending = pending;
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
            _ => target = motion(&chars, at, key, counted),
        }
        if let Some((mut to, mut span)) = target {
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
        if !self.replaying {
            // A command that changed the text, or went to insert mode, is what `.` repeats.
            let complete = self.pending.is_none() && self.waiting.is_none() && !keep_count;
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

    /// `.`: the last command that changed the text, done again where the caret is now,
    /// with what was typed after it in insert mode.
    fn repeat(&mut self, text: &mut String, selection: &Range<usize>) -> Done {
        let (keys, typed) = (self.last.clone(), self.last_typed.clone());
        let mut done = Done {
            selection: selection.clone(),
            ..Done::default()
        };
        let step = |vim: &mut Self, text: &mut String, done: &mut Done, key: Stroke| {
            if let Some(did) = vim.key(text, &done.selection.clone(), key) {
                done.selection = did.selection;
                done.changed |= did.changed;
                done.copied = did.copied.or(done.copied.take());
            }
        };
        self.replaying = true;
        for key in keys {
            step(self, text, &mut done, key);
        }
        if self.mode == Mode::Insert {
            let at = done.selection.start;
            text.insert_str(crate::byte_index(text, at), &typed);
            let after = at + typed.chars().count();
            done.selection = after..after;
            done.changed = true;
            step(self, text, &mut done, Stroke::Escape);
        }
        self.replaying = false;
        self.shown = done.selection.clone();
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
        done.copied = Some(taken.clone());
        self.register = (taken, whole_lines);
        self.mode = Mode::Normal;
        if op == 'y' {
            // Copying lines leaves the caret where it is.
            return if whole_lines { at } else { range.start };
        }
        chars.drain(range.clone());
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
        let insert = |chars: &mut Vec<char>, at: usize, written: &str| {
            chars.splice(at..at, written.chars());
        };
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
            'd' | 'c' | 'y' | 'g' | 'r' => self.pending = Some(named),
            'x' => {
                let range = at..(at + times).min(end);
                return self.operate(chars, ('d', at), range, false, done);
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
                    insert(chars, end, &format!("\n{indent}"));
                    return end + 1 + indent.chars().count();
                }
                insert(chars, start, &format!("{indent}\n"));
                return start + indent.chars().count();
            }
            'p' | 'P' => {
                let (written, whole_lines) = self.register.clone();
                let length = written.chars().count();
                if length == 0 {
                    return at;
                }
                let after = named == 'p';
                return match (whole_lines, after) {
                    (true, true) => {
                        let lines = format!("\n{}", written.trim_end_matches('\n'));
                        insert(chars, end, &lines);
                        end + 1
                    }
                    (true, false) => {
                        insert(chars, start, &written);
                        start
                    }
                    (false, _) => {
                        let to = if after { (at + 1).min(end) } else { at };
                        insert(chars, to, &written);
                        to + length - 1
                    }
                };
            }
            'J' if end < chars.len() => {
                // The next line comes up, its indent becoming one space.
                let next = first_text(chars, end + 1);
                chars.splice(end..next, [' ']);
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
        let mut vim = Vim::default();
        let mut text = text.to_owned();
        let mut selection = at..at;
        for key in keys.chars() {
            let stroke = match key {
                '\u{1b}' => Stroke::Escape,
                key => Stroke::Char(key),
            };
            match vim.key(&mut text, &selection, stroke) {
                Some(done) => selection = done.selection,
                // Insert mode: the text field would write it.
                None => {
                    let byte = crate::byte_index(&text, selection.start);
                    text.insert(byte, key);
                    selection = selection.start + 1..selection.start + 1;
                }
            }
        }
        (text, selection.start)
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
