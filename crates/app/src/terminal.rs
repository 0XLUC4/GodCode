//! The terminal panel: a shell fed line by line, its output shown as it comes.
//!
//! The shell runs on pipes, not on a real console: programs print and read lines as usual,
//! but full-screen programs (editors, pagers) have nothing to draw on.

use std::io::{Read, Write as _};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use egui::{
    Context, Event, EventFilter, FontId, Id, Key, Label, RichText, ScrollArea, Sense, Ui, pos2,
    vec2,
};

use crate::theme::Palette;
use crate::tools;

const MAX_LINES: usize = 5000;
const NEWLINE: &str = if cfg!(windows) { "\r\n" } else { "\n" };

/// Where the output is inside an escape sequence, which is dropped.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Escape {
    None,
    Started,
    /// `ESC [ … letter`: colours, cursor moves.
    Control,
    /// `ESC ] … BEL`: window title and the like.
    System,
}

pub struct Terminal {
    child: Child,
    stdin: Option<ChildStdin>,
    incoming: Receiver<Vec<u8>>,
    /// What was printed; the last line is the one still being written.
    lines: Vec<String>,
    /// Bytes of a character cut in two by the pipe.
    undecoded: Vec<u8>,
    escape: Escape,
    /// A carriage return was printed: the next character starts the line over.
    returned: bool,
    input: String,
    history: Vec<String>,
    /// Entry of the history shown in the input, while walking through it.
    recalled: Option<usize>,
    /// Give the keyboard to the input on the next frame.
    pub focus: bool,
    running: bool,
}

fn shell() -> Command {
    if cfg!(windows) {
        // UTF-8 in and out, then PowerShell: on pipes it still prints its prompt.
        let mut command = Command::new("cmd");
        command.args(["/D", "/C", "chcp 65001 >NUL & powershell -NoLogo"]);
        command
    } else {
        let name = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_owned());
        let mut command = Command::new(name);
        command.arg("-i");
        command
    }
}

impl Terminal {
    /// Start a shell in `folder`, with `values` added to its environment.
    pub fn start(
        folder: &Path,
        ctx: &Context,
        values: &[(String, String)],
    ) -> Result<Self, String> {
        let mut command = shell();
        command
            .current_dir(folder)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Colours would arrive as escape codes; unbuffered so prints show at once.
            .env("NO_COLOR", "1")
            .env("PYTHONUNBUFFERED", "1")
            .env("PYTHONUTF8", "1")
            .envs(values.iter().map(|(name, value)| (name, value)));
        tools::hidden(&mut command);
        let mut child = command
            .spawn()
            .map_err(|err| format!("Cannot start the terminal: {err}"))?;
        let (sender, incoming) = channel();
        let pipes: [Option<Box<dyn Read + Send>>; 2] = [
            child.stdout.take().map(|pipe| Box::new(pipe) as _),
            child.stderr.take().map(|pipe| Box::new(pipe) as _),
        ];
        for mut pipe in pipes.into_iter().flatten() {
            let sender = sender.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let mut buffer = [0_u8; 4096];
                while let Ok(read) = pipe.read(&mut buffer) {
                    if read == 0 || sender.send(buffer[..read].to_vec()).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
                ctx.request_repaint();
            });
        }
        Ok(Self {
            stdin: child.stdin.take(),
            child,
            incoming,
            lines: vec![String::new()],
            undecoded: Vec::new(),
            escape: Escape::None,
            returned: false,
            input: String::new(),
            history: Vec::new(),
            recalled: None,
            focus: true,
            running: true,
        })
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
    }

    /// Hand `line` to the shell, or to the program reading its input.
    pub fn send(&mut self, line: &str) {
        if !line.trim().is_empty() && self.history.last().map(String::as_str) != Some(line) {
            self.history.push(line.to_owned());
        }
        self.recalled = None;
        // The shell at its prompt repeats the command; a program reading its input does
        // not, so the line is written here to stay on screen.
        let at_prompt = self.lines.iter().all(String::is_empty)
            || (cfg!(windows)
                && self
                    .lines
                    .last()
                    .is_some_and(|last| last.starts_with("PS ") && last.trim_end().ends_with('>')));
        if !at_prompt {
            self.print(&format!("{line}\n"));
        }
        // Nothing to clear on a pipe: wipe the panel and ask for a fresh prompt.
        let line = if matches!(line.trim(), "cls" | "clear") {
            self.clear();
            ""
        } else {
            line
        };
        let written = self.stdin.as_mut().is_some_and(|stdin| {
            write!(stdin, "{line}{NEWLINE}")
                .and_then(|()| stdin.flush())
                .is_ok()
        });
        if !written {
            self.print("\n[The shell has ended. Restart the terminal.]\n");
        }
    }

    /// End the shell and everything it started.
    pub fn stop(&mut self) {
        self.stdin = None;
        #[cfg(windows)]
        {
            let mut kill = Command::new("taskkill");
            kill.args(["/PID", &self.child.id().to_string(), "/T", "/F"]);
            tools::hidden(&mut kill);
            // Best effort: the shell may be gone already.
            let _ = kill.output();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.running = false;
    }

    /// Take in what the shell printed since the last frame.
    pub fn poll(&mut self) {
        loop {
            match self.incoming.try_recv() {
                Ok(bytes) => self.feed(&bytes),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.running {
                        self.running = false;
                        self.print("\n[The shell has ended.]\n");
                    }
                    break;
                }
            }
        }
    }

    /// Decode output as UTF-8, keeping a character split across two reads for the next one.
    fn feed(&mut self, bytes: &[u8]) {
        let mut pending = std::mem::take(&mut self.undecoded);
        pending.extend_from_slice(bytes);
        let mut rest = pending.as_slice();
        while !rest.is_empty() {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    self.print(text);
                    break;
                }
                Err(error) => {
                    let (valid, after) = rest.split_at(error.valid_up_to());
                    self.print(&String::from_utf8_lossy(valid));
                    match error.error_len() {
                        Some(len) => {
                            self.print("\u{FFFD}");
                            rest = &after[len..];
                        }
                        None => {
                            self.undecoded = after.to_vec();
                            break;
                        }
                    }
                }
            }
        }
    }

    fn print(&mut self, text: &str) {
        for ch in text.chars() {
            match (self.escape, ch) {
                (Escape::None, '\u{1b}') => self.escape = Escape::Started,
                (Escape::None, '\n') => {
                    self.lines.push(String::new());
                    self.returned = false;
                }
                (Escape::None, '\r') => self.returned = true,
                (Escape::None, '\u{8}') => {
                    self.line().pop();
                }
                (Escape::None, _) if ch == '\t' || !ch.is_control() => {
                    if std::mem::take(&mut self.returned) {
                        self.line().clear();
                    }
                    self.line().push(ch);
                }
                (Escape::None, _) => {}
                (Escape::Started, '[') => self.escape = Escape::Control,
                (Escape::Started, ']') => self.escape = Escape::System,
                (Escape::Started, _) => self.escape = Escape::None,
                (Escape::Control, '@'..='~') | (Escape::System, '\u{7}') => {
                    self.escape = Escape::None;
                }
                (Escape::System, '\u{1b}') => self.escape = Escape::Started,
                (Escape::Control | Escape::System, _) => {}
            }
        }
        if self.lines.len() > MAX_LINES {
            let extra = self.lines.len() - MAX_LINES;
            self.lines.drain(..extra);
        }
    }

    fn line(&mut self) -> &mut String {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        let last = self.lines.len() - 1;
        &mut self.lines[last]
    }

    /// Up / Down walk through the commands typed before.
    fn recall(&mut self, older: bool) {
        let last = self.history.len().checked_sub(1);
        self.recalled = match (self.recalled, older) {
            (None, true) => last,
            (Some(index), true) => Some(index.saturating_sub(1)),
            (Some(index), false) if Some(index) < last => Some(index + 1),
            _ => None,
        };
        self.input = self
            .recalled
            .map(|index| self.history[index].clone())
            .unwrap_or_default();
    }

    /// Take what was typed since the last frame into the command line.
    fn keys(&mut self, ui: &Ui) -> bool {
        let mut typed = false;
        for event in ui.input(|input| input.events.clone()) {
            match event {
                // Only the first line of a paste: the rest would run unseen.
                Event::Text(text) | Event::Paste(text) => {
                    self.input.push_str(text.lines().next().unwrap_or_default());
                }
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if !modifiers.command => match key {
                    Key::Enter => {
                        let line = std::mem::take(&mut self.input);
                        self.send(&line);
                    }
                    Key::Backspace => {
                        self.input.pop();
                    }
                    Key::ArrowUp => self.recall(true),
                    Key::ArrowDown => self.recall(false),
                    Key::Escape => self.input.clear(),
                    _ => continue,
                },
                _ => continue,
            }
            typed = true;
        }
        typed
    }

    /// Show the terminal over the rest of `ui`. A click anywhere in it takes the keyboard;
    /// what is typed is written after the prompt.
    pub fn show(&mut self, ui: &mut Ui) {
        let palette = Palette::of(ui);
        let font = FontId::monospace(13.0);
        let (row_height, glyph) =
            ui.fonts_mut(|fonts| (fonts.row_height(&font), fonts.glyph_width(&font, ' ')));
        let id = Id::new("terminal");
        let area = ui.available_rect_before_wrap();
        let panel = ui.interact(area, id, Sense::click());
        // The lines of output sit on top and take their own clicks (to select text).
        let clicked_inside = ui.input(|input| {
            input.pointer.primary_clicked()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|pos| area.contains(pos))
        });
        if std::mem::take(&mut self.focus) || clicked_inside {
            panel.request_focus();
        }
        let focused = panel.has_focus();
        let mut typed = false;
        if focused {
            // Tab, the arrows and Escape belong to the command line, not to the focus.
            let filter = EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: true,
            };
            ui.memory_mut(|memory| memory.set_focus_lock_filter(id, filter));
            typed = self.keys(ui);
        }
        ui.spacing_mut().item_spacing.y = 0.0;
        let last = self.lines.len().saturating_sub(1);
        ScrollArea::both()
            .id_salt("terminal-output")
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show_rows(ui, row_height, self.lines.len(), |ui, range| {
                let text =
                    |shown: &str| RichText::new(shown).font(font.clone()).color(palette.text);
                for index in range {
                    let line = &self.lines[index];
                    if index < last {
                        // An empty label has no height: keep the blank lines.
                        let shown = if line.is_empty() { " " } else { line.as_str() };
                        ui.add(Label::new(text(shown)).extend());
                        continue;
                    }
                    // The line being written: what the shell printed, what is typed, and
                    // room for the caret.
                    let shown = format!("{line}{} ", self.input);
                    let row = ui.add(Label::new(text(&shown)).extend());
                    if focused {
                        let blink_on = ui.input(|input| input.time) % 1.0 < 0.5;
                        if blink_on {
                            let top = pos2(row.rect.max.x - glyph, row.rect.min.y);
                            let caret = egui::Rect::from_min_size(top, vec2(2.0, row_height));
                            ui.painter().rect_filled(caret, 0.0, palette.text);
                        }
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(500));
                    }
                    if typed {
                        row.scroll_to_me(None);
                    }
                }
            });
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn the_shell_runs_what_it_is_sent() {
        let ctx = Context::default();
        let folder = std::env::temp_dir();
        let mut terminal = Terminal::start(&folder, &ctx, &[]).expect("a shell");
        // The answer differs from the command, which the shell may echo.
        terminal.send("echo (40+2)");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let answered = |terminal: &Terminal| terminal.lines.iter().any(|line| line.trim() == "42");
        while !answered(&terminal) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
            terminal.poll();
        }
        assert!(answered(&terminal), "printed: {:?}", terminal.lines);
        terminal.stop();
        assert!(!terminal.is_running());
    }

    /// One frame of the panel, with scripted input.
    #[cfg(windows)]
    fn frame(ctx: &Context, terminal: &mut Terminal, time: f64, events: Vec<Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                pos2(0.0, 0.0),
                vec2(800.0, 300.0),
            )),
            events,
            time: Some(time),
            focused: true,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| terminal.show(ui));
    }

    #[cfg(windows)]
    #[test]
    fn typing_in_the_panel_writes_the_command_line() {
        let ctx = Context::default();
        let mut terminal = Terminal::start(&std::env::temp_dir(), &ctx, &[]).expect("a shell");
        frame(&ctx, &mut terminal, 0.0, Vec::new());
        frame(
            &ctx,
            &mut terminal,
            0.1,
            vec![Event::Text("echo (40+3)x".to_owned())],
        );
        let key = |key| Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, &mut terminal, 0.2, vec![key(Key::Backspace)]);
        assert_eq!(terminal.input, "echo (40+3)");
        frame(&ctx, &mut terminal, 0.3, vec![key(Key::Enter)]);
        assert_eq!(terminal.input, "");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let answered = |terminal: &Terminal| terminal.lines.iter().any(|line| line.trim() == "43");
        while !answered(&terminal) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
            terminal.poll();
        }
        assert!(answered(&terminal), "printed: {:?}", terminal.lines);
        // The arrow brings the command back.
        frame(&ctx, &mut terminal, 0.4, vec![key(Key::ArrowUp)]);
        assert_eq!(terminal.input, "echo (40+3)");
    }

    #[test]
    fn output_is_cleaned_of_escape_codes() {
        let ctx = Context::default();
        let mut terminal = Terminal::start(&std::env::temp_dir(), &ctx, &[]).expect("a shell");
        terminal.stop();
        terminal.clear();
        terminal.feed(b"\x1b[31mred\x1b[0m\r\n10%\r50%\r\ncaf\xC3");
        terminal.feed(b"\xA9\x1b]0;title\x07!");
        assert_eq!(terminal.lines, ["red", "50%", "café!"]);
    }
}
