//! Application shell: menus, side bar, editor tabs, bottom panel, dialogs.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, Align2, Button, CentralPanel, Color32, Context, CornerRadius, Event, FontId, Frame, Id,
    Key, KeyboardShortcut, Layout, Margin, Modal, Modifiers, Panel, Rect, Response, RichText,
    ScrollArea, Sense, Stroke, TextEdit, TextureHandle, Ui, ViewportCommand, pos2, vec2,
};
use godcode_core::check::{self, Problem, Severity};
use godcode_core::git::{self, Change, Hunk};
use godcode_core::outline::{self, Symbol};
use godcode_core::search::{self, Hit, Match};
use godcode_core::syntax::Language;
use godcode_core::{Decoded, Indent, LineEnding, complete, edit, format, fuzzy};

use crate::explorer::{Action, Explorer};
use crate::terminal::Terminal;
use crate::theme::{self, Palette};
use crate::toasts::Toasts;
use crate::widgets::{self, icon, with_icon};
use crate::{APP_NAME, branding, editor, platform, tools};

const TAB_HEIGHT: f32 = 34.0;
/// Widest a tab's file name gets before it is cut with an ellipsis.
const TAB_TEXT_MAX: f32 = 180.0;
const ACTIVITY_WIDTH: f32 = 44.0;
const LIST_ROW: f32 = 22.0;
const MAX_RECENT: usize = 10;
const FONT_SIZE: f32 = 14.0;
const ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.6..=3.0;
/// Larger files are refused: the editor lays the whole text out.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_HITS: usize = 5000;
const MAX_PICKS: usize = 50;
const MAX_REPORTS: usize = 500;
/// A file with a line this long (in bytes) opens with soft wrap: it is minified, or prose.
const LONG_LINE: usize = 400;
const MAX_HISTORY: usize = 100;
const MAX_TODOS: usize = 2000;
/// Commands and symbols offered next to the files by "search everywhere".
const MAX_EXTRA_PICKS: usize = 8;
/// Above this size the word at the caret is not looked for on every move.
const MAX_OCCURRENCE_BYTES: usize = 512 * 1024;
/// Shift pressed twice within this many seconds opens "search everywhere".
const DOUBLE_TAP: f64 = 0.4;
/// What the TODO panel looks for.
const TODO_MARKS: [&str; 2] = ["TODO", "FIXME"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cmd {
    NewFile,
    OpenFile,
    OpenFolder,
    CloseFolder,
    Save,
    SaveAs,
    SaveAll,
    CloseTab,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Find,
    FindNext,
    FindPrevious,
    ReplaceOne,
    ReplaceAll,
    CloseFind,
    SearchInFiles,
    ToggleComment,
    Indent,
    Outdent,
    DuplicateLine,
    DeleteLine,
    MoveLineUp,
    MoveLineDown,
    Complete,
    GoToFile,
    GoToLine,
    GoToSymbol,
    Commands,
    NextTab,
    PreviousTab,
    Explorer,
    ToggleSidebar,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ToggleTheme,
    Home,
    Run,
    Check,
    NextProblem,
    Problems,
    ToggleTerminal,
    RestartTerminal,
    ClearTerminal,
    Shortcuts,
    SourceControl,
    Rename,
    Reformat,
    ToggleWrap,
    JoinLines,
    ToggleCase,
    ExpandSelection,
    ShrinkSelection,
    Declaration,
    FindUsages,
    Back,
    Forward,
    Structure,
    Todo,
    RecentFiles,
    CloseOthers,
    CloseAll,
    Reveal,
    CopyPath,
}

/// How a command shows in the menus, the command palette and the shortcut list.
struct Spec {
    cmd: Cmd,
    glyph: &'static str,
    label: &'static str,
    shortcut: &'static str,
}

const fn spec(cmd: Cmd, glyph: &'static str, label: &'static str, shortcut: &'static str) -> Spec {
    Spec {
        cmd,
        glyph,
        label,
        shortcut,
    }
}

const COMMANDS: &[Spec] = &[
    spec(Cmd::NewFile, icon::FILE_PLUS, "New File", "Ctrl+N"),
    spec(Cmd::OpenFile, icon::FILE_CODE, "Open File…", "Ctrl+O"),
    spec(
        Cmd::OpenFolder,
        icon::FOLDER_OPEN,
        "Open Folder…",
        "Ctrl+Alt+O",
    ),
    spec(Cmd::Save, icon::FLOPPY_DISK, "Save", "Ctrl+S"),
    spec(
        Cmd::SaveAs,
        icon::FLOPPY_DISK_BACK,
        "Save As…",
        "Ctrl+Shift+S",
    ),
    spec(Cmd::SaveAll, icon::FLOPPY_DISK, "Save All", "Ctrl+Alt+S"),
    spec(Cmd::CloseTab, icon::X, "Close Editor", "Ctrl+W"),
    spec(Cmd::CloseFolder, icon::FOLDER_MINUS, "Close Folder", ""),
    spec(Cmd::Quit, icon::SIGN_OUT, "Quit", ""),
    spec(Cmd::Undo, icon::ARROW_COUNTER_CLOCKWISE, "Undo", "Ctrl+Z"),
    spec(Cmd::Redo, icon::ARROW_CLOCKWISE, "Redo", "Ctrl+Y"),
    spec(Cmd::Cut, icon::SCISSORS, "Cut", "Ctrl+X"),
    spec(Cmd::Copy, icon::COPY, "Copy", "Ctrl+C"),
    spec(Cmd::Paste, icon::CLIPBOARD, "Paste", "Ctrl+V"),
    spec(Cmd::SelectAll, icon::SELECTION_ALL, "Select All", "Ctrl+A"),
    spec(
        Cmd::Find,
        icon::MAGNIFYING_GLASS,
        "Find and Replace",
        "Ctrl+F",
    ),
    spec(Cmd::FindNext, icon::ARROW_DOWN, "Find Next", "F3"),
    spec(
        Cmd::FindPrevious,
        icon::ARROW_UP,
        "Find Previous",
        "Shift+F3",
    ),
    spec(
        Cmd::SearchInFiles,
        icon::LIST_MAGNIFYING_GLASS,
        "Find in Files",
        "Ctrl+Shift+F",
    ),
    spec(
        Cmd::ToggleComment,
        icon::HASH,
        "Toggle Line Comment",
        "Ctrl+/",
    ),
    spec(Cmd::Indent, icon::TEXT_INDENT, "Indent Lines", "Tab"),
    spec(Cmd::Outdent, icon::ARROW_LEFT, "Outdent Lines", "Shift+Tab"),
    spec(
        Cmd::DuplicateLine,
        icon::COPY_SIMPLE,
        "Duplicate Line",
        "Ctrl+D",
    ),
    spec(Cmd::DeleteLine, icon::TRASH, "Delete Line", "Ctrl+Shift+K"),
    spec(Cmd::MoveLineUp, icon::ARROW_UP, "Move Line Up", "Alt+Up"),
    spec(
        Cmd::MoveLineDown,
        icon::ARROW_DOWN,
        "Move Line Down",
        "Alt+Down",
    ),
    spec(
        Cmd::Complete,
        icon::LIGHTNING,
        "Suggest Completions",
        "Ctrl+Space",
    ),
    spec(
        Cmd::GoToFile,
        icon::MAGNIFYING_GLASS,
        "Search Everywhere…",
        "Shift Shift",
    ),
    spec(
        Cmd::RecentFiles,
        icon::CLOCK_COUNTER_CLOCKWISE,
        "Recent Files",
        "Ctrl+E",
    ),
    spec(
        Cmd::GoToSymbol,
        icon::TREE_STRUCTURE,
        "Go to Symbol…",
        "Ctrl+Shift+O",
    ),
    spec(Cmd::GoToLine, icon::ARROW_RIGHT, "Go to Line…", "Ctrl+G"),
    spec(
        Cmd::Commands,
        icon::COMMAND,
        "Command Palette…",
        "Ctrl+Shift+P",
    ),
    spec(Cmd::NextTab, icon::ARROW_RIGHT, "Next Editor", "Ctrl+Tab"),
    spec(
        Cmd::PreviousTab,
        icon::ARROW_LEFT,
        "Previous Editor",
        "Ctrl+Shift+Tab",
    ),
    spec(Cmd::Explorer, icon::FILES, "Show Explorer", "Ctrl+Shift+E"),
    spec(
        Cmd::ToggleSidebar,
        icon::SIDEBAR_SIMPLE,
        "Toggle Side Bar",
        "Ctrl+B",
    ),
    spec(
        Cmd::ZoomIn,
        icon::MAGNIFYING_GLASS_PLUS,
        "Zoom In",
        "Ctrl++",
    ),
    spec(
        Cmd::ZoomOut,
        icon::MAGNIFYING_GLASS_MINUS,
        "Zoom Out",
        "Ctrl+-",
    ),
    spec(
        Cmd::ZoomReset,
        icon::MAGNIFYING_GLASS,
        "Reset Zoom",
        "Ctrl+0",
    ),
    spec(
        Cmd::ToggleTheme,
        icon::MOON,
        "Toggle Light / Dark Theme",
        "",
    ),
    spec(Cmd::Home, icon::HOUSE, "Welcome Page", ""),
    spec(Cmd::Run, icon::PLAY, "Run File", "F5"),
    spec(Cmd::Check, icon::BUG, "Check for Problems", "Ctrl+Shift+B"),
    spec(Cmd::NextProblem, icon::WARNING, "Go to Next Problem", "F8"),
    spec(
        Cmd::Problems,
        icon::WARNING_CIRCLE,
        "Toggle Problems",
        "Ctrl+Shift+M",
    ),
    spec(
        Cmd::ToggleTerminal,
        icon::TERMINAL_WINDOW,
        "Toggle Terminal",
        "Ctrl+J",
    ),
    spec(
        Cmd::RestartTerminal,
        icon::ARROW_CLOCKWISE,
        "Restart Terminal",
        "",
    ),
    spec(Cmd::ClearTerminal, icon::BROOM, "Clear Terminal", ""),
    spec(
        Cmd::SourceControl,
        icon::GIT_BRANCH,
        "Show Source Control",
        "Ctrl+Shift+G",
    ),
    spec(
        Cmd::Rename,
        icon::PENCIL_SIMPLE,
        "Rename All Occurrences…",
        "F2",
    ),
    spec(
        Cmd::Reformat,
        icon::MAGIC_WAND,
        "Reformat Code",
        "Ctrl+Alt+L",
    ),
    spec(
        Cmd::ToggleWrap,
        icon::ARROW_ELBOW_DOWN_LEFT,
        "Toggle Soft Wrap",
        "Alt+Z",
    ),
    spec(
        Cmd::JoinLines,
        icon::ARROWS_MERGE,
        "Join Lines",
        "Ctrl+Shift+J",
    ),
    spec(
        Cmd::ToggleCase,
        icon::TEXT_AA,
        "Toggle Case",
        "Ctrl+Shift+U",
    ),
    spec(
        Cmd::ExpandSelection,
        icon::SELECTION_PLUS,
        "Extend Selection",
        "Alt+Shift+Right",
    ),
    spec(
        Cmd::ShrinkSelection,
        icon::SELECTION,
        "Shrink Selection",
        "Alt+Shift+Left",
    ),
    spec(
        Cmd::Declaration,
        icon::CROSSHAIR,
        "Go to Declaration",
        "F12",
    ),
    spec(Cmd::FindUsages, icon::BINOCULARS, "Find Usages", "Alt+F7"),
    spec(Cmd::Back, icon::ARROW_BEND_UP_LEFT, "Back", "Ctrl+Alt+Left"),
    spec(
        Cmd::Forward,
        icon::ARROW_BEND_UP_RIGHT,
        "Forward",
        "Ctrl+Alt+Right",
    ),
    spec(Cmd::Structure, icon::TREE_STRUCTURE, "Show Structure", ""),
    spec(Cmd::Todo, icon::LIST_CHECKS, "Toggle TODO", ""),
    spec(Cmd::CloseOthers, icon::X_SQUARE, "Close Other Editors", ""),
    spec(Cmd::CloseAll, icon::X_SQUARE, "Close All Editors", ""),
    spec(
        Cmd::Reveal,
        icon::TARGET,
        "Select File in Explorer",
        "Alt+F1",
    ),
    spec(Cmd::CopyPath, icon::COPY, "Copy Path", "Ctrl+Shift+C"),
    spec(Cmd::Shortcuts, icon::KEYBOARD, "Keyboard Shortcuts", "F1"),
];

/// Keys of the editor that are not commands, for the shortcut list.
const EDITOR_KEYS: [(&str, &str); 8] = [
    ("Go to the declaration of a name", "Ctrl+Click"),
    ("Search everywhere", "Ctrl+P"),
    ("Close the tag being typed", ">"),
    ("Accept the completion", "Tab"),
    ("Move in the completions", "Up / Down"),
    ("Close the completions or the find bar", "Esc"),
    ("New line, indented like the code", "Enter"),
    ("Previous commands, in the terminal", "Up / Down"),
];

/// The menu bar; `None` is a separator.
const MENUS: &[(&str, &[Option<Cmd>])] = &[
    (
        "File",
        &[
            Some(Cmd::NewFile),
            Some(Cmd::OpenFile),
            Some(Cmd::OpenFolder),
            None,
            Some(Cmd::Save),
            Some(Cmd::SaveAs),
            Some(Cmd::SaveAll),
            None,
            Some(Cmd::CloseTab),
            Some(Cmd::CloseOthers),
            Some(Cmd::CloseAll),
            Some(Cmd::CloseFolder),
            Some(Cmd::Quit),
        ],
    ),
    (
        "Edit",
        &[
            Some(Cmd::Undo),
            Some(Cmd::Redo),
            None,
            Some(Cmd::Cut),
            Some(Cmd::Copy),
            Some(Cmd::Paste),
            Some(Cmd::SelectAll),
            None,
            Some(Cmd::Find),
            Some(Cmd::SearchInFiles),
            None,
            Some(Cmd::ExpandSelection),
            Some(Cmd::ShrinkSelection),
            Some(Cmd::CopyPath),
        ],
    ),
    (
        "Code",
        &[
            Some(Cmd::Reformat),
            Some(Cmd::Complete),
            Some(Cmd::Rename),
            None,
            Some(Cmd::ToggleComment),
            Some(Cmd::Indent),
            Some(Cmd::Outdent),
            None,
            Some(Cmd::DuplicateLine),
            Some(Cmd::DeleteLine),
            Some(Cmd::MoveLineUp),
            Some(Cmd::MoveLineDown),
            Some(Cmd::JoinLines),
            Some(Cmd::ToggleCase),
        ],
    ),
    (
        "View",
        &[
            Some(Cmd::Commands),
            Some(Cmd::Home),
            None,
            Some(Cmd::Explorer),
            Some(Cmd::SearchInFiles),
            Some(Cmd::ToggleSidebar),
            Some(Cmd::SourceControl),
            Some(Cmd::Structure),
            None,
            Some(Cmd::Problems),
            Some(Cmd::Todo),
            Some(Cmd::ToggleTerminal),
            None,
            Some(Cmd::ToggleWrap),
            Some(Cmd::ZoomIn),
            Some(Cmd::ZoomOut),
            Some(Cmd::ZoomReset),
            None,
            Some(Cmd::ToggleTheme),
        ],
    ),
    (
        "Navigate",
        &[
            Some(Cmd::GoToFile),
            Some(Cmd::RecentFiles),
            Some(Cmd::GoToSymbol),
            Some(Cmd::GoToLine),
            None,
            Some(Cmd::Declaration),
            Some(Cmd::FindUsages),
            Some(Cmd::Back),
            Some(Cmd::Forward),
            Some(Cmd::Reveal),
            None,
            Some(Cmd::FindNext),
            Some(Cmd::FindPrevious),
            Some(Cmd::NextProblem),
            None,
            Some(Cmd::NextTab),
            Some(Cmd::PreviousTab),
        ],
    ),
    (
        "Run",
        &[
            Some(Cmd::Run),
            Some(Cmd::Check),
            None,
            Some(Cmd::ToggleTerminal),
            Some(Cmd::RestartTerminal),
            Some(Cmd::ClearTerminal),
        ],
    ),
    ("Help", &[Some(Cmd::Shortcuts), Some(Cmd::Commands)]),
];

/// The right-click menu of the editor.
const EDITOR_MENU: &[Option<Cmd>] = &[
    Some(Cmd::Cut),
    Some(Cmd::Copy),
    Some(Cmd::Paste),
    Some(Cmd::SelectAll),
    None,
    Some(Cmd::Declaration),
    Some(Cmd::FindUsages),
    Some(Cmd::Rename),
    None,
    Some(Cmd::Reformat),
    Some(Cmd::ToggleComment),
    Some(Cmd::Find),
    None,
    Some(Cmd::Run),
];

fn spec_of(cmd: Cmd) -> Option<&'static Spec> {
    COMMANDS.iter().find(|spec| spec.cmd == cmd)
}

fn hash_of(value: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Where `problem` is underlined in `text`: the word at its place.
fn squiggle(text: &str, problem: &Problem) -> editor::Squiggle {
    let line_start = godcode_core::line_start(text, problem.line + 1);
    let line_len = text
        .chars()
        .skip(line_start)
        .take_while(|ch| *ch != '\n')
        .count();
    let start = line_start + problem.column.min(line_len);
    let len = text
        .chars()
        .skip(start)
        .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
        .count();
    editor::Squiggle {
        start,
        len: len.max(1),
        line: problem.line,
        error: problem.severity == Severity::Error,
        message: problem.message.clone(),
    }
}

/// One open file.
struct Tab {
    id: u64,
    name: String,
    path: Option<PathBuf>,
    text: String,
    /// Hash of the text as last saved or loaded.
    saved: u64,
    dirty: bool,
    /// Bumped on every change, so derived data knows when it is stale.
    revision: u64,
    language: &'static Language,
    ending: LineEnding,
    bom: bool,
    indent: Indent,
    view: editor::View,
    /// Sorted char range of the selection; empty for a plain caret.
    selection: Range<usize>,
    /// Problems found while typing (brackets).
    problems: Vec<Problem>,
    /// Those and the compiler's, placed in the text.
    squiggles: Vec<editor::Squiggle>,
    /// What `problems` and `squiggles` were computed for.
    checked: Option<u64>,
    /// Revision when the running check started.
    check_started: u64,
    /// Revision the compiler's reports are about; they hide once the text moves on.
    reports_at: Option<u64>,
    /// Lines changed since the last commit, and the revision they were found for.
    changes: Vec<Hunk>,
    changes_at: u64,
    /// Long lines go on below instead of running off the view.
    wrap: bool,
    /// The selections "extend selection" grew from, for "shrink selection" to return to.
    grown: Vec<Range<usize>>,
    /// When it was last in front, by the clock of the app: the order of the recent files.
    used: u64,
}

impl Tab {
    fn new(id: u64, name: String, path: Option<PathBuf>, decoded: Decoded) -> Self {
        Self {
            id,
            name,
            language: path.as_deref().map_or_else(Language::plain, Language::of),
            path,
            saved: hash_of(&decoded.text),
            dirty: false,
            revision: 0,
            ending: decoded.ending,
            bom: decoded.bom,
            indent: Indent::detect(&decoded.text),
            text: decoded.text,
            view: editor::View::focused(),
            selection: 0..0,
            problems: Vec::new(),
            squiggles: Vec::new(),
            checked: None,
            check_started: 0,
            reports_at: None,
            changes: Vec::new(),
            changes_at: 0,
            wrap: false,
            grown: Vec::new(),
            used: 0,
        }
    }

    fn editor_id(&self) -> Id {
        Id::new(("editor", self.id))
    }

    /// The text changed: derived data is stale, and it may differ from the file again.
    fn touched(&mut self) {
        self.revision += 1;
        self.dirty = hash_of(&self.text) != self.saved;
    }

    /// Select `range`, bring it into view and give the keyboard to the editor.
    fn select(&mut self, ctx: &Context, range: Range<usize>) {
        let len = self.text.chars().count();
        let range = range.start.min(len)..range.end.min(len);
        editor::set_selection(ctx, self.editor_id(), &range);
        self.view.scroll_to = Some(range.start);
        self.view.focus = true;
        self.selection = range;
    }

    fn set_path(&mut self, path: PathBuf) {
        self.name = file_name(&path);
        self.language = Language::of(&path);
        self.path = Some(path);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Explorer,
    Search,
    Git,
    Structure,
}

/// What the panel under the editor shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bottom {
    Problems,
    Todo,
    Terminal,
}

/// Find and replace in the active file.
#[derive(Default)]
struct Find {
    open: bool,
    query: String,
    replace: String,
    match_case: bool,
    /// Give the keyboard to the query field on the next frame.
    focus: bool,
    /// What `matches` was computed for.
    key: Option<u64>,
    matches: Vec<Match>,
}

/// Find in the files of the folder.
#[derive(Default)]
struct Search {
    query: String,
    match_case: bool,
    hits: Vec<Hit>,
    /// Results still coming from the search thread.
    incoming: Option<Receiver<Vec<Hit>>>,
    cancel: Arc<AtomicBool>,
    /// A search ran to its end, so "no results" means something.
    done: bool,
    focus: bool,
}

impl Search {
    fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.incoming = None;
    }
}

/// The palette: go to a file, run a command (`>`), go to a line (`:`) or a symbol (`@`).
struct Quick {
    query: String,
    selected: usize,
    focus: bool,
}

enum Pick {
    /// Path relative to the folder.
    File(String),
    /// An open editor, by its index.
    Tab(usize),
    Command(&'static Spec),
    Line(usize),
    Symbol(Symbol),
}

enum PromptKind {
    /// Create in this folder.
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    /// Rename every occurrence of this word in the active file.
    Word(String),
}

/// A question about a file of the tree.
struct Prompt {
    kind: PromptKind,
    name: String,
    focus: bool,
}

#[derive(Clone, Copy)]
enum Closing {
    Tab(usize),
    Window,
}

/// The Git state of the open folder.
struct Repo {
    branch: Option<String>,
    changes: Vec<Change>,
}

/// A place the caret was at, to come back to.
#[derive(Clone, PartialEq, Eq)]
struct Place {
    /// The tab it was in, and its file to open it again once closed.
    tab: u64,
    path: Option<PathBuf>,
    at: usize,
}

/// Shift pressed and released alone, twice in a row, opens "search everywhere".
#[derive(Default)]
struct ShiftTaps {
    down: bool,
    /// Nothing else was pressed while Shift was down.
    alone: bool,
    last: Option<f64>,
}

/// What the menu of a tab asks for.
#[derive(Clone, Copy)]
enum TabAction {
    CloseOthers,
    CloseRight,
    CloseAll,
    CopyPath,
    Reveal,
}

/// The changed lines of a tab: its id, the revision they are about, the lines.
type Diff = (u64, u64, Vec<Hunk>);

/// What a checker printed, with the folder it ran in; or why it could not run.
type CheckResult = Result<(PathBuf, String), String>;

pub struct GodApp {
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    dark: bool,
    zoom: f32,
    logo: Option<TextureHandle>,
    toasts: Toasts,
    closing: Option<Closing>,
    /// The user agreed to close the window with unsaved files.
    quit: bool,
    title: String,
    /// The home screen is shown instead of the editor.
    home: bool,
    /// Folders and files opened lately, latest first.
    recent: Vec<PathBuf>,
    explorer: Option<Explorer>,
    side: Side,
    sidebar: bool,
    bottom: Option<Bottom>,
    terminal: Option<Terminal>,
    find: Find,
    search: Search,
    /// Files of the folder, relative, for "go to file".
    index: Vec<String>,
    indexing: Option<Receiver<Vec<String>>>,
    /// The check in progress, and whether the user asked for it.
    checking: Option<(Receiver<CheckResult>, bool)>,
    /// What the last check found, by file.
    reports: Vec<(PathBuf, Problem)>,
    reports_version: u64,
    quick: Option<Quick>,
    prompt: Option<Prompt>,
    shortcuts: bool,
    /// Commands chosen in a dialog, run at the start of the next frame.
    pending: Vec<Cmd>,
    /// `None` when the folder is not a repository, or Git is not installed.
    repo: Option<Repo>,
    repo_incoming: Option<Receiver<Option<Repo>>>,
    commit_message: String,
    /// Where the diff threads send what they found.
    diffs: (Sender<Diff>, Receiver<Diff>),
    /// Soft wrap for the files opened from now on.
    wrap: bool,
    /// Places left by a jump, oldest first, and where "back" stands among them.
    history: Vec<Place>,
    history_at: usize,
    /// The outline of the active file, and what it was computed for.
    symbols: (u64, Vec<Symbol>),
    /// Line of the caret in the active file, counted from 0.
    caret_line: usize,
    /// The places of the word at the caret, and what they were computed for.
    occurrences: (u64, Vec<Match>),
    /// TODO and FIXME notes of the folder.
    todos: Vec<Hit>,
    todos_incoming: Option<Receiver<Vec<Hit>>>,
    shift: ShiftTaps,
    /// Counts the changes of the tab in front.
    clock: u64,
}

impl GodApp {
    /// `paths` (files or one folder) are opened at startup.
    pub fn new(cc: &eframe::CreationContext<'_>, paths: Vec<PathBuf>) -> Self {
        let get = |key: &str| cc.storage.and_then(|storage| storage.get_string(key));
        let dark = get("dark_mode").is_none_or(|value| value == "1");
        let zoom = get("zoom")
            .and_then(|value| value.parse::<f32>().ok())
            .map_or(1.0, |zoom| {
                zoom.clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end())
            });
        theme::install(&cc.egui_ctx, dark);
        widgets::install_icons(&cc.egui_ctx);
        let mut app = Self {
            tabs: Vec::new(),
            active: 0,
            next_id: 0,
            dark,
            zoom,
            logo: branding::logo_texture(&cc.egui_ctx),
            toasts: Toasts::default(),
            closing: None,
            quit: false,
            title: String::new(),
            home: paths.is_empty(),
            recent: get("recent")
                .map(|list| {
                    list.lines()
                        .filter(|line| !line.is_empty())
                        .map(PathBuf::from)
                        .collect()
                })
                .unwrap_or_default(),
            explorer: None,
            side: Side::Explorer,
            sidebar: get("sidebar").is_none_or(|value| value == "1"),
            bottom: None,
            terminal: None,
            find: Find::default(),
            search: Search::default(),
            index: Vec::new(),
            indexing: None,
            checking: None,
            reports: Vec::new(),
            reports_version: 0,
            quick: None,
            prompt: None,
            shortcuts: false,
            pending: Vec::new(),
            repo: None,
            repo_incoming: None,
            commit_message: String::new(),
            diffs: channel(),
            wrap: get("wrap").is_some_and(|value| value == "1"),
            history: Vec::new(),
            history_at: 0,
            symbols: (0, Vec::new()),
            caret_line: 0,
            occurrences: (0, Vec::new()),
            todos: Vec::new(),
            todos_incoming: None,
            shift: ShiftTaps::default(),
            clock: 0,
        };
        for path in paths {
            app.open_path(&cc.egui_ctx, &path);
        }
        app
    }

    fn at_home(&self) -> bool {
        self.home || (self.tabs.is_empty() && self.explorer.is_none())
    }

    fn folder(&self) -> Option<PathBuf> {
        self.explorer
            .as_ref()
            .map(|explorer| explorer.root().to_path_buf())
    }

    fn font_size(&self) -> f32 {
        FONT_SIZE * self.zoom
    }

    /// Put `path` first among the recent folders and files.
    fn remember(&mut self, path: &Path) {
        self.recent.retain(|known| known != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(MAX_RECENT);
    }

    // ------------------------------------------------------------ files and folders

    fn open_path(&mut self, ctx: &Context, path: &Path) {
        if path.is_dir() {
            self.open_folder(ctx, path.to_path_buf());
        } else {
            self.open_file(ctx, path);
        }
    }

    fn open_folder(&mut self, ctx: &Context, root: PathBuf) {
        self.search.stop();
        self.search.hits.clear();
        self.search.done = false;
        self.remember(&root);
        self.explorer = Some(Explorer::new(root));
        self.side = Side::Explorer;
        self.sidebar = true;
        self.home = false;
        self.reindex(ctx);
    }

    fn close_folder(&mut self) {
        self.search.stop();
        self.search.hits.clear();
        self.explorer = None;
        self.repo = None;
        self.index.clear();
        self.indexing = None;
    }

    /// Read the folder again, off the interface thread: its files and its Git state.
    fn reindex(&mut self, ctx: &Context) {
        self.refresh_git(ctx);
        let Some(root) = self.folder() else {
            return;
        };
        let (sender, receiver) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let files = search::files(&root, &AtomicBool::new(false));
            let relative: Vec<String> = files
                .iter()
                .filter_map(|path| path.strip_prefix(&root).ok())
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .collect();
            // The folder may have been closed meanwhile: nobody listens then.
            let _ = sender.send(relative);
            ctx.request_repaint();
        });
        self.indexing = Some(receiver);
    }

    /// Show the file at `path` in a tab, reading it unless it is open already.
    fn open_file(&mut self, ctx: &Context, path: &Path) -> bool {
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.path.as_deref() == Some(path))
        {
            self.active = index;
            self.home = false;
            self.tabs[index].view.focus = true;
            return true;
        }
        let name = file_name(path);
        if std::fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_FILE_BYTES) {
            self.toasts
                .error(format!("{name} is too large to edit here (over 2 MB)"));
            return false;
        }
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.toasts.error(format!("Cannot open {name}: {err}"));
                return false;
            }
        };
        let Some(decoded) = godcode_core::decode(&bytes) else {
            self.toasts
                .error(format!("{name} is not a UTF-8 text file"));
            return false;
        };
        // Files of the open folder are one click away already.
        if !self.folder().is_some_and(|root| path.starts_with(root)) {
            self.remember(path);
        }
        self.push_tab(name, Some(path.to_path_buf()), decoded);
        // The caret starts at the top, not where the text field would put it.
        let tab = &mut self.tabs[self.active];
        editor::set_selection(ctx, tab.editor_id(), &(0..0));
        // A minified file is one endless line: wrapped, it can at least be read.
        if !tab.wrap && format::longest_line(&tab.text) > LONG_LINE {
            tab.wrap = true;
            let hint = if format::restructures(tab.language) {
                ". Ctrl+Alt+L reformats it."
            } else {
                "."
            };
            let name = &tab.name;
            self.toasts
                .success(format!("{name} has very long lines: soft wrap is on{hint}"));
        }
        self.refresh_diff(ctx, self.active);
        ctx.request_repaint();
        true
    }

    fn push_tab(&mut self, name: String, path: Option<PathBuf>, decoded: Decoded) {
        self.next_id += 1;
        let mut tab = Tab::new(self.next_id, name, path, decoded);
        tab.wrap = self.wrap;
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        self.home = false;
    }

    fn new_file(&mut self) {
        let decoded = Decoded {
            text: String::new(),
            ending: LineEnding::Lf,
            bom: false,
        };
        let number = self.tabs.iter().filter(|tab| tab.path.is_none()).count() + 1;
        self.push_tab(format!("Untitled-{number}"), None, decoded);
    }

    /// Write tab `index` to disk, asking for a place first when it has none.
    fn save_tab(&mut self, ctx: &Context, index: usize, save_as: bool) -> bool {
        let Some(tab) = self.tabs.get(index) else {
            return false;
        };
        let path = match (&tab.path, save_as) {
            (Some(path), false) => path.clone(),
            _ => {
                let start = tab
                    .path
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
                    .or_else(|| self.folder());
                match platform::pick_save(&tab.name, start.as_deref()) {
                    Some(path) => path,
                    None => return false,
                }
            }
        };
        let bytes = godcode_core::encode(&tab.text, tab.ending, tab.bom);
        if let Err(err) = platform::write_atomic(&path, &bytes) {
            self.toasts.error(err);
            return false;
        }
        let tab = &mut self.tabs[index];
        tab.saved = hash_of(&tab.text);
        tab.dirty = false;
        if tab.path.as_deref() != Some(path.as_path()) {
            tab.set_path(path);
            self.files_changed(ctx);
        }
        // What is on disk is what the compiler sees: look for problems now.
        self.start_check(ctx, index, false);
        self.refresh_git(ctx);
        self.refresh_diff(ctx, index);
        true
    }

    /// Files were created, moved or removed: the tree and the file list are stale.
    fn files_changed(&mut self, ctx: &Context) {
        if let Some(explorer) = &mut self.explorer {
            explorer.refresh();
        }
        self.reindex(ctx);
    }

    fn close_tab(&mut self, ctx: &Context, index: usize) {
        let tab = self.tabs.remove(index);
        // Drop the undo history kept with the text field.
        TextEdit::store_state(ctx, tab.editor_id(), Default::default());
        if self.active > index || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
    }

    fn request_close(&mut self, ctx: &Context, index: usize) {
        match self.tabs.get(index) {
            Some(tab) if tab.dirty => self.closing = Some(Closing::Tab(index)),
            Some(_) => self.close_tab(ctx, index),
            None => {}
        }
    }

    // ------------------------------------------------------------ find

    /// Keep the occurrences in step with the query and the text.
    fn refresh_find(&mut self) {
        let find = &mut self.find;
        let Some(tab) = self.tabs.get(self.active).filter(|_| find.open) else {
            find.matches.clear();
            find.key = None;
            return;
        };
        let key = hash_of((tab.id, tab.revision, &find.query, find.match_case));
        if find.key != Some(key) {
            find.matches = search::find(&tab.text, &find.query, find.match_case);
            find.key = Some(key);
        }
    }

    /// The occurrence the selection sits on.
    fn current_match(&self) -> Option<usize> {
        let selection = &self.tabs.get(self.active)?.selection;
        self.find
            .matches
            .iter()
            .position(|found| found.start == selection.start && found.len == selection.len())
    }

    /// Select the next / previous occurrence, wrapping around.
    fn find_step(&mut self, ctx: &Context, forward: bool) {
        self.refresh_find();
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let matches = &self.find.matches;
        let found = if forward {
            matches
                .iter()
                .find(|found| found.start >= tab.selection.end && found.start > tab.selection.start)
                .or_else(|| {
                    matches
                        .iter()
                        .find(|found| found.start >= tab.selection.end)
                })
                .or(matches.first())
        } else {
            matches
                .iter()
                .rfind(|found| found.start < tab.selection.start)
                .or(matches.last())
        };
        if let Some(found) = found {
            tab.select(ctx, found.start..found.start + found.len);
            // The find bar keeps the keyboard: Enter goes on to the next one.
            tab.view.focus = false;
        }
    }

    fn replace_one(&mut self, ctx: &Context) {
        self.refresh_find();
        if self.current_match().is_some()
            && let Some(tab) = self.tabs.get_mut(self.active)
        {
            let start = godcode_core::byte_index(&tab.text, tab.selection.start);
            let end = godcode_core::byte_index(&tab.text, tab.selection.end);
            tab.text.replace_range(start..end, &self.find.replace);
            let caret = tab.selection.start + self.find.replace.chars().count();
            tab.touched();
            tab.select(ctx, caret..caret);
        }
        self.find_step(ctx, true);
    }

    fn replace_all(&mut self, ctx: &Context) {
        let find = &self.find;
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let (text, count) =
            search::replace_all(&tab.text, &find.query, &find.replace, find.match_case);
        if count > 0 {
            tab.text = text;
            tab.touched();
            let caret = tab.selection.start;
            tab.select(ctx, caret..caret);
        }
        let plural = if count == 1 { "" } else { "s" };
        self.toasts
            .success(format!("Replaced {count} occurrence{plural}"));
    }

    /// Search the files of the folder on another thread; results arrive in [`Self::poll`].
    fn start_search(&mut self, ctx: &Context) {
        self.search.stop();
        self.search.hits.clear();
        self.search.done = false;
        let Some(root) = self.folder() else {
            return;
        };
        if self.search.query.is_empty() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.search.cancel = Arc::clone(&cancel);
        let (sender, receiver) = channel();
        self.search.incoming = Some(receiver);
        let query = self.search.query.clone();
        let match_case = self.search.match_case;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for path in search::files(&root, &cancel) {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let hits = search::in_file(&path, &query, match_case);
                if !hits.is_empty() {
                    if sender.send(hits).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            }
            // Dropping the sender tells the interface the search is over.
            drop(sender);
            ctx.request_repaint();
        });
    }

    fn open_hit(&mut self, ctx: &Context, hit: &Hit) {
        self.record_jump();
        if self.open_file(ctx, &hit.path)
            && let Some(tab) = self.tabs.get_mut(self.active)
        {
            tab.select(ctx, hit.at.start..hit.at.start + hit.at.len);
        }
    }

    // ------------------------------------------------------------ problems, run, terminal

    /// Run the checker of tab `index`'s language on another thread; `manual` when the user
    /// asked, so that the outcome is reported either way.
    fn start_check(&mut self, ctx: &Context, index: usize, manual: bool) {
        if self.checking.is_some() {
            return;
        }
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let checker = tab
            .path
            .as_deref()
            .and_then(|path| tools::checker(tab.language.name, path));
        let Some(checker) = checker else {
            if manual {
                let language = tab.language.name;
                self.toasts
                    .error(format!("No checker known for {language} files"));
            }
            return;
        };
        for tab in &mut self.tabs {
            tab.check_started = tab.revision;
        }
        let (sender, receiver) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = tools::check(&checker).map(|printed| (checker.cwd.clone(), printed));
            // The window may be gone: nobody listens then.
            let _ = sender.send(result);
            ctx.request_repaint();
        });
        self.checking = Some((receiver, manual));
    }

    /// Collect what the background threads produced.
    fn poll(&mut self) {
        if let Some(receiver) = &self.indexing
            && let Ok(index) = receiver.try_recv()
        {
            self.index = index;
            self.indexing = None;
        }
        if let Some(terminal) = &mut self.terminal {
            terminal.poll();
        }
        if let Some(receiver) = &self.repo_incoming
            && let Ok(repo) = receiver.try_recv()
        {
            self.repo = repo;
            self.repo_incoming = None;
        }
        while let Ok((id, revision, changes)) = self.diffs.1.try_recv() {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                tab.changes = changes;
                tab.changes_at = revision;
            }
        }
        let search = &mut self.search;
        while let Some(receiver) = &search.incoming {
            match receiver.try_recv() {
                Ok(hits) => {
                    search.hits.extend(hits);
                    if search.hits.len() >= MAX_HITS {
                        search.hits.truncate(MAX_HITS);
                        search.stop();
                        search.done = true;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    search.incoming = None;
                    search.done = true;
                }
            }
        }
        while let Some(receiver) = &self.todos_incoming {
            match receiver.try_recv() {
                Ok(hits) => {
                    self.todos.extend(hits);
                    if self.todos.len() >= MAX_TODOS {
                        self.todos.truncate(MAX_TODOS);
                        self.todos_incoming = None;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => self.todos_incoming = None,
            }
        }
        if let Some((receiver, manual)) = &self.checking {
            let manual = *manual;
            match receiver.try_recv() {
                Ok(result) => {
                    self.checking = None;
                    self.checked(result, manual);
                }
                Err(TryRecvError::Disconnected) => self.checking = None,
                Err(TryRecvError::Empty) => {}
            }
        }
    }

    /// Take in the outcome of a check.
    fn checked(&mut self, result: CheckResult, manual: bool) {
        let (folder, printed) = match result {
            Ok(outcome) => outcome,
            Err(err) => {
                // Not having the compiler installed is no news on every save.
                if manual {
                    self.toasts.error(err);
                }
                return;
            }
        };
        self.reports = check::parse_output(&printed)
            .into_iter()
            .take(MAX_REPORTS)
            .map(|report| {
                let path = report.path.strip_prefix(".").unwrap_or(&report.path);
                (folder.join(path), report.problem)
            })
            .collect();
        self.reports_version += 1;
        for tab in &mut self.tabs {
            tab.reports_at = Some(tab.check_started);
        }
        if manual && self.reports.is_empty() {
            self.toasts.success("No problems found");
        } else if manual {
            self.bottom = Some(Bottom::Problems);
        }
    }

    /// Keep the problems of the active file in step with its text and the last check.
    fn refresh_problems(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let key = hash_of((tab.revision, self.reports_version));
        if tab.checked == Some(key) {
            return;
        }
        tab.checked = Some(key);
        tab.problems = check::brackets(&tab.text, tab.language);
        let fresh = tab.reports_at == Some(tab.revision);
        let reported = self
            .reports
            .iter()
            .filter(|(path, _)| fresh && tab.path.as_deref() == Some(path.as_path()))
            .map(|(_, problem)| problem);
        tab.squiggles = tab
            .problems
            .iter()
            .chain(reported)
            .map(|problem| squiggle(&tab.text, problem))
            .collect();
    }

    /// Every known problem: those of the active file found while typing, then the
    /// compiler's, leaving out files edited since.
    fn problems(&self) -> Vec<(Option<PathBuf>, Problem)> {
        let live = self.tabs.get(self.active).into_iter().flat_map(|tab| {
            tab.problems
                .iter()
                .map(|problem| (tab.path.clone(), problem.clone()))
        });
        let reported = self
            .reports
            .iter()
            .filter(|(path, _)| {
                self.tabs
                    .iter()
                    .find(|tab| tab.path.as_deref() == Some(path.as_path()))
                    .is_none_or(|tab| tab.reports_at == Some(tab.revision))
            })
            .map(|(path, problem)| (Some(path.clone()), problem.clone()));
        live.chain(reported).collect()
    }

    fn open_problem(&mut self, ctx: &Context, path: Option<&Path>, problem: &Problem) {
        self.record_jump();
        if path.is_some_and(|path| !self.open_file(ctx, path)) {
            return;
        }
        if let Some(tab) = self.tabs.get_mut(self.active) {
            let start = squiggle(&tab.text, problem).start;
            tab.select(ctx, start..start);
        }
    }

    fn next_problem(&mut self, ctx: &Context) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let mut starts: Vec<usize> = tab.squiggles.iter().map(|mark| mark.start).collect();
        starts.sort_unstable();
        let next = starts
            .iter()
            .find(|start| **start > tab.selection.start)
            .or(starts.first());
        if let Some(start) = next.copied() {
            tab.select(ctx, start..start);
        }
    }

    /// Make sure a shell is running; says why when it cannot start.
    fn ensure_terminal(&mut self, ctx: &Context) -> bool {
        if self.terminal.as_ref().is_some_and(Terminal::is_running) {
            return true;
        }
        let beside_file = || {
            let path = self.tabs.get(self.active)?.path.as_deref()?;
            path.parent().map(Path::to_path_buf)
        };
        let folder = self
            .folder()
            .or_else(beside_file)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        match Terminal::start(&folder, ctx) {
            Ok(terminal) => {
                self.terminal = Some(terminal);
                true
            }
            Err(err) => {
                self.toasts.error(err);
                false
            }
        }
    }

    fn show_terminal(&mut self, ctx: &Context) {
        if self.ensure_terminal(ctx) {
            self.bottom = Some(Bottom::Terminal);
            self.home = false;
            if let Some(terminal) = &mut self.terminal {
                terminal.focus = true;
            }
        }
    }

    /// Save the active file and run it in the terminal.
    fn run_file(&mut self, ctx: &Context) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        if (tab.dirty || tab.path.is_none()) && !self.save_tab(ctx, self.active, false) {
            return;
        }
        let tab = &self.tabs[self.active];
        let language = tab.language.name;
        let command = tab
            .path
            .as_deref()
            .and_then(|path| tools::run_command(language, path));
        let Some(command) = command else {
            self.toasts
                .error(format!("GodCode does not know how to run {language} files"));
            return;
        };
        self.show_terminal(ctx);
        if let Some(terminal) = &mut self.terminal {
            terminal.send(&command);
        }
    }

    // ------------------------------------------------------------ git

    /// Read the Git state of the folder again, off the interface thread.
    fn refresh_git(&mut self, ctx: &Context) {
        let Some(root) = self.folder() else {
            self.repo = None;
            return;
        };
        let (sender, receiver) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let args = [
                "-c",
                "core.quotepath=off",
                "status",
                "--porcelain=v1",
                "-b",
                "-uall",
            ];
            let repo = tools::git(&root, &args).ok().map(|printed| {
                let (branch, changes) = git::parse_status(&printed);
                Repo { branch, changes }
            });
            // The folder may have been closed meanwhile: nobody listens then.
            let _ = sender.send(repo);
            ctx.request_repaint();
        });
        self.repo_incoming = Some(receiver);
    }

    /// Find the lines of tab `index` changed since the last commit, off the interface thread.
    fn refresh_diff(&mut self, ctx: &Context, index: usize) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let Some(path) = tab.path.clone() else {
            return;
        };
        let (id, revision) = (tab.id, tab.revision);
        let sender = self.diffs.0.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
            let file = path.display().to_string();
            // Outside a repository, or before the first commit, nothing is marked.
            let changes = tools::git(&folder, &["diff", "--no-color", "-U0", "HEAD", "--", &file])
                .map(|printed| git::parse_diff(&printed))
                .unwrap_or_default();
            let _ = sender.send((id, revision, changes));
            ctx.request_repaint();
        });
    }

    /// Commit every change of the folder with the message typed in the panel.
    fn commit(&mut self, ctx: &Context) {
        let Some(root) = self.folder() else {
            return;
        };
        let message = self.commit_message.trim();
        if message.is_empty() {
            self.toasts.error("Write a commit message first");
            return;
        }
        let committed = tools::git(&root, &["add", "-A"])
            .and_then(|_| tools::git(&root, &["commit", "-m", message]));
        match committed {
            Ok(_) => {
                self.toasts.success("Committed");
                self.commit_message.clear();
            }
            Err(err) => self.toasts.error(err),
        }
        self.refresh_git(ctx);
        for index in 0..self.tabs.len() {
            self.refresh_diff(ctx, index);
        }
    }

    /// Replace every whole word `old` of the active file by `new`.
    fn rename_word(&mut self, ctx: &Context, old: &str, new: &str) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if new.is_empty() || new == old {
            return;
        }
        let (text, count) = search::rename_word(&tab.text, old, new);
        if count > 0 {
            tab.text = text;
            tab.touched();
            let caret = tab.selection.start;
            tab.select(ctx, caret..caret);
        }
        let plural = if count == 1 { "" } else { "s" };
        self.toasts
            .success(format!("Renamed {count} occurrence{plural}"));
    }

    fn git_panel(&mut self, ui: &mut Ui) {
        let palette = Palette::of(ui);
        let ctx = ui.ctx().clone();
        let root = self.folder();
        let mut refresh = false;
        let mut commit = false;
        let mut init = false;
        let mut in_terminal = None;
        let mut open = None;
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(RichText::new("SOURCE CONTROL").size(12.0).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(4.0);
                refresh =
                    widgets::small_icon_button(ui, icon::ARROWS_CLOCKWISE, "Refresh").clicked();
            });
        });
        ui.add_space(4.0);
        let note = |ui: &mut Ui, text: &str| {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(10.0);
                ui.label(RichText::new(text).size(12.0).color(palette.text_dim));
            });
        };
        if root.is_none() {
            note(ui, "Open a folder to see its changes.");
        } else if let Some(repo) = &self.repo {
            Frame::new()
                .inner_margin(Margin::symmetric(10, 0))
                .show(ui, |ui| {
                    let branch = repo.branch.as_deref().unwrap_or("no branch");
                    ui.label(format!("{} {branch}", icon::GIT_BRANCH));
                    ui.add_space(4.0);
                    ui.add(
                        TextEdit::multiline(&mut self.commit_message)
                            .margin(crate::widgets::FIELD_MARGIN)
                            .hint_text("Commit message")
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        commit = ui
                            .button(with_icon(icon::CHECK_CIRCLE, "Commit all"))
                            .clicked();
                        if ui.button("Pull").clicked() {
                            in_terminal = Some("git pull");
                        }
                        if ui.button("Push").clicked() {
                            in_terminal = Some("git push");
                        }
                    });
                    ui.add_space(8.0);
                    let count = repo.changes.len();
                    let title = format!("CHANGES  {count}");
                    ui.label(RichText::new(title).size(12.0).color(palette.text_dim));
                });
            ui.add_space(2.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            ScrollArea::vertical().auto_shrink(false).show_rows(
                ui,
                LIST_ROW,
                repo.changes.len(),
                |ui, range| {
                    for change in &repo.changes[range] {
                        let label = format!("{}    {}", change.state.letter(), change.path);
                        if list_row(ui, &palette, &label, 12.0, palette.text).clicked()
                            && change.state != git::State::Deleted
                        {
                            open = Some(change.path.clone());
                        }
                    }
                },
            );
        } else {
            note(
                ui,
                "This folder is not a Git repository, or Git is not installed.",
            );
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                init = ui.button("Initialize repository").clicked();
            });
        }
        let Some(root) = root else {
            return;
        };
        if init && let Err(err) = tools::git(&root, &["init"]) {
            self.toasts.error(err);
        }
        if commit {
            self.commit(&ctx);
        } else if refresh || init {
            self.refresh_git(&ctx);
        }
        if let Some(command) = in_terminal {
            self.show_terminal(&ctx);
            if let Some(terminal) = &mut self.terminal {
                terminal.send(command);
            }
        }
        if let Some(path) = open {
            self.open_file(&ctx, &root.join(path));
        }
    }

    // ------------------------------------------------------------ code and navigation

    /// Where the caret is now.
    fn here(&self) -> Option<Place> {
        let tab = self.tabs.get(self.active)?;
        Some(Place {
            tab: tab.id,
            path: tab.path.clone(),
            at: tab.selection.start,
        })
    }

    /// Remember where the caret is before it jumps elsewhere, for "back" to return to.
    fn record_jump(&mut self) {
        let Some(here) = self.here() else {
            return;
        };
        self.history.truncate(self.history_at);
        if self.history.last() != Some(&here) {
            self.history.push(here);
        }
        if self.history.len() > MAX_HISTORY {
            self.history.remove(0);
        }
        self.history_at = self.history.len();
    }

    /// Step back or forward through the places the caret jumped from.
    fn travel(&mut self, ctx: &Context, back: bool) {
        if back {
            if self.history_at == 0 {
                return;
            }
            // The place left now is where "forward" comes back to.
            if self.history_at == self.history.len()
                && let Some(here) = self.here()
            {
                self.history.push(here);
            }
            self.history_at -= 1;
        } else {
            if self.history_at + 1 >= self.history.len() {
                return;
            }
            self.history_at += 1;
        }
        let place = self.history[self.history_at].clone();
        let open = self.tabs.iter().position(|tab| tab.id == place.tab);
        match (open, &place.path) {
            (Some(index), _) => self.active = index,
            (None, Some(path)) if self.open_file(ctx, path) => {}
            _ => return,
        }
        self.home = false;
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.select(ctx, place.at..place.at);
        }
    }

    /// The word at the caret of the active file, or at the char `at`.
    fn word_at(&self, at: Option<usize>) -> Option<String> {
        let tab = self.tabs.get(self.active)?;
        let start = complete::word_start(&tab.text, at.unwrap_or(tab.selection.start));
        let word: String = tab
            .text
            .chars()
            .skip(start)
            .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
            .collect();
        (!word.is_empty()).then_some(word)
    }

    /// Go to where the word at the caret (or at the char `at`) is defined: in the active
    /// file, then in the other open ones. Failing that, or from the definition itself, its
    /// usages are listed.
    fn go_to_declaration(&mut self, ctx: &Context, at: Option<usize>) {
        let Some(word) = self.word_at(at) else {
            return;
        };
        let others = (0..self.tabs.len()).filter(|index| *index != self.active);
        let found = std::iter::once(self.active)
            .chain(others)
            .find_map(|index| {
                let tab = self.tabs.get(index)?;
                let line = outline::declaration(&tab.text, tab.language, &word)?;
                Some((index, line))
            });
        let on_it = found == Some((self.active, self.caret_line)) && at.is_none();
        let Some((index, line)) = found.filter(|_| !on_it) else {
            self.find_usages(ctx, Some(word));
            return;
        };
        self.record_jump();
        self.active = index;
        self.home = false;
        let tab = &mut self.tabs[index];
        let line_start = godcode_core::line_start(&tab.text, line + 1);
        let content: String = tab
            .text
            .chars()
            .skip(line_start)
            .take_while(|ch| *ch != '\n')
            .collect();
        // On the name itself rather than at the start of its line.
        let column = content
            .find(&word)
            .map_or(0, |byte| content[..byte].chars().count());
        let start = line_start + column;
        tab.select(ctx, start..start + word.chars().count());
    }

    /// List where `word` (or the word at the caret) is written: in the files of the folder,
    /// or in the active file when no folder is open.
    fn find_usages(&mut self, ctx: &Context, word: Option<String>) {
        let Some(word) = word.or_else(|| self.word_at(None)) else {
            return;
        };
        if self.explorer.is_none() {
            self.find.query = word;
            self.find.match_case = true;
            self.find.open = true;
            return;
        }
        self.search.query = word;
        self.search.match_case = true;
        self.side = Side::Search;
        self.sidebar = true;
        self.start_search(ctx);
    }

    /// Rebuild the line breaks and the indentation of the active file.
    fn reformat(&mut self, ctx: &Context) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        // The formatter of the language when it is installed, else the built-in one.
        let formatted = match tools::format_external(tab.language.name, &tab.text) {
            Some(Ok(formatted)) => formatted,
            Some(Err(complaint)) => {
                self.toasts.error(format!("Cannot reformat: {complaint}"));
                return;
            }
            None => format::format(&tab.text, tab.language, &tab.indent.unit()),
        };
        if formatted == tab.text {
            self.toasts.success("Already formatted");
            return;
        }
        let line = godcode_core::line_col(&tab.text, tab.selection.start).0;
        let before = tab.text.lines().count();
        tab.text = formatted;
        tab.indent = Indent::detect(&tab.text);
        tab.wrap = self.wrap || format::longest_line(&tab.text) > LONG_LINE;
        tab.touched();
        let caret = godcode_core::line_start(&tab.text, line);
        tab.select(ctx, caret..caret);
        let after = tab.text.lines().count();
        self.toasts
            .success(format!("Reformatted: {before} lines, now {after}"));
    }

    /// Grow the selection to the next thing around it, or return to what it grew from.
    fn grow_selection(&mut self, ctx: &Context, grow: bool) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let target = if grow {
            let wider = edit::expand_selection(&tab.text, tab.language, &tab.selection);
            if wider == tab.selection {
                return;
            }
            tab.grown.push(tab.selection.clone());
            wider
        } else {
            let Some(narrower) = tab.grown.pop() else {
                return;
            };
            narrower
        };
        editor::set_selection(ctx, tab.editor_id(), &target);
        tab.selection = target;
        tab.view.focus = true;
    }

    /// Close the editors `wanted` picks by index; those with unsaved changes stay.
    fn close_many(&mut self, ctx: &Context, wanted: impl Fn(usize) -> bool) {
        let front = self.tabs.get(self.active).map(|tab| tab.id);
        let mut kept = 0;
        for index in (0..self.tabs.len()).rev().filter(|index| wanted(*index)) {
            if self.tabs[index].dirty {
                kept += 1;
            } else {
                self.close_tab(ctx, index);
            }
        }
        if let Some(index) = front.and_then(|id| self.tabs.iter().position(|tab| tab.id == id)) {
            self.active = index;
        }
        if kept > 0 {
            let plural = if kept == 1 { "" } else { "s" };
            self.toasts
                .success(format!("{kept} unsaved file{plural} left open"));
        }
    }

    /// Show `path` in the file tree.
    fn reveal(&mut self, path: &Path) {
        match &mut self.explorer {
            Some(explorer) if path.starts_with(explorer.root()) => {
                explorer.reveal(path);
                self.side = Side::Explorer;
                self.sidebar = true;
            }
            _ => self.toasts.error("This file is not in the open folder"),
        }
    }

    /// Keep the outline of the active file and the line of its caret in step with it.
    fn refresh_symbols(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else {
            self.symbols = (0, Vec::new());
            return;
        };
        self.caret_line = godcode_core::line_col(&tab.text, tab.selection.start).0 - 1;
        let key = hash_of((tab.id, tab.revision, tab.language.name));
        if self.symbols.0 != key {
            self.symbols = (key, outline::symbols(&tab.text, tab.language));
        }
    }

    /// Keep the other places of the word at the caret in step with the caret.
    fn refresh_occurrences(&mut self) {
        let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
        let word = self
            .tabs
            .get(self.active)
            .filter(|tab| tab.text.len() <= MAX_OCCURRENCE_BYTES)
            .and_then(|tab| {
                let word = self.word_at(None)?;
                let selected = tab.selection.len();
                // A selection counts when it is that word, not a stretch of code.
                (word.chars().count() > 1 && (selected == 0 || selected == word.chars().count()))
                    .then_some((tab, word))
            });
        let Some((tab, word)) = word.filter(|(_, word)| word.chars().all(is_word)) else {
            self.occurrences = (0, Vec::new());
            return;
        };
        let key = hash_of((tab.id, tab.revision, &word));
        if self.occurrences.0 != key {
            let mut found = search::find_word(&tab.text, &word);
            // Alone, the word needs no pointing out.
            if found.len() < 2 {
                found.clear();
            }
            self.occurrences = (key, found);
        }
    }

    /// Look for TODO and FIXME notes in the files of the folder, off the interface thread.
    fn start_todos(&mut self, ctx: &Context) {
        self.todos.clear();
        self.todos_incoming = None;
        let Some(root) = self.folder() else {
            return;
        };
        let (sender, receiver) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for path in search::files(&root, &AtomicBool::new(false)) {
                let mut hits: Vec<Hit> = TODO_MARKS
                    .iter()
                    .flat_map(|mark| search::in_file(&path, mark, true))
                    .collect();
                hits.sort_by_key(|hit| hit.at.start);
                // The panel may have been closed meanwhile: nobody listens then.
                if !hits.is_empty() && sender.send(hits).is_err() {
                    return;
                }
            }
            drop(sender);
            ctx.request_repaint();
        });
        self.todos_incoming = Some(receiver);
    }

    // ------------------------------------------------------------ commands

    /// Apply a code-aware edit to the active file.
    fn edit(&mut self, ctx: &Context, op: editor::Op) {
        let font_size = self.font_size();
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let unit = tab.indent.unit();
        let config = editor::Config {
            language: tab.language,
            font_size,
            indent: &unit,
            marks: &[],
            current_mark: None,
            squiggles: &[],
            changes: &[],
            occurrences: &[],
            wrap: false,
        };
        let id = tab.editor_id();
        if editor::run(ctx, id, &mut tab.text, &config, op) {
            tab.touched();
        }
        tab.view.focus = true;
    }

    fn run(&mut self, ctx: &Context, cmd: Cmd) {
        match cmd {
            Cmd::NewFile => self.new_file(),
            Cmd::OpenFile => {
                for path in platform::pick_files(self.folder().as_deref()) {
                    self.open_file(ctx, &path);
                }
            }
            Cmd::OpenFolder => {
                if let Some(root) = platform::pick_folder(self.folder().as_deref()) {
                    self.open_folder(ctx, root);
                }
            }
            Cmd::CloseFolder => self.close_folder(),
            Cmd::Save => {
                self.save_tab(ctx, self.active, false);
            }
            Cmd::SaveAs => {
                self.save_tab(ctx, self.active, true);
            }
            Cmd::SaveAll => {
                for index in 0..self.tabs.len() {
                    if self.tabs[index].dirty && !self.save_tab(ctx, index, false) {
                        break;
                    }
                }
            }
            Cmd::CloseTab => self.request_close(ctx, self.active),
            Cmd::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
            Cmd::Undo | Cmd::Redo => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    let undo = cmd == Cmd::Undo;
                    let id = tab.editor_id();
                    if let Some(selection) = editor::history(ctx, id, &mut tab.text, undo) {
                        tab.selection = selection;
                        tab.touched();
                    }
                    tab.view.focus = true;
                }
            }
            Cmd::Cut | Cmd::Copy | Cmd::Paste => {
                if let Some(tab) = self.tabs.get(self.active) {
                    // The clipboard event goes to whoever has the keyboard.
                    ctx.memory_mut(|memory| memory.request_focus(tab.editor_id()));
                    ctx.send_viewport_cmd(match cmd {
                        Cmd::Cut => ViewportCommand::RequestCut,
                        Cmd::Copy => ViewportCommand::RequestCopy,
                        _ => ViewportCommand::RequestPaste,
                    });
                }
            }
            Cmd::SelectAll => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    let all = 0..tab.text.chars().count();
                    editor::set_selection(ctx, tab.editor_id(), &all);
                    tab.selection = all;
                    tab.view.focus = true;
                }
            }
            Cmd::Find => {
                if let Some(tab) = self.tabs.get(self.active) {
                    // Start from the selected word, as long as it is on one line.
                    let selected: String = tab
                        .text
                        .chars()
                        .skip(tab.selection.start)
                        .take(tab.selection.len())
                        .collect();
                    if !selected.is_empty() && !selected.contains('\n') {
                        self.find.query = selected;
                    }
                    self.find.open = true;
                    self.find.focus = true;
                }
            }
            Cmd::FindNext => self.find_step(ctx, true),
            Cmd::FindPrevious => self.find_step(ctx, false),
            Cmd::ReplaceOne => self.replace_one(ctx),
            Cmd::ReplaceAll => self.replace_all(ctx),
            Cmd::CloseFind => {
                self.find.open = false;
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.view.focus = true;
                }
            }
            Cmd::SearchInFiles => {
                self.side = Side::Search;
                self.sidebar = true;
                self.home = false;
                self.search.focus = true;
            }
            Cmd::ToggleComment => self.edit(ctx, editor::Op::Comment),
            Cmd::Indent => self.edit(ctx, editor::Op::Indent),
            Cmd::Outdent => self.edit(ctx, editor::Op::Outdent),
            Cmd::DuplicateLine => self.edit(ctx, editor::Op::Duplicate),
            Cmd::DeleteLine => self.edit(ctx, editor::Op::DeleteLine),
            Cmd::MoveLineUp => self.edit(ctx, editor::Op::MoveUp),
            Cmd::MoveLineDown => self.edit(ctx, editor::Op::MoveDown),
            Cmd::Complete => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.view.complete_now = true;
                    tab.view.focus = true;
                }
            }
            Cmd::GoToFile => self.open_quick(""),
            Cmd::GoToLine => self.open_quick(":"),
            Cmd::GoToSymbol => self.open_quick("@"),
            Cmd::Commands => self.open_quick(">"),
            Cmd::NextTab | Cmd::PreviousTab => {
                let count = self.tabs.len();
                if count > 0 {
                    let step = if cmd == Cmd::NextTab { 1 } else { count - 1 };
                    self.active = (self.active + step) % count;
                    self.tabs[self.active].view.focus = true;
                    self.home = false;
                }
            }
            Cmd::Explorer => {
                self.side = Side::Explorer;
                self.sidebar = true;
                self.home = false;
            }
            Cmd::ToggleSidebar => self.sidebar = !self.sidebar,
            Cmd::ZoomIn => self.set_zoom(self.zoom + 0.1),
            Cmd::ZoomOut => self.set_zoom(self.zoom - 0.1),
            Cmd::ZoomReset => self.set_zoom(1.0),
            Cmd::ToggleTheme => {
                self.dark = !self.dark;
                theme::set_dark(ctx, self.dark);
            }
            Cmd::Home => self.home = !self.home,
            Cmd::Run => self.run_file(ctx),
            Cmd::Check => {
                let index = self.active;
                let dirty = self.tabs.get(index).is_some_and(|tab| tab.dirty);
                // Saving starts a check of its own; that one must speak up too.
                if !dirty || self.save_tab(ctx, index, false) {
                    if let Some((_, manual)) = &mut self.checking {
                        *manual = true;
                    } else {
                        self.start_check(ctx, index, true);
                    }
                }
            }
            Cmd::NextProblem => self.next_problem(ctx),
            Cmd::Problems => {
                let shown = self.bottom == Some(Bottom::Problems);
                self.bottom = (!shown).then_some(Bottom::Problems);
            }
            Cmd::ToggleTerminal => {
                if self.bottom == Some(Bottom::Terminal) {
                    self.bottom = None;
                } else {
                    self.show_terminal(ctx);
                }
            }
            Cmd::RestartTerminal => {
                self.terminal = None;
                self.show_terminal(ctx);
            }
            Cmd::ClearTerminal => {
                if let Some(terminal) = &mut self.terminal {
                    terminal.clear();
                }
            }
            Cmd::Reformat => self.reformat(ctx),
            Cmd::ToggleWrap => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.wrap = !tab.wrap;
                    tab.view.focus = true;
                    self.wrap = tab.wrap;
                }
            }
            Cmd::JoinLines => self.edit(ctx, editor::Op::Join),
            Cmd::ToggleCase => self.edit(ctx, editor::Op::ToggleCase),
            Cmd::ExpandSelection => self.grow_selection(ctx, true),
            Cmd::ShrinkSelection => self.grow_selection(ctx, false),
            Cmd::Declaration => self.go_to_declaration(ctx, None),
            Cmd::FindUsages => self.find_usages(ctx, None),
            Cmd::Back => self.travel(ctx, true),
            Cmd::Forward => self.travel(ctx, false),
            Cmd::Structure => {
                self.side = Side::Structure;
                self.sidebar = true;
                self.home = false;
            }
            Cmd::Todo => {
                let shown = self.bottom == Some(Bottom::Todo);
                self.bottom = (!shown).then_some(Bottom::Todo);
                if !shown {
                    self.start_todos(ctx);
                }
            }
            Cmd::RecentFiles => self.open_quick(""),
            Cmd::CloseOthers => {
                let front = self.active;
                self.close_many(ctx, |index| index != front);
            }
            Cmd::CloseAll => self.close_many(ctx, |_| true),
            Cmd::Reveal | Cmd::CopyPath => {
                let path = self.tabs.get(self.active).and_then(|tab| tab.path.clone());
                match path {
                    Some(path) if cmd == Cmd::Reveal => self.reveal(&path),
                    Some(path) => {
                        ctx.copy_text(path.display().to_string());
                        self.toasts.success("Path copied");
                    }
                    None => self.toasts.error("This file is not saved yet"),
                }
            }
            Cmd::Shortcuts => self.shortcuts = true,
            Cmd::SourceControl => {
                self.side = Side::Git;
                self.sidebar = true;
                self.home = false;
                self.refresh_git(ctx);
            }
            Cmd::Rename => {
                if let Some(tab) = self.tabs.get(self.active) {
                    let start = complete::word_start(&tab.text, tab.selection.start);
                    let word: String = tab
                        .text
                        .chars()
                        .skip(start)
                        .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
                        .collect();
                    if !word.is_empty() {
                        self.prompt = Some(Prompt {
                            kind: PromptKind::Word(word.clone()),
                            name: word,
                            focus: true,
                        });
                    }
                }
            }
        }
    }

    fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom.clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end());
    }

    fn open_quick(&mut self, prefix: &str) {
        self.quick = Some(Quick {
            query: prefix.to_owned(),
            selected: 0,
            focus: true,
        });
    }

    fn shortcuts(&mut self, ctx: &Context, clicked: &mut Vec<Cmd>) {
        let ctrl = Modifiers::COMMAND;
        let shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let alt = Modifiers::COMMAND | Modifiers::ALT;
        // More specific (shifted) shortcuts first.
        let alt_shift = Modifiers::ALT | Modifiers::SHIFT;
        let table = [
            (alt_shift, Key::ArrowRight, Cmd::ExpandSelection),
            (alt_shift, Key::ArrowLeft, Cmd::ShrinkSelection),
            (alt, Key::L, Cmd::Reformat),
            (alt, Key::ArrowLeft, Cmd::Back),
            (alt, Key::ArrowRight, Cmd::Forward),
            (Modifiers::ALT, Key::Z, Cmd::ToggleWrap),
            (Modifiers::ALT, Key::F7, Cmd::FindUsages),
            (Modifiers::ALT, Key::F1, Cmd::Reveal),
            (Modifiers::ALT, Key::F12, Cmd::ToggleTerminal),
            (Modifiers::SHIFT, Key::F6, Cmd::Rename),
            (ctrl, Key::F12, Cmd::GoToSymbol),
            (ctrl, Key::F4, Cmd::CloseTab),
            (shift, Key::J, Cmd::JoinLines),
            (shift, Key::U, Cmd::ToggleCase),
            (shift, Key::C, Cmd::CopyPath),
            (shift, Key::N, Cmd::GoToFile),
            (shift, Key::A, Cmd::Commands),
            (shift, Key::S, Cmd::SaveAs),
            (shift, Key::O, Cmd::GoToSymbol),
            (shift, Key::F, Cmd::SearchInFiles),
            (shift, Key::P, Cmd::Commands),
            (shift, Key::E, Cmd::Explorer),
            (shift, Key::M, Cmd::Problems),
            (shift, Key::G, Cmd::SourceControl),
            (shift, Key::B, Cmd::Check),
            (shift, Key::K, Cmd::DeleteLine),
            (shift, Key::Z, Cmd::Redo),
            (shift, Key::Tab, Cmd::PreviousTab),
            (alt, Key::S, Cmd::SaveAll),
            (alt, Key::O, Cmd::OpenFolder),
            (ctrl, Key::Tab, Cmd::NextTab),
            (ctrl, Key::N, Cmd::NewFile),
            (ctrl, Key::E, Cmd::RecentFiles),
            (ctrl, Key::O, Cmd::OpenFile),
            (ctrl, Key::S, Cmd::Save),
            (ctrl, Key::W, Cmd::CloseTab),
            (ctrl, Key::F, Cmd::Find),
            (ctrl, Key::H, Cmd::Find),
            (ctrl, Key::P, Cmd::GoToFile),
            (ctrl, Key::G, Cmd::GoToLine),
            (ctrl, Key::B, Cmd::ToggleSidebar),
            (ctrl, Key::D, Cmd::DuplicateLine),
            (ctrl, Key::J, Cmd::ToggleTerminal),
            (ctrl, Key::Backtick, Cmd::ToggleTerminal),
            (ctrl, Key::Space, Cmd::Complete),
            (ctrl, Key::Slash, Cmd::ToggleComment),
            // Where "/" needs Shift (AZERTY), the key next to it does the same.
            (ctrl, Key::Colon, Cmd::ToggleComment),
            (ctrl, Key::Plus, Cmd::ZoomIn),
            (ctrl, Key::Equals, Cmd::ZoomIn),
            (ctrl, Key::Minus, Cmd::ZoomOut),
            (ctrl, Key::Num0, Cmd::ZoomReset),
            (Modifiers::ALT, Key::ArrowUp, Cmd::MoveLineUp),
            (Modifiers::ALT, Key::ArrowDown, Cmd::MoveLineDown),
            (Modifiers::SHIFT, Key::F3, Cmd::FindPrevious),
            (Modifiers::NONE, Key::F3, Cmd::FindNext),
            (Modifiers::NONE, Key::F5, Cmd::Run),
            (Modifiers::NONE, Key::F8, Cmd::NextProblem),
            (Modifiers::NONE, Key::F1, Cmd::Shortcuts),
            (Modifiers::NONE, Key::F2, Cmd::Rename),
            (Modifiers::NONE, Key::F12, Cmd::Declaration),
        ];
        ctx.input_mut(|input| {
            for (modifiers, key, cmd) in table {
                if input.consume_shortcut(&KeyboardShortcut::new(modifiers, key)) {
                    clicked.push(cmd);
                }
            }
        });
        let dialog = self.quick.is_some()
            || self.prompt.is_some()
            || self.closing.is_some()
            || self.shortcuts;
        // Escape closes the completion list first, when one is open.
        let completing = self
            .tabs
            .get(self.active)
            .is_some_and(|tab| tab.view.completing());
        if self.find.open
            && !dialog
            && !completing
            && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            clicked.push(Cmd::CloseFind);
        }
        // Shift tapped twice, nothing else touched in between: search everywhere.
        let (shift_held, busy, now) = ctx.input(|input| {
            let busy = input.events.iter().any(|event| {
                matches!(
                    event,
                    Event::Key { .. }
                        | Event::Text(_)
                        | Event::Paste(_)
                        | Event::PointerButton { .. }
                        | Event::MouseWheel { .. }
                )
            });
            (input.modifiers.shift_only(), busy, input.time)
        });
        let taps = &mut self.shift;
        if shift_held && !taps.down {
            taps.alone = true;
        }
        if busy {
            taps.alone = false;
            taps.last = None;
        }
        if !shift_held && taps.down && taps.alone {
            if taps.last.is_some_and(|last| now - last < DOUBLE_TAP) && !dialog {
                clicked.push(Cmd::GoToFile);
                taps.last = None;
            } else {
                taps.last = Some(now);
            }
        }
        taps.down = shift_held;
        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        for path in dropped.into_iter().filter_map(|file| file.path) {
            self.open_path(ctx, &path);
        }
    }

    /// Hold the window open while files are unsaved, until the user decides.
    fn guard_close(&mut self, ctx: &Context) {
        let requested = ctx.input(|input| input.viewport().close_requested());
        if requested && !self.quit && self.tabs.iter().any(|tab| tab.dirty) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.closing = Some(Closing::Window);
        }
    }

    fn update_title(&mut self, ctx: &Context) {
        let folder = self.folder().map(|root| file_name(&root));
        let file = self
            .tabs
            .get(self.active)
            .filter(|_| !self.at_home())
            .map(|tab| format!("{}{}", tab.name, if tab.dirty { "*" } else { "" }));
        let title = [file, folder, Some(APP_NAME.to_owned())]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" - ");
        if title != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    // ------------------------------------------------------------ menu, tabs, status

    fn menu_bar(&self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        egui::MenuBar::new().ui(ui, |ui| {
            for (title, items) in MENUS {
                ui.menu_button(*title, |ui| {
                    ui.set_min_width(260.0);
                    menu_items(ui, clicked, items);
                });
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (glyph, tip) = if self.dark {
                    (icon::SUN, "Light mode")
                } else {
                    (icon::MOON, "Dark mode")
                };
                if widgets::small_icon_button(ui, glyph, tip).clicked() {
                    clicked.push(Cmd::ToggleTheme);
                }
            });
        });
    }

    /// The open files, above the editor.
    fn editor_tabs(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = Palette::of(ui);
        let mut select = None;
        let mut close = None;
        let mut asked = None;
        let row = vec2(ui.available_width(), TAB_HEIGHT);
        ui.allocate_ui_with_layout(row, Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.add_space(2.0);
            // Share what is left between the tabs; names shrink before anything overflows.
            let reserved = 5.0 * widgets::ICON_BUTTON.x + 50.0;
            let chrome = 12.0 + 16.0 + 8.0 + 8.0 + widgets::SMALL_ICON_BUTTON.x + 22.0;
            let per_tab = (ui.available_width() - reserved) / self.tabs.len().max(1) as f32;
            let text_max = (per_tab - chrome).clamp(40.0, TAB_TEXT_MAX);
            for (index, tab) in self.tabs.iter().enumerate() {
                let active = index == self.active;
                let (select_it, close_it, response) =
                    doc_tab(ui, &palette, &tab.name, tab.dirty, active, text_max);
                if select_it {
                    select = Some(index);
                }
                if close_it {
                    close = Some(index);
                }
                response.context_menu(|ui| {
                    ui.set_min_width(220.0);
                    if ui.button(with_icon(icon::X, "Close")).clicked() {
                        close = Some(index);
                    }
                    let entries = [
                        (icon::X_SQUARE, "Close Others", TabAction::CloseOthers),
                        (
                            icon::ARROW_RIGHT,
                            "Close to the Right",
                            TabAction::CloseRight,
                        ),
                        (icon::X_SQUARE, "Close All", TabAction::CloseAll),
                        (icon::COPY, "Copy Path", TabAction::CopyPath),
                        (icon::TARGET, "Select in Explorer", TabAction::Reveal),
                    ];
                    for (glyph, label, action) in entries {
                        if matches!(action, TabAction::CopyPath) {
                            ui.separator();
                        }
                        if ui.button(with_icon(glyph, label)).clicked() {
                            asked = Some((index, action));
                        }
                    }
                });
            }
            ui.add_space(4.0);
            if widgets::icon_button(ui, icon::PLUS, "New file (Ctrl+N)").clicked() {
                clicked.push(Cmd::NewFile);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(4.0);
                let tools = [
                    (icon::PLAY, "Run file (F5)", Cmd::Run),
                    (
                        icon::MAGIC_WAND,
                        "Reformat code (Ctrl+Alt+L)",
                        Cmd::Reformat,
                    ),
                    (
                        icon::ARROW_BEND_UP_RIGHT,
                        "Forward (Ctrl+Alt+Right)",
                        Cmd::Forward,
                    ),
                    (icon::ARROW_BEND_UP_LEFT, "Back (Ctrl+Alt+Left)", Cmd::Back),
                ];
                for (glyph, tip, cmd) in tools {
                    if widgets::icon_button(ui, glyph, tip).clicked() {
                        clicked.push(cmd);
                    }
                }
            });
        });
        let ctx = ui.ctx().clone();
        if let Some(index) = select {
            self.record_jump();
            self.active = index;
            self.tabs[index].view.focus = true;
        }
        if let Some(index) = close {
            self.request_close(&ctx, index);
        }
        let Some((index, action)) = asked else {
            return;
        };
        let path = self.tabs.get(index).and_then(|tab| tab.path.clone());
        match (action, path) {
            (TabAction::CloseOthers, _) => {
                self.active = index;
                self.close_many(&ctx, |other| other != index);
            }
            (TabAction::CloseRight, _) => self.close_many(&ctx, |other| other > index),
            (TabAction::CloseAll, _) => self.close_many(&ctx, |_| true),
            (TabAction::CopyPath, Some(path)) => ctx.copy_text(path.display().to_string()),
            (TabAction::Reveal, Some(path)) => self.reveal(&path),
            (TabAction::CopyPath | TabAction::Reveal, None) => {
                self.toasts.error("This file is not saved yet");
            }
        }
    }

    fn status_bar(&self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = Palette::of(ui);
        let dim = |text: String| RichText::new(text).size(13.0).color(palette.text_dim);
        let tab = self.tabs.get(self.active);
        let problems = self.problems();
        let errors = problems
            .iter()
            .filter(|(_, problem)| problem.severity == Severity::Error)
            .count();
        ui.horizontal(|ui| {
            let counts = format!(
                "{} {errors}   {} {}",
                icon::X_CIRCLE,
                icon::WARNING,
                problems.len() - errors
            );
            let counts = Button::new(dim(counts))
                .frame_when_inactive(false)
                .stroke(Stroke::NONE);
            if ui.add(counts).on_hover_text("Problems").clicked() {
                clicked.push(Cmd::Problems);
            }
            if let Some(branch) = self.repo.as_ref().and_then(|repo| repo.branch.as_deref()) {
                let branch = Button::new(dim(format!("{} {branch}", icon::GIT_BRANCH)))
                    .frame_when_inactive(false)
                    .stroke(Stroke::NONE);
                if ui.add(branch).on_hover_text("Source control").clicked() {
                    clicked.push(Cmd::SourceControl);
                }
            }
            if self.checking.is_some() {
                ui.label(dim("Checking…".to_owned()));
            }
            if let Some(tab) = tab {
                let (line, column) = godcode_core::line_col(&tab.text, tab.selection.end);
                let mut place = format!("Ln {line}, Col {column}");
                if !tab.selection.is_empty() {
                    place.push_str(&format!("  ({} selected)", tab.selection.len()));
                }
                ui.label(dim(place));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if widgets::small_icon_button(ui, icon::PLUS, "Zoom in").clicked() {
                    clicked.push(Cmd::ZoomIn);
                }
                let zoom = RichText::new(format!("{:.0}%", self.zoom * 100.0)).size(13.0);
                ui.add_sized(vec2(44.0, 22.0), egui::Label::new(zoom).selectable(false));
                if widgets::small_icon_button(ui, icon::MINUS, "Zoom out").clicked() {
                    clicked.push(Cmd::ZoomOut);
                }
                if let Some(tab) = tab {
                    ui.spacing_mut().item_spacing.x = 18.0;
                    ui.add_space(12.0);
                    ui.label(dim(tab.language.name.to_owned()));
                    ui.label(dim(tab.ending.name().to_owned()));
                    ui.label(dim("UTF-8".to_owned()));
                    ui.label(dim(tab.indent.name()));
                    let wrap = if tab.wrap { "Wrap: on" } else { "Wrap: off" };
                    let wrap = Button::new(dim(wrap.to_owned()))
                        .frame_when_inactive(false)
                        .stroke(Stroke::NONE);
                    if ui.add(wrap).on_hover_text("Soft wrap (Alt+Z)").clicked() {
                        clicked.push(Cmd::ToggleWrap);
                    }
                }
            });
        });
    }

    // ------------------------------------------------------------ side bar

    fn activity_bar(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(6.0);
            let views = [
                (Side::Explorer, icon::FILES, "Explorer (Ctrl+Shift+E)"),
                (
                    Side::Search,
                    icon::MAGNIFYING_GLASS,
                    "Search (Ctrl+Shift+F)",
                ),
                (Side::Git, icon::GIT_BRANCH, "Source control (Ctrl+Shift+G)"),
                (Side::Structure, icon::TREE_STRUCTURE, "Structure"),
            ];
            for (side, glyph, tip) in views {
                let shown = self.sidebar && self.side == side;
                if widgets::toggle_button(ui, glyph, tip, shown).clicked() {
                    // The icon of the view in front folds the side bar away.
                    self.sidebar = !shown;
                    self.side = side;
                    self.search.focus = side == Side::Search && !shown;
                }
            }
        });
    }

    fn explorer_panel(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = Palette::of(ui);
        let active = self.tabs.get(self.active).and_then(|tab| tab.path.clone());
        let Some(explorer) = &mut self.explorer else {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No folder opened").color(palette.text_dim));
                ui.add_space(6.0);
                if ui
                    .button(with_icon(icon::FOLDER_OPEN, "Open Folder…"))
                    .clicked()
                {
                    clicked.push(Cmd::OpenFolder);
                }
            });
            return;
        };
        let Some(action) = explorer.show(ui, active.as_deref()) else {
            return;
        };
        let prompt = |kind, name: String| Prompt {
            kind,
            name,
            focus: true,
        };
        match action {
            Action::Open(path) => {
                self.record_jump();
                self.open_file(ui.ctx(), &path);
            }
            Action::NewFile(dir) => {
                self.prompt = Some(prompt(PromptKind::NewFile(dir), String::new()));
            }
            Action::NewFolder(dir) => {
                self.prompt = Some(prompt(PromptKind::NewFolder(dir), String::new()));
            }
            Action::Rename(path) => {
                let name = file_name(&path);
                self.prompt = Some(prompt(PromptKind::Rename(path), name));
            }
            Action::Delete(path) => {
                let name = file_name(&path);
                self.prompt = Some(prompt(PromptKind::Delete(path), name));
            }
        }
    }

    /// The outline of the active file: its definitions, one click away.
    fn structure_panel(&mut self, ui: &mut Ui) {
        let palette = Palette::of(ui);
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(RichText::new("STRUCTURE").size(12.0).strong());
        });
        ui.add_space(4.0);
        let symbols = &self.symbols.1;
        if symbols.is_empty() {
            let none = match self.tabs.get(self.active) {
                Some(tab) => format!("Nothing to outline in this {} file.", tab.language.name),
                None => "No file is open.".to_owned(),
            };
            ui.horizontal_wrapped(|ui| {
                ui.add_space(10.0);
                ui.label(RichText::new(none).size(12.0).color(palette.text_dim));
            });
            return;
        }
        // The definition the caret is in, or just after.
        let current = symbols
            .iter()
            .rposition(|symbol| symbol.line <= self.caret_line);
        let mut open = None;
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::vertical().auto_shrink(false).show_rows(
            ui,
            LIST_ROW,
            symbols.len(),
            |ui, range| {
                for index in range {
                    let symbol = &symbols[index];
                    let row = list_row(ui, &palette, &symbol.name, 34.0, palette.text);
                    let painter = ui.painter();
                    let mid = row.rect.center().y;
                    if current == Some(index) {
                        let bar = Rect::from_min_size(row.rect.min, vec2(2.0, LIST_ROW));
                        painter.rect_filled(bar, 0.0, palette.accent);
                    }
                    let (glyph, color) = symbol_icon(symbol.kind);
                    painter.text(
                        pos2(row.rect.min.x + 20.0, mid),
                        Align2::CENTER_CENTER,
                        glyph,
                        widgets::filled(14.0),
                        color,
                    );
                    let tip = format!("{}, line {}", symbol.kind, symbol.line + 1);
                    if row.on_hover_text(tip).clicked() {
                        open = Some(symbol.line);
                    }
                }
            },
        );
        if let Some(line) = open {
            self.record_jump();
            if let Some(tab) = self.tabs.get_mut(self.active) {
                let start = godcode_core::line_start(&tab.text, line + 1);
                tab.select(ui.ctx(), start..start);
            }
        }
    }

    fn search_panel(&mut self, ui: &mut Ui) {
        let palette = Palette::of(ui);
        let ctx = ui.ctx().clone();
        let root = self.folder();
        let mut start = false;
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(RichText::new("SEARCH").size(12.0).strong());
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let width = (ui.available_width() - widgets::ICON_BUTTON.x - 18.0).max(40.0);
            let field = ui.add(
                TextEdit::singleline(&mut self.search.query)
                    .margin(crate::widgets::FIELD_MARGIN)
                    .hint_text("Search in files")
                    .desired_width(width),
            );
            if std::mem::take(&mut self.search.focus) {
                field.request_focus();
            }
            if field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
                start = true;
                self.search.focus = true;
            }
            let case = self.search.match_case;
            if widgets::toggle_button(ui, icon::TEXT_AA, "Match case", case).clicked() {
                self.search.match_case = !case;
                start = true;
            }
        });
        if start {
            self.start_search(&ctx);
        }
        let search = &self.search;
        let files = search
            .hits
            .iter()
            .zip(search.hits.iter().skip(1))
            .filter(|(hit, next)| hit.path != next.path)
            .count()
            + usize::from(!search.hits.is_empty());
        let status = if root.is_none() {
            "Open a folder to search in its files.".to_owned()
        } else if search.incoming.is_some() {
            format!("Searching… {} results", search.hits.len())
        } else if !search.done {
            "Press Enter to search.".to_owned()
        } else if search.hits.is_empty() {
            "No results.".to_owned()
        } else if search.hits.len() >= MAX_HITS {
            format!("First {MAX_HITS} results, in {files} files")
        } else {
            format!("{} results in {files} files", search.hits.len())
        };
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(RichText::new(status).size(12.0).color(palette.text_dim));
        });
        ui.add_space(4.0);

        // A line per file, then a line per occurrence in it.
        let mut lines: Vec<(bool, usize)> = Vec::with_capacity(search.hits.len() + files);
        for (index, hit) in search.hits.iter().enumerate() {
            if index == 0 || search.hits[index - 1].path != hit.path {
                lines.push((true, index));
            }
            lines.push((false, index));
        }
        let mut open = None;
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::vertical().auto_shrink(false).show_rows(
            ui,
            LIST_ROW,
            lines.len(),
            |ui, range| {
                for (is_file, index) in &lines[range] {
                    let hit = &search.hits[*index];
                    if *is_file {
                        let shown = root
                            .as_deref()
                            .and_then(|root| hit.path.strip_prefix(root).ok())
                            .unwrap_or(&hit.path);
                        let label = with_icon(icon::FILE, &shown.display().to_string());
                        list_row(ui, &palette, &label, 8.0, palette.text)
                            .on_hover_text(hit.path.display().to_string());
                    } else {
                        let label = format!("{}:  {}", hit.at.line + 1, hit.preview);
                        if list_row(ui, &palette, &label, 30.0, palette.text_dim).clicked() {
                            open = Some(hit.clone());
                        }
                    }
                }
            },
        );
        if let Some(hit) = open {
            self.open_hit(&ctx, &hit);
        }
    }

    // ------------------------------------------------------------ bottom panel

    fn bottom_panel(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let Some(shown) = self.bottom else {
            return;
        };
        let palette = Palette::of(ui);
        let problems = self.problems();
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            let views = [
                (Bottom::Problems, format!("PROBLEMS  {}", problems.len())),
                (Bottom::Todo, format!("TODO  {}", self.todos.len())),
                (Bottom::Terminal, "TERMINAL".to_owned()),
            ];
            for (view, title) in views {
                let active = view == shown;
                let color = if active {
                    palette.text
                } else {
                    palette.text_dim
                };
                let title = RichText::new(title).size(12.0).strong().color(color);
                let tab = Button::selectable(active, title)
                    .frame_when_inactive(active)
                    .stroke(Stroke::NONE);
                if ui.add(tab).clicked() && !active {
                    clicked.push(match view {
                        Bottom::Problems => Cmd::Problems,
                        Bottom::Todo => Cmd::Todo,
                        Bottom::Terminal => Cmd::ToggleTerminal,
                    });
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(4.0);
                let (close, tools): (Cmd, &[(&str, &str, Cmd)]) = match shown {
                    Bottom::Problems => (
                        Cmd::Problems,
                        &[(icon::BUG, "Check for problems (Ctrl+Shift+B)", Cmd::Check)],
                    ),
                    Bottom::Todo => (Cmd::Todo, &[]),
                    Bottom::Terminal => (
                        Cmd::ToggleTerminal,
                        &[
                            (
                                icon::ARROW_CLOCKWISE,
                                "Restart terminal",
                                Cmd::RestartTerminal,
                            ),
                            (icon::BROOM, "Clear terminal", Cmd::ClearTerminal),
                            (icon::PLAY, "Run file (F5)", Cmd::Run),
                        ],
                    ),
                };
                if widgets::small_icon_button(ui, icon::X, "Close panel").clicked() {
                    clicked.push(close);
                }
                for (glyph, tip, cmd) in tools {
                    if widgets::small_icon_button(ui, glyph, tip).clicked() {
                        clicked.push(*cmd);
                    }
                }
            });
        });
        ui.add_space(4.0);
        match shown {
            Bottom::Terminal => match &mut self.terminal {
                Some(terminal) => {
                    Frame::new()
                        .inner_margin(Margin::symmetric(10, 0))
                        .show(ui, |ui| terminal.show(ui));
                }
                None => {
                    ui.label(RichText::new("The terminal is not running.").color(palette.text_dim));
                }
            },
            Bottom::Problems => self.problem_list(ui, &problems),
            Bottom::Todo => self.todo_list(ui),
        }
    }

    /// The TODO and FIXME notes of the folder, each leading to its line.
    fn todo_list(&mut self, ui: &mut Ui) {
        let palette = Palette::of(ui);
        let root = self.folder();
        if self.todos.is_empty() {
            let none = if root.is_none() {
                "Open a folder to list the TODO and FIXME notes of its files."
            } else if self.todos_incoming.is_some() {
                "Looking for TODO and FIXME notes…"
            } else {
                "No TODO nor FIXME note in this folder."
            };
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.label(RichText::new(none).color(palette.text_dim));
            });
            return;
        }
        let mut open = None;
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::vertical().auto_shrink(false).show_rows(
            ui,
            LIST_ROW,
            self.todos.len(),
            |ui, range| {
                for hit in &self.todos[range] {
                    let shown = root
                        .as_deref()
                        .and_then(|root| hit.path.strip_prefix(root).ok())
                        .unwrap_or(&hit.path);
                    let label =
                        format!("{}    {}:{}", hit.preview, shown.display(), hit.at.line + 1);
                    let row = list_row(ui, &palette, &label, 30.0, palette.text);
                    ui.painter().text(
                        pos2(row.rect.min.x + 10.0, row.rect.center().y),
                        Align2::LEFT_CENTER,
                        icon::CHECK_SQUARE,
                        FontId::proportional(14.0),
                        palette.text_dim,
                    );
                    if row.clicked() {
                        open = Some(hit.clone());
                    }
                }
            },
        );
        if let Some(hit) = open {
            self.open_hit(ui.ctx(), &hit);
        }
    }

    fn problem_list(&mut self, ui: &mut Ui, problems: &[(Option<PathBuf>, Problem)]) {
        let palette = Palette::of(ui);
        if problems.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                let none = "No problems. Brackets are checked as you type; saving a Rust, \
                            Python, JavaScript or Go file runs its compiler.";
                ui.label(RichText::new(none).color(palette.text_dim));
            });
            return;
        }
        let root = self.folder();
        let mut open = None;
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::vertical().auto_shrink(false).show_rows(
            ui,
            LIST_ROW,
            problems.len(),
            |ui, range| {
                for (path, problem) in &problems[range] {
                    let error = problem.severity == Severity::Error;
                    let (glyph, color) = if error {
                        (icon::X_CIRCLE, editor::ERROR_COLOR)
                    } else {
                        (icon::WARNING, editor::WARNING_COLOR)
                    };
                    let file = path.as_deref().map_or_else(String::new, |path| {
                        let shown = root
                            .as_deref()
                            .and_then(|root| path.strip_prefix(root).ok())
                            .unwrap_or(path);
                        shown.display().to_string()
                    });
                    let label = format!(
                        "{}    {file}:{}:{}",
                        problem.message,
                        problem.line + 1,
                        problem.column + 1
                    );
                    let row = list_row(ui, &palette, &label, 30.0, palette.text);
                    ui.painter().text(
                        pos2(row.rect.min.x + 10.0, row.rect.center().y),
                        Align2::LEFT_CENTER,
                        glyph,
                        FontId::proportional(14.0),
                        color,
                    );
                    if row.clicked() {
                        open = Some((path.clone(), problem.clone()));
                    }
                }
            },
        );
        if let Some((path, problem)) = open {
            self.open_problem(ui.ctx(), path.as_deref(), &problem);
        }
    }

    // ------------------------------------------------------------ editor

    fn find_bar(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = Palette::of(ui);
        let current = self.current_match();
        let find = &mut self.find;
        ui.horizontal(|ui| {
            let enter = |ui: &Ui| ui.input(|input| input.key_pressed(Key::Enter));
            let query = ui.add(
                TextEdit::singleline(&mut find.query)
                    .margin(crate::widgets::FIELD_MARGIN)
                    .hint_text("Find")
                    .desired_width(220.0),
            );
            if std::mem::take(&mut find.focus) {
                query.request_focus();
            }
            if query.lost_focus() && enter(ui) {
                clicked.push(Cmd::FindNext);
                find.focus = true;
            }
            let count = match (current, find.matches.len()) {
                (_, 0) if find.query.is_empty() => String::new(),
                (_, 0) => "No results".to_owned(),
                (Some(index), total) => format!("{} of {total}", index + 1),
                (None, total) => format!("{total} found"),
            };
            let count = RichText::new(count).size(13.0).color(palette.text_dim);
            ui.add_sized(vec2(84.0, 22.0), egui::Label::new(count).selectable(false));
            if widgets::toggle_button(ui, icon::TEXT_AA, "Match case", find.match_case).clicked() {
                find.match_case = !find.match_case;
            }
            if widgets::icon_button(ui, icon::ARROW_UP, "Previous (Shift+F3)").clicked() {
                clicked.push(Cmd::FindPrevious);
            }
            if widgets::icon_button(ui, icon::ARROW_DOWN, "Next (F3)").clicked() {
                clicked.push(Cmd::FindNext);
            }
            ui.add_space(10.0);
            let replace = ui.add(
                TextEdit::singleline(&mut find.replace)
                    .margin(crate::widgets::FIELD_MARGIN)
                    .hint_text("Replace")
                    .desired_width(180.0),
            );
            if replace.lost_focus() && enter(ui) {
                clicked.push(Cmd::ReplaceOne);
            }
            if ui.button("Replace").clicked() {
                clicked.push(Cmd::ReplaceOne);
            }
            if ui.button("Replace all").clicked() {
                clicked.push(Cmd::ReplaceAll);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::icon_button(ui, icon::X, "Close (Esc)").clicked() {
                    clicked.push(Cmd::CloseFind);
                }
            });
        });
    }

    fn editor_area(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = Palette::of(ui);
        let font_size = self.font_size();
        let current_mark = self.current_match();
        let Some(tab) = self.tabs.get_mut(self.active) else {
            ui.painter().rect_filled(ui.max_rect(), 0.0, palette.canvas);
            ui.centered_and_justified(|ui| {
                let hints = "Shift Shift  Search everywhere\n\nCtrl+Shift+P  Commands\n\n\
                             Ctrl+Shift+F  Find in files\n\nCtrl+J  Terminal\n\n\
                             F1  All shortcuts";
                ui.label(RichText::new(hints).size(15.0).color(palette.text_dim));
            });
            return;
        };
        let unit = tab.indent.unit();
        let config = editor::Config {
            language: tab.language,
            font_size,
            indent: &unit,
            marks: &self.find.matches,
            current_mark,
            squiggles: &tab.squiggles,
            // Typing moves the lines: the marks come back with the next save.
            changes: if tab.changes_at == tab.revision {
                &tab.changes
            } else {
                &[]
            },
            occurrences: &self.occurrences.1,
            wrap: tab.wrap,
        };
        let id = tab.editor_id();
        let mut menu = |ui: &mut Ui| {
            ui.set_min_width(260.0);
            menu_items(ui, clicked, EDITOR_MENU);
        };
        let output = editor::show(ui, id, &mut tab.text, &config, &mut tab.view, &mut menu);
        if output.changed {
            tab.touched();
        }
        if let Some(selection) = output.selection {
            // Moved by hand: there is nothing to shrink back to any more.
            if selection != tab.selection {
                tab.grown.clear();
            }
            tab.selection = selection;
        }
        if let Some(at) = output.jump {
            let ctx = ui.ctx().clone();
            self.go_to_declaration(&ctx, Some(at));
        }
    }

    /// Where the caret is: the folders of the file, the file, the definition around it.
    fn breadcrumbs(&self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = Palette::of(ui);
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let root = self.folder();
        let inside = tab
            .path
            .as_deref()
            .zip(root.as_deref())
            .and_then(|(path, root)| path.strip_prefix(root).ok());
        let parts: Vec<String> = match inside {
            Some(relative) => relative
                .iter()
                .map(|part| part.to_string_lossy().into_owned())
                .collect(),
            None => vec![tab.name.clone()],
        };
        let symbol = self
            .symbols
            .1
            .iter()
            .rfind(|symbol| symbol.line <= self.caret_line);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.spacing_mut().button_padding = vec2(5.0, 1.0);
            let crumb = |ui: &mut Ui, text: String, strong: bool| {
                let color = if strong {
                    palette.text
                } else {
                    palette.text_dim
                };
                let text = RichText::new(text).size(12.0).color(color);
                ui.add(
                    Button::new(text)
                        .frame_when_inactive(false)
                        .stroke(Stroke::NONE),
                )
            };
            let arrow = |ui: &mut Ui| {
                let arrow = RichText::new(icon::CARET_RIGHT)
                    .size(9.0)
                    .color(palette.text_dim);
                ui.add(egui::Label::new(arrow).selectable(false));
            };
            let file = parts.len().saturating_sub(1);
            for (index, part) in parts.iter().enumerate() {
                if index > 0 {
                    arrow(ui);
                }
                let is_file = index == file;
                if crumb(ui, part.clone(), is_file && symbol.is_none()).clicked() {
                    clicked.push(if is_file {
                        Cmd::GoToSymbol
                    } else {
                        Cmd::Reveal
                    });
                }
            }
            if let Some(symbol) = symbol {
                arrow(ui);
                let label = with_icon(symbol_icon(symbol.kind).0, &symbol.name);
                if crumb(ui, label, true).clicked() {
                    clicked.push(Cmd::GoToSymbol);
                }
            }
        });
    }

    // ------------------------------------------------------------ home screen

    /// Start here: a new file, a folder, or something worked on lately.
    fn welcome(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = Palette::of(ui);
        let mut open_recent = None;
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.set_max_width(600.0);
                ui.add_space(48.0);
                if let Some(logo) = &self.logo {
                    ui.add(
                        egui::Image::new(logo)
                            .fit_to_exact_size(vec2(112.0, 112.0))
                            .corner_radius(CornerRadius::same(24)),
                    );
                }
                ui.add_space(10.0);
                ui.label(RichText::new(APP_NAME).size(40.0).strong());
                let tagline = "Edit, run and check code. Local. Pure Rust.";
                ui.label(RichText::new(tagline).size(17.0).color(palette.text_dim));
                ui.add_space(24.0);
                ui.horizontal(|ui| {
                    // Center the three primary buttons.
                    let width = 3.0 * 180.0 + 2.0 * ui.spacing().item_spacing.x;
                    ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                    let label = with_icon(icon::FOLDER_OPEN, "Open folder…");
                    let folder = Button::new(RichText::new(label).size(16.0).color(palette.bg))
                        .fill(palette.accent)
                        .corner_radius(CornerRadius::same(10))
                        .min_size(vec2(180.0, 44.0));
                    if ui.add(folder).clicked() {
                        clicked.push(Cmd::OpenFolder);
                    }
                    let outlined = |glyph, label| {
                        Button::new(RichText::new(with_icon(glyph, label)).size(16.0))
                            .stroke(Stroke::new(1.0, palette.accent))
                            .corner_radius(CornerRadius::same(10))
                            .min_size(vec2(180.0, 44.0))
                    };
                    if ui.add(outlined(icon::FILE_CODE, "Open file…")).clicked() {
                        clicked.push(Cmd::OpenFile);
                    }
                    if ui.add(outlined(icon::FILE_PLUS, "New file")).clicked() {
                        clicked.push(Cmd::NewFile);
                    }
                });
                let hint = "…or drop a folder or files anywhere on this window";
                ui.label(RichText::new(hint).color(palette.text_dim));
                ui.add_space(30.0);
                ui.label(RichText::new("RECENT").size(12.0).color(palette.text_dim));
                ui.add_space(4.0);
                if self.recent.is_empty() {
                    let none = "Folders and files you open will be listed here.";
                    ui.label(RichText::new(none).color(palette.text_dim));
                }
                for path in &self.recent {
                    let parent = path
                        .parent()
                        .map(|folder| folder.display().to_string())
                        .unwrap_or_default();
                    let name = file_name(path);
                    let glyph = if path.extension().is_some() {
                        tools::file_icon(&name).glyph
                    } else {
                        icon::FOLDER_SIMPLE
                    };
                    let row = Button::new(RichText::new(with_icon(glyph, &name)).size(15.0))
                        .frame_when_inactive(false)
                        .stroke(Stroke::NONE)
                        .right_text(RichText::new(parent).size(12.0).color(palette.text_dim))
                        .wrap_mode(egui::TextWrapMode::Truncate)
                        .min_size(vec2(ui.available_width(), 34.0));
                    if ui
                        .add(row)
                        .on_hover_text(path.display().to_string())
                        .clicked()
                    {
                        open_recent = Some(path.clone());
                    }
                }
                ui.add_space(30.0);
            });
        });
        if let Some(path) = open_recent {
            if path.exists() {
                self.open_path(ui.ctx(), &path);
            } else {
                self.recent.retain(|known| *known != path);
                self.toasts
                    .error(format!("{} no longer exists", path.display()));
            }
        }
    }

    // ------------------------------------------------------------ dialogs

    /// What the palette offers for `query`.
    fn picks(&self, query: &str) -> Vec<Pick> {
        if let Some(wanted) = query.strip_prefix('>') {
            let wanted = wanted.trim().to_lowercase();
            return COMMANDS
                .iter()
                .filter(|spec| spec.label.to_lowercase().contains(&wanted))
                .map(Pick::Command)
                .collect();
        }
        if let Some(line) = query.strip_prefix(':') {
            let line = line.trim().parse::<usize>().ok().filter(|line| *line > 0);
            return line.map(Pick::Line).into_iter().collect();
        }
        if let Some(wanted) = query.strip_prefix('@') {
            let wanted = wanted.trim().to_lowercase();
            let Some(tab) = self.tabs.get(self.active) else {
                return Vec::new();
            };
            return outline::symbols(&tab.text, tab.language)
                .into_iter()
                .filter(|symbol| symbol.name.to_lowercase().contains(&wanted))
                .map(Pick::Symbol)
                .collect();
        }
        // Search everywhere: the files, then the commands and the definitions of this file.
        let query = query.trim();
        let mut picks = Vec::new();
        if query.is_empty() {
            // The recent files, the one left last first: Enter goes back to it.
            let mut recent: Vec<usize> = (0..self.tabs.len())
                .filter(|index| *index != self.active)
                .collect();
            recent.sort_by_key(|index| std::cmp::Reverse(self.tabs[*index].used));
            picks.extend(recent.into_iter().map(Pick::Tab));
        }
        let mut scored: Vec<(i32, &String)> = self
            .index
            .iter()
            .filter_map(|path| fuzzy::score(query, path).map(|score| (score, path)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        let files = scored.into_iter().take(MAX_PICKS);
        picks.extend(files.map(|(_, path)| Pick::File(path.clone())));
        if !query.is_empty() {
            let wanted = query.to_lowercase();
            let commands = COMMANDS
                .iter()
                .filter(|spec| spec.label.to_lowercase().contains(&wanted));
            picks.extend(commands.take(MAX_EXTRA_PICKS).map(Pick::Command));
            let symbols = self
                .symbols
                .1
                .iter()
                .filter(|symbol| symbol.name.to_lowercase().contains(&wanted));
            picks.extend(symbols.take(MAX_EXTRA_PICKS).cloned().map(Pick::Symbol));
        }
        picks
    }

    fn quick_dialog(&mut self, ctx: &Context) {
        let Some(mut quick) = self.quick.take() else {
            return;
        };
        let picks = self.picks(&quick.query);
        // Taken before the text field sees them: the arrows move in the list.
        let (up, down, enter) = ctx.input_mut(|input| {
            (
                input.consume_key(Modifiers::NONE, Key::ArrowUp),
                input.consume_key(Modifiers::NONE, Key::ArrowDown),
                input.consume_key(Modifiers::NONE, Key::Enter),
            )
        });
        if down {
            quick.selected += 1;
        }
        if up {
            quick.selected = quick.selected.saturating_sub(1);
        }
        quick.selected = quick.selected.min(picks.len().saturating_sub(1));
        let palette = Palette::of_ctx(ctx);
        let root = self.folder();
        let mut chosen = None;
        let response = Modal::new(Id::new("quick-open")).show(ctx, |ui| {
            ui.set_width(560.0);
            let field = ui.add(
                TextEdit::singleline(&mut quick.query)
                    .margin(crate::widgets::FIELD_MARGIN)
                    .hint_text("Search everywhere      >command      :line      @symbol")
                    .desired_width(f32::INFINITY),
            );
            if std::mem::take(&mut quick.focus) {
                field.request_focus();
            }
            if field.changed() {
                quick.selected = 0;
            }
            ui.add_space(6.0);
            ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (index, pick) in picks.iter().enumerate() {
                    let (label, detail): (egui::WidgetText, String) = match pick {
                        Pick::File(path) => {
                            let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
                            (widgets::after_icon(ui, name).into(), folder.to_owned())
                        }
                        Pick::Tab(index) => {
                            let tab = &self.tabs[*index];
                            // Its folder, from the open folder when it is inside.
                            let folder = tab.path.as_deref().and_then(Path::parent);
                            let folder = folder.map(|folder| {
                                let inside = root
                                    .as_deref()
                                    .and_then(|root| folder.strip_prefix(root).ok());
                                let name = folder.file_name().map(Path::new);
                                let shown = inside.or(name).unwrap_or(folder);
                                shown.display().to_string().replace('\\', "/")
                            });
                            (
                                widgets::after_icon(ui, &tab.name).into(),
                                folder.unwrap_or_default(),
                            )
                        }
                        Pick::Command(spec) => (
                            with_icon(spec.glyph, spec.label).into(),
                            spec.shortcut.to_owned(),
                        ),
                        Pick::Line(line) => (
                            with_icon(icon::ARROW_RIGHT, &format!("Go to line {line}")).into(),
                            String::new(),
                        ),
                        Pick::Symbol(symbol) => (
                            with_icon(symbol_icon(symbol.kind).0, &symbol.name).into(),
                            format!("{}, line {}", symbol.kind, symbol.line + 1),
                        ),
                    };
                    let detail = RichText::new(detail).size(12.0).color(palette.text_dim);
                    let row = Button::selectable(index == quick.selected, label)
                        .frame_when_inactive(index == quick.selected)
                        .stroke(Stroke::NONE)
                        .right_text(detail)
                        .wrap_mode(egui::TextWrapMode::Truncate)
                        .min_size(vec2(ui.available_width(), 28.0));
                    let row = ui.add(row);
                    let file = match pick {
                        Pick::File(path) => Some(path.rsplit('/').next().unwrap_or(path)),
                        Pick::Tab(index) => Some(self.tabs[*index].name.as_str()),
                        _ => None,
                    };
                    if let Some(name) = file {
                        let center = pos2(row.rect.min.x + 19.0, row.rect.center().y);
                        widgets::paint_file_icon(ui.painter(), center, &tools::file_icon(name));
                    }
                    if row.clicked() {
                        chosen = Some(index);
                    }
                    if index == quick.selected && (up || down) {
                        row.scroll_to_me(None);
                    }
                }
                if picks.is_empty() {
                    let files = !quick.query.starts_with(['>', ':', '@']);
                    let none = if files && self.explorer.is_none() {
                        "Open a folder to go to its files."
                    } else {
                        "No match."
                    };
                    ui.label(RichText::new(none).color(palette.text_dim));
                }
            });
        });
        if enter && !picks.is_empty() {
            chosen = Some(quick.selected);
        }
        let Some(pick) = chosen.and_then(|index| picks.into_iter().nth(index)) else {
            if !response.should_close() {
                self.quick = Some(quick);
            }
            return;
        };
        let line = match pick {
            Pick::File(relative) => {
                if let Some(root) = self.folder() {
                    self.record_jump();
                    self.open_file(ctx, &root.join(relative));
                }
                return;
            }
            Pick::Tab(index) => {
                self.record_jump();
                self.active = index;
                self.home = false;
                self.tabs[index].view.focus = true;
                return;
            }
            Pick::Command(spec) => {
                self.pending.push(spec.cmd);
                ctx.request_repaint();
                return;
            }
            Pick::Line(line) => line,
            Pick::Symbol(symbol) => symbol.line + 1,
        };
        self.record_jump();
        if let Some(tab) = self.tabs.get_mut(self.active) {
            let start = godcode_core::line_start(&tab.text, line);
            tab.select(ctx, start..start);
        }
    }

    fn shortcuts_dialog(&mut self, ctx: &Context) {
        if !self.shortcuts {
            return;
        }
        let palette = Palette::of_ctx(ctx);
        let mut close = false;
        let response = Modal::new(Id::new("shortcuts")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading("Keyboard shortcuts");
            ui.add_space(6.0);
            ScrollArea::vertical().max_height(440.0).show(ui, |ui| {
                let commands = COMMANDS
                    .iter()
                    .filter(|spec| !spec.shortcut.is_empty())
                    .map(|spec| (with_icon(spec.glyph, spec.label), spec.shortcut));
                let keys = EDITOR_KEYS
                    .iter()
                    .map(|(label, keys)| (with_icon(icon::KEYBOARD, label), *keys));
                egui::Grid::new("shortcut-list")
                    .num_columns(2)
                    .spacing(vec2(40.0, 8.0))
                    .show(ui, |ui| {
                        for (label, keys) in commands.chain(keys) {
                            ui.label(label);
                            ui.label(RichText::new(keys).monospace().color(palette.text_dim));
                            ui.end_row();
                        }
                    });
            });
            ui.add_space(10.0);
            close = ui.button("Close").clicked();
        });
        if close || response.should_close() {
            self.shortcuts = false;
        }
    }

    fn prompt_dialog(&mut self, ctx: &Context) {
        let Some(mut prompt) = self.prompt.take() else {
            return;
        };
        let (title, action) = match &prompt.kind {
            PromptKind::NewFile(_) => ("New file", "Create"),
            PromptKind::NewFolder(_) => ("New folder", "Create"),
            PromptKind::Rename(_) => ("Rename", "Rename"),
            PromptKind::Word(_) => ("Rename all occurrences", "Rename"),
            PromptKind::Delete(_) => ("Delete", "Move to recycle bin"),
        };
        let mut confirmed = false;
        let mut cancelled = false;
        let response = Modal::new(Id::new("file-prompt")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(title);
            ui.add_space(6.0);
            if matches!(prompt.kind, PromptKind::Delete(_)) {
                ui.label(format!("Move {} to the recycle bin?", prompt.name));
            } else {
                let field = ui.add(
                    TextEdit::singleline(&mut prompt.name)
                        .margin(crate::widgets::FIELD_MARGIN)
                        .hint_text("Name")
                        .desired_width(f32::INFINITY),
                );
                if std::mem::take(&mut prompt.focus) {
                    field.request_focus();
                }
                confirmed = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
            }
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                confirmed |= ui.button(action).clicked();
                cancelled = ui.button("Cancel").clicked();
            });
        });
        if confirmed && let PromptKind::Word(old) = &prompt.kind {
            self.rename_word(ctx, old, prompt.name.trim());
        } else if confirmed {
            match self.file_operation(ctx, &prompt) {
                Ok(created) => {
                    self.files_changed(ctx);
                    if let Some(path) = created {
                        self.open_file(ctx, &path);
                    }
                }
                Err(err) => self.toasts.error(err),
            }
        } else if !cancelled && !response.should_close() {
            self.prompt = Some(prompt);
        }
    }

    /// Do what `prompt` asked on disk. Returns the file to open, if one was created.
    fn file_operation(
        &mut self,
        ctx: &Context,
        prompt: &Prompt,
    ) -> Result<Option<PathBuf>, String> {
        let name = prompt.name.trim();
        // A name, possibly with sub-folders, that stays inside its folder.
        let parts = Path::new(name).components();
        let valid = !name.is_empty()
            && parts
                .clone()
                .all(|part| matches!(part, Component::Normal(_)));
        match &prompt.kind {
            // Nothing on disk: the caller renames in the text.
            PromptKind::Word(_) => Ok(None),
            PromptKind::Delete(path) => {
                trash::delete(path).map_err(|err| format!("Cannot delete {name}: {err}"))?;
                // Unchanged editors of what is gone close; edited ones keep their text.
                let mut index = 0;
                while index < self.tabs.len() {
                    let tab = &self.tabs[index];
                    let gone = tab
                        .path
                        .as_deref()
                        .is_some_and(|open| open.starts_with(path));
                    if gone && !tab.dirty {
                        self.close_tab(ctx, index);
                    } else {
                        index += 1;
                    }
                }
                Ok(None)
            }
            PromptKind::NewFile(dir) | PromptKind::NewFolder(dir) => {
                if !valid {
                    return Err("Enter a file name".to_owned());
                }
                let path = dir.join(name);
                if path.exists() {
                    return Err(format!("{name} already exists"));
                }
                let failed = |err: std::io::Error| format!("Cannot create {name}: {err}");
                let is_file = matches!(prompt.kind, PromptKind::NewFile(_));
                let folder = if is_file {
                    path.parent()
                } else {
                    Some(path.as_path())
                };
                if let Some(folder) = folder {
                    std::fs::create_dir_all(folder).map_err(failed)?;
                    if let Some(explorer) = &mut self.explorer {
                        explorer.expand(folder);
                    }
                }
                if is_file {
                    std::fs::File::create_new(&path).map_err(failed)?;
                    return Ok(Some(path));
                }
                Ok(None)
            }
            PromptKind::Rename(old) => {
                if !valid || parts.count() != 1 {
                    return Err("Enter a file name".to_owned());
                }
                let new = old.with_file_name(name);
                if new == *old {
                    return Ok(None);
                }
                // On Windows a change of case only "exists" already: it is the same file.
                let same = file_name(old).eq_ignore_ascii_case(name);
                if new.exists() && !same {
                    return Err(format!("{name} already exists"));
                }
                std::fs::rename(old, &new)
                    .map_err(|err| format!("Cannot rename to {name}: {err}"))?;
                for tab in &mut self.tabs {
                    let Some(inside) = tab
                        .path
                        .as_deref()
                        .and_then(|open| open.strip_prefix(old).ok())
                    else {
                        continue;
                    };
                    let moved = if inside.as_os_str().is_empty() {
                        new.clone()
                    } else {
                        new.join(inside)
                    };
                    tab.set_path(moved);
                }
                Ok(None)
            }
        }
    }

    fn closing_dialog(&mut self, ctx: &Context) {
        let Some(closing) = self.closing else {
            return;
        };
        let question = match closing {
            Closing::Tab(index) => match self.tabs.get(index) {
                Some(tab) => format!("Save changes to {}?", tab.name),
                None => {
                    self.closing = None;
                    return;
                }
            },
            Closing::Window => "Save changes before closing?".to_owned(),
        };
        let mut choice = None;
        // A modal greys out the window behind it until the question is answered.
        let response = Modal::new(Id::new("unsaved-changes")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Unsaved changes");
            ui.add_space(6.0);
            ui.label(question);
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    choice = Some(Some(true));
                }
                if ui.button("Don't save").clicked() {
                    choice = Some(Some(false));
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(None);
                }
            });
        });
        // Escape or a click outside cancels.
        if choice.is_none() && response.should_close() {
            choice = Some(None);
        }
        let Some(choice) = choice else {
            return;
        };
        self.closing = None;
        let Some(save) = choice else {
            return;
        };
        match closing {
            Closing::Tab(index) => {
                if !save || self.save_tab(ctx, index, false) {
                    self.close_tab(ctx, index);
                }
            }
            Closing::Window => {
                let dirty: Vec<usize> = (0..self.tabs.len())
                    .filter(|index| self.tabs[*index].dirty)
                    .collect();
                if !save
                    || dirty
                        .into_iter()
                        .all(|index| self.save_tab(ctx, index, false))
                {
                    self.quit = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
    }
}

impl eframe::App for GodApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut clicked = std::mem::take(&mut self.pending);
        self.poll();
        self.shortcuts(&ctx, &mut clicked);
        self.guard_close(&ctx);
        self.update_title(&ctx);
        self.refresh_find();
        self.refresh_problems();
        self.refresh_symbols();
        self.refresh_occurrences();
        // The tab in front is the most recent of the recent files.
        if let Some(tab) = self.tabs.get_mut(self.active)
            && tab.used != self.clock
        {
            self.clock += 1;
            tab.used = self.clock;
        }
        let palette = Palette::of(ui);
        let plain = |fill, margin: Margin| Frame::new().fill(fill).inner_margin(margin);

        Panel::top("menu-bar")
            .frame(plain(palette.surface, Margin::symmetric(8, 3)))
            .show_separator_line(false)
            .show(ui, |ui| self.menu_bar(ui, &mut clicked));
        if self.at_home() {
            CentralPanel::default()
                .frame(Frame::new().fill(palette.bg))
                .show(ui, |ui| self.welcome(ui, &mut clicked));
        } else {
            Panel::bottom("status")
                .frame(plain(palette.bg, Margin::symmetric(12, 3)))
                .show(ui, |ui| self.status_bar(ui, &mut clicked));
            Panel::left("activity-bar")
                .resizable(false)
                .exact_size(ACTIVITY_WIDTH)
                .frame(plain(palette.surface, Margin::ZERO))
                .show(ui, |ui| self.activity_bar(ui));
            if self.sidebar {
                Panel::left("side-bar")
                    .resizable(true)
                    .default_size(260.0)
                    .size_range(180.0..=520.0)
                    .frame(plain(palette.bg, Margin::symmetric(0, 8)))
                    .show(ui, |ui| match self.side {
                        Side::Explorer => self.explorer_panel(ui, &mut clicked),
                        Side::Search => self.search_panel(ui),
                        Side::Git => self.git_panel(ui),
                        Side::Structure => self.structure_panel(ui),
                    });
            }
            if self.bottom.is_some() {
                Panel::bottom("bottom-panel")
                    .resizable(true)
                    .default_size(240.0)
                    .size_range(120.0..=600.0)
                    .frame(plain(palette.bg, Margin::symmetric(0, 6)))
                    .show(ui, |ui| self.bottom_panel(ui, &mut clicked));
            }
            if !self.tabs.is_empty() {
                let margin = Margin {
                    left: 4,
                    right: 4,
                    top: 4,
                    bottom: 0,
                };
                Panel::top("editor-tabs")
                    .frame(plain(palette.surface, margin))
                    .show_separator_line(false)
                    .show(ui, |ui| self.editor_tabs(ui, &mut clicked));
                Panel::top("breadcrumbs")
                    .frame(plain(palette.bg, Margin::symmetric(8, 2)))
                    .show_separator_line(false)
                    .show(ui, |ui| self.breadcrumbs(ui, &mut clicked));
            }
            if self.find.open && !self.tabs.is_empty() {
                Panel::top("find-bar")
                    .frame(plain(palette.surface, Margin::symmetric(12, 5)))
                    .show(ui, |ui| self.find_bar(ui, &mut clicked));
            }
            CentralPanel::default()
                .frame(Frame::new().fill(palette.bg))
                .show(ui, |ui| self.editor_area(ui, &mut clicked));
        }
        for cmd in clicked {
            self.run(&ctx, cmd);
        }
        self.closing_dialog(&ctx);
        self.prompt_dialog(&ctx);
        self.quick_dialog(&ctx);
        self.shortcuts_dialog(&ctx);
        self.toasts.show(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let flag = |on: bool| if on { "1" } else { "0" }.to_owned();
        storage.set_string("dark_mode", flag(self.dark));
        storage.set_string("sidebar", flag(self.sidebar));
        storage.set_string("wrap", flag(self.wrap));
        storage.set_string("zoom", self.zoom.to_string());
        let recent: Vec<String> = self
            .recent
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        storage.set_string("recent", recent.join("\n"));
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Icon and colour of a definition of the outline, from the word that defines it.
fn symbol_icon(kind: &str) -> (&'static str, Color32) {
    let rgb = |hex: u32| Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
    match kind {
        "fn" | "def" | "function" | "func" => (icon::FUNCTION, rgb(0xC6_78_DD)),
        "struct" | "class" | "impl" => (icon::CUBE, rgb(0x4F_9C_E8)),
        "enum" | "type" => (icon::STACK, rgb(0xD0_A8_3F)),
        "trait" | "interface" => (icon::PUZZLE_PIECE, rgb(0x4E_AA_25)),
        "mod" | "module" | "namespace" => (icon::PACKAGE, rgb(0xC9_8A_4B)),
        "id" => (icon::HASH, rgb(0xE4_4D_26)),
        "rule" => (icon::PALETTE, rgb(0x5B_8D_EF)),
        "heading" => (icon::TEXT_H, rgb(0x8F_9B_A8)),
        _ => (icon::DOT_OUTLINE, rgb(0x8F_9B_A8)),
    }
}

/// The entries of a menu: commands with their icon and shortcut, `None` being a separator.
fn menu_items(ui: &mut Ui, clicked: &mut Vec<Cmd>, items: &[Option<Cmd>]) {
    for item in items {
        match item.and_then(spec_of) {
            Some(spec) => {
                let label = with_icon(spec.glyph, spec.label);
                let button = Button::new(label).shortcut_text(spec.shortcut);
                if ui.add(button).clicked() {
                    clicked.push(spec.cmd);
                }
            }
            None => {
                ui.separator();
            }
        }
    }
}

/// A full-width clickable line of text, cut to the width.
fn list_row(ui: &mut Ui, palette: &Palette, text: &str, indent: f32, color: Color32) -> Response {
    let size = vec2(ui.available_width(), LIST_ROW);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 0.0, palette.hover);
    }
    let mut job = LayoutJob::simple_singleline(text.to_owned(), FontId::proportional(13.0), color);
    job.wrap = TextWrapping::truncate_at_width((rect.width() - indent - 6.0).max(0.0));
    let galley = painter.layout_job(job);
    let top = rect.center().y - galley.size().y / 2.0;
    painter.galley(pos2(rect.min.x + indent, top), galley, color);
    response
}

/// One file tab; returns (selected, close requested) and the tab itself, for its menu.
fn doc_tab(
    ui: &mut Ui,
    palette: &Palette,
    title: &str,
    dirty: bool,
    active: bool,
    text_max: f32,
) -> (bool, bool, Response) {
    let text_color = if active {
        palette.text
    } else {
        palette.text_dim
    };
    let mut job =
        LayoutJob::simple_singleline(title.to_owned(), FontId::proportional(14.0), text_color);
    job.wrap = TextWrapping::truncate_at_width(text_max);
    let galley = ui.painter().layout_job(job);
    let star = if dirty { 14.0 } else { 0.0 };
    let close_size = widgets::SMALL_ICON_BUTTON;
    let width = 12.0 + 16.0 + 8.0 + galley.size().x + star + 8.0 + close_size.x + 8.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, TAB_HEIGHT), Sense::click());
    let painter = ui.painter();
    // The active tab reaches the bottom edge so it merges with the editor below.
    if active {
        let radius = CornerRadius {
            nw: 8,
            ne: 8,
            sw: 0,
            se: 0,
        };
        painter.rect_filled(rect, radius, palette.bg);
    } else if response.hovered() {
        let shape = rect.shrink2(vec2(0.0, 4.0));
        painter.rect_filled(shape, CornerRadius::same(8), palette.hover);
    }
    let mid = rect.center().y;
    widgets::paint_file_icon(
        painter,
        pos2(rect.min.x + 20.0, mid),
        &tools::file_icon(title),
    );
    let text_x = rect.min.x + 36.0;
    let text_top = mid - galley.size().y / 2.0;
    let text_width = galley.size().x;
    painter.galley(pos2(text_x, text_top), galley, text_color);
    if dirty {
        // Unsaved changes: a plain asterisk after the name.
        painter.text(
            pos2(text_x + text_width + 4.0, mid),
            Align2::LEFT_CENTER,
            "*",
            FontId::proportional(15.0),
            text_color,
        );
    }
    let close_rect =
        Rect::from_center_size(pos2(rect.max.x - 8.0 - close_size.x / 2.0, mid), close_size);
    let close = ui
        .put(close_rect, widgets::icon_widget(icon::X, 13.0))
        .on_hover_text("Close");
    let response = response.on_hover_text(title);
    (
        response.clicked(),
        close.clicked() || response.middle_clicked(),
        response,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_entry_is_a_known_command() {
        for (title, items) in MENUS {
            for cmd in items.iter().flatten() {
                assert!(spec_of(*cmd).is_some(), "{title}: {cmd:?} has no label");
            }
        }
    }

    #[test]
    fn a_problem_is_underlined_at_its_word() {
        let problem = Problem {
            line: 1,
            column: 4,
            severity: Severity::Warning,
            message: "unused".to_owned(),
        };
        let mark = squiggle("fn a() {\nlet total = 1;\n}", &problem);
        assert_eq!((mark.start, mark.len, mark.error), (13, 5, false));
        // A column past the end of the line stays on the line.
        let far = Problem {
            column: 99,
            ..problem
        };
        assert_eq!(squiggle("ab\ncd\nef", &far).start, 5);
    }
}
