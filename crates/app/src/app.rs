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
use godcode_core::markdown;
use godcode_core::modal::{self, Ask, Stroke as Typed};
use godcode_core::outline::{self, Symbol};
use godcode_core::search::{self, Hit, Match};
use godcode_core::syntax::Language;
use godcode_core::{Decoded, Indent, LineEnding, complete, edit, format, fuzzy};
use godcode_lsp::Event as ServerEvent;

use crate::explorer::{Action, Explorer};
use crate::field::Hint;
use crate::servers::{self, Servers};
use crate::terminal::Terminal;
use crate::theme::{self, Palette};
use crate::toasts::Toasts;
use crate::widgets::{self, icon, with_icon};
use crate::{APP_NAME, branding, editor, platform, preview, tools};

const TAB_HEIGHT: f32 = 34.0;
/// Widest a tab's file name gets before it is cut with an ellipsis.
const TAB_TEXT_MAX: f32 = 180.0;
const ACTIVITY_WIDTH: f32 = 44.0;
const LIST_ROW: f32 = 22.0;
const MAX_RECENT: usize = 10;
const FONT_SIZE: f32 = 14.0;
const ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.6..=3.0;
/// Larger files are refused: the editor lays the whole text out.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_HITS: usize = 5000;
const MAX_PICKS: usize = 50;
const MAX_REPORTS: usize = 500;
/// A file with a line this long (in bytes) is minified: opening it says how to read it.
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
/// With auto save on, a file is written this many seconds after its last change.
const AUTOSAVE_DELAY: f64 = 1.0;
/// Completions of a language server shown at once.
const MAX_SERVED: usize = 60;
/// The pointer rests this many seconds on a name before its documentation is asked for.
const HOVER_DELAY: f64 = 0.6;
/// Versions of a file kept by the local history.
const MAX_VERSIONS: usize = 40;

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
    Preview,
    Minimap,
    Keys,
    Hints,
    Fold,
    UnfoldAll,
    StickyScroll,
    AutoSave,
    FormatOnSave,
    Documentation,
    LanguageServer,
    QuickFix,
    Implementation,
    Split,
    Branches,
    Diff,
    Blame,
    History,
    RunTests,
    TestResults,
    RunCoverage,
    MergeView,
    Coverage,
    Environment,
    Themes,
    Font,
    FontReset,
    Zen,
    CaretAbove,
    CaretBelow,
    Tasks,
    RunLast,
    RunSettings,
    GitLog,
    GitGraph,
    Stash,
    StashPop,
    Snippets,
    EditThemes,
    Response,
    Debug,
    StopDebug,
    StepOver,
    StepInto,
    StepOut,
    Breakpoint,
    BreakpointCondition,
    DebugPanel,
    LivePreview,
    LivePreviewBrowser,
    TestAtCaret,
    NewProject,
    Keymap,
    SplitDown,
    SplitSame,
    Callers,
    Callees,
    TestsPanel,
    Merge,
    Rebase,
    AcceptOurs,
    AcceptTheirs,
    AcceptBoth,
    DebugSettings,
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
    spec(
        Cmd::Callers,
        icon::TREE_STRUCTURE,
        "Call Hierarchy: Callers",
        "Ctrl+Alt+H",
    ),
    spec(
        Cmd::Callees,
        icon::TREE_STRUCTURE,
        "Call Hierarchy: Calls Made",
        "",
    ),
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
    spec(
        Cmd::Preview,
        icon::EYE,
        "Toggle Markdown Preview",
        "Ctrl+Shift+V",
    ),
    spec(Cmd::Minimap, icon::ROWS, "Toggle Minimap", ""),
    spec(
        Cmd::Keys,
        icon::KEYBOARD,
        "Switch Keys: Default / Vim / Emacs",
        "",
    ),
    spec(Cmd::Hints, icon::ROWS, "Toggle Inlay Hints", ""),
    spec(Cmd::Fold, icon::ROWS, "Fold / Unfold Block", ""),
    spec(Cmd::UnfoldAll, icon::ROWS, "Unfold All", ""),
    spec(
        Cmd::StickyScroll,
        icon::PUSH_PIN,
        "Toggle Sticky Scroll",
        "",
    ),
    spec(Cmd::AutoSave, icon::CLOCK_CLOCKWISE, "Toggle Auto Save", ""),
    spec(
        Cmd::FormatOnSave,
        icon::CLOCK_CLOCKWISE,
        "Toggle Format on Save",
        "",
    ),
    spec(
        Cmd::Documentation,
        icon::INFO,
        "Quick Documentation",
        "Ctrl+Q",
    ),
    spec(
        Cmd::QuickFix,
        icon::LIGHTBULB,
        "Show Quick Fixes",
        "Alt+Enter",
    ),
    spec(
        Cmd::Implementation,
        icon::TREE_STRUCTURE,
        "Go to Implementation",
        "Ctrl+Alt+B",
    ),
    spec(
        Cmd::Split,
        icon::SQUARE_SPLIT_HORIZONTAL,
        "Split Editor Right",
        "Ctrl+\\",
    ),
    spec(Cmd::Branches, icon::GIT_BRANCH, "Git Branches…", ""),
    spec(Cmd::Diff, icon::ROWS, "Compare with Last Commit", ""),
    spec(Cmd::Blame, icon::INFO, "Toggle Git Blame", ""),
    spec(
        Cmd::History,
        icon::CLOCK_COUNTER_CLOCKWISE,
        "Local History…",
        "",
    ),
    spec(
        Cmd::RunTests,
        icon::CHECK_SQUARE,
        "Run Tests",
        "Ctrl+Shift+T",
    ),
    spec(
        Cmd::TestResults,
        icon::CHECK_SQUARE,
        "Run Tests and Show Results",
        "Ctrl+Alt+T",
    ),
    spec(
        Cmd::RunCoverage,
        icon::CHECK_SQUARE,
        "Run Tests with Coverage",
        "",
    ),
    spec(
        Cmd::Coverage,
        icon::CHECK_SQUARE,
        "Show / Hide Coverage",
        "",
    ),
    spec(
        Cmd::Environment,
        icon::ROWS,
        "HTTP: Choose Environment…",
        "",
    ),
    spec(Cmd::Themes, icon::PALETTE, "Color Theme…", ""),
    spec(Cmd::Font, icon::PALETTE, "Editor Font…", ""),
    spec(Cmd::FontReset, icon::PALETTE, "Reset Editor Font", ""),
    spec(Cmd::Zen, icon::EYE, "Toggle Zen Mode", "Ctrl+Alt+Z"),
    spec(
        Cmd::CaretAbove,
        icon::ARROW_UP,
        "Add Caret Above",
        "Ctrl+Alt+Up",
    ),
    spec(
        Cmd::CaretBelow,
        icon::ARROW_DOWN,
        "Add Caret Below",
        "Ctrl+Alt+Down",
    ),
    spec(Cmd::Tasks, icon::PLAY, "Run Task…", "Ctrl+Shift+R"),
    spec(Cmd::RunLast, icon::PLAY, "Run Last Task", "Ctrl+Alt+R"),
    spec(Cmd::RunSettings, icon::PLAY, "Edit Run Configurations", ""),
    spec(Cmd::GitLog, icon::GIT_BRANCH, "Git Log…", ""),
    spec(Cmd::GitGraph, icon::GIT_BRANCH, "Git Graph…", ""),
    spec(Cmd::Stash, icon::PACKAGE, "Git Stash", ""),
    spec(Cmd::StashPop, icon::PACKAGE, "Git Stash Pop", ""),
    spec(Cmd::Snippets, icon::CODE_BLOCK, "Edit Snippets", ""),
    spec(Cmd::EditThemes, icon::PALETTE, "Edit My Themes", ""),
    spec(Cmd::Response, icon::GLOBE, "Toggle HTTP Response", ""),
    spec(Cmd::Debug, icon::BUG, "Debug File", "Ctrl+F5"),
    spec(Cmd::StopDebug, icon::STOP, "Stop Debugging", "Shift+F5"),
    spec(
        Cmd::StepOver,
        icon::ARROW_ELBOW_DOWN_RIGHT,
        "Step Over",
        "F10",
    ),
    spec(Cmd::StepInto, icon::ARROW_LINE_DOWN, "Step Into", "F11"),
    spec(Cmd::StepOut, icon::ARROW_LINE_UP, "Step Out", "Shift+F11"),
    spec(
        Cmd::Breakpoint,
        icon::DOT_OUTLINE,
        "Toggle Breakpoint",
        "F9",
    ),
    spec(
        Cmd::BreakpointCondition,
        icon::DOT_OUTLINE,
        "Breakpoint Condition…",
        "Shift+F9",
    ),
    spec(Cmd::DebugPanel, icon::BUG, "Toggle Debug Panel", ""),
    spec(Cmd::LivePreview, icon::BROWSER, "Live Preview", ""),
    spec(
        Cmd::LivePreviewBrowser,
        icon::BROWSER,
        "Live Preview in Browser",
        "",
    ),
    spec(
        Cmd::TestAtCaret,
        icon::CHECK_SQUARE,
        "Run Test at Caret",
        "Ctrl+Shift+F10",
    ),
    spec(Cmd::NewProject, icon::FOLDER_PLUS, "New Project…", ""),
    spec(Cmd::Keymap, icon::KEYBOARD, "Edit Keyboard Shortcuts", ""),
    spec(Cmd::SplitDown, icon::ROWS, "Split Editor Down", ""),
    spec(
        Cmd::SplitSame,
        icon::SQUARE_SPLIT_HORIZONTAL,
        "Split Editor: Same File",
        "",
    ),
    spec(Cmd::TestsPanel, icon::CHECK_SQUARE, "Toggle Tests", ""),
    spec(Cmd::Merge, icon::ARROWS_MERGE, "Git Merge Branch…", ""),
    spec(Cmd::Rebase, icon::GIT_BRANCH, "Git Rebase onto…", ""),
    spec(
        Cmd::MergeView,
        icon::ARROWS_MERGE,
        "Conflict: Open Merge View",
        "",
    ),
    spec(Cmd::AcceptOurs, icon::CHECK, "Conflict: Keep Mine", ""),
    spec(Cmd::AcceptTheirs, icon::CHECK, "Conflict: Keep Theirs", ""),
    spec(Cmd::AcceptBoth, icon::CHECK, "Conflict: Keep Both", ""),
    spec(
        Cmd::DebugSettings,
        icon::BUG,
        "Edit Debug Configurations",
        "",
    ),
    spec(
        Cmd::LanguageServer,
        icon::PLUGS_CONNECTED,
        "Language Server Status",
        "",
    ),
    spec(Cmd::Shortcuts, icon::KEYBOARD, "Keyboard Shortcuts", "F1"),
];

/// Keys of the editor that are not commands, for the shortcut list.
const EDITOR_KEYS: [(&str, &str); 10] = [
    ("Select a rectangle of text", "Alt+Shift+Drag"),
    ("Grow the rectangle", "Ctrl+Alt+Shift+Arrows"),
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
            Some(Cmd::NewProject),
            Some(Cmd::OpenFile),
            Some(Cmd::OpenFolder),
            None,
            Some(Cmd::Save),
            Some(Cmd::SaveAs),
            Some(Cmd::SaveAll),
            Some(Cmd::AutoSave),
            Some(Cmd::FormatOnSave),
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
            None,
            Some(Cmd::CaretAbove),
            Some(Cmd::CaretBelow),
        ],
    ),
    (
        "Code",
        &[
            Some(Cmd::Reformat),
            Some(Cmd::QuickFix),
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
            None,
            Some(Cmd::Snippets),
            Some(Cmd::EditThemes),
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
            Some(Cmd::Split),
            Some(Cmd::SplitDown),
            Some(Cmd::SplitSame),
            Some(Cmd::Zen),
            Some(Cmd::Themes),
            Some(Cmd::Font),
            Some(Cmd::FontReset),
            Some(Cmd::Preview),
            Some(Cmd::Minimap),
            Some(Cmd::Keys),
            Some(Cmd::Hints),
            Some(Cmd::Fold),
            Some(Cmd::UnfoldAll),
            Some(Cmd::StickyScroll),
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
            Some(Cmd::Implementation),
            Some(Cmd::FindUsages),
            Some(Cmd::Callers),
            Some(Cmd::Callees),
            Some(Cmd::Documentation),
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
            Some(Cmd::Debug),
            Some(Cmd::Breakpoint),
            Some(Cmd::BreakpointCondition),
            Some(Cmd::StepOver),
            Some(Cmd::StepInto),
            Some(Cmd::StepOut),
            Some(Cmd::StopDebug),
            Some(Cmd::DebugSettings),
            None,
            Some(Cmd::TestsPanel),
            Some(Cmd::RunTests),
            Some(Cmd::TestResults),
            Some(Cmd::TestAtCaret),
            Some(Cmd::RunCoverage),
            Some(Cmd::Coverage),
            Some(Cmd::Tasks),
            Some(Cmd::RunLast),
            Some(Cmd::RunSettings),
            Some(Cmd::LivePreview),
            Some(Cmd::LivePreviewBrowser),
            Some(Cmd::Response),
            Some(Cmd::Check),
            None,
            Some(Cmd::ToggleTerminal),
            Some(Cmd::RestartTerminal),
            Some(Cmd::ClearTerminal),
        ],
    ),
    (
        "Git",
        &[
            Some(Cmd::SourceControl),
            Some(Cmd::Branches),
            None,
            Some(Cmd::Diff),
            Some(Cmd::Blame),
            Some(Cmd::History),
            None,
            Some(Cmd::GitLog),
            Some(Cmd::GitGraph),
            Some(Cmd::Stash),
            Some(Cmd::StashPop),
            None,
            Some(Cmd::Merge),
            Some(Cmd::Rebase),
            Some(Cmd::MergeView),
            Some(Cmd::AcceptOurs),
            Some(Cmd::AcceptTheirs),
            Some(Cmd::AcceptBoth),
        ],
    ),
    (
        "Help",
        &[Some(Cmd::Shortcuts), Some(Cmd::Keymap), Some(Cmd::Commands)],
    ),
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
    Some(Cmd::QuickFix),
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
    /// The revision last seen by auto save, and when it appeared.
    seen: (u64, f64),
    /// Kept at the start of the tabs, and out of "close others".
    pinned: bool,
    /// The lines (from 0) the debugger stops on; and for some of them the condition under
    /// which it does.
    breakpoints: Vec<usize>,
    conditions: std::collections::HashMap<usize, String>,
    /// How many lines the text had when it last showed; 0 before it ever did.
    lines: usize,
    /// What the language server writes in the lines; and what it was last asked: the
    /// revision of the text, the lines, whether they go to the end of the text. The hints
    /// stay as they are until it has answered about the text typed since.
    hints: Vec<Hint>,
    hints_at: Option<(u64, Range<usize>, bool)>,
    /// What the language server says folds, with the revision of the text it said it of;
    /// and the revision it was last asked about.
    folds: Option<(u64, Vec<crate::field::Fold>)>,
    folds_at: Option<u64>,
    /// The view of the other editor, when this file shows in both.
    other: Option<Twin>,
    /// `view` and `selection` are those of the second editor; `other` holds the first's.
    in_second: bool,
}

/// A second view of a file: where it is scrolled, what it selects.
struct Twin {
    view: editor::View,
    selection: Range<usize>,
    grown: Vec<Range<usize>>,
    /// The revision of the text this view last showed.
    revision: u64,
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
            seen: (0, 0.0),
            pinned: false,
            breakpoints: Vec::new(),
            conditions: Default::default(),
            lines: 0,
            hints: Vec::new(),
            hints_at: None,
            folds: None,
            folds_at: None,
            other: None,
            in_second: false,
        }
    }

    fn editor_id(&self) -> Id {
        if self.in_second {
            Id::new(("editor", self.id, "second"))
        } else {
            Id::new(("editor", self.id))
        }
    }

    /// Trade the view in use for the one of the other editor showing this file. A view
    /// that never showed starts where this one is.
    fn swap_views(&mut self, ctx: &Context) {
        let other = self.other.get_or_insert_with(|| {
            let mut view = editor::View::default();
            view.scroll_to = Some(self.selection.start);
            Twin {
                view,
                selection: self.selection.clone(),
                grown: Vec::new(),
                // Never shown: its caret is still to be placed.
                revision: u64::MAX,
            }
        });
        std::mem::swap(&mut self.view, &mut other.view);
        std::mem::swap(&mut self.selection, &mut other.selection);
        std::mem::swap(&mut self.grown, &mut other.grown);
        let shown = std::mem::replace(&mut other.revision, self.revision);
        self.in_second = !self.in_second;
        if shown != self.revision {
            // The text changed in the other editor: what this one pointed at may be gone.
            let len = self.text.chars().count();
            self.selection = self.selection.start.min(len)..self.selection.end.min(len);
            self.view.extra.clear();
            self.grown.clear();
            editor::set_selection(ctx, self.editor_id(), &self.selection);
        }
    }

    /// The text changed: derived data is stale, and it may differ from the file again.
    fn touched(&mut self) {
        self.revision += 1;
        // A big text is not read through at every key to see if it is back as saved.
        self.dirty = self.text.len() > editor::LARGE_BYTES || hash_of(&self.text) != self.saved;
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
    Response,
    Debug,
    Tests,
    Terminal,
}

/// Find and replace in the active file.
#[derive(Default)]
struct Find {
    open: bool,
    query: String,
    replace: String,
    options: search::Options,
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
    /// A definition of the project, as the language server knows it.
    Place(godcode_lsp::Named),
}

enum PromptKind {
    /// Create in this folder.
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    /// Rename every occurrence of this word in the active file.
    Word(String),
    /// The condition under which the debugger stops on this line of the active file.
    Condition(usize),
    /// The value to give the variable of this name of the program being debugged: a part
    /// of the variable with this reference, or a variable of the call looked at.
    Value(Option<u64>, String),
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

/// What a language server was asked, to know what to do with its answer.
enum Asked {
    /// The definition of what is at this char (the caret when `None`).
    Definition(Option<usize>),
    Documentation,
    /// The documentation of the name starting at this char, the pointer resting on it.
    Tip(usize),
    Rename,
    Format,
    /// The usages of this word.
    References(String),
    /// The call written on this line.
    Signature(usize),
    /// The definitions of the project named like this.
    Symbols(String),
    Fixes,
    /// What is written in the lines of a tab, as its text then was.
    Hints {
        tab: u64,
        revision: u64,
    },
    /// What folds in a tab, as its text then was.
    Folds {
        tab: u64,
        revision: u64,
    },
    /// The completions at a caret of a tab, as its text then was.
    Completion {
        tab: u64,
        revision: u64,
        caret: usize,
    },
}

/// The lines under the top of the view that are to have their hints: more than a view shows.
const HINTS_IN_VIEW: usize = 120;

/// Files whose folded blocks are remembered: the ones last closed.
const MAX_FOLDED_FILES: usize = 200;

/// Remember the blocks folded in `tab`, or that it has none.
fn keep_folds(folds: &mut std::collections::HashMap<PathBuf, Vec<usize>>, tab: &Tab) {
    let Some(path) = &tab.path else {
        return;
    };
    if tab.view.folded.is_empty() {
        folds.remove(path);
    } else {
        folds.insert(path.clone(), tab.view.folded.clone());
    }
}

/// The folded blocks of each file as kept in the settings: a line per file, its path, a
/// tab, then the lines that open the blocks.
fn folds_text(folds: &std::collections::HashMap<PathBuf, Vec<usize>>) -> String {
    let lines = folds.iter().take(MAX_FOLDED_FILES).map(|(path, folded)| {
        let folded: Vec<String> = folded.iter().map(usize::to_string).collect();
        format!("{}\t{}", path.display(), folded.join(","))
    });
    lines.collect::<Vec<_>>().join("\n")
}

/// The reverse of [`folds_text`]. What cannot be read is left out.
fn folds_of(text: &str) -> std::collections::HashMap<PathBuf, Vec<usize>> {
    let files = text.lines().filter_map(|line| {
        let (path, folded) = line.split_once('\t')?;
        let folded = folded.split(',').filter_map(|line| line.parse().ok());
        Some((PathBuf::from(path), folded.collect()))
    });
    files.collect()
}

/// Where a selection starts and ends, as (line, column) from 1, and what that was found
/// for: a tab, its revision, its selection.
type CaretPlace = ((u64, u64, Range<usize>), (usize, usize), (usize, usize));

/// An address as it was typed, made whole: without a scheme it is a page served over HTTP,
/// as the development servers of this computer do.
fn whole_address(typed: &str) -> Option<String> {
    let typed = typed.trim();
    match typed {
        "" => None,
        typed if typed.contains("://") => Some(typed.to_owned()),
        typed => Some(format!("http://{typed}")),
    }
}

/// The tests of a file of the project: its path from the root, then each test with the
/// line (from 0) it starts on.
type FileTests = (String, Vec<(usize, String)>);

/// Files bigger than this are not read for their tests: they are data, or generated.
const MAX_TESTED_BYTES: u64 = 512 * 1024;

/// The tests of the files `listed` under `root`, for the files that have some.
fn tests_under(root: &Path, listed: &[String]) -> Vec<FileTests> {
    let of_file = |relative: &String| {
        let path = root.join(relative);
        let language = Language::of(&path);
        let small = std::fs::metadata(&path).is_ok_and(|meta| meta.len() <= MAX_TESTED_BYTES);
        let text = std::fs::read_to_string(&path).ok().filter(|_| small)?;
        let tests = outline::tests(&text, language);
        (!tests.is_empty()).then(|| (relative.clone(), tests))
    };
    listed.iter().filter_map(of_file).collect()
}

/// Whose keys the editor obeys.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Keys {
    Default,
    Vim,
    Emacs,
}

/// The key of Vim (`commands`: it is not in insert mode) or of Emacs that a key of the
/// keyboard is, if it is one. What is typed in Vim comes as text, not from here.
fn stroke_of(key: Key, modifiers: Modifiers, vim: bool, commands: bool) -> Option<Typed> {
    let (ctrl, alt) = (modifiers.command || modifiers.ctrl, modifiers.alt);
    if vim {
        return Some(match key {
            Key::Escape if modifiers.is_none() => Typed::Escape,
            Key::R if commands && ctrl && !alt && !modifiers.shift => Typed::Ctrl('r'),
            _ if !commands || !modifiers.is_none() => return None,
            Key::Enter => Typed::Enter,
            Key::Backspace => Typed::Backspace,
            Key::Delete => Typed::Delete,
            Key::ArrowLeft => Typed::Left,
            Key::ArrowRight => Typed::Right,
            Key::ArrowUp => Typed::Up,
            Key::ArrowDown => Typed::Down,
            _ => return None,
        });
    }
    let named = match key {
        Key::Space => ' ',
        Key::Slash => '/',
        Key::Minus => '_',
        key => {
            let mut name = key.name().chars();
            match (name.next(), name.next()) {
                (Some(letter), None) if letter.is_ascii_alphabetic() => letter.to_ascii_lowercase(),
                _ => return None,
            }
        }
    };
    match (ctrl, alt, modifiers.shift) {
        (true, false, false) => Some(Typed::Ctrl(named)),
        (false, true, false) => Some(Typed::Alt(named)),
        _ => None,
    }
}

/// Something the project can run: what it is called, and the command.
type Task = (String, String);

/// A call being written: its line, its signature, the chars of the argument typed.
type Call = (usize, String, Option<(usize, usize)>);

/// What the menu of a tab asks for.
#[derive(Clone, Copy)]
enum TabAction {
    CloseOthers,
    CloseRight,
    CloseAll,
    CopyPath,
    Reveal,
    Pin,
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
    /// Markdown files show their rendered page beside their text.
    preview: bool,
    /// The blocks of the Markdown file in front, and what they were read from.
    blocks: (u64, Vec<markdown::Placed>),
    /// Where the preview drew each block: its line, its distance from the top of the page.
    preview_starts: Vec<(f32, f32)>,
    /// The line at the top of the editor, as of the last frame.
    top_line: f32,
    /// The whole file shows as a strip right of the editor.
    minimap: bool,
    /// Whose keys the editor obeys, and what Vim and Emacs remember between keys.
    keys: Keys,
    vim: modal::Vim,
    emacs: modal::Emacs,
    /// The language server writes hints in the lines: types, names of arguments.
    hints: bool,
    /// The blocks folded in each file, kept from one session to the next.
    folds: std::collections::HashMap<PathBuf, Vec<usize>>,
    /// The lines that open the blocks stay in view above the code.
    sticky: bool,
    /// Files are written a moment after each change.
    autosave: bool,
    /// A file is formatted when it is saved by hand.
    format_on_save: bool,
    /// The language servers of the open files.
    servers: Servers,
    /// The requests they have not answered yet.
    asked: std::collections::HashMap<u64, Asked>,
    /// The problems they found, by file; bumped `diagnosed` tells when they changed.
    diagnostics: std::collections::HashMap<PathBuf, Vec<Problem>>,
    diagnosed: u64,
    /// The documentation on show, as Markdown.
    documentation: Option<Vec<markdown::Placed>>,
    /// The name the pointer rests on: where it starts, since when, and whether its
    /// documentation was asked for.
    rest: Option<(usize, f64, bool)>,
    pointer: egui::Pos2,
    /// The documentation shown at the pointer, for the name starting at this char.
    tip: Option<(usize, egui::Pos2, Vec<markdown::Placed>)>,
    /// The name the debugger was asked the worth of, the pointer resting on it: where it
    /// starts, and the name.
    hovered: Option<(usize, String)>,
    /// The fixes offered for the line of the caret, and the one chosen.
    fixes: Option<(Vec<godcode_lsp::Action>, usize)>,
    /// The call being written: its line, its signature, the chars of the argument typed.
    signature: Option<Call>,
    /// Line and column (from 1) of the start and of the end of the selection, and what
    /// they were found for: a tab, its revision, its selection.
    caret_place: CaretPlace,
    /// The foot of the caret on screen.
    caret_pos: Option<egui::Pos2>,
    /// The editor in front had the keyboard the last time it showed.
    editor_focused: bool,
    /// The definitions of the project last asked for by name, and that name.
    project_symbols: Vec<godcode_lsp::Named>,
    symbols_asked: String,
    /// The completions of the language server last offered: one picked may add an import.
    served: Vec<godcode_lsp::Item>,
    /// The tab of the editor on the left, of the one on the right when the view is
    /// split, and which of the two has the keyboard.
    left: u64,
    right: Option<u64>,
    right_front: bool,
    /// Both editors show the same file, on purpose.
    twin: bool,
    /// The branches to pick from, what was typed to narrow them, the one chosen.
    branches: Option<(Vec<String>, String, usize)>,
    /// A file compared with its last commit: its name and the rows.
    diff: Option<(String, Vec<git::Row>)>,
    /// The file whose conflicts are being settled side by side.
    merging: Option<Merging>,
    /// The database being looked into.
    database: Option<Browsing>,
    /// Who last changed each line shows after the line of the caret.
    blame: bool,
    blamed: (u64, Vec<git::Blame>),
    note: Option<String>,
    /// The saved versions of the file in front to pick from, and the one chosen.
    versions: Option<(Vec<(String, PathBuf)>, usize)>,
    /// The window had the keyboard at the last frame.
    was_focused: bool,
    /// The colour scheme, by its place in the list.
    scheme: usize,
    /// The file of the font the code is drawn with, when it is not the built-in one.
    font: Option<PathBuf>,
    /// The schemes to pick from are shown, and the one chosen.
    themes: Option<usize>,
    /// Only the code shows: no bars, no panels.
    zen: bool,
    /// What the project can run, what was typed to narrow it, the task chosen.
    tasks: Option<(Vec<Task>, String, usize)>,
    /// The command of the task last run, to run again.
    last_task: Option<String>,
    /// The commits to pick from, what was typed to narrow them, the one chosen.
    log: Option<(Vec<String>, String, usize)>,
    /// The commits listed are drawn as a graph of all the branches.
    log_graph: bool,
    /// The snippets of the user, by language.
    snippets: std::collections::HashMap<String, Vec<(String, String)>>,
    /// What the last HTTP request answered, and the answer still awaited.
    response: String,
    requesting: Option<Receiver<Result<String, String>>>,
    /// The request awaited, as it is listed; the last ones sent with what each answered,
    /// the latest first; and the one of them on show.
    requested: String,
    responses: Vec<(String, String)>,
    response_at: usize,
    /// The program being debugged, and whether it is stopped.
    debug: Option<godcode_lsp::dap::Session>,
    debug_stopped: bool,
    /// Where it is stopped: the calls, the variables of one of them, the file and line.
    frames: Vec<godcode_lsp::dap::Frame>,
    variables: Vec<godcode_lsp::dap::Variable>,
    /// The parts of the variables that were opened, by the reference of each.
    parts: std::collections::HashMap<u64, Vec<godcode_lsp::dap::Variable>>,
    /// The expressions watched, each with what it was last worth; and the one being typed.
    watches: Vec<(String, String)>,
    watch_typed: String,
    debug_at: Option<(PathBuf, usize)>,
    /// What it printed.
    debug_output: String,
    /// The folder served to the browser, pages reloading at each save.
    live: Option<crate::live::Live>,
    /// The page shown inside the window, once one was; whether its panel shows; and the
    /// address it is to show next, taken up when the window can be reached.
    web: Option<crate::webview::Web>,
    web_open: bool,
    web_wanted: Option<String>,
    /// The address of that page, as it shows and as it is typed.
    web_address: String,
    /// The address the page was last seen at, and the file last shown as a page.
    web_shown: String,
    web_file: Option<PathBuf>,
    /// The project templates are shown, and the one chosen.
    templates: Option<usize>,
    /// The shortcuts of the user: they win over the built-in ones.
    keymap: Vec<(Modifiers, Key, Cmd)>,
    /// The second editor is below the first rather than beside it.
    split_down: bool,
    /// The tests of the file in front, and what they were found in.
    tests: (u64, Vec<(usize, String)>),
    /// The tests of the project being run here; what the last run said of each test it
    /// ran; and all it printed.
    test_run: Option<Receiver<String>>,
    /// That run also writes down which lines ran: its report is read when it ends.
    covering: bool,
    /// That run is of one test: what the others were last said to be is kept.
    test_single: bool,
    /// The lines the tests ran, file by file, as the last report read says; empty when
    /// they are not shown.
    coverage: Vec<godcode_core::coverage::FileCoverage>,
    /// The environment whose values fill the requests of `.http` files, by its name; and
    /// the ones to pick from, with the one chosen.
    environment: Option<String>,
    environments: Option<(Vec<String>, usize)>,
    test_results: Vec<(String, bool)>,
    test_output: String,
    /// The tests of the whole project, file by file, once looked for; and the search for
    /// them while it runs.
    project_tests: Vec<FileTests>,
    test_scan: Option<Receiver<Vec<FileTests>>>,
    /// What picking a branch does: switch to it, merge it, rebase onto it.
    branch_action: Cmd,
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
        // The scheme last chosen; before there were schemes, dark or light.
        // A theme of the user that cannot be read is told when its file is next saved.
        let _ = load_themes();
        let named = get("scheme_name").and_then(|name| theme::scheme_index(&name));
        let numbered = get("scheme")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|index| *index < theme::SCHEMES.len());
        let scheme = named.or(numbered).unwrap_or(usize::from(!dark));
        let dark = theme::scheme(scheme).is_none_or(|scheme| scheme.dark);
        theme::install(&cc.egui_ctx, scheme);
        // The font the user chose for the code, when its file is still there and is one.
        let font = get("font")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        let code = font.as_deref().and_then(|path| std::fs::read(path).ok());
        widgets::install_fonts(&cc.egui_ctx, code.filter(|bytes| widgets::is_font(bytes)));
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
            preview: get("preview").is_none_or(|value| value == "1"),
            blocks: (0, Vec::new()),
            preview_starts: Vec::new(),
            top_line: 0.0,
            minimap: get("minimap").is_none_or(|value| value == "1"),
            keys: match get("keys").as_deref() {
                Some("1") => Keys::Vim,
                Some("2") => Keys::Emacs,
                _ => Keys::Default,
            },
            vim: modal::Vim::default(),
            emacs: modal::Emacs::default(),
            hints: get("hints").is_none_or(|value| value == "1"),
            folds: folds_of(&get("folds").unwrap_or_default()),
            sticky: get("sticky").is_none_or(|value| value == "1"),
            autosave: get("autosave").is_some_and(|value| value == "1"),
            format_on_save: get("format_on_save").is_some_and(|value| value == "1"),
            servers: Servers::default(),
            asked: std::collections::HashMap::new(),
            diagnostics: std::collections::HashMap::new(),
            diagnosed: 0,
            documentation: None,
            rest: None,
            pointer: egui::Pos2::ZERO,
            tip: None,
            hovered: None,
            fixes: None,
            signature: None,
            caret_place: ((0, u64::MAX, 0..0), (1, 1), (1, 1)),
            caret_pos: None,
            editor_focused: false,
            project_symbols: Vec::new(),
            symbols_asked: String::new(),
            served: Vec::new(),
            left: 0,
            right: None,
            right_front: false,
            twin: false,
            branches: None,
            diff: None,
            blame: get("blame").is_some_and(|value| value == "1"),
            blamed: (0, Vec::new()),
            note: None,
            versions: None,
            was_focused: false,
            scheme,
            font,
            themes: None,
            zen: false,
            tasks: None,
            merging: None,
            database: None,
            last_task: None,
            log: None,
            log_graph: false,
            snippets: load_snippets(),
            response: String::new(),
            requesting: None,
            requested: String::new(),
            responses: Vec::new(),
            response_at: 0,
            debug: None,
            debug_stopped: false,
            frames: Vec::new(),
            variables: Vec::new(),
            parts: Default::default(),
            watches: get("watches")
                .unwrap_or_default()
                .lines()
                .map(|expression| (expression.to_owned(), String::new()))
                .collect(),
            watch_typed: String::new(),
            debug_at: None,
            debug_output: String::new(),
            live: None,
            web: None,
            web_open: false,
            web_wanted: None,
            web_address: String::new(),
            web_shown: String::new(),
            web_file: None,
            templates: None,
            keymap: load_keymap(),
            split_down: false,
            tests: (0, Vec::new()),
            test_run: None,
            covering: false,
            test_single: false,
            coverage: Vec::new(),
            environment: get("environment").filter(|name| !name.is_empty()),
            environments: None,
            test_results: Vec::new(),
            test_output: String::new(),
            project_tests: Vec::new(),
            test_scan: None,
            branch_action: Cmd::Branches,
        };
        if paths.is_empty() {
            // Nothing was asked for: back to where the last session was left.
            let folder = get("session_folder").map(PathBuf::from);
            if let Some(folder) = folder.filter(|folder| folder.is_dir()) {
                app.open_folder(&cc.egui_ctx, folder);
            }
            let files = get("session_files").unwrap_or_default();
            for file in files.lines().map(Path::new).filter(|file| file.is_file()) {
                app.open_file(&cc.egui_ctx, file);
            }
            let front = get("session_front").map(PathBuf::from);
            let front = front.and_then(|front| {
                let is_front = |tab: &Tab| tab.path.as_deref() == Some(front.as_path());
                app.tabs.iter().position(is_front)
            });
            app.active = front.unwrap_or(app.active);
        }
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
                .error(format!("{name} is too large to edit here (over 64 MB)"));
            return false;
        }
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.toasts.error(format!("Cannot open {name}: {err}"));
                return false;
            }
        };
        if bytes.starts_with(b"SQLite format 3\0") {
            // Not a text: it is looked into, no tab holds it.
            match godcode_core::sqlite::Database::open(bytes) {
                Ok(database) => {
                    let log = PathBuf::from(format!("{}-wal", path.display()));
                    let behind = database.uses_wal()
                        && std::fs::metadata(log).is_ok_and(|log| log.len() > 0);
                    let mut browsing = Browsing {
                        name,
                        database,
                        table: 0,
                        page: 0,
                        count: 0,
                        rows: Vec::new(),
                        error: None,
                        behind,
                    };
                    browsing.read();
                    self.database = Some(browsing);
                }
                Err(err) => self.toasts.error(format!("Cannot read {name}: {err}")),
            }
            return false;
        }
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
        // A big file is plain text: colouring it again and telling a language server about
        // it at every key would make typing wait.
        if self.tabs[self.active].text.len() > editor::LARGE_BYTES {
            self.tabs[self.active].language = Language::plain();
            self.toasts
                .success("Large file: plain text, without colours or language server");
        }
        // The caret starts at the top, not where the text field would put it.
        let tab = &mut self.tabs[self.active];
        // The blocks it had folded the last time are folded again.
        tab.view.folded = self.folds.get(path).cloned().unwrap_or_default();
        editor::set_selection(ctx, tab.editor_id(), &(0..0));
        // A minified file is one endless line running off the view: say what helps.
        if !tab.wrap && format::longest_line(&tab.text) > LONG_LINE {
            let help = if format::restructures(tab.language) {
                "Ctrl+Alt+L reformats it, Alt+Z wraps it"
            } else {
                "Alt+Z wraps them"
            };
            let name = &tab.name;
            self.toasts
                .success(format!("{name} has very long lines. {help}."));
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
        // Forty versions of a big file would fill the disk.
        if bytes.len() <= editor::LARGE_BYTES {
            keep_version(&path, &bytes);
        }
        if let Some(live) = &self.live {
            live.changed();
        }
        if snippets_file().as_deref() == Some(path.as_path()) {
            self.snippets = load_snippets();
        }
        if settings_file("themes.json").as_deref() == Some(path.as_path()) {
            // The one in use is found again by its name, and shows as it is now written.
            let name = theme::current().name;
            match load_themes() {
                Ok(()) => self.set_scheme(ctx, theme::scheme_index(name).unwrap_or(0)),
                Err(err) => self.toasts.error(err),
            }
        }
        if settings_file("keymap.json").as_deref() == Some(path.as_path()) {
            self.keymap = load_keymap();
        }
        let tab = &mut self.tabs[index];
        tab.saved = hash_of(&tab.text);
        tab.dirty = false;
        if tab.path.as_deref() != Some(path.as_path()) {
            tab.set_path(path);
            self.files_changed(ctx);
        }
        if let Some(path) = &self.tabs[index].path {
            self.servers.saved(path);
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
        keep_folds(&mut self.folds, &tab);
        if let Some(path) = &tab.path {
            self.servers.closed(path);
            self.diagnostics.remove(path);
        }
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
        let key = hash_of((tab.id, tab.revision, &find.query, find.options));
        if find.key != Some(key) {
            find.matches = search::find_with(&tab.text, &find.query, find.options);
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
            let find = &self.find;
            let with = search::expand(
                &tab.text[start..end],
                &find.query,
                &find.replace,
                find.options,
            );
            tab.text.replace_range(start..end, &with);
            let caret = tab.selection.start + with.chars().count();
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
            search::replace_all_with(&tab.text, &find.query, &find.replace, find.options);
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
        // A language server already reports the problems of this file as it is typed.
        let served = tab
            .path
            .as_deref()
            .is_some_and(|path| self.servers.serves(path));
        if served && !manual {
            return;
        }
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
        if let Some(receiver) = &self.requesting
            && let Ok(answer) = receiver.try_recv()
        {
            self.requesting = None;
            self.response = answer.unwrap_or_else(|err| format!("The request failed: {err}"));
            let sent = (std::mem::take(&mut self.requested), self.response.clone());
            self.responses.insert(0, sent);
            self.responses.truncate(RESPONSES_KEPT);
            self.response_at = 0;
        }
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
        let key = hash_of((tab.revision, self.reports_version, self.diagnosed));
        if tab.checked == Some(key) {
            return;
        }
        tab.checked = Some(key);
        tab.problems = if tab.text.len() > editor::LARGE_BYTES {
            Vec::new()
        } else {
            check::brackets(&tab.text, tab.language)
        };
        let fresh = tab.reports_at == Some(tab.revision);
        let reported = self
            .reports
            .iter()
            .filter(|(path, _)| fresh && tab.path.as_deref() == Some(path.as_path()))
            .map(|(_, problem)| problem);
        let path = tab.path.as_deref();
        let served = path.and_then(|path| self.diagnostics.get(path));
        tab.squiggles = tab
            .problems
            .iter()
            .chain(reported)
            .chain(served.into_iter().flatten())
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
        let served = self.diagnostics.iter().flat_map(|(path, list)| {
            list.iter()
                .map(move |problem| (Some(path.clone()), problem.clone()))
        });
        live.chain(reported).chain(served).collect()
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
        if tab.language.name == "HTTP" {
            self.send_request(ctx);
            return;
        }
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
        // The language server renames what the name means, in every file; without one,
        // the word is replaced in this file.
        let place = tab.path.clone().map(|path| {
            let (line, column) = servers::position(&tab.text, tab.selection.start);
            (path, line, column)
        });
        self.sync_server(ctx);
        if let Some((path, line, column)) = place
            && let Some(id) = self.servers.rename(&path, line, column, new)
        {
            self.asked.insert(id, Asked::Rename);
            return;
        }
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
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
                    let label = format!("{} {branch}", icon::GIT_BRANCH);
                    let pick = ui.button(label).on_hover_text("Switch or create a branch");
                    if pick.clicked() {
                        self.pending.push(Cmd::Branches);
                    }
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

    /// Tell the language server of the file in front what its text is now.
    fn sync_server(&mut self, ctx: &Context) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let Some(path) = tab.path.as_deref() else {
            return;
        };
        // The project is the open folder, or for a lone file the folder it is in.
        let folder = self.explorer.as_ref().map(Explorer::root);
        let root = folder
            .filter(|root| path.starts_with(root))
            .or(path.parent())
            .unwrap_or(path);
        self.servers
            .sync(ctx, root, path, tab.language.name, &tab.text, tab.revision);
    }

    /// Ask the language server what it writes in the lines of the file in front that are
    /// in view or near: again when its text changes, and when the view leaves those lines.
    fn ask_hints(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active).filter(|_| self.hints) else {
            return;
        };
        // At the end of the text the top of the view is not told: it is far down.
        let top = if self.top_line.is_finite() {
            self.top_line as usize
        } else {
            usize::MAX / 2
        };
        let covered = |(revision, lines, to_end): &(u64, Range<usize>, bool)| {
            let below = *to_end || top + HINTS_IN_VIEW <= lines.end;
            *revision == tab.revision && lines.start <= top && below
        };
        if tab.hints_at.as_ref().is_some_and(covered) {
            return;
        }
        let Some(path) = tab.path.as_deref() else {
            return;
        };
        let asked = self.servers.hints(path, &tab.text, tab.revision, top);
        if let Some((asked, lines, to_end)) = asked {
            tab.hints_at = Some((tab.revision, lines, to_end));
            let (tab, revision) = (tab.id, tab.revision);
            self.asked.insert(asked, Asked::Hints { tab, revision });
        }
    }

    /// Ask the language server what folds in the file in front, again when its text changes.
    fn ask_folds(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let Some(path) = tab.path.as_deref() else {
            return;
        };
        if tab.folds_at == Some(tab.revision) {
            return;
        }
        if let Some(asked) = self.servers.folds(path, tab.revision) {
            tab.folds_at = Some(tab.revision);
            let (tab, revision) = (tab.id, tab.revision);
            self.asked.insert(asked, Asked::Folds { tab, revision });
        }
    }

    /// Take in what the language servers answered.
    fn poll_servers(&mut self, ctx: &Context) {
        for event in self.servers.poll() {
            match event {
                ServerEvent::Diagnostics { path, list } => {
                    let tab = self
                        .tabs
                        .iter()
                        .find(|tab| tab.path.as_deref() == Some(&*path));
                    let problems = list.into_iter().map(|found| Problem {
                        line: found.line,
                        // Counted in chars here, in UTF-16 units there.
                        column: tab.map_or(found.column, |tab| {
                            servers::index_of(&tab.text, found.line, found.column).1
                        }),
                        severity: if found.error {
                            Severity::Error
                        } else {
                            Severity::Warning
                        },
                        message: found.message,
                    });
                    self.diagnostics.insert(path, problems.collect());
                    self.diagnosed += 1;
                }
                ServerEvent::Hover { id, text } => {
                    let asked = self.asked.remove(&id);
                    if let Some(Asked::Tip(start)) = asked {
                        // Shown only if the pointer still rests on that name.
                        let resting = self.rest.is_some_and(|(name, ..)| name == start);
                        if resting && !text.trim().is_empty() {
                            self.tip = Some((start, self.pointer, markdown::blocks(&text)));
                        }
                        continue;
                    }
                    if asked.is_none() {
                        continue;
                    }
                    if text.trim().is_empty() {
                        self.toasts.error("No documentation here");
                    } else {
                        self.documentation = Some(markdown::blocks(&text));
                    }
                }
                ServerEvent::Definition { id, places } => {
                    let Some(Asked::Definition(at)) = self.asked.remove(&id) else {
                        continue;
                    };
                    let Some(place) = places.first() else {
                        self.declaration_in_open_files(ctx, at);
                        continue;
                    };
                    self.record_jump();
                    if self.open_file(ctx, &place.path)
                        && let Some(tab) = self.tabs.get_mut(self.active)
                    {
                        let start = servers::index_of(&tab.text, place.line, place.column).0;
                        tab.select(ctx, start..start);
                    }
                }
                ServerEvent::Completion { id, items } => {
                    let Some(Asked::Completion {
                        tab,
                        revision,
                        caret,
                    }) = self.asked.remove(&id)
                    else {
                        continue;
                    };
                    // Asked for a text that has changed since: the next answer is the one.
                    let current = self.tabs.iter_mut().find(|open| {
                        open.id == tab
                            && open.revision == revision
                            && open.selection == (caret..caret)
                    });
                    let Some(tab) = current else {
                        continue;
                    };
                    let start = complete::word_start(&tab.text, caret);
                    let typed: String = tab.text.chars().skip(start).take(caret - start).collect();
                    let typed = typed.to_lowercase();
                    let offered = items
                        .into_iter()
                        .filter(|item| item.label.to_lowercase().starts_with(&typed))
                        .filter(|item| item.label.len() > typed.len())
                        .take(MAX_SERVED);
                    self.served = offered.collect();
                    let offered = self.served.iter().map(|item| complete::Completion {
                        label: item.label.clone(),
                        body: item.insert.clone(),
                        kind: complete::Kind::Symbol,
                    });
                    tab.view.offer(caret, offered.collect());
                }
                ServerEvent::References { id, places } => {
                    let Some(Asked::References(word)) = self.asked.remove(&id) else {
                        continue;
                    };
                    let mut texts: std::collections::HashMap<PathBuf, String> = Default::default();
                    let mut hits = Vec::with_capacity(places.len());
                    for place in places {
                        let text = texts
                            .entry(place.path.clone())
                            .or_insert_with(|| self.text_of(&place.path));
                        let (start, column) = servers::index_of(text, place.line, place.column);
                        let line = text.lines().nth(place.line).unwrap_or("");
                        // The name found there: not always the one asked about.
                        let named = text.chars().skip(start);
                        let named = named.take_while(|ch| ch.is_alphanumeric() || *ch == '_');
                        let len = match named.count() {
                            0 => word.chars().count(),
                            len => len,
                        };
                        hits.push(Hit {
                            path: place.path,
                            at: Match {
                                start,
                                len,
                                line: place.line,
                                column,
                            },
                            preview: line.trim().chars().take(200).collect(),
                        });
                    }
                    hits.sort_by(|a, b| (&a.path, a.at.start).cmp(&(&b.path, b.at.start)));
                    self.search.stop();
                    self.search.hits = hits;
                    self.search.query = word;
                    self.search.done = true;
                    self.side = Side::Search;
                    self.sidebar = true;
                }
                ServerEvent::Edits { id, files } => {
                    let asked = self.asked.remove(&id);
                    let changed = self.apply_edits(ctx, &files);
                    let plural = if changed == 1 { "" } else { "s" };
                    match asked {
                        Some(Asked::Rename) => self
                            .toasts
                            .success(format!("Renamed in {changed} file{plural}")),
                        Some(Asked::Format) if changed == 0 => {
                            self.toasts.success("Already formatted");
                        }
                        Some(Asked::Format) => self.toasts.success("Reformatted"),
                        _ => {}
                    }
                }
                ServerEvent::Actions { id, actions } => {
                    if self.asked.remove(&id).is_none() {
                        continue;
                    }
                    if actions.is_empty() {
                        self.toasts.error("No quick fix on this line");
                    } else {
                        self.fixes = Some((actions, 0));
                    }
                }
                ServerEvent::Signature { id, label, active } => {
                    if let Some(Asked::Signature(line)) = self.asked.remove(&id) {
                        self.signature = (!label.is_empty()).then_some((line, label, active));
                    }
                }
                ServerEvent::Symbols { id, list } => {
                    let fresh = matches!(
                        self.asked.remove(&id),
                        Some(Asked::Symbols(named)) if named == self.symbols_asked
                    );
                    if fresh {
                        self.project_symbols = list;
                    }
                }
                ServerEvent::Hints { id, list } => {
                    let Some(Asked::Hints { tab, revision }) = self.asked.remove(&id) else {
                        continue;
                    };
                    // Asked for a text that has changed since: the next answer is the one.
                    let mut tabs = self.tabs.iter_mut();
                    if let Some(tab) = tabs.find(|open| open.id == tab && open.revision == revision)
                    {
                        tab.hints = servers::placed(&tab.text, list);
                    }
                }
                ServerEvent::Folds { id, list } => {
                    let Some(Asked::Folds { tab, revision }) = self.asked.remove(&id) else {
                        continue;
                    };
                    let mut tabs = self.tabs.iter_mut();
                    if let Some(tab) = tabs.find(|open| open.id == tab && open.revision == revision)
                    {
                        let lines = list.into_iter().map(|fold| (fold.first, fold.last));
                        tab.folds = Some((revision, lines.collect()));
                    }
                }
                ServerEvent::Exited { .. } => {}
            }
        }
    }

    /// Which tab each editor shows: the tab in front goes to the editor that has the
    /// keyboard, and an editor whose tab was closed goes away.
    fn settle_panes(&mut self) {
        let Some(front) = self.tabs.get(self.active).map(|tab| tab.id) else {
            self.right = None;
            return;
        };
        if self.twin && self.left == front && self.right == Some(front) {
            // One file in both editors: the keyboard stays where it was put.
            return;
        }
        self.twin = false;
        if self.right == Some(front) {
            self.right_front = true;
        } else if self.left == front {
            self.right_front = false;
        } else if self.right_front && self.right.is_some() {
            self.right = Some(front);
        } else {
            self.left = front;
            self.right_front = false;
        }
        let open = |id: u64| self.tabs.iter().any(|tab| tab.id == id);
        if self.right.is_some_and(|id| !open(id) || id == self.left) {
            self.right = None;
            self.right_front = false;
        }
        if !open(self.left) {
            self.left = front;
        }
    }

    /// Take the files changed on disk by something else (a branch switch, another
    /// program) into their editors, unless they have unsaved changes.
    fn reload_from_disk(&mut self, ctx: &Context) {
        let mut reloaded = Vec::new();
        let mut kept = Vec::new();
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let read = tab
                .path
                .as_deref()
                .and_then(|path| std::fs::read(path).ok());
            let Some(decoded) = read.and_then(|bytes| godcode_core::decode(&bytes)) else {
                continue;
            };
            let on_disk = hash_of(&decoded.text);
            if on_disk == tab.saved {
                continue;
            }
            // Either way the file is told about once, not at every return to the window.
            tab.saved = on_disk;
            if tab.dirty {
                kept.push(tab.name.clone());
            } else {
                tab.text = decoded.text;
                tab.touched();
                reloaded.push(index);
            }
        }
        if !kept.is_empty() {
            self.toasts.error(format!(
                "{} changed on disk: your unsaved changes are kept here",
                kept.join(", ")
            ));
        }
        if !reloaded.is_empty() {
            let plural = if reloaded.len() == 1 { "" } else { "s" };
            self.toasts.success(format!(
                "Reloaded {} file{plural} changed on disk",
                reloaded.len()
            ));
        }
        for index in reloaded {
            self.refresh_diff(ctx, index);
        }
    }

    /// Who last changed the line of the caret, when blame is on and the file is as saved.
    fn refresh_blame(&mut self) {
        self.note = None;
        let Some(tab) = self.tabs.get(self.active).filter(|_| self.blame) else {
            return;
        };
        let Some(path) = tab.path.as_deref() else {
            return;
        };
        let key = hash_of((tab.id, tab.saved));
        if self.blamed.0 != key {
            let folder = path.parent().unwrap_or(path);
            let file = path.display().to_string();
            let printed = tools::git(folder, &["blame", "--line-porcelain", "--", &file]);
            self.blamed = (key, git::parse_blame(&printed.unwrap_or_default()));
        }
        // Once edited, the lines no longer are where blame saw them.
        let line = self.blamed.1.get(self.caret_line).filter(|_| !tab.dirty);
        self.note = line.map(|line| format!("{}, {} · {}", line.author, line.date, line.summary));
    }

    /// Show the file in front beside what it was at the last commit.
    fn compare_with_commit(&mut self, ctx: &Context) {
        let index = self.active;
        let dirty = self.tabs.get(index).is_some_and(|tab| tab.dirty);
        // Git compares what is on disk.
        if dirty && !self.save_tab(ctx, index, false) {
            return;
        }
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let Some(path) = tab.path.as_deref() else {
            self.toasts.error("This file is not saved yet");
            return;
        };
        let folder = path.parent().unwrap_or(path);
        let file = path.display().to_string();
        let args = ["diff", "--no-color", "-U1000000", "HEAD", "--", &file];
        match tools::git(folder, &args).map(|printed| git::side_by_side(&printed, false)) {
            Ok(rows) if rows.is_empty() => {
                self.toasts.success("No change since the last commit");
            }
            Ok(rows) => self.diff = Some((format!("{}: last commit  |  now", tab.name), rows)),
            Err(err) => self.toasts.error(err),
        }
    }

    /// The comparison asked for, the old lines on the left and the new ones on the right.
    fn diff_window(&mut self, ctx: &Context) {
        let Some((name, rows)) = &self.diff else {
            return;
        };
        let palette = Palette::of_ctx(ctx);
        let removed = Color32::from_rgba_unmultiplied(0xF1, 0x4C, 0x4C, 46);
        let added = Color32::from_rgba_unmultiplied(0x58, 0xAC, 0x2C, 46);
        let font = FontId::monospace(13.0);
        let mut open = true;
        egui::Window::new(name.as_str())
            .id(Id::new("diff-window"))
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .default_size(vec2(1180.0, 640.0))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let height = 18.0;
                let area = ScrollArea::vertical().auto_shrink(false);
                area.show_rows(ui, height, rows.len(), |ui, range| {
                    for row in &rows[range] {
                        let size = vec2(ui.available_width(), height);
                        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                        let half = rect.width() / 2.0;
                        let changed = row.old.as_ref().map(|line| &line.1)
                            != row.new.as_ref().map(|line| &line.1);
                        let sides = [
                            (&row.old, rect.min.x, removed),
                            (&row.new, rect.min.x + half, added),
                        ];
                        for (side, left, tint) in sides {
                            let cell = Rect::from_min_size(
                                pos2(left, rect.min.y),
                                vec2(half - 4.0, height),
                            );
                            let painter = ui.painter_at(cell);
                            let Some((number, text)) = side else {
                                painter.rect_filled(cell, 0.0, palette.surface);
                                continue;
                            };
                            if changed {
                                painter.rect_filled(cell, 0.0, tint);
                            }
                            let mid = cell.center().y;
                            painter.text(
                                pos2(left + 44.0, mid),
                                Align2::RIGHT_CENTER,
                                // A row numbered 0 is the name of a file.
                                if *number > 0 {
                                    number.to_string()
                                } else {
                                    String::new()
                                },
                                font.clone(),
                                palette.text_dim,
                            );
                            painter.text(
                                pos2(left + 56.0, mid),
                                Align2::LEFT_CENTER,
                                text,
                                font.clone(),
                                palette.text,
                            );
                        }
                    }
                });
            });
        if !open || ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.diff = None;
        }
    }

    /// The branches of the folder: pick one to switch to it, type a new name to create it.
    fn branches_dialog(&mut self, ctx: &Context) {
        let Some((all, mut typed, mut selected)) = self.branches.take() else {
            return;
        };
        let wanted = typed.trim().to_owned();
        let narrowed = all
            .iter()
            .filter(|branch| branch.to_lowercase().contains(&wanted.to_lowercase()));
        let mut rows: Vec<String> = narrowed.cloned().collect();
        let action = self.branch_action;
        let new = action == Cmd::Branches && !wanted.is_empty() && !all.contains(&wanted);
        if new {
            rows.push(format!("Create branch {wanted}"));
        }
        let Some(answer) = list_dialog(ctx, "Branches", &rows, &mut selected, Some(&mut typed))
        else {
            self.branches = Some((all, typed, selected));
            return;
        };
        let (Some(row), Some(root)) = (answer, self.folder()) else {
            return;
        };
        let create = new && row + 1 == rows.len();
        let name = if create { wanted } else { rows[row].clone() };
        let (done, said) = match action {
            Cmd::Merge => (
                tools::git(&root, &["merge", &name]),
                format!("Merged {name}"),
            ),
            Cmd::Rebase => (
                tools::git(&root, &["rebase", &name]),
                format!("Rebased onto {name}"),
            ),
            _ if create => (
                tools::git(&root, &["switch", "-c", &name]),
                format!("On new branch {name}"),
            ),
            _ => (
                tools::git(&root, &["switch", &name]),
                format!("On branch {name}"),
            ),
        };
        match done {
            Ok(_) => self.toasts.success(said),
            // A merge that stops on conflicts leaves its markers in the files.
            Err(err) => self.toasts.error(format!(
                "{err}. Conflicts are marked in the files: settle them from the Git menu."
            )),
        }
        self.reload_from_disk(ctx);
        self.files_changed(ctx);
    }

    /// The versions kept of the file in front: pick one to bring its text back.
    fn versions_dialog(&mut self, ctx: &Context) {
        let Some((versions, mut selected)) = self.versions.take() else {
            return;
        };
        let rows: Vec<String> = versions.iter().map(|(label, _)| label.clone()).collect();
        let Some(answer) = list_dialog(ctx, "Local history", &rows, &mut selected, None) else {
            self.versions = Some((versions, selected));
            return;
        };
        let Some((label, file)) = answer.and_then(|row| versions.get(row)) else {
            return;
        };
        let read = std::fs::read(file).ok();
        let decoded = read.and_then(|bytes| godcode_core::decode(&bytes));
        match (decoded, self.tabs.get_mut(self.active)) {
            (Some(decoded), Some(tab)) => {
                tab.text = decoded.text;
                tab.touched();
                tab.select(ctx, 0..0);
                // Not saved: Ctrl+Z, or closing without saving, takes it back.
                self.toasts.success(format!("Text of {label} brought back"));
            }
            _ => self.toasts.error("This version cannot be read any more"),
        }
    }

    /// The signature of the call being written, above the caret.
    fn signature_window(&self, ctx: &Context) {
        let (Some((_, label, active)), Some(caret)) = (&self.signature, self.caret_pos) else {
            return;
        };
        let palette = Palette::of_ctx(ctx);
        let font = FontId::monospace(self.font_size() * 0.93);
        let mut job = LayoutJob::default();
        let chars: Vec<char> = label.chars().collect();
        let (from, to) = active.unwrap_or((0, 0));
        let (from, to) = (
            from.min(chars.len()),
            to.min(chars.len()).max(from.min(chars.len())),
        );
        let parts = [(0..from, false), (from..to, true), (to..chars.len(), false)];
        for (range, typed) in parts {
            let text: String = chars[range].iter().collect();
            let color = if typed {
                palette.text
            } else {
                palette.text_dim
            };
            let mut format = egui::TextFormat::simple(font.clone(), color);
            if typed {
                format.underline = Stroke::new(1.0, palette.text);
            }
            job.append(&text, 0.0, format);
        }
        egui::Area::new(Id::new("signature"))
            .order(egui::Order::Tooltip)
            .pivot(Align2::LEFT_BOTTOM)
            .fixed_pos(caret - vec2(0.0, self.font_size() * 1.6))
            .show(ctx, |ui| {
                Frame::popup(ui.style())
                    .inner_margin(Margin::symmetric(8, 4))
                    .show(ui, |ui| ui.label(job));
            });
    }

    /// Set or remove the breakpoint of a line of tab `index`, and tell the debugger.
    fn toggle_breakpoint(&mut self, index: usize, line: usize) {
        if let Some(tab) = self.tabs.get_mut(index) {
            toggle_breakpoint_of(tab, &mut self.debug, line);
        }
    }

    /// Have the debugger stop on `line` of the file in front only when `condition` holds;
    /// always, when it is empty. The line gets a breakpoint if it had none.
    fn set_condition(&mut self, line: usize, condition: &str) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if !tab.breakpoints.contains(&line) {
            tab.breakpoints.push(line);
        }
        match condition {
            "" => tab.conditions.remove(&line),
            condition => tab.conditions.insert(line, condition.to_owned()),
        };
        if let (Some(session), Some(path)) = (&mut self.debug, &tab.path) {
            session.set_breakpoints(path, &stops_of(tab));
        }
    }

    /// The program runs again: nothing to show of where it was.
    fn debug_running(&mut self) {
        self.debug_stopped = false;
        self.debug_at = None;
        self.frames.clear();
        self.variables.clear();
        self.parts.clear();
        // Running, nothing can be said of what the expressions are worth.
        self.watches.iter_mut().for_each(|(_, worth)| worth.clear());
    }

    /// Run the file in front under the debugger of its language.
    fn start_debug(&mut self, ctx: &Context) {
        let index = self.active;
        let unsaved = self
            .tabs
            .get(index)
            .is_some_and(|tab| tab.dirty || tab.path.is_none());
        if unsaved && !self.save_tab(ctx, index, false) {
            return;
        }
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let Some(path) = tab.path.clone() else {
            return;
        };
        let project = self.folder().filter(|root| path.starts_with(root));
        let folder = project
            .or_else(|| path.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        let language = tab.language.name;
        // The settings of the user for this language, else the built-in ones.
        let configured = debug_settings(language, &path, &folder);
        let (adapter, launch, install) = match (configured, language) {
            (Some((adapter, launch)), _) => (
                adapter,
                launch,
                "check the adapter named in the debug configurations",
            ),
            (None, "Python") => (
                ["python", "-m", "debugpy.adapter"]
                    .map(str::to_owned)
                    .to_vec(),
                serde_json::json!({
                    "request": "launch",
                    "program": path,
                    "cwd": folder,
                    "console": "internalConsole",
                    "justMyCode": true,
                }),
                "pip install debugpy",
            ),
            (None, other) => {
                self.toasts.error(format!(
                    "No debugger is set for {other}: add one with \"Edit Debug Configurations\""
                ));
                return;
            }
        };
        let Some((program, args)) = adapter.split_first() else {
            return;
        };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let breakpoints = self.tabs.iter().filter_map(|tab| {
            Some((tab.path.clone()?, stops_of(tab))).filter(|(_, stops)| !stops.is_empty())
        });
        let wake = ctx.clone();
        let wake = move || wake.request_repaint();
        match godcode_lsp::dap::Session::start(program, &args, launch, breakpoints.collect(), wake)
        {
            Ok(session) => {
                self.debug = Some(session);
                self.debug_running();
                self.debug_output.clear();
                self.bottom = Some(Bottom::Debug);
            }
            Err(err) => self.toasts.error(format!(
                "Cannot start the debugger ({err}). To get it: {install}"
            )),
        }
    }

    /// Take in what the debugger said.
    fn poll_debug(&mut self, ctx: &Context) {
        use godcode_lsp::dap::Event;
        let Some(session) = &mut self.debug else {
            return;
        };
        for event in session.poll() {
            match event {
                Event::Stopped { .. } => {
                    self.debug_stopped = true;
                    self.bottom = Some(Bottom::Debug);
                }
                Event::Running => self.debug_running(),
                Event::Stack(frames) => {
                    // The innermost call of a file that can be shown is where the eye goes.
                    let shown = frames
                        .iter()
                        .find_map(|frame| Some((frame.path.clone()?, frame.line)));
                    self.frames = frames;
                    if let Some((path, line)) = shown {
                        if self.open_file(ctx, &path)
                            && let Some(tab) = self.tabs.get_mut(self.active)
                        {
                            let start = godcode_core::line_start(&tab.text, line + 1);
                            tab.select(ctx, start..start);
                        }
                        self.debug_at = Some((path, line));
                    }
                }
                Event::Children { reference, list } => {
                    self.parts.insert(reference, list);
                }
                Event::Variables(variables) => {
                    self.variables = variables;
                    // Those of another call: what was opened is not theirs.
                    self.parts.clear();
                    // The call looked at changed: so may what the expressions are worth.
                    let watched: Vec<String> = self
                        .watches
                        .iter()
                        .map(|(expression, _)| expression.clone())
                        .collect();
                    if let Some(session) = &mut self.debug {
                        session.watch(&watched);
                    }
                }
                Event::Watched {
                    expression,
                    value,
                    failed,
                } => {
                    // The name the pointer rests on: what it is worth shows next to it.
                    let resting = self.hovered.as_ref().filter(|(start, name)| {
                        *name == expression && self.rest.is_some_and(|(at, ..)| at == *start)
                    });
                    if let (Some((start, _)), false) = (resting, failed) {
                        let said = format!("`{expression}` = `{value}`");
                        self.tip = Some((*start, self.pointer, markdown::blocks(&said)));
                    }
                    let same = self
                        .watches
                        .iter_mut()
                        .filter(|(watched, _)| *watched == expression);
                    same.for_each(|(_, worth)| worth.clone_from(&value));
                }
                Event::Output(printed) => self.debug_output.push_str(&printed),
                Event::Ended => {
                    // Having said nothing at all, it never started.
                    if self.debug_output.is_empty() && self.frames.is_empty() {
                        self.debug_output
                            .push_str("The debugger ended at once. Is debugpy installed? pip install debugpy\n");
                    }
                    self.debug = None;
                    self.debug_running();
                    self.debug_output.push_str("\n[ended]\n");
                    return;
                }
                Event::Failed(why) => {
                    self.toasts.error(format!("The debugger refused: {why}"));
                    self.debug = None;
                    self.debug_running();
                    return;
                }
            }
        }
    }

    /// The debugger: the calls, the variables of the one chosen, what the program printed.
    fn debug_panel(&mut self, ui: &mut Ui) {
        let palette = Palette::of(ui);
        let state = match (&self.debug, self.debug_stopped) {
            (None, _) => {
                "No program is being debugged. Ctrl+F5 debugs the file in front; \
                          click a line number to stop there."
            }
            (Some(_), true) => "Stopped.",
            (Some(_), false) => "Running…",
        };
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(RichText::new(state).size(12.0).color(palette.text_dim));
        });
        ui.add_space(4.0);
        let mut inspect = None;
        // An expression to watch from now on, and one to watch no more.
        let (mut watch, mut unwatch) = (None, None);
        // A variable to open or to close; one to give another value.
        let mut toggle = None;
        let mut change = None;
        ui.columns(3, |columns| {
            let titled = |ui: &mut Ui, title: &str| {
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(title)
                            .size(11.0)
                            .strong()
                            .color(palette.text_dim),
                    );
                });
            };
            titled(&mut columns[0], "CALL STACK");
            ScrollArea::vertical()
                .id_salt("debug-stack")
                .auto_shrink(false)
                .show(&mut columns[0], |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for frame in &self.frames {
                        let file = frame.path.as_deref().map(file_name).unwrap_or_default();
                        let label = format!("{}    {file}:{}", frame.name, frame.line + 1);
                        if list_row(ui, &palette, &label, 10.0, palette.text).clicked() {
                            inspect = Some(frame.clone());
                        }
                    }
                });
            titled(&mut columns[1], "WATCH");
            columns[1].horizontal(|ui| {
                ui.add_space(10.0);
                let field = TextEdit::singleline(&mut self.watch_typed)
                    .hint_text("An expression, then Enter")
                    .desired_width(ui.available_width() - 10.0);
                let typed = ui.add(field);
                let entered = ui.input(|input| input.key_pressed(Key::Enter));
                let expression = self.watch_typed.trim();
                if typed.lost_focus() && entered && !expression.is_empty() {
                    watch = Some(expression.to_owned());
                    self.watch_typed.clear();
                }
            });
            for (index, (expression, worth)) in self.watches.iter().enumerate() {
                let worth = if worth.is_empty() { "…" } else { worth };
                let label = format!("{expression} = {worth}");
                let row = list_row(&mut columns[1], &palette, &label, 10.0, palette.text);
                if row.on_hover_text("Click to stop watching it").clicked() {
                    unwatch = Some(index);
                }
            }
            columns[1].add_space(6.0);
            titled(&mut columns[1], "VARIABLES");
            ScrollArea::vertical()
                .id_salt("debug-variables")
                .auto_shrink(false)
                .show(&mut columns[1], |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    // Each with its parts under it once opened, a level further in.
                    let mut rows: Vec<(usize, Option<u64>, &godcode_lsp::dap::Variable)> = self
                        .variables
                        .iter()
                        .rev()
                        .map(|found| (0, None, found))
                        .collect();
                    while let Some((depth, inside, found)) = rows.pop() {
                        let opened = self
                            .parts
                            .get(&found.reference)
                            .filter(|_| found.reference != 0);
                        let arrow = match (found.reference, opened) {
                            (0, _) => "   ",
                            (_, Some(_)) => "▾ ",
                            (_, None) => "▸ ",
                        };
                        let label = format!("{arrow}{} = {}", found.name, found.value);
                        let indent = 10.0 + depth as f32 * 14.0;
                        let row = list_row(ui, &palette, &label, indent, palette.text);
                        let row = row.on_hover_text(format!(
                            "{}\nRight click to give it another value",
                            found.value
                        ));
                        if row.clicked() && found.reference != 0 {
                            toggle = Some(found.reference);
                        }
                        if row.secondary_clicked() {
                            change = Some((inside, found.name.clone(), found.value.clone()));
                        }
                        let parts = opened.into_iter().flatten().rev();
                        rows.extend(parts.map(|part| (depth + 1, Some(found.reference), part)));
                    }
                });
            titled(&mut columns[2], "OUTPUT");
            ScrollArea::vertical()
                .id_salt("debug-output")
                .auto_shrink(false)
                .stick_to_bottom(true)
                .show(&mut columns[2], |ui| {
                    let text = RichText::new(&self.debug_output).monospace().size(12.5);
                    ui.add(egui::Label::new(text).selectable(true));
                });
        });
        if let Some(index) = unwatch {
            self.watches.remove(index);
        }
        if let Some((inside, name, value)) = change {
            self.prompt = Some(Prompt {
                kind: PromptKind::Value(inside, name),
                name: value,
                focus: true,
            });
        }
        if let Some(reference) = toggle {
            // Closed, its parts are forgotten; opened, they are asked for.
            if self.parts.remove(&reference).is_none()
                && let Some(session) = &mut self.debug
            {
                session.expand(reference);
            }
        }
        if let Some(expression) = watch {
            // Stopped, it is worked out at once; running, at the next stop.
            if let (Some(session), true) = (&mut self.debug, self.debug_stopped) {
                session.watch(std::slice::from_ref(&expression));
            }
            self.watches.push((expression, String::new()));
        }
        // A call of the stack is looked at: its line shows, its variables are asked for.
        let Some(frame) = inspect else {
            return;
        };
        if let Some(session) = &mut self.debug {
            session.inspect(frame.id);
        }
        if let Some(path) = frame.path {
            let ctx = ui.ctx().clone();
            if self.open_file(&ctx, &path)
                && let Some(tab) = self.tabs.get_mut(self.active)
            {
                let start = godcode_core::line_start(&tab.text, frame.line + 1);
                tab.select(&ctx, start..start);
            }
            self.debug_at = Some((path, frame.line));
        }
    }

    /// Open a settings file of the user, written with `example` the first time.
    fn open_settings(&mut self, ctx: &Context, name: &str, example: &str) {
        let Some(file) = settings_file(name) else {
            self.toasts
                .error("No place to keep settings on this system");
            return;
        };
        if !file.exists() {
            let folder = file.parent().unwrap_or(&file);
            let _ = std::fs::create_dir_all(folder);
            let _ = std::fs::write(&file, example);
        }
        self.open_file(ctx, &file);
    }

    /// Run the tests of the project here, out of sight, to read what passed.
    fn start_tests(&mut self, ctx: &Context, covering: bool) {
        if self.test_run.is_some() {
            return;
        }
        let known = |root: &Path| match covering {
            true => tools::coverage_command(root).map(str::to_owned),
            false => tools::test_command(root).map(tools::with_results),
        };
        let command = self.run_root().and_then(|root| Some((known(&root)?, root)));
        let Some((command, root)) = command else {
            self.toasts.error(match covering {
                true => "No way is known to tell which lines the tests of this project run",
                false => "No tests are known for this project",
            });
            return;
        };
        self.covering = covering;
        self.test_results.clear();
        self.capture_tests(ctx, root, command);
    }

    /// Run one test of the file in front here, and show whether it passed.
    fn start_test(&mut self, ctx: &Context, name: &str) {
        let command = self
            .tabs
            .get(self.active)
            .and_then(|tab| tools::test_one(tab.language.name, tab.path.as_deref()?, name));
        let (Some(command), Some(root), None) = (command, self.run_root(), &self.test_run) else {
            return;
        };
        self.test_single = true;
        self.capture_tests(ctx, root, tools::with_results(&command));
    }

    /// Run `command`, which runs tests, in `root`: what it prints is read when it ends.
    fn capture_tests(&mut self, ctx: &Context, root: PathBuf, command: String) {
        self.test_output = format!("$ {command}\n");
        let (sender, receiver) = channel();
        let waking = ctx.clone();
        std::thread::spawn(move || {
            // The window may be gone when the tests end: there is then no one to tell.
            let _ = sender.send(tools::run_captured(&root, &command));
            waking.request_repaint();
        });
        self.test_run = Some(receiver);
        self.bottom = Some(Bottom::Tests);
    }

    /// Read the report the tests left of the lines they ran: they show in the margin.
    fn load_coverage(&mut self) {
        let read = self.run_root().and_then(|root| {
            let mut reports = tools::COVERAGE_REPORTS.iter();
            reports.find_map(|name| std::fs::read_to_string(root.join(name)).ok())
        });
        self.coverage = read.map_or_else(Vec::new, |report| godcode_core::coverage::read(&report));
        if self.coverage.is_empty() {
            self.toasts.error(
                "No coverage report found (lcov.info, coverage/lcov.info, coverage.out): \
                 Run Tests with Coverage writes one",
            );
        } else {
            let percent = godcode_core::coverage::total_percent(&self.coverage);
            self.toasts
                .success(format!("{percent:.0}% of the lines ran in the tests"));
        }
    }

    /// The environments written for the requests of the file in front: in
    /// `http-client.env.json` beside it, or else at the root of the folder.
    fn http_environments(&self) -> Vec<godcode_core::http::Environment> {
        let front = self.tabs.get(self.active);
        let beside = front.and_then(|tab| Some(tab.path.as_deref()?.parent()?.to_path_buf()));
        let folders = beside.into_iter().chain(self.folder());
        let mut written = folders
            .filter_map(|folder| std::fs::read_to_string(folder.join("http-client.env.json")).ok());
        let read = |json: String| godcode_core::http::environments(&json);
        written.next().map_or_else(Vec::new, read)
    }

    /// The tables of a database and the rows of one of them, a page at a time.
    fn database_window(&mut self, ctx: &Context) {
        let Some(mut browsing) = self.database.take() else {
            return;
        };
        let palette = Palette::of_ctx(ctx);
        let (mut table, mut page) = (browsing.table, browsing.page);
        let mut open = true;
        egui::Window::new(format!("Database: {}", browsing.name))
            .id(Id::new("database-window"))
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .default_size(vec2(1180.0, 640.0))
            .show(ctx, |ui| {
                if browsing.behind {
                    let note = "Changes still in its write-ahead log (-wal file) do not show here";
                    ui.colored_label(palette.text_dim, note);
                }
                let tables = browsing.database.tables();
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(200.0);
                        let list = ScrollArea::vertical().id_salt("database-tables");
                        list.auto_shrink(false).show(ui, |ui| {
                            if tables.is_empty() {
                                ui.colored_label(palette.text_dim, "No table");
                            }
                            for (index, found) in tables.iter().enumerate() {
                                let row = ui.selectable_label(index == table, &found.name);
                                if row.on_hover_text(&found.sql).clicked() {
                                    (table, page) = (index, 0);
                                }
                            }
                        });
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        let pages = browsing.count.div_ceil(DATABASE_PAGE).max(1);
                        ui.horizontal(|ui| {
                            ui.label(format!("{} rows", browsing.count));
                            let earlier = Button::new("Previous");
                            if ui.add_enabled(page > 0, earlier).clicked() {
                                page -= 1;
                            }
                            ui.label(format!("{} / {pages}", page + 1));
                            if ui
                                .add_enabled(page + 1 < pages, Button::new("Next"))
                                .clicked()
                            {
                                page += 1;
                            }
                        });
                        if let Some(error) = &browsing.error {
                            ui.colored_label(palette.error, error);
                        }
                        let columns = tables.get(browsing.table).map(|found| &found.columns);
                        let grid = ScrollArea::both().id_salt("database-rows");
                        grid.auto_shrink(false).show(ui, |ui| {
                            egui::Grid::new("database-grid")
                                .striped(true)
                                .show(ui, |ui| {
                                    for column in columns.into_iter().flatten() {
                                        ui.strong(column);
                                    }
                                    ui.end_row();
                                    for row in &browsing.rows {
                                        for value in row {
                                            // One line of it, and not all of a long one.
                                            let written =
                                                value.to_string().replace(['\n', '\r'], " ");
                                            let mut shown: String =
                                                written.chars().take(DATABASE_CELL).collect();
                                            if shown.len() < written.len() {
                                                shown.push('…');
                                            }
                                            ui.label(shown);
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                    });
                });
            });
        if (table, page) != (browsing.table, browsing.page) {
            (browsing.table, browsing.page) = (table, page);
            browsing.read();
        }
        if open {
            self.database = Some(browsing);
        }
    }

    /// The conflicts of a file in three columns: mine, the result, theirs. Each conflict
    /// is settled by a button on its first row; `Apply` writes the result in the file.
    fn merge_window(&mut self, ctx: &Context) {
        use godcode_core::merge::{self, Choice, Kind};
        let Some(mut merging) = self.merging.take() else {
            return;
        };
        let rows = merge::rows(&merging.parts, &merging.choices);
        let left = merge::unresolved(&merging.parts, &merging.choices);
        let palette = Palette::of_ctx(ctx);
        let mine = Color32::from_rgba_unmultiplied(0x3B, 0x82, 0xF6, 44);
        let theirs = Color32::from_rgba_unmultiplied(0x58, 0xAC, 0x2C, 46);
        let unsettled = Color32::from_rgba_unmultiplied(0xF1, 0x4C, 0x4C, 46);
        let font = FontId::monospace(13.0);
        let (mut open, mut apply) = (true, false);
        let mut chosen = None;
        let mut all = None;
        egui::Window::new("Merge: mine  |  result  |  theirs")
            .id(Id::new("merge-window"))
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .default_size(vec2(1280.0, 660.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(match left {
                        0 => "Every conflict is settled".to_owned(),
                        1 => "1 conflict left".to_owned(),
                        left => format!("{left} conflicts left"),
                    });
                    if ui.button("All mine").clicked() {
                        all = Some(Choice::Ours);
                    }
                    if ui.button("All theirs").clicked() {
                        all = Some(Choice::Theirs);
                    }
                    let tip =
                        "Write the result in the file; a conflict left open keeps its markers";
                    apply = ui.button("Apply").on_hover_text(tip).clicked();
                });
                ui.separator();
                ui.spacing_mut().item_spacing.y = 0.0;
                let height = 20.0;
                let area = ScrollArea::vertical().auto_shrink(false);
                area.show_rows(ui, height, rows.len(), |ui, range| {
                    for index in range {
                        let row = &rows[index];
                        let size = vec2(ui.available_width(), height);
                        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                        let third = rect.width() / 3.0;
                        let settled = |conflict: usize| {
                            let choice = merging.choices.get(conflict);
                            choice.copied().flatten().is_some()
                        };
                        let waiting = row.conflict.is_some_and(|conflict| !settled(conflict));
                        let tints = match row.kind {
                            Kind::Same => [None; 3],
                            Kind::Ours => [Some(mine), Some(mine), None],
                            Kind::Theirs => [None, Some(theirs), Some(theirs)],
                            Kind::Conflict if waiting => [Some(unsettled); 3],
                            Kind::Both | Kind::Conflict => [Some(mine), None, Some(theirs)],
                        };
                        let texts = [&row.ours, &row.result, &row.theirs];
                        for (column, (text, tint)) in texts.into_iter().zip(tints).enumerate() {
                            let cell = Rect::from_min_size(
                                pos2(rect.min.x + third * column as f32, rect.min.y),
                                vec2(third - 4.0, height),
                            );
                            let painter = ui.painter_at(cell);
                            match (text, tint) {
                                // A side that has no line here.
                                (None, _) => {
                                    painter.rect_filled(cell, 0.0, palette.surface);
                                }
                                (Some(_), Some(tint)) => {
                                    painter.rect_filled(cell, 0.0, tint);
                                }
                                (Some(_), None) => {}
                            }
                            if let Some(text) = text {
                                painter.text(
                                    pos2(cell.min.x + 8.0, cell.center().y),
                                    Align2::LEFT_CENTER,
                                    text,
                                    font.clone(),
                                    palette.text,
                                );
                            }
                        }
                        // The first row of a conflict holds what settles it.
                        let starts = |conflict: &usize| {
                            index == 0 || rows[index - 1].conflict != Some(*conflict)
                        };
                        if let Some(conflict) = row.conflict.filter(starts) {
                            let offered = [
                                (1.0, "Mine", Choice::Ours),
                                (2.0, "Both", Choice::OursThenTheirs),
                                (3.0, "Theirs", Choice::Theirs),
                            ];
                            for (column, label, choice) in offered {
                                let size = vec2(52.0, height - 2.0);
                                let corner = pos2(
                                    rect.min.x + third * column - size.x - 10.0,
                                    rect.min.y + 1.0,
                                );
                                let button = Button::new(RichText::new(label).size(11.0));
                                if ui.put(Rect::from_min_size(corner, size), button).clicked() {
                                    chosen = Some((conflict, choice));
                                }
                            }
                        }
                    }
                });
            });
        if let Some((conflict, choice)) = chosen
            && let Some(slot) = merging.choices.get_mut(conflict)
        {
            *slot = Some(choice);
        }
        if let Some(choice) = all {
            merging.choices.fill(Some(choice));
        }
        if apply {
            let text = merge::resolved(&merging.parts, &merging.choices);
            match self.tabs.iter_mut().find(|tab| tab.id == merging.tab) {
                Some(tab) => {
                    tab.text = text;
                    tab.touched();
                }
                None => self
                    .toasts
                    .error("The file was closed: nothing was written"),
            }
        } else if open {
            self.merging = Some(merging);
        }
    }

    /// The environments to fill the requests with: pick one, or none.
    fn environments_dialog(&mut self, ctx: &Context) {
        let Some((rows, mut selected)) = self.environments.take() else {
            return;
        };
        match list_dialog(ctx, "HTTP environment", &rows, &mut selected, None) {
            None => self.environments = Some((rows, selected)),
            // The first row is for none of them.
            Some(Some(row)) => self.environment = rows.get(row).filter(|_| row > 0).cloned(),
            Some(None) => {}
        }
    }

    /// Look for the tests of every file of the project, out of the way.
    fn scan_tests(&mut self, ctx: &Context) {
        let Some(root) = self.folder().filter(|_| self.test_scan.is_none()) else {
            return;
        };
        let listed = self.index.clone();
        let (sender, receiver) = channel();
        let waking = ctx.clone();
        std::thread::spawn(move || {
            // The folder may have been closed meanwhile: nobody listens then.
            let _ = sender.send(tests_under(&root, &listed));
            waking.request_repaint();
        });
        self.test_scan = Some(receiver);
    }

    /// Take in what the tests run here printed, once they ended.
    fn poll_tests(&mut self) {
        if let Some(scan) = &self.test_scan {
            match scan.try_recv() {
                Ok(found) => {
                    self.project_tests = found;
                    self.test_scan = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.test_scan = None,
            }
        }
        let Some(run) = &self.test_run else {
            return;
        };
        match run.try_recv() {
            Ok(printed) => {
                let read = outline::results(&printed);
                let failed = read.iter().filter(|(_, passed)| !passed).count();
                let passed = read.len() - failed;
                if std::mem::take(&mut self.test_single) {
                    // What was said before of the tests it ran is out of date.
                    let ran_again = |name: &String| read.iter().any(|(ran, _)| ran == name);
                    self.test_results.retain(|(name, _)| !ran_again(name));
                    self.test_results.extend(read);
                } else {
                    self.test_results = read;
                }
                self.test_output.push_str(&printed);
                self.test_run = None;
                if std::mem::take(&mut self.covering) {
                    self.load_coverage();
                }
                match (passed, failed) {
                    (0, 0) => self
                        .toasts
                        .error("The tests ran, but none could be read: see what was printed"),
                    (passed, 0) => self.toasts.success(format!("{passed} tests passed")),
                    (passed, failed) => self
                        .toasts
                        .error(format!("{failed} tests failed, {passed} passed")),
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.test_run = None,
        }
    }

    /// The tests: those of the project that failed in the last run made here, those of the
    /// file in front (a click runs one in the terminal), and what that run printed.
    fn tests_list(&mut self, ui: &mut Ui) {
        const PASSED: Color32 = Color32::from_rgb(0x3F, 0xB9, 0x50);
        const FAILED: Color32 = Color32::from_rgb(0xE5, 0x53, 0x4B);
        let palette = Palette::of(ui);
        let running = self.test_run.is_some();
        let failed = self
            .test_results
            .iter()
            .filter(|(_, passed)| !passed)
            .count();
        let passed = self.test_results.len() - failed;
        let mut run_all = false;
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            let all = Button::new(with_icon(icon::PLAY, "Run all"));
            let tip = "Run the tests of the project here and show what passed (Ctrl+Alt+T)";
            run_all = ui.add_enabled(!running, all).on_hover_text(tip).clicked();
            if running {
                ui.spinner();
                ui.label(RichText::new("Running…").color(palette.text_dim));
            } else if !self.test_results.is_empty() {
                let ink = if failed > 0 { FAILED } else { PASSED };
                ui.label(RichText::new(format!("{passed} passed, {failed} failed")).color(ink));
            }
        });
        ui.add_space(4.0);
        let mut run = None;
        // The tests of the project are to be looked for; one of them is to be shown.
        let (mut scan, mut go_to) = (false, None);
        // What the last run said of a test: the last word, when it ran more than once.
        let verdict = |name: &str| {
            let said = self.test_results.iter().rev().find(|(ran, _)| ran == name);
            said.map(|(_, passed)| *passed)
        };
        let marked = |ui: &Ui, row: &Response, glyph: &str, ink: Color32| {
            ui.painter().text(
                pos2(row.rect.min.x + 10.0, row.rect.center().y),
                Align2::LEFT_CENTER,
                glyph,
                FontId::proportional(13.0),
                ink,
            );
        };
        ui.columns(2, |columns| {
            ScrollArea::vertical()
                .id_salt("tests-list")
                .auto_shrink(false)
                .show(&mut columns[0], |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    // Wherever they are in the project, the failed ones come first.
                    for (name, _) in self.test_results.iter().filter(|(_, passed)| !passed) {
                        let row = list_row(ui, &palette, name, 30.0, FAILED);
                        marked(ui, &row, icon::X_CIRCLE, FAILED);
                    }
                    for (line, name) in &self.tests.1 {
                        let label = format!("{name}    line {}", line + 1);
                        let row = list_row(ui, &palette, &label, 30.0, palette.text);
                        match verdict(name) {
                            Some(true) => marked(ui, &row, icon::CHECK_CIRCLE, PASSED),
                            Some(false) => marked(ui, &row, icon::X_CIRCLE, FAILED),
                            None => marked(ui, &row, icon::PLAY, palette.text_dim),
                        }
                        if row.on_hover_text("Run this test and show whether it passes").clicked() {
                            run = Some(name.clone());
                        }
                    }
                    if self.tests.1.is_empty() && self.test_results.is_empty() {
                        let none =
                            "No test found in this file (Rust, Python, Go, JavaScript, TypeScript).";
                        ui.label(RichText::new(none).color(palette.text_dim));
                    }
                    // Every test of the project, file by file: a click goes to it.
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.add_space(10.0);
                        let count: usize = self.project_tests.iter().map(|file| file.1.len()).sum();
                        let title = match (self.test_scan.is_some(), count) {
                            (true, _) => "PROJECT: looking…".to_owned(),
                            (false, 0) => "PROJECT".to_owned(),
                            (false, count) => format!("PROJECT: {count} tests"),
                        };
                        ui.label(RichText::new(title).size(11.0).strong().color(palette.text_dim));
                        let look = Button::new("Find all").frame_when_inactive(false);
                        let tip = "Look for the tests of every file of the folder";
                        scan = ui.add(look).on_hover_text(tip).clicked();
                    });
                    for (file, tests) in &self.project_tests {
                        list_row(ui, &palette, file, 10.0, palette.text_dim);
                        for (line, name) in tests {
                            let row = list_row(ui, &palette, name, 44.0, palette.text);
                            // Told by the last run made here, when it ran this one.
                            let said = verdict(name).map(|passed| match passed {
                                true => (icon::CHECK_CIRCLE, PASSED),
                                false => (icon::X_CIRCLE, FAILED),
                            });
                            if let Some((glyph, ink)) = said {
                                ui.painter().text(
                                    pos2(row.rect.min.x + 24.0, row.rect.center().y),
                                    Align2::LEFT_CENTER,
                                    glyph,
                                    FontId::proportional(13.0),
                                    ink,
                                );
                            }
                            if row.on_hover_text("Go to this test").clicked() {
                                go_to = Some((file.clone(), *line));
                            }
                        }
                    }
                });
            ScrollArea::both()
                .id_salt("tests-output")
                .auto_shrink(false)
                .stick_to_bottom(true)
                .show(&mut columns[1], |ui| {
                    let printed = RichText::new(&self.test_output).monospace().size(12.5);
                    ui.add(egui::Label::new(printed).selectable(true));
                });
        });
        let ctx = ui.ctx().clone();
        if run_all {
            self.start_tests(&ctx, false);
        }
        if scan {
            self.scan_tests(&ctx);
        }
        if let (Some((file, line)), Some(root)) = (go_to, self.folder())
            && self.open_file(&ctx, &root.join(file))
            && let Some(tab) = self.tabs.get_mut(self.active)
        {
            let start = godcode_core::line_start(&tab.text, line + 1);
            tab.select(&ctx, start..start);
        }
        if let Some(name) = run {
            self.start_test(&ctx, &name);
        }
    }

    /// The address the file in front is served at: it is saved first, and its folder
    /// served when it is not yet.
    fn live_url(&mut self, ctx: &Context) -> Option<String> {
        let index = self.active;
        let unsaved = self
            .tabs
            .get(index)
            .is_some_and(|tab| tab.dirty || tab.path.is_none());
        if unsaved && !self.save_tab(ctx, index, false) {
            return None;
        }
        let path = self.tabs.get(index).and_then(|tab| tab.path.clone())?;
        let beside = path.parent().map(Path::to_path_buf);
        let root = self
            .folder()
            .filter(|root| path.starts_with(root))
            .or(beside)?;
        // One folder is served at a time: another one replaces it.
        if self.live.as_ref().is_none_or(|live| live.root != root) {
            match crate::live::Live::start(root) {
                Ok(live) => self.live = Some(live),
                Err(err) => {
                    self.toasts.error(format!("Cannot serve the folder: {err}"));
                    return None;
                }
            }
        }
        self.live.as_ref().and_then(|live| live.url_of(&path))
    }

    /// Show the file in front as a page, inside the window or in the browser.
    fn live_preview(&mut self, ctx: &Context, in_browser: bool) {
        let Some(url) = self.live_url(ctx) else {
            return;
        };
        if !in_browser {
            self.web_file = self.tabs.get(self.active).and_then(|tab| tab.path.clone());
            self.web_wanted = Some(url);
            return;
        }
        match tools::open_in_browser(&url) {
            Ok(()) => self.toasts.success(format!("Live preview at {url}")),
            Err(err) => self.toasts.error(err),
        }
    }

    /// The preview follows the editor: another page of the folder served, brought in
    /// front, takes the place of the one on show. An address typed by hand is left alone.
    fn follow_preview(&mut self) {
        let front = self.tabs.get(self.active).and_then(|tab| tab.path.as_ref());
        let (true, Some(live), Some(path)) = (self.web_open, &self.live, front) else {
            return;
        };
        if self.web_file.as_ref() == Some(path) {
            return;
        }
        self.web_file = Some(path.clone());
        let extension = path.extension().and_then(|extension| extension.to_str());
        let page = matches!(extension, Some("html" | "htm"));
        let ours = self
            .web_shown
            .starts_with(&format!("http://127.0.0.1:{}/", live.port));
        if page && ours {
            self.web_wanted = live.url_of(path);
        }
    }

    /// The panel of the page shown inside the window: its address and what can be done
    /// with it. Returns the place left for the page itself.
    fn web_panel(&mut self, ui: &mut Ui) -> Option<Rect> {
        let web = self.web.as_ref()?;
        let url = web.url();
        let mut close = false;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(6.0);
                let button = |ui: &mut Ui, glyph: &str, tip: &str| {
                    let button = Button::new(glyph).frame_when_inactive(false);
                    ui.add(button).on_hover_text(tip).clicked()
                };
                close = button(ui, icon::X, "Close the preview");
                if button(
                    ui,
                    icon::BUG,
                    "Inspect the page: elements, console, network",
                ) {
                    web.inspect();
                }
                if button(ui, icon::ARROW_SQUARE_OUT, "Open in the browser")
                    && let Err(err) = tools::open_in_browser(&url)
                {
                    self.toasts.error(err);
                }
                if button(ui, icon::ARROW_CLOCKWISE, "Reload") {
                    web.reload();
                }
                if button(ui, icon::ARROW_RIGHT, "Forward") {
                    web.step(false);
                }
                if button(ui, icon::ARROW_LEFT, "Back") {
                    web.step(true);
                }
                // The address takes what is left: any page can be typed there, a
                // development server of this computer to begin with.
                let address = TextEdit::singleline(&mut self.web_address)
                    .desired_width(ui.available_width() - 10.0)
                    .hint_text("localhost:5173");
                let typed = ui.add(address);
                let entered = ui.input(|input| input.key_pressed(Key::Enter));
                if typed.lost_focus() && entered {
                    self.web_wanted = whole_address(&self.web_address);
                }
            });
        });
        ui.add_space(4.0);
        if close {
            self.web_open = false;
            return None;
        }
        // The edge of the panel stays ours, to drag it wider or narrower.
        let rect = ui.available_rect_before_wrap().shrink2(vec2(4.0, 0.0));
        ui.allocate_rect(rect, Sense::hover());
        Some(rect)
    }

    /// Put the page shown inside the window where its panel is, or hide it: it is a
    /// window of its own, over anything drawn there.
    fn place_web(&mut self, ctx: &Context, frame: &eframe::Frame, rect: Option<Rect>) {
        if let Some(url) = self.web_wanted.take() {
            self.web_address.clone_from(&url);
            match &mut self.web {
                Some(web) => web.go(&url),
                None => {
                    let waking = ctx.clone();
                    let wake = move || waking.request_repaint();
                    match crate::webview::Web::open(frame, &url, wake) {
                        Ok(web) => self.web = Some(web),
                        // No web view to show it in: the browser has one.
                        Err(err) => {
                            self.toasts.error(err);
                            let _ = tools::open_in_browser(&url);
                        }
                    }
                }
            }
            self.web_open = self.web.is_some();
            ctx.request_repaint();
        }
        // A menu or a dialog would be drawn under the page: the page gives way.
        let covered =
            ctx.any_popup_open() || ctx.memory(|memory| memory.top_modal_layer().is_some());
        if let Some(web) = &mut self.web {
            // A press seen here was not in the page: the keyboard comes back with it.
            if ctx.input(|input| input.pointer.any_pressed()) {
                web.release_keyboard();
            }
            web.place(rect.filter(|_| !covered), ctx.pixels_per_point());
            // A link followed in the page: the address says where it led.
            let now = web.url();
            if now != self.web_shown {
                self.web_address.clone_from(&now);
                self.web_shown = now;
            }
        }
    }

    /// The projects to start from: pick one, then the folder to write it in.
    fn templates_dialog(&mut self, ctx: &Context) {
        let Some(mut selected) = self.templates.take() else {
            return;
        };
        let rows: Vec<String> = tools::TEMPLATES
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect();
        let Some(answer) = list_dialog(ctx, "New project", &rows, &mut selected, None) else {
            self.templates = Some(selected);
            return;
        };
        let Some((_, files)) = answer.and_then(|row| tools::TEMPLATES.get(row)) else {
            return;
        };
        let Some(root) = platform::pick_folder(self.folder().as_deref()) else {
            return;
        };
        // What is already there stays as it is.
        let mut first = None;
        for (name, content) in *files {
            let file = root.join(name);
            first.get_or_insert_with(|| file.clone());
            if file.exists() {
                continue;
            }
            let written = file.parent().map_or(Ok(()), std::fs::create_dir_all);
            if let Err(err) = written.and_then(|()| std::fs::write(&file, content)) {
                self.toasts.error(format!("Cannot write {name}: {err}"));
                return;
            }
        }
        self.open_folder(ctx, root);
        if let Some(file) = first {
            self.open_file(ctx, &file);
        }
    }

    /// Switch to the colour scheme at `index`.
    fn set_scheme(&mut self, ctx: &Context, index: usize) {
        let Some(scheme) = theme::scheme(index) else {
            return;
        };
        self.scheme = index;
        self.dark = scheme.dark;
        theme::apply(ctx, index);
    }

    /// The colour schemes: the one chosen shows at once, Escape keeps it.
    fn themes_dialog(&mut self, ctx: &Context) {
        let Some(mut selected) = self.themes.take() else {
            return;
        };
        let names = theme::scheme_names();
        let rows: Vec<String> = names.into_iter().map(str::to_owned).collect();
        let before = selected;
        let answer = list_dialog(ctx, "Color theme", &rows, &mut selected, None);
        // Moving in the list tries the scheme on.
        if selected != before {
            self.set_scheme(ctx, selected);
        }
        match answer {
            None => self.themes = Some(selected),
            Some(Some(row)) => self.set_scheme(ctx, row),
            Some(None) => {}
        }
    }

    /// Add a caret on the line above or below the carets there are, in the same column.
    fn add_caret(&mut self, below: bool) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let carets = tab.view.extra.iter().copied().chain([tab.selection.start]);
        let edge = if below { carets.max() } else { carets.min() };
        let (line, column) = godcode_core::line_col(&tab.text, edge.unwrap_or(0));
        let lines = tab.text.split('\n').count();
        let target = if below { line + 1 } else { line - 1 };
        if target == 0 || target > lines {
            return;
        }
        let start = godcode_core::line_start(&tab.text, target);
        let length = tab
            .text
            .chars()
            .skip(start)
            .take_while(|ch| *ch != '\n')
            .count();
        let caret = start + (column - 1).min(length);
        if caret != tab.selection.start && !tab.view.extra.contains(&caret) {
            tab.view.extra.push(caret);
        }
        tab.view.focus = true;
    }

    /// What the project can run: pick a task to run it in the terminal.
    fn tasks_dialog(&mut self, ctx: &Context) {
        let Some((all, mut typed, mut selected)) = self.tasks.take() else {
            return;
        };
        let wanted = typed.to_lowercase();
        let fits = |task: &&(String, String)| task.0.to_lowercase().contains(&wanted);
        let shown: Vec<&(String, String)> = all.iter().filter(fits).collect();
        let rows: Vec<String> = shown
            .iter()
            .map(|(name, command)| format!("{name}      {command}"))
            .collect();
        let answer = list_dialog(ctx, "Run task", &rows, &mut selected, Some(&mut typed));
        let command = answer
            .flatten()
            .and_then(|row| Some(shown.get(row)?.1.clone()));
        if answer.is_none() {
            self.tasks = Some((all, typed, selected));
        } else if let Some(command) = command {
            // What stands for the file in front and its project is written in now.
            let file = self.tabs.get(self.active).and_then(|tab| tab.path.clone());
            let command = tools::with_places(&command, file.as_deref(), self.folder().as_deref());
            self.run_task(ctx, command);
        }
    }

    /// Run `command` in the terminal, and remember it as the task to run again.
    fn run_task(&mut self, ctx: &Context, command: String) {
        self.show_terminal(ctx);
        if let Some(terminal) = &mut self.terminal {
            terminal.send(&command);
        }
        self.last_task = Some(command);
    }

    /// The folder commands are run for: the one open, or the one of the file in front.
    fn run_root(&self) -> Option<PathBuf> {
        let beside = self
            .tabs
            .get(self.active)
            .and_then(|tab| Some(tab.path.as_deref()?.parent()?.to_path_buf()));
        self.folder().or(beside)
    }

    /// The commits of the folder: pick one to see what it changed.
    fn log_dialog(&mut self, ctx: &Context) {
        let Some((all, mut typed, mut selected)) = self.log.take() else {
            return;
        };
        let wanted = typed.to_lowercase();
        let fits = |commit: &&String| commit.to_lowercase().contains(&wanted);
        let rows: Vec<String> = all.iter().filter(fits).cloned().collect();
        let (title, drawn) = match self.log_graph {
            true => ("Git graph", true),
            false => ("Git log", false),
        };
        let answer = list_dialog_as(ctx, title, &rows, &mut selected, Some(&mut typed), drawn);
        let Some(answer) = answer else {
            self.log = Some((all, typed, selected));
            return;
        };
        let picked = answer.and_then(|row| rows.get(row));
        let hash = picked.and_then(|row| git::hash_in(row)).map(str::to_owned);
        // A line that only draws the branches is no commit to show: the list stays.
        if picked.is_some() && hash.is_none() {
            self.log = Some((all, typed, selected));
            return;
        }
        let (Some(hash), Some(root)) = (hash, self.folder()) else {
            return;
        };
        match tools::git(&root, &["show", "--no-color", "--format=", &hash]) {
            Ok(printed) => {
                let title = format!("Commit {hash}: before  |  after");
                self.diff = Some((title, git::side_by_side(&printed, true)));
            }
            Err(err) => self.toasts.error(err),
        }
    }

    /// Send the request written around the caret of a `.http` file.
    fn send_request(&mut self, ctx: &Context) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let chosen = self.environment.as_ref();
        let mut environments = self.http_environments().into_iter();
        let values = environments
            .find(|(name, _)| Some(name) == chosen)
            .map_or_else(Vec::new, |(_, values)| values);
        let request = godcode_core::http::request_in(&tab.text, self.caret_line, &values);
        let Some(request) = request else {
            self.toasts
                .error("No request here: write a line such as GET https://example.com");
            return;
        };
        self.requested = format!("{} {}", request.method, request.url);
        let (sender, receiver) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // The window may be gone: nobody listens then.
            let _ = sender.send(tools::send(&request));
            ctx.request_repaint();
        });
        self.requesting = Some(receiver);
        self.bottom = Some(Bottom::Response);
    }

    /// The text of the file at `path`: as edited when it is open, else as on disk.
    fn text_of(&self, path: &Path) -> String {
        let open = self
            .tabs
            .iter()
            .find(|tab| tab.path.as_deref() == Some(path));
        open.map(|tab| tab.text.clone())
            .or_else(|| Some(godcode_core::decode(&std::fs::read(path).ok()?)?.text))
            .unwrap_or_default()
    }

    /// Make the changes a language server asks for: in the editors of the open files, on
    /// disk for the others. Returns how many files changed.
    fn apply_edits(&mut self, ctx: &Context, files: &[godcode_lsp::FileEdits]) -> usize {
        // All of it or nothing: half a change (code taken out of a file, not put in the
        // other) is worse than none.
        let is_open = |path: &Path| {
            self.tabs
                .iter()
                .any(|tab| tab.path.as_deref() == Some(path))
        };
        let closed = files.iter().filter(|file| !is_open(&file.path));
        if let Some(file) = closed
            .into_iter()
            .find(|file| text_on_disk(&file.path).is_none())
        {
            let name = file_name(&file.path);
            self.toasts
                .error(format!("Nothing changed: {name} cannot be read as text"));
            return 0;
        }
        let mut changed = 0;
        for file in files {
            let path = file.path.as_path();
            let open = self
                .tabs
                .iter_mut()
                .find(|tab| tab.path.as_deref() == Some(path));
            if let Some(tab) = open {
                let text = servers::rewritten(&tab.text, &file.edits);
                if text != tab.text {
                    // The caret stays on what it was on, lines added above or not.
                    let caret = servers::moved(&tab.text, &file.edits, tab.selection.start);
                    tab.text = text;
                    tab.touched();
                    tab.select(ctx, caret..caret);
                    changed += 1;
                }
                continue;
            }
            let Some(decoded) = text_on_disk(path) else {
                continue;
            };
            let text = servers::rewritten(&decoded.text, &file.edits);
            let bytes = godcode_core::encode(&text, decoded.ending, decoded.bom);
            if text != decoded.text {
                match platform::write_atomic(path, &bytes) {
                    Ok(()) => changed += 1,
                    Err(err) => self.toasts.error(err),
                }
            }
        }
        changed
    }

    /// Ask the language server what can be fixed on the line of the caret.
    fn ask_fixes(&mut self, ctx: &Context) {
        self.sync_server(ctx);
        let asked = self.tabs.get(self.active).and_then(|tab| {
            let line = servers::position(&tab.text, tab.selection.start).0;
            self.servers.actions(tab.path.as_deref()?, line)
        });
        match asked {
            Some(id) => {
                self.asked.insert(id, Asked::Fixes);
            }
            None => self.tell_server_state(ctx),
        }
    }

    /// The fixes the language server offers: pick one to apply it.
    fn fixes_dialog(&mut self, ctx: &Context) {
        let Some((actions, mut selected)) = self.fixes.take() else {
            return;
        };
        let (up, down, enter) = ctx.input_mut(|input| {
            (
                input.consume_key(Modifiers::NONE, Key::ArrowUp),
                input.consume_key(Modifiers::NONE, Key::ArrowDown),
                input.consume_key(Modifiers::NONE, Key::Enter),
            )
        });
        selected = (selected + usize::from(down)).min(actions.len() - 1);
        selected = selected.saturating_sub(usize::from(up));
        let mut chosen = enter.then_some(selected);
        let response = Modal::new(Id::new("quick-fixes")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.heading("Quick fixes");
            ui.add_space(6.0);
            for (index, action) in actions.iter().enumerate() {
                let row = Button::selectable(
                    index == selected,
                    with_icon(icon::LIGHTBULB, &action.title),
                )
                .frame_when_inactive(index == selected)
                .stroke(Stroke::NONE)
                .wrap_mode(egui::TextWrapMode::Truncate)
                .min_size(vec2(ui.available_width(), 28.0));
                if ui.add(row).clicked() {
                    chosen = Some(index);
                }
            }
        });
        match chosen.and_then(|index| actions.get(index)) {
            Some(action) => {
                self.apply_edits(ctx, &action.files);
                // The rest is the server's to do, on the text as it now is.
                self.sync_server(ctx);
                let front = self.tabs.get(self.active);
                if let Some(path) = front.and_then(|tab| tab.path.clone()) {
                    self.servers.carry_out(&path, action);
                }
                self.toasts.success(action.title.clone());
            }
            None if !response.should_close() => self.fixes = Some((actions, selected)),
            None => {}
        }
    }

    /// Follow the pointer over the code: once it rests on a name, its documentation is
    /// asked for, and dropped when the pointer leaves.
    fn rest_pointer(&mut self, ctx: &Context, pointed: Option<(usize, egui::Pos2)>) {
        let now = ctx.input(|input| input.time);
        let is_word = |ch: &char| ch.is_alphanumeric() || *ch == '_';
        let name = pointed.and_then(|(index, pointer)| {
            let tab = self.tabs.get(self.active)?;
            let start = complete::word_start(&tab.text, index);
            let named = tab.text.chars().skip(start).take_while(is_word).count() > 0;
            named.then_some((start, pointer))
        });
        let Some((start, pointer)) = name else {
            self.rest = None;
            self.tip = None;
            return;
        };
        self.pointer = pointer;
        match self.rest {
            Some((name, since, false)) if name == start && now - since >= HOVER_DELAY => {
                self.rest = Some((start, since, true));
                // Stopped in the debugger, a name is worth something: it is asked.
                if let (Some(session), true, Some(tab)) = (
                    &mut self.debug,
                    self.debug_stopped,
                    self.tabs.get(self.active),
                ) {
                    let name: String = tab.text.chars().skip(start).take_while(is_word).collect();
                    session.watch(std::slice::from_ref(&name));
                    self.hovered = Some((start, name));
                }
                self.sync_server(ctx);
                let asked = self.tabs.get(self.active).and_then(|tab| {
                    let (line, column) = servers::position(&tab.text, start);
                    self.servers.hover(tab.path.as_deref()?, line, column)
                });
                if let Some(id) = asked {
                    self.asked.insert(id, Asked::Tip(start));
                }
            }
            Some((name, ..)) if name == start => {}
            _ => {
                self.rest = Some((start, now, false));
                self.tip = None;
            }
        }
        if matches!(self.rest, Some((_, _, false))) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(HOVER_DELAY));
        }
    }

    /// The documentation of the name under the pointer, next to it.
    fn tip_window(&self, ctx: &Context) {
        let Some((_, pointer, blocks)) = &self.tip else {
            return;
        };
        egui::Area::new(Id::new("code-tip"))
            .order(egui::Order::Tooltip)
            .fixed_pos(*pointer + vec2(12.0, 18.0))
            .show(ctx, |ui| {
                Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(520.0);
                    preview::show(ui, blocks, self.zoom);
                });
            });
    }

    /// Ask the language server what is known about what the caret is on.
    fn ask_documentation(&mut self, ctx: &Context) {
        self.sync_server(ctx);
        let asked = self.tabs.get(self.active).and_then(|tab| {
            let path = tab.path.as_deref()?;
            let (line, column) = servers::position(&tab.text, tab.selection.start);
            self.servers.hover(path, line, column)
        });
        match asked {
            Some(id) => {
                self.asked.insert(id, Asked::Documentation);
            }
            None => self.tell_server_state(ctx),
        }
    }

    /// Say which language server serves the file in front, or how to get one.
    fn tell_server_state(&mut self, ctx: &Context) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let language = tab.language.name;
        match self.servers.state(language) {
            Some(servers::State::On(program)) => {
                self.toasts
                    .success(format!("{language} is served by {program}"));
            }
            Some(servers::State::Off(known, why)) if !why.is_empty() => {
                self.toasts
                    .error(format!("{} stopped: {why}", known.program));
            }
            Some(servers::State::Off(known, _)) => {
                ctx.copy_text(known.install.to_owned());
                self.toasts.error(format!(
                    "{} is not running. To install it (copied): {}",
                    known.program, known.install
                ));
            }
            None => self
                .toasts
                .error(format!("No language server is known for {language}")),
        }
    }

    /// The documentation the language server answered, in a window of its own.
    fn documentation_window(&mut self, ctx: &Context) {
        let Some(blocks) = &self.documentation else {
            return;
        };
        let mut open = true;
        let zoom = self.zoom;
        egui::Window::new("Documentation")
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, -60.0))
            .default_width(560.0)
            .default_height(320.0)
            .vscroll(true)
            .show(ctx, |ui| {
                ui.set_width(ui.available_width());
                preview::show(ui, blocks, zoom);
            });
        if !open || ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.documentation = None;
        }
    }

    /// With auto save on, write the files left alone for a moment since their last change.
    fn save_idle(&mut self, ctx: &Context) {
        let now = ctx.input(|input| input.time);
        let on = self.autosave;
        let mut due = Vec::new();
        let mut waiting = false;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if tab.seen.0 != tab.revision {
                tab.seen = (tab.revision, now);
            }
            if on && tab.dirty && tab.path.is_some() {
                if now - tab.seen.1 >= AUTOSAVE_DELAY {
                    due.push(index);
                } else {
                    waiting = true;
                }
            }
        }
        for index in due {
            if !self.save_tab(ctx, index, false) {
                // It said why once: no new attempt before the next change.
                self.tabs[index].seen.1 = f64::INFINITY;
            }
        }
        if waiting {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(AUTOSAVE_DELAY));
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
        // The language server knows best, across files; without one, the outline does.
        self.sync_server(ctx);
        if let Some(tab) = self.tabs.get(self.active)
            && let Some(path) = tab.path.as_deref()
        {
            let (line, column) = servers::position(&tab.text, at.unwrap_or(tab.selection.start));
            if let Some(id) = self.servers.definition(path, line, column) {
                self.asked.insert(id, Asked::Definition(at));
                return;
            }
        }
        self.declaration_in_open_files(ctx, at);
    }

    /// [`Self::go_to_declaration`] without a language server: by the outline of the open
    /// files.
    fn declaration_in_open_files(&mut self, ctx: &Context, at: Option<usize>) {
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
        let at_caret = word.is_none();
        let Some(word) = word.or_else(|| self.word_at(None)) else {
            return;
        };
        // The language server knows what the name means; a search only knows how it is
        // written.
        self.sync_server(ctx);
        let asked = self
            .tabs
            .get(self.active)
            .filter(|_| at_caret)
            .and_then(|tab| {
                let (line, column) = servers::position(&tab.text, tab.selection.start);
                self.servers.references(tab.path.as_deref()?, line, column)
            });
        if let Some(id) = asked {
            self.asked.insert(id, Asked::References(word));
            return;
        }
        if self.explorer.is_none() {
            self.find.query = word;
            self.find.options.match_case = true;
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
        self.sync_server(ctx);
        let asked = self.tabs.get(self.active).and_then(|tab| {
            let (width, spaces) = match tab.indent {
                Indent::Spaces(width) => (width, true),
                Indent::Tabs => (4, false),
            };
            self.servers.format(tab.path.as_deref()?, width, spaces)
        });
        if let Some(id) = asked {
            self.asked.insert(id, Asked::Format);
            return;
        }
        match self.format_now(ctx) {
            Ok(Some((before, after))) => self
                .toasts
                .success(format!("Reformatted: {before} lines, now {after}")),
            Ok(None) => self.toasts.success("Already formatted"),
            Err(complaint) => self.toasts.error(format!("Cannot reformat: {complaint}")),
        }
    }

    /// Format the file in front with the formatter of its language when it is installed,
    /// else the built-in one. Returns its lines before and after when it changed.
    fn format_now(&mut self, ctx: &Context) -> Result<Option<(usize, usize)>, String> {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return Ok(None);
        };
        let formatted = match tools::format_external(tab.language.name, &tab.text) {
            Some(formatted) => formatted?,
            None => format::format(&tab.text, tab.language, &tab.indent.unit()),
        };
        if formatted == tab.text {
            return Ok(None);
        }
        let line = godcode_core::line_col(&tab.text, tab.selection.start).0;
        let before = tab.text.lines().count();
        tab.text = formatted;
        tab.indent = Indent::detect(&tab.text);
        tab.touched();
        let caret = godcode_core::line_start(&tab.text, line);
        tab.select(ctx, caret..caret);
        Ok(Some((before, tab.text.lines().count())))
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
        let targets = (0..self.tabs.len()).rev();
        let targets: Vec<usize> = targets
            .filter(|index| wanted(*index) && !self.tabs[*index].pinned)
            .collect();
        for index in targets {
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
        // Where the caret is, found again only when it or the text moved.
        let at = (tab.id, tab.revision, tab.selection.clone());
        if self.caret_place.0 != at {
            let start = godcode_core::line_col(&tab.text, tab.selection.start);
            let end = match tab.selection.is_empty() {
                true => start,
                false => godcode_core::line_col(&tab.text, tab.selection.end),
            };
            self.caret_place = (at, start, end);
        }
        self.caret_line = self.caret_place.1.0 - 1;
        let key = hash_of((tab.id, tab.revision, tab.language.name));
        // A big text is not read through at every key for what it defines.
        let small = tab.text.len() <= editor::LARGE_BYTES;
        if self.symbols.0 != key {
            let symbols = small.then(|| outline::symbols(&tab.text, tab.language));
            self.symbols = (key, symbols.unwrap_or_default());
        }
        if self.tests.0 != key {
            let tests = small.then(|| outline::tests(&tab.text, tab.language));
            self.tests = (key, tests.unwrap_or_default());
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
            minimap: false,
            sticky: false,
            note: None,
            snippets: &[],
            breakpoints: &[],
            conditional: &[],
            stopped: None,
            hints: &[],
            folds: None,
            coverage: &[],
            block_caret: false,
            revision: 0,
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
                // A file that would not format is saved as it is: the complaint is told.
                let small = |tab: &Tab| tab.text.len() <= editor::LARGE_BYTES;
                if self.format_on_save
                    && self.tabs.get(self.active).is_some_and(small)
                    && let Err(complaint) = self.format_now(ctx)
                {
                    self.toasts
                        .error(format!("Saved without formatting: {complaint}"));
                }
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
                    if let Some(selection) =
                        editor::history(ctx, id, &mut tab.text, &mut tab.view, undo)
                    {
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
                // From a dark scheme to the light one, and back.
                self.set_scheme(ctx, usize::from(self.dark));
            }
            Cmd::Home => self.home = !self.home,
            // While a program is stopped under the debugger, running means going on.
            Cmd::Run if self.debug_stopped && self.debug.is_some() => {
                if let Some(session) = &mut self.debug {
                    session.resume();
                }
                self.debug_running();
            }
            Cmd::Run => self.run_file(ctx),
            Cmd::Debug => self.start_debug(ctx),
            Cmd::StopDebug => {
                if let Some(session) = &mut self.debug {
                    session.stop();
                }
            }
            Cmd::StepOver | Cmd::StepInto | Cmd::StepOut => {
                if let Some(session) = self.debug.as_mut().filter(|_| self.debug_stopped) {
                    match cmd {
                        Cmd::StepOver => session.step_over(),
                        Cmd::StepInto => session.step_into(),
                        _ => session.step_out(),
                    }
                    self.debug_running();
                }
            }
            Cmd::Breakpoint => {
                let line = self.caret_line;
                self.toggle_breakpoint(self.active, line);
            }
            Cmd::BreakpointCondition => {
                let line = self.caret_line;
                if let Some(tab) = self.tabs.get(self.active) {
                    self.prompt = Some(Prompt {
                        kind: PromptKind::Condition(line),
                        name: tab.conditions.get(&line).cloned().unwrap_or_default(),
                        focus: true,
                    });
                }
            }
            Cmd::LivePreview => self.live_preview(ctx, false),
            Cmd::LivePreviewBrowser => self.live_preview(ctx, true),
            Cmd::Keymap => {
                let example = "{\n  \"Reformat Code\": \"Ctrl+Alt+L\",\n  \"Toggle Terminal\": \"Ctrl+J\"\n}\n";
                self.open_settings(ctx, "keymap.json", example);
            }
            Cmd::DebugSettings => {
                let example = "{\n  \"Rust\": {\n    \"adapter\": [\"lldb-dap\"],\n    \"launch\": {\n      \
                               \"program\": \"${folder}/target/debug/app.exe\",\n      \"cwd\": \"${folder}\"\n    \
                               }\n  }\n}\n";
                self.open_settings(ctx, "debug.json", example);
            }
            Cmd::TestsPanel => {
                let shown = self.bottom == Some(Bottom::Tests);
                self.bottom = (!shown).then_some(Bottom::Tests);
            }
            Cmd::MergeView => {
                let found = self
                    .tabs
                    .get(self.active)
                    .and_then(|tab| Some((tab.id, godcode_core::merge::from_markers(&tab.text)?)));
                match found {
                    Some((tab, parts)) => {
                        let choices = vec![None; parts.len()];
                        self.merging = Some(Merging {
                            tab,
                            parts,
                            choices,
                        });
                    }
                    None => self.toasts.error("This file has no conflict to settle"),
                }
            }
            Cmd::AcceptOurs | Cmd::AcceptTheirs | Cmd::AcceptBoth => {
                let keep = match cmd {
                    Cmd::AcceptOurs => git::Keep::Ours,
                    Cmd::AcceptTheirs => git::Keep::Theirs,
                    _ => git::Keep::Both,
                };
                let line = self.caret_line;
                let settled = self.tabs.get_mut(self.active).and_then(|tab| {
                    tab.text = git::resolve_conflict(&tab.text, line, keep)?;
                    tab.touched();
                    Some(())
                });
                if settled.is_none() {
                    self.toasts.error("The caret is not in a conflict");
                }
            }
            Cmd::TestAtCaret => {
                let found = self.tabs.get(self.active).and_then(|tab| {
                    let name = outline::test_at(&tab.text, tab.language, self.caret_line)?;
                    tools::test_one(tab.language.name, tab.path.as_deref()?, &name)
                });
                match found {
                    Some(command) => {
                        self.show_terminal(ctx);
                        if let Some(terminal) = &mut self.terminal {
                            terminal.send(&command);
                        }
                    }
                    None => self.toasts.error("No test around the caret"),
                }
            }
            Cmd::NewProject => self.templates = Some(0),
            Cmd::DebugPanel => {
                let shown = self.bottom == Some(Bottom::Debug);
                self.bottom = (!shown).then_some(Bottom::Debug);
            }
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
            Cmd::Preview => self.preview = !self.preview,
            Cmd::Minimap => self.minimap = !self.minimap,
            Cmd::Fold | Cmd::UnfoldAll => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.view.fold_at_caret = cmd == Cmd::Fold;
                    if cmd == Cmd::UnfoldAll {
                        tab.view.folded.clear();
                    }
                }
            }
            Cmd::Hints => {
                self.hints = !self.hints;
                if !self.hints {
                    for tab in &mut self.tabs {
                        tab.hints.clear();
                        tab.hints_at = None;
                    }
                }
                self.toasts.success(if self.hints {
                    "Inlay hints on"
                } else {
                    "Inlay hints off"
                });
            }
            Cmd::Keys => {
                self.keys = match self.keys {
                    Keys::Default => Keys::Vim,
                    Keys::Vim => Keys::Emacs,
                    Keys::Emacs => Keys::Default,
                };
                self.vim = modal::Vim::default();
                self.emacs = modal::Emacs::default();
                self.toasts.success(match self.keys {
                    Keys::Default => "Keys: default",
                    Keys::Vim => "Keys: Vim (normal mode; i to type, Esc to come back)",
                    Keys::Emacs => "Keys: Emacs (Ctrl+X Ctrl+S saves, Ctrl+S searches)",
                });
            }
            Cmd::Documentation => self.ask_documentation(ctx),
            Cmd::LanguageServer => self.tell_server_state(ctx),
            Cmd::QuickFix => self.ask_fixes(ctx),
            Cmd::Implementation => {
                self.sync_server(ctx);
                let asked = self.tabs.get(self.active).and_then(|tab| {
                    let (line, column) = servers::position(&tab.text, tab.selection.start);
                    self.servers
                        .implementation(tab.path.as_deref()?, line, column)
                });
                match asked {
                    Some(id) => {
                        self.asked.insert(id, Asked::Definition(None));
                    }
                    None => self.tell_server_state(ctx),
                }
            }
            Cmd::Callers | Cmd::Callees => {
                self.sync_server(ctx);
                let word = self.word_at(None);
                let front = self.tabs.get(self.active).zip(word);
                let asked = front.and_then(|(tab, word)| {
                    let (line, column) = servers::position(&tab.text, tab.selection.start);
                    let incoming = cmd == Cmd::Callers;
                    let path = tab.path.as_deref()?;
                    Some((self.servers.calls(path, line, column, incoming)?, word))
                });
                match asked {
                    // Listed like usages: the calls for the callers, the definitions for
                    // what is called.
                    Some((id, word)) => {
                        self.asked.insert(id, Asked::References(word));
                    }
                    None => self.tell_server_state(ctx),
                }
            }
            Cmd::Split | Cmd::SplitDown | Cmd::SplitSame => {
                self.split_down = cmd == Cmd::SplitDown;
                let alone = self.tabs.len() == 1;
                if self.right.is_some() && (self.twin || cmd != Cmd::SplitSame) {
                    // Back to one editor: the one that had the keyboard stays.
                    self.right = None;
                    self.right_front = false;
                    self.twin = false;
                    self.left = self.tabs.get(self.active).map_or(0, |tab| tab.id);
                } else if cmd == Cmd::SplitSame || alone {
                    // The file in front, twice: each editor scrolls and selects on its own.
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        tab.other = None;
                        tab.view.focus = true;
                        (self.left, self.right) = (tab.id, Some(tab.id));
                        // The keyboard stays with the view it is in.
                        self.right_front = tab.in_second;
                        self.twin = true;
                    }
                } else {
                    // The file used before this one goes to the right.
                    let front = self.active;
                    let others = self
                        .tabs
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| *index != front);
                    match others.max_by_key(|(_, tab)| tab.used) {
                        Some((_, tab)) => self.right = Some(tab.id),
                        None => self
                            .toasts
                            .error("Open another file to put beside this one"),
                    }
                }
            }
            Cmd::Branches | Cmd::Merge | Cmd::Rebase => {
                self.branch_action = cmd;
                let listed = self
                    .folder()
                    .map(|root| tools::git(&root, &["branch", "--format=%(refname:short)"]));
                match listed {
                    Some(Ok(printed)) => {
                        let all = printed.lines().map(str::to_owned).collect();
                        self.branches = Some((all, String::new(), 0));
                    }
                    Some(Err(err)) => self.toasts.error(err),
                    None => self.toasts.error("Open a folder to see its branches"),
                }
            }
            Cmd::Diff => self.compare_with_commit(ctx),
            Cmd::Blame => self.blame = !self.blame,
            Cmd::Themes => self.themes = Some(self.scheme),
            Cmd::Font => {
                if let Some(path) = platform::pick_font() {
                    match std::fs::read(&path) {
                        Ok(bytes) if widgets::is_font(&bytes) => {
                            widgets::install_fonts(ctx, Some(bytes));
                            self.toasts
                                .success(format!("Code is drawn with {}", file_name(&path)));
                            self.font = Some(path);
                        }
                        Ok(_) => self.toasts.error("This file is not a font"),
                        Err(err) => self.toasts.error(format!("Cannot read the font: {err}")),
                    }
                }
            }
            Cmd::FontReset => {
                widgets::install_fonts(ctx, None);
                self.font = None;
                self.toasts.success("Code is drawn with the built-in font");
            }
            Cmd::Zen => self.zen = !self.zen,
            Cmd::CaretAbove => self.add_caret(false),
            Cmd::CaretBelow => self.add_caret(true),
            Cmd::Tasks => {
                let tasks = self.run_root().map(|root| tools::tasks(&root));
                match tasks.filter(|tasks| !tasks.is_empty()) {
                    Some(tasks) => self.tasks = Some((tasks, String::new(), 0)),
                    None => self.toasts.error(
                        "No task found: write some with Edit Run Configurations, or they come \
                         from package.json, Cargo.toml, go.mod, Makefile",
                    ),
                }
            }
            Cmd::RunLast => match self.last_task.clone() {
                Some(command) => self.run_task(ctx, command),
                // Nothing was run yet: there is one to choose.
                None => self.pending.push(Cmd::Tasks),
            },
            Cmd::RunSettings => match self.run_root() {
                Some(root) => {
                    let file = root.join(tools::RUN_FILE);
                    if !file.exists() {
                        let example = "{\n  \"Serve\": \"npm run dev\",\n  \"Run this file\": \
                                       \"python ${file}\"\n}\n";
                        let _ = std::fs::create_dir_all(file.parent().unwrap_or(&root));
                        let _ = std::fs::write(&file, example);
                    }
                    self.open_file(ctx, &file);
                }
                None => self.toasts.error("Open a folder or a file first"),
            },
            Cmd::GitLog | Cmd::GitGraph => {
                // The graph draws every branch and how they part and join, each commit
                // with the names that point at it.
                let graph = cmd == Cmd::GitGraph;
                let args: &[&str] = if graph {
                    &[
                        "log",
                        "--graph",
                        "--all",
                        "--pretty=format:%h  %ad  %an%d  %s",
                        "--date=short",
                        "-n",
                        "600",
                    ]
                } else {
                    &[
                        "log",
                        "--pretty=format:%h  %ad  %an  %s",
                        "--date=short",
                        "-n",
                        "400",
                    ]
                };
                match self.folder().map(|root| tools::git(&root, args)) {
                    Some(Ok(printed)) => {
                        let commits = printed.lines().map(str::to_owned).collect();
                        self.log = Some((commits, String::new(), 0));
                        self.log_graph = graph;
                    }
                    Some(Err(err)) => self.toasts.error(err),
                    None => self.toasts.error("Open a folder to see its commits"),
                }
            }
            Cmd::Stash | Cmd::StashPop => {
                let args: &[&str] = if cmd == Cmd::Stash {
                    &["stash", "push", "--include-untracked"]
                } else {
                    &["stash", "pop"]
                };
                match self.folder().map(|root| tools::git(&root, args)) {
                    Some(Ok(printed)) => {
                        let said = printed.lines().next().unwrap_or("Done").to_owned();
                        self.toasts.success(said);
                        self.reload_from_disk(ctx);
                        self.files_changed(ctx);
                    }
                    Some(Err(err)) => self.toasts.error(err),
                    None => self.toasts.error("Open a folder first"),
                }
            }
            Cmd::EditThemes => match settings_file("themes.json") {
                Some(file) => {
                    if !file.exists() {
                        let folder = file.parent().unwrap_or(&file);
                        let _ = std::fs::create_dir_all(folder);
                        let _ = std::fs::write(&file, theme::EXAMPLE);
                    }
                    self.open_file(ctx, &file);
                    self.toasts
                        .success("Saved, its themes are listed in Color Theme…");
                }
                None => self
                    .toasts
                    .error("No place to keep the themes on this system"),
            },
            Cmd::Snippets => match snippets_file() {
                Some(file) => {
                    if !file.exists() {
                        let example = "{\n  \"JavaScript\": {\n    \"clg\": \"console.log($0);\"\n  },\n  \
                                       \"Rust\": {\n    \"dbg\": \"dbg!($0);\"\n  }\n}\n";
                        let folder = file.parent().unwrap_or(&file);
                        let _ = std::fs::create_dir_all(folder);
                        let _ = std::fs::write(&file, example);
                    }
                    self.open_file(ctx, &file);
                }
                None => self
                    .toasts
                    .error("No place to keep the snippets on this system"),
            },
            Cmd::Response => {
                let shown = self.bottom == Some(Bottom::Response);
                self.bottom = (!shown).then_some(Bottom::Response);
            }
            Cmd::History => {
                let path = self.tabs.get(self.active).and_then(|tab| tab.path.clone());
                let versions = path.as_deref().map(versions_of).unwrap_or_default();
                if versions.is_empty() {
                    self.toasts
                        .error("No saved version of this file yet: one is kept at each save");
                } else {
                    self.versions = Some((versions, 0));
                }
            }
            Cmd::TestResults => self.start_tests(ctx, false),
            Cmd::RunCoverage => self.start_tests(ctx, true),
            Cmd::Coverage => match self.coverage.is_empty() {
                true => self.load_coverage(),
                false => self.coverage.clear(),
            },
            Cmd::Environment => {
                let found = self.http_environments();
                if found.is_empty() {
                    self.toasts.error(
                        "No environment: write some in http-client.env.json beside the file, \
                         such as {\"dev\": {\"host\": \"localhost:3000\"}}",
                    );
                } else {
                    let names = found.into_iter().map(|(name, _)| name);
                    let rows: Vec<String> = std::iter::once("No environment".to_owned())
                        .chain(names)
                        .collect();
                    let chosen = self.environment.as_ref();
                    let selected = rows.iter().position(|row| Some(row) == chosen);
                    self.environments = Some((rows, selected.unwrap_or(0)));
                }
            }
            Cmd::RunTests => {
                let beside = self
                    .tabs
                    .get(self.active)
                    .and_then(|tab| Some(tab.path.as_deref()?.parent()?.to_path_buf()));
                let command = self
                    .folder()
                    .or(beside)
                    .and_then(|root| tools::test_command(&root));
                match command {
                    Some(command) => {
                        self.show_terminal(ctx);
                        if let Some(terminal) = &mut self.terminal {
                            terminal.send(command);
                        }
                    }
                    None => self.toasts.error("No tests are known for this project"),
                }
            }
            Cmd::StickyScroll => self.sticky = !self.sticky,
            Cmd::FormatOnSave => {
                self.format_on_save = !self.format_on_save;
                let state = if self.format_on_save { "on" } else { "off" };
                self.toasts.success(format!("Format on save is {state}"));
            }
            Cmd::AutoSave => {
                self.autosave = !self.autosave;
                let state = if self.autosave { "on" } else { "off" };
                self.toasts.success(format!("Auto save is {state}"));
            }
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

    /// With the keys of Vim or Emacs, the keyboard goes through them first while the editor
    /// has it: what they take reaches neither the text field nor the shortcuts.
    fn modal_keys(&mut self, ctx: &Context, clicked: &mut Vec<Cmd>) {
        let vim = self.keys == Keys::Vim;
        let Some(tab) = self
            .tabs
            .get_mut(self.active)
            .filter(|_| self.keys != Keys::Default)
        else {
            return;
        };
        let id = tab.editor_id();
        // Escape takes the keyboard away from a text field before anything here sees it:
        // in Vim it is a key like the others, and the editor keeps the keyboard.
        let escaped =
            vim && self.editor_focused && ctx.input(|input| input.key_pressed(Key::Escape));
        if escaped {
            tab.view.focus = true;
        } else if !ctx.memory(|memory| memory.has_focus(id)) {
            return;
        }
        let events = ctx.input(|input| input.events.clone());
        let mut kept = Vec::with_capacity(events.len());
        for event in events {
            let commands = vim && self.vim.mode != modal::Mode::Insert;
            let strokes: Vec<Typed> = match &event {
                Event::Text(typed) if commands => typed.chars().map(Typed::Char).collect(),
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => stroke_of(*key, *modifiers, vim, commands)
                    .into_iter()
                    .collect(),
                // Ctrl+X starts a command of Emacs: it cuts nothing.
                Event::Cut if !vim => continue,
                _ => Vec::new(),
            };
            let mut taken = false;
            for stroke in strokes {
                let done = if vim {
                    // One level of indent is what it is in this file.
                    self.vim.unit = tab.indent.unit();
                    self.vim.key(&mut tab.text, &tab.selection, stroke)
                } else {
                    self.emacs.key(&mut tab.text, &tab.selection, stroke)
                };
                let Some(done) = done else {
                    continue;
                };
                taken = true;
                if done.changed {
                    tab.touched();
                }
                editor::set_selection(ctx, id, &done.selection);
                tab.selection = done.selection;
                tab.view.reveal = true;
                if let Some(copied) = done.copied {
                    ctx.copy_text(copied);
                }
                clicked.extend(done.ask.into_iter().flat_map(|ask| match ask {
                    Ask::Undo => vec![Cmd::Undo],
                    Ask::Redo => vec![Cmd::Redo],
                    Ask::Find => vec![Cmd::Find],
                    Ask::Save => vec![Cmd::Save],
                    Ask::Open => vec![Cmd::OpenFile],
                    Ask::Close => vec![Cmd::CloseTab],
                    Ask::SaveClose => vec![Cmd::Save, Cmd::CloseTab],
                }));
            }
            if !taken {
                kept.push(event);
            }
        }
        ctx.input_mut(|input| input.events = kept);
    }

    fn shortcuts(&mut self, ctx: &Context, clicked: &mut Vec<Cmd>) {
        self.modal_keys(ctx, clicked);
        // A rectangle of text grows with Ctrl+Alt+Shift and the arrows: seen first, or they
        // would be taken for Ctrl+Alt and an arrow.
        let all = Modifiers::COMMAND | Modifiers::ALT | Modifiers::SHIFT;
        let steps = [
            (Key::ArrowUp, (-1, 0)),
            (Key::ArrowDown, (1, 0)),
            (Key::ArrowLeft, (0, -1)),
            (Key::ArrowRight, (0, 1)),
        ];
        for (key, step) in steps {
            if ctx.input_mut(|input| input.consume_key(all, key))
                && let Some(tab) = self.tabs.get_mut(self.active)
            {
                tab.view.block_step = Some(step);
            }
        }
        let ctrl = Modifiers::COMMAND;
        let shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let alt = Modifiers::COMMAND | Modifiers::ALT;
        // More specific (shifted) shortcuts first.
        let alt_shift = Modifiers::ALT | Modifiers::SHIFT;
        let table = [
            (ctrl, Key::F5, Cmd::Debug),
            (shift, Key::F10, Cmd::TestAtCaret),
            (Modifiers::SHIFT, Key::F5, Cmd::StopDebug),
            (Modifiers::SHIFT, Key::F11, Cmd::StepOut),
            (Modifiers::NONE, Key::F11, Cmd::StepInto),
            (Modifiers::NONE, Key::F10, Cmd::StepOver),
            (Modifiers::SHIFT, Key::F9, Cmd::BreakpointCondition),
            (Modifiers::NONE, Key::F9, Cmd::Breakpoint),
            (alt_shift, Key::ArrowRight, Cmd::ExpandSelection),
            (alt_shift, Key::ArrowLeft, Cmd::ShrinkSelection),
            (alt, Key::L, Cmd::Reformat),
            (alt, Key::ArrowLeft, Cmd::Back),
            (alt, Key::ArrowRight, Cmd::Forward),
            (Modifiers::ALT, Key::Z, Cmd::ToggleWrap),
            (Modifiers::ALT, Key::Enter, Cmd::QuickFix),
            (alt, Key::B, Cmd::Implementation),
            (alt, Key::H, Cmd::Callers),
            (alt, Key::Z, Cmd::Zen),
            (alt, Key::ArrowUp, Cmd::CaretAbove),
            (alt, Key::ArrowDown, Cmd::CaretBelow),
            (alt, Key::R, Cmd::RunLast),
            (shift, Key::R, Cmd::Tasks),
            (alt, Key::T, Cmd::TestResults),
            (shift, Key::T, Cmd::RunTests),
            (ctrl, Key::Backslash, Cmd::Split),
            (Modifiers::ALT, Key::F7, Cmd::FindUsages),
            (Modifiers::ALT, Key::F1, Cmd::Reveal),
            (Modifiers::ALT, Key::F12, Cmd::ToggleTerminal),
            (Modifiers::SHIFT, Key::F6, Cmd::Rename),
            (ctrl, Key::F12, Cmd::GoToSymbol),
            (ctrl, Key::F4, Cmd::CloseTab),
            (shift, Key::J, Cmd::JoinLines),
            (shift, Key::U, Cmd::ToggleCase),
            (shift, Key::C, Cmd::CopyPath),
            (shift, Key::V, Cmd::Preview),
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
            (ctrl, Key::Q, Cmd::Documentation),
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
        let own = self.keymap.clone();
        ctx.input_mut(|input| {
            // The shortcuts of the user first: they win over the built-in ones.
            for (modifiers, key, cmd) in own.into_iter().chain(table) {
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
            let reserved = 6.0 * widgets::ICON_BUTTON.x + 60.0;
            let chrome = 12.0 + 16.0 + 8.0 + 8.0 + widgets::SMALL_ICON_BUTTON.x + 22.0;
            let per_tab = (ui.available_width() - reserved) / self.tabs.len().max(1) as f32;
            let text_max = (per_tab - chrome).clamp(40.0, TAB_TEXT_MAX);
            for (index, tab) in self.tabs.iter().enumerate() {
                let active = index == self.active;
                let (select_it, close_it, response) = doc_tab(
                    ui, &palette, &tab.name, tab.dirty, active, text_max, tab.pinned,
                );
                if select_it {
                    select = Some(index);
                }
                if close_it && tab.pinned {
                    asked = Some((index, TabAction::Pin));
                } else if close_it {
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
                        (icon::PUSH_PIN, "Pin or Unpin", TabAction::Pin),
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
                let tip = "Markdown preview (Ctrl+Shift+V)";
                if self.is_markdown()
                    && widgets::toggle_button(ui, icon::EYE, tip, self.preview).clicked()
                {
                    clicked.push(Cmd::Preview);
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
            (TabAction::Pin, _) => {
                // Pinned tabs come first, in the order they were pinned.
                let front = self.tabs.get(self.active).map(|tab| tab.id);
                let mut tab = self.tabs.remove(index);
                tab.pinned = !tab.pinned;
                let pinned = self.tabs.iter().filter(|tab| tab.pinned).count();
                self.tabs.insert(pinned, tab);
                let front = front.and_then(|id| self.tabs.iter().position(|tab| tab.id == id));
                self.active = front.unwrap_or(self.active);
            }
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
                let (line, column) = self.caret_place.2;
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
                    if let Some(state) = self.servers.state(tab.language.name) {
                        let (label, tip) = match state {
                            servers::State::On(program) => (
                                format!("{} {program}", icon::PLUGS_CONNECTED),
                                "Language server".to_owned(),
                            ),
                            servers::State::Off(known, _) => (
                                format!("{} LSP off", icon::PLUGS),
                                format!("{} is not running: click to see why", known.program),
                            ),
                        };
                        let server = Button::new(dim(label))
                            .frame_when_inactive(false)
                            .stroke(Stroke::NONE);
                        if ui.add(server).on_hover_text(tip).clicked() {
                            clicked.push(Cmd::LanguageServer);
                        }
                    }
                    let keys = match (self.keys, self.vim.mode) {
                        (Keys::Default, _) => "",
                        (Keys::Emacs, _) => "Emacs",
                        (Keys::Vim, modal::Mode::Normal) => "-- NORMAL --",
                        (Keys::Vim, modal::Mode::Insert) => "-- INSERT --",
                        (Keys::Vim, modal::Mode::Visual) => "-- VISUAL --",
                        (Keys::Vim, modal::Mode::VisualLine) => "-- VISUAL LINE --",
                    };
                    // A command being typed after `:` shows as it is typed.
                    match self.vim.typing().filter(|_| self.keys == Keys::Vim) {
                        Some(typed) => {
                            ui.label(dim(format!(":{typed}")));
                        }
                        None if !keys.is_empty() => {
                            ui.label(dim(keys.to_owned()));
                        }
                        None => {}
                    }
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
                (Bottom::Response, "RESPONSE".to_owned()),
                (Bottom::Debug, "DEBUG".to_owned()),
                (Bottom::Tests, format!("TESTS  {}", self.tests.1.len())),
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
                        Bottom::Response => Cmd::Response,
                        Bottom::Debug => Cmd::DebugPanel,
                        Bottom::Tests => Cmd::TestsPanel,
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
                    Bottom::Tests => (
                        Cmd::TestsPanel,
                        &[(
                            icon::PLAY,
                            "Run all the tests and show what passed (Ctrl+Alt+T)",
                            Cmd::TestResults,
                        )],
                    ),
                    Bottom::Debug => (
                        Cmd::DebugPanel,
                        &[
                            (icon::STOP, "Stop (Shift+F5)", Cmd::StopDebug),
                            (icon::ARROW_LINE_UP, "Step out (Shift+F11)", Cmd::StepOut),
                            (icon::ARROW_LINE_DOWN, "Step into (F11)", Cmd::StepInto),
                            (
                                icon::ARROW_ELBOW_DOWN_RIGHT,
                                "Step over (F10)",
                                Cmd::StepOver,
                            ),
                            (icon::PLAY, "Continue (F5)", Cmd::Run),
                            (icon::BUG, "Debug this file (Ctrl+F5)", Cmd::Debug),
                        ],
                    ),
                    Bottom::Response => (
                        Cmd::Response,
                        &[(icon::PLAY, "Send the request at the caret (F5)", Cmd::Run)],
                    ),
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
            Bottom::Debug => self.debug_panel(ui),
            Bottom::Tests => self.tests_list(ui),
            Bottom::Response => {
                let shown = if self.requesting.is_some() {
                    "Waiting for the answer…"
                } else if self.response.is_empty() {
                    "In a .http file, write a request (GET https://…) and press F5 on it."
                } else {
                    &self.response
                };
                let mut picked = None;
                ui.horizontal_top(|ui| {
                    // The requests sent before: a click shows again what one answered.
                    if self.responses.len() > 1 {
                        ui.vertical(|ui| {
                            ui.set_width(300.0);
                            let list = ScrollArea::vertical().id_salt("responses");
                            list.auto_shrink(false).show(ui, |ui| {
                                for (index, (sent, _)) in self.responses.iter().enumerate() {
                                    let mut title: String = sent.chars().take(44).collect();
                                    if title.len() < sent.len() {
                                        title.push('…');
                                    }
                                    let chosen = index == self.response_at;
                                    let row = ui.selectable_label(chosen, title);
                                    if row.on_hover_text(sent).clicked() {
                                        picked = Some(index);
                                    }
                                }
                            });
                        });
                        ui.separator();
                    }
                    let answer = ScrollArea::both().id_salt("response");
                    answer.auto_shrink(false).show(ui, |ui| {
                        Frame::new()
                            .inner_margin(Margin::symmetric(12, 0))
                            .show(ui, |ui| {
                                let text = RichText::new(shown).monospace().size(13.0);
                                ui.add(egui::Label::new(text).selectable(true).extend());
                            });
                    });
                });
                if let Some((_, answered)) = picked.and_then(|index| self.responses.get(index)) {
                    self.response.clone_from(answered);
                    self.response_at = picked.unwrap_or(0);
                }
            }
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
            let toggles = [
                (icon::TEXT_AA, "Match case", &mut find.options.match_case),
                (icon::TEXT_T, "Whole word", &mut find.options.whole_word),
                (
                    icon::ASTERISK,
                    "Regular expression",
                    &mut find.options.regex,
                ),
            ];
            for (glyph, tip, on) in toggles {
                if widgets::toggle_button(ui, glyph, tip, *on).clicked() {
                    *on = !*on;
                }
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
        let Some(index) = self.tabs.iter().position(|tab| tab.id == self.left) else {
            ui.painter().rect_filled(ui.max_rect(), 0.0, palette.canvas);
            ui.centered_and_justified(|ui| {
                let hints = "Shift Shift  Search everywhere\n\nCtrl+Shift+P  Commands\n\n\
                             Ctrl+Shift+F  Find in files\n\nCtrl+J  Terminal\n\n\
                             F1  All shortcuts";
                ui.label(RichText::new(hints).size(15.0).color(palette.text_dim));
            });
            return;
        };
        self.editor_pane(ui, clicked, index, false);
    }

    /// The second editor, right of the first: the name of its file, then its text.
    fn split_area(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = Palette::of(ui);
        let shown = self
            .right
            .and_then(|id| self.tabs.iter().position(|tab| tab.id == id));
        let Some(index) = shown else {
            return;
        };
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            let name = RichText::new(&self.tabs[index].name).size(12.0);
            ui.label(name.color(palette.text_dim));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(4.0);
                if widgets::small_icon_button(ui, icon::X, "Close the split (Ctrl+\\)").clicked() {
                    clicked.push(Cmd::Split);
                }
            });
        });
        self.editor_pane(ui, clicked, index, true);
    }

    /// One editor, the `second` one or the first, showing tab `index`. What follows the
    /// caret (completions, signature, documentation, the page of a Markdown file) is for
    /// the editor that has the keyboard.
    fn editor_pane(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>, index: usize, second: bool) {
        // The same file in both editors: only one of them has the keyboard.
        let twin = self.right == Some(self.left);
        let front = index == self.active && (!twin || second == self.right_front);
        let font_size = self.font_size();
        let current_mark = self.current_match().filter(|_| front);
        let note = self.note.clone().filter(|_| front);
        let (marks, occurrences): (&[Match], &[Match]) = if front {
            (&self.find.matches, &self.occurrences.1)
        } else {
            (&[], &[])
        };
        let language = self.tabs[index].language.name;
        let snippets = self.snippets.get(language).map_or(&[][..], Vec::as_slice);
        let at = self.debug_at.as_ref();
        let stopped = at.filter(|(path, _)| self.tabs[index].path.as_deref() == Some(path));
        let stopped = stopped.map(|(_, line)| *line);
        let root = self.run_root().filter(|_| !self.coverage.is_empty());
        let coverage = match (&self.tabs[index].path, &root) {
            (Some(path), Some(root)) => godcode_core::coverage::of_file(&self.coverage, path, root)
                .map_or(&[][..], |file| &file.lines),
            _ => &[],
        };
        let tab = &mut self.tabs[index];
        // The view of this editor is the one put aside: it is brought in to be shown.
        let borrowed = twin && second != tab.in_second;
        if borrowed {
            tab.swap_views(ui.ctx());
        }
        if !front {
            // It takes the keyboard when clicked, not because it was just opened.
            tab.view.focus = false;
        }
        let unit = tab.indent.unit();
        let conditional: Vec<usize> = tab.conditions.keys().copied().collect();
        let config = editor::Config {
            language: tab.language,
            font_size,
            indent: &unit,
            marks,
            current_mark,
            squiggles: &tab.squiggles,
            // Typing moves the lines: the marks come back with the next save.
            changes: if tab.changes_at == tab.revision {
                &tab.changes
            } else {
                &[]
            },
            occurrences,
            wrap: tab.wrap,
            minimap: self.minimap,
            sticky: self.sticky,
            note: note.as_deref(),
            snippets,
            breakpoints: &tab.breakpoints,
            conditional: &conditional,
            stopped,
            coverage,
            hints: &tab.hints,
            folds: tab.folds.as_ref().map(|(stamp, list)| (*stamp, &**list)),
            revision: tab.revision,
            block_caret: front && self.keys == Keys::Vim && self.vim.mode != modal::Mode::Insert,
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
        // Lines typed or erased above a breakpoint take it along: it stays on its code.
        let grown = output.lines as isize - tab.lines as isize;
        let known = std::mem::replace(&mut tab.lines, output.lines) != 0;
        if let (true, true, Some(((line, _), _))) = (output.changed, known, output.places) {
            // The caret ends under what was typed, or where what was erased began.
            let edited = (line - 1).saturating_sub(grown.max(0) as usize);
            if grown != 0
                && move_stops(&mut tab.breakpoints, &mut tab.conditions, edited, grown)
                && let (Some(session), Some(path)) = (&mut self.debug, &tab.path)
            {
                session.set_breakpoints(path, &stops_of(tab));
            }
        }
        if let Some(selection) = output.selection {
            // Moved by hand: there is nothing to shrink back to any more.
            if selection != tab.selection {
                tab.grown.clear();
            }
            tab.selection = selection;
        }
        let gutter = output.gutter;
        let focused = output.focused;
        if let Some(line) = gutter {
            toggle_breakpoint_of(tab, &mut self.debug, line);
        }
        if !front {
            if focused {
                self.active = index;
                if twin {
                    self.right_front = second;
                }
            } else if borrowed {
                tab.swap_views(ui.ctx());
            }
            return;
        }
        self.top_line = output.top_line;
        self.caret_pos = output.caret_pos;
        // Where the caret is, as the editor knows it: not looked for in the text again.
        if let Some((start, end)) = output.places {
            let at = (tab.id, tab.revision, tab.selection.clone());
            self.caret_place = (at, start, end);
        }
        self.editor_focused = focused;
        let pointed = output.pointed;
        // After a letter, a dot or a colon, the language server says what can follow; after
        // a bracket or a comma, which argument is being written.
        let caret = tab.selection.start;
        let typed = output.changed && tab.selection.is_empty();
        // A completion just picked is not typing: asking again would have the server
        // forget the list it is about to be asked about.
        let typed = typed && tab.view.accepted.is_none();
        // The char before the caret is for the language server: looked for only when
        // something was typed in a file one of them knows.
        let served = tab
            .path
            .as_deref()
            .is_some_and(|path| self.servers.serves(path));
        let before = (typed && served)
            .then(|| {
                tab.text[..godcode_core::byte_index(&tab.text, caret)]
                    .chars()
                    .next_back()
            })
            .flatten();
        let completes =
            typed && before.is_some_and(|ch| ch.is_alphanumeric() || "_.:".contains(ch));
        let calls = typed && matches!(before, Some('(' | ','));
        if typed && before == Some(')') {
            self.signature = None;
        }
        let ctx = ui.ctx().clone();
        if completes || calls {
            let (id, revision) = (tab.id, tab.revision);
            self.sync_server(&ctx);
            let place = self.tabs.get(self.active).and_then(|tab| {
                let (line, column) = servers::position(&tab.text, caret);
                Some((tab.path.clone()?, line, column))
            });
            if let Some((path, line, column)) = place {
                if completes && let Some(asked) = self.servers.completion(&path, line, column) {
                    let completion = Asked::Completion {
                        tab: id,
                        revision,
                        caret,
                    };
                    self.asked.insert(asked, completion);
                }
                if calls && let Some(asked) = self.servers.signature(&path, line, column) {
                    self.asked.insert(asked, Asked::Signature(line));
                }
            }
        }
        self.import_for_accepted(&ctx);
        if let Some(at) = output.jump {
            self.go_to_declaration(&ctx, Some(at));
        }
        self.rest_pointer(&ctx, pointed);
    }

    /// A completion of the language server was just picked in the file in front: what
    /// else it changes (an import to add) is made, or asked for.
    fn import_for_accepted(&mut self, ctx: &Context) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let Some(picked) = tab.view.accepted.take() else {
            return;
        };
        let (Some(path), complete::Kind::Symbol) = (tab.path.clone(), picked.kind) else {
            return;
        };
        let served = self.served.iter();
        let Some(item) = served.into_iter().find(|item| item.label == picked.label) else {
            return;
        };
        let item = item.clone();
        // Asked before the server is told the word was written: it answers about the text
        // its list was made for, and fails on any other.
        let edits = self.servers.accepted(&path, &item);
        if !edits.is_empty() {
            self.apply_edits(ctx, &[godcode_lsp::FileEdits { path, edits }]);
        }
    }

    /// The file in front is Markdown: it has a page to show.
    fn is_markdown(&self) -> bool {
        self.tabs
            .get(self.active)
            .is_some_and(|tab| tab.language.name == "Markdown")
    }

    /// The Markdown file in front as a page, scrolling with its text.
    fn preview_panel(&mut self, ui: &mut Ui) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let key = hash_of((tab.id, tab.revision));
        if self.blocks.0 != key {
            self.blocks = (key, markdown::blocks(&tab.text));
        }
        // The view being scrolled leads, the other one follows it.
        let leads = ui.rect_contains_pointer(ui.max_rect())
            && ui.input(|input| input.smooth_scroll_delta.y != 0.0 || input.pointer.any_down());
        let mut page = ScrollArea::vertical()
            .id_salt("markdown-preview")
            .auto_shrink(false);
        if !leads {
            let starts = self.preview_starts.iter().copied();
            let wanted = preview::interpolate(starts, self.top_line);
            // Never past the last screenful of the page.
            let height = self.preview_starts.last().map_or(0.0, |(_, y)| *y);
            let furthest = (height - ui.available_height()).max(0.0);
            page = page.vertical_scroll_offset(wanted.min(furthest));
        }
        let (blocks, zoom) = (&self.blocks.1, self.zoom);
        let shown = page.show(ui, |ui| {
            Frame::new()
                .inner_margin(Margin::symmetric(24, 16))
                .show(ui, |ui| preview::show(ui, blocks, zoom))
                .inner
        });
        // The end of the text is the end of the page.
        let mut starts = shown.inner;
        let lines = tab.text.split('\n').count() as f32;
        starts.push((lines, shown.content_size.y));
        if leads {
            let lines = starts.iter().map(|(line, y)| (*y, *line));
            tab.view.scroll_to_line = Some(preview::interpolate(lines, shown.state.offset.y));
        }
        self.preview_starts = starts;
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
        if query.starts_with('#') {
            let found = self.project_symbols.iter().take(MAX_PICKS);
            return found.cloned().map(Pick::Place).collect();
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
        // The definitions of the whole project are the language server's to find.
        if let Some(wanted) = quick.query.strip_prefix('#')
            && wanted != self.symbols_asked
        {
            self.symbols_asked = wanted.to_owned();
            self.sync_server(ctx);
            let front = self.tabs.get(self.active);
            let path = front.and_then(|tab| tab.path.clone());
            let asked = path.and_then(|path| self.servers.symbols(&path, wanted));
            match asked {
                Some(id) => {
                    self.asked.insert(id, Asked::Symbols(wanted.to_owned()));
                }
                None => self.project_symbols.clear(),
            }
        }
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
                    .hint_text(
                        "Search everywhere    >command    :line    @symbol    #project symbol",
                    )
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
                        Pick::Place(named) => {
                            let file = named.place.path.file_name();
                            let file = file.map_or_else(String::new, |name| {
                                name.to_string_lossy().into_owned()
                            });
                            let detail =
                                format!("{}  {file}:{}", named.container, named.place.line + 1);
                            (with_icon(icon::CUBE, &named.name).into(), detail)
                        }
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
                    let files = !quick.query.starts_with(['>', ':', '@', '#']);
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
                // The keyboard goes back to the code the command acts on.
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.view.focus = true;
                }
                self.pending.push(spec.cmd);
                ctx.request_repaint();
                return;
            }
            Pick::Place(named) => {
                self.record_jump();
                let place = named.place;
                if self.open_file(ctx, &place.path)
                    && let Some(tab) = self.tabs.get_mut(self.active)
                {
                    let start = servers::index_of(&tab.text, place.line, place.column).0;
                    tab.select(ctx, start..start);
                }
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
            PromptKind::Condition(_) => ("Stop here when… (empty: always)", "Set"),
            PromptKind::Value(..) => ("New value", "Set"),
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
        } else if confirmed && let PromptKind::Condition(line) = prompt.kind {
            self.set_condition(line, prompt.name.trim());
        } else if confirmed && let PromptKind::Value(inside, name) = &prompt.kind {
            // What it had opened is asked for again with it: its parts may have changed.
            if let Some(session) = &mut self.debug {
                session.set(*inside, name, prompt.name.trim());
            }
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
            // Nothing on disk: the caller renames in the text, or tells the debugger.
            PromptKind::Word(_) | PromptKind::Condition(_) | PromptKind::Value(..) => Ok(None),
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
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut clicked = std::mem::take(&mut self.pending);
        self.poll();
        self.shortcuts(&ctx, &mut clicked);
        self.guard_close(&ctx);
        self.update_title(&ctx);
        self.refresh_find();
        self.refresh_problems();
        self.settle_panes();
        self.refresh_symbols();
        self.refresh_blame();
        // Left for another line, the call is no longer the one being written.
        if self
            .signature
            .as_ref()
            .is_some_and(|(line, ..)| *line != self.caret_line)
        {
            self.signature = None;
        }
        // Back from another program: it may have changed the files.
        let focused = ctx.input(|input| input.focused);
        if focused && !std::mem::replace(&mut self.was_focused, focused) {
            self.reload_from_disk(&ctx);
        }
        self.was_focused = focused;
        self.refresh_occurrences();
        self.save_idle(&ctx);
        self.sync_server(&ctx);
        self.poll_servers(&ctx);
        // After the servers were heard: one that just said what it can do is asked at once.
        self.ask_hints();
        self.ask_folds();
        self.poll_debug(&ctx);
        self.poll_tests();
        // The tab in front is the most recent of the recent files.
        if let Some(tab) = self.tabs.get_mut(self.active)
            && tab.used != self.clock
        {
            self.clock += 1;
            tab.used = self.clock;
        }
        let palette = Palette::of(ui);
        let plain = |fill, margin: Margin| Frame::new().fill(fill).inner_margin(margin);
        // Where the page shown inside the window goes, when its panel shows.
        let mut web_rect = None;

        Panel::top("menu-bar")
            .frame(plain(palette.surface, Margin::symmetric(8, 3)))
            .show_separator_line(false)
            .show(ui, |ui| self.menu_bar(ui, &mut clicked));
        if self.at_home() {
            CentralPanel::default()
                .frame(Frame::new().fill(palette.bg))
                .show(ui, |ui| self.welcome(ui, &mut clicked));
        } else {
            // In zen mode only the code shows.
            let zen = self.zen;
            if !zen {
                Panel::bottom("status")
                    .frame(plain(palette.bg, Margin::symmetric(12, 3)))
                    .show(ui, |ui| self.status_bar(ui, &mut clicked));
                Panel::left("activity-bar")
                    .resizable(false)
                    .exact_size(ACTIVITY_WIDTH)
                    .frame(plain(palette.surface, Margin::ZERO))
                    .show(ui, |ui| self.activity_bar(ui));
            }
            if self.sidebar && !zen {
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
            if self.bottom.is_some() && !zen {
                Panel::bottom("bottom-panel")
                    .resizable(true)
                    .default_size(240.0)
                    .size_range(120.0..=600.0)
                    .frame(plain(palette.bg, Margin::symmetric(0, 6)))
                    .show(ui, |ui| self.bottom_panel(ui, &mut clicked));
            }
            if !self.tabs.is_empty() && !zen {
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
            if self.web_open && !zen {
                Panel::right("web-preview")
                    .resizable(true)
                    .default_size(560.0)
                    .size_range(240.0..=1600.0)
                    .frame(plain(palette.bg, Margin::ZERO))
                    .show(ui, |ui| web_rect = self.web_panel(ui));
            }
            if self.preview && self.is_markdown() {
                Panel::right("markdown-preview")
                    .resizable(true)
                    .default_size(560.0)
                    .size_range(240.0..=1400.0)
                    .frame(plain(palette.bg, Margin::ZERO))
                    .show(ui, |ui| self.preview_panel(ui));
            }
            if self.right.is_some() && self.split_down {
                Panel::bottom("split-editor-down")
                    .resizable(true)
                    .default_size(ui.available_height() / 2.0)
                    .size_range(120.0..=2400.0)
                    .frame(plain(palette.bg, Margin::ZERO))
                    .show(ui, |ui| self.split_area(ui, &mut clicked));
            } else if self.right.is_some() {
                Panel::right("split-editor")
                    .resizable(true)
                    .default_size(ui.available_width() / 2.0)
                    .size_range(240.0..=2400.0)
                    .frame(plain(palette.bg, Margin::ZERO))
                    .show(ui, |ui| self.split_area(ui, &mut clicked));
            }
            CentralPanel::default()
                .frame(Frame::new().fill(palette.bg))
                .show(ui, |ui| self.editor_area(ui, &mut clicked));
        }
        self.follow_preview();
        self.place_web(&ctx, frame, web_rect);
        for cmd in clicked {
            self.run(&ctx, cmd);
        }
        self.closing_dialog(&ctx);
        self.prompt_dialog(&ctx);
        self.quick_dialog(&ctx);
        self.shortcuts_dialog(&ctx);
        self.documentation_window(&ctx);
        self.tip_window(&ctx);
        self.fixes_dialog(&ctx);
        self.signature_window(&ctx);
        self.diff_window(&ctx);
        self.branches_dialog(&ctx);
        self.versions_dialog(&ctx);
        self.themes_dialog(&ctx);
        self.tasks_dialog(&ctx);
        self.environments_dialog(&ctx);
        self.merge_window(&ctx);
        self.database_window(&ctx);
        self.log_dialog(&ctx);
        self.templates_dialog(&ctx);
        self.toasts.show(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let flag = |on: bool| if on { "1" } else { "0" }.to_owned();
        storage.set_string("dark_mode", flag(self.dark));
        storage.set_string("sidebar", flag(self.sidebar));
        storage.set_string("wrap", flag(self.wrap));
        storage.set_string("preview", flag(self.preview));
        storage.set_string("minimap", flag(self.minimap));
        storage.set_string("keys", (self.keys as u8).to_string());
        storage.set_string("hints", flag(self.hints));
        storage.set_string("environment", self.environment.clone().unwrap_or_default());
        let watched: Vec<&str> = self
            .watches
            .iter()
            .map(|(expression, _)| &**expression)
            .collect();
        storage.set_string("watches", watched.join("\n"));
        for tab in &self.tabs {
            keep_folds(&mut self.folds, tab);
        }
        storage.set_string("folds", folds_text(&self.folds));
        storage.set_string("sticky", flag(self.sticky));
        storage.set_string("autosave", flag(self.autosave));
        storage.set_string("format_on_save", flag(self.format_on_save));
        storage.set_string("blame", flag(self.blame));
        storage.set_string("scheme", self.scheme.to_string());
        // The themes of the user move in the list: theirs is known by its name.
        storage.set_string("scheme_name", theme::current().name.to_owned());
        let font = self.font.as_ref().map(|path| path.display().to_string());
        storage.set_string("font", font.unwrap_or_default());
        // What is open, to come back to it at the next start.
        let shown = |path: &Path| path.display().to_string();
        let folder = self.folder().map(|folder| shown(&folder));
        storage.set_string("session_folder", folder.unwrap_or_default());
        let files = self.tabs.iter().filter_map(|tab| tab.path.as_deref());
        let files: Vec<String> = files.map(shown).collect();
        storage.set_string("session_files", files.join("\n"));
        let front = self
            .tabs
            .get(self.active)
            .and_then(|tab| tab.path.as_deref());
        storage.set_string("session_front", front.map(shown).unwrap_or_default());
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

/// Set or remove the breakpoint of a line of `tab`, and tell the debugger when one runs.
fn toggle_breakpoint_of(
    tab: &mut Tab,
    session: &mut Option<godcode_lsp::dap::Session>,
    line: usize,
) {
    match tab.breakpoints.iter().position(|set| *set == line) {
        Some(at) => {
            tab.breakpoints.remove(at);
            tab.conditions.remove(&line);
        }
        None => tab.breakpoints.push(line),
    }
    if let (Some(session), Some(path)) = (session, &tab.path) {
        session.set_breakpoints(path, &stops_of(tab));
    }
}

/// The text grew by `grown` lines (or shrank) under the line `edited`: the breakpoints
/// below it, and their conditions, move as much. Those on lines that went join the line
/// where they went. Returns whether one moved.
fn move_stops(
    lines: &mut Vec<usize>,
    conditions: &mut std::collections::HashMap<usize, String>,
    edited: usize,
    grown: isize,
) -> bool {
    let moved = |line: usize| match line > edited {
        true => line.saturating_add_signed(grown).max(edited),
        false => line,
    };
    let before = lines.clone();
    lines.iter_mut().for_each(|line| *line = moved(*line));
    lines.sort_unstable();
    lines.dedup();
    let kept = std::mem::take(conditions);
    conditions.extend(
        kept.into_iter()
            .map(|(line, condition)| (moved(line), condition)),
    );
    *lines != before
}

/// The places the debugger stops at in `tab`, each with its condition when it has one.
fn stops_of(tab: &Tab) -> Vec<godcode_lsp::dap::Stop> {
    let stops = tab.breakpoints.iter();
    stops
        .map(|line| (*line, tab.conditions.get(line).cloned()))
        .collect()
}

/// A settings file of the user, by its name.
/// Read the themes of the user from their file; none when there is no file. What cannot
/// be read leaves the themes as they were and says why.
fn load_themes() -> Result<(), String> {
    let written = settings_file("themes.json").and_then(|file| std::fs::read_to_string(file).ok());
    let themes = written.map_or(Ok(Vec::new()), |text| theme::parse_user_themes(&text))?;
    theme::set_user_themes(themes);
    Ok(())
}

/// A file with conflicts laid out in three columns: mine, the result, theirs.
struct Merging {
    /// The tab the result goes to.
    tab: u64,
    parts: Vec<godcode_core::merge::Part>,
    /// What was chosen for each conflict, in their order.
    choices: Vec<Option<godcode_core::merge::Choice>>,
}

/// How many requests are kept with what they answered.
const RESPONSES_KEPT: usize = 20;

/// Rows of a table shown at a time.
const DATABASE_PAGE: usize = 200;
/// Chars of a value shown in the grid.
const DATABASE_CELL: usize = 120;

/// A SQLite file being looked into: one page of one of its tables.
struct Browsing {
    name: String,
    database: godcode_core::sqlite::Database,
    table: usize,
    page: usize,
    /// How many rows the table has, and those of the page.
    count: usize,
    rows: Vec<Vec<godcode_core::sqlite::Value>>,
    /// Why the table could not be read.
    error: Option<String>,
    /// Its write-ahead log holds changes that are not read here.
    behind: bool,
}

impl Browsing {
    /// Read the page of the table on show.
    fn read(&mut self) {
        let (database, start) = (&self.database, self.page * DATABASE_PAGE);
        let read = database.tables().get(self.table).map(|table| {
            let rows = database.rows(table, start, DATABASE_PAGE)?;
            Ok((database.count(table)?, rows))
        });
        (self.count, self.rows, self.error) = match read {
            Some(Ok((count, rows))) => (count, rows, None),
            Some(Err(err)) => (0, Vec::new(), Some(err)),
            None => (0, Vec::new(), None),
        };
    }
}

fn settings_file(name: &str) -> Option<PathBuf> {
    Some(eframe::storage_dir(APP_NAME)?.join(name))
}

/// A shortcut as it is written: `Ctrl+Shift+K`.
fn parse_shortcut(written: &str) -> Option<(Modifiers, Key)> {
    let mut modifiers = Modifiers::NONE;
    let mut key = None;
    for part in written.split('+').map(str::trim) {
        match part.to_lowercase().as_str() {
            "ctrl" | "cmd" => modifiers |= Modifiers::COMMAND,
            "shift" => modifiers |= Modifiers::SHIFT,
            "alt" => modifiers |= Modifiers::ALT,
            _ => key = Key::from_name(part),
        }
    }
    Some((modifiers, key?))
}

/// The shortcuts of the user: `{ "Reformat Code": "Ctrl+Alt+F" }`, a command by its name
/// in the menus. Those with more modifier keys come first, as they must be tried first.
fn load_keymap() -> Vec<(Modifiers, Key, Cmd)> {
    let text = settings_file("keymap.json").and_then(|file| std::fs::read_to_string(file).ok());
    let parsed: Option<serde_json::Value> = text.and_then(|text| serde_json::from_str(&text).ok());
    let written = parsed.as_ref().and_then(|parsed| parsed.as_object());
    let mut keymap: Vec<(Modifiers, Key, Cmd)> = written
        .into_iter()
        .flatten()
        .filter_map(|(label, shortcut)| {
            let named = |spec: &&Spec| spec.label.trim_end_matches('…').eq_ignore_ascii_case(label);
            let (modifiers, key) = parse_shortcut(shortcut.as_str()?)?;
            Some((modifiers, key, COMMANDS.iter().find(named)?.cmd))
        })
        .collect();
    let held = |modifiers: &Modifiers| {
        u8::from(modifiers.alt) + u8::from(modifiers.shift) + u8::from(modifiers.command)
    };
    keymap.sort_by_key(|(modifiers, ..)| std::cmp::Reverse(held(modifiers)));
    keymap
}

/// The debug adapter and the launch settings the user wrote for `language`, `${file}` and
/// `${folder}` standing for the file in front and its project.
fn debug_settings(
    language: &str,
    file: &Path,
    folder: &Path,
) -> Option<(Vec<String>, serde_json::Value)> {
    let text = std::fs::read_to_string(settings_file("debug.json")?).ok()?;
    // Written into JSON text: back slashes and quotes of the paths must be escaped.
    let quoted = |path: &Path| {
        let json = serde_json::to_string(&path.display().to_string()).unwrap_or_default();
        json.trim_matches('"').to_owned()
    };
    let text = text
        .replace("${file}", &quoted(file))
        .replace("${folder}", &quoted(folder));
    let parsed: serde_json::Value = serde_json::from_str(&text).ok()?;
    let entry = parsed.get(language)?;
    let adapter = entry["adapter"].as_array()?.iter();
    let adapter: Vec<String> = adapter
        .filter_map(|part| Some(part.as_str()?.to_owned()))
        .collect();
    let mut launch = entry["launch"].clone();
    if launch.get("request").is_none() {
        launch["request"] = "launch".into();
    }
    (!adapter.is_empty()).then_some((adapter, launch))
}

/// The file the snippets of the user are written in.
fn snippets_file() -> Option<PathBuf> {
    Some(eframe::storage_dir(APP_NAME)?.join("snippets.json"))
}

/// The snippets of the user, by language: `{ "Rust": { "dbg": "dbg!($0);" } }`. None when
/// the file is missing or is not that.
fn load_snippets() -> std::collections::HashMap<String, Vec<(String, String)>> {
    let text = snippets_file().and_then(|file| std::fs::read_to_string(file).ok());
    let parsed: Option<serde_json::Value> = text.and_then(|text| serde_json::from_str(&text).ok());
    let languages = parsed.as_ref().and_then(|parsed| parsed.as_object());
    let read = |(language, snippets): (&String, &serde_json::Value)| {
        let snippets = snippets.as_object()?.iter();
        let snippets =
            snippets.filter_map(|(label, body)| Some((label.clone(), body.as_str()?.to_owned())));
        Some((language.clone(), snippets.collect()))
    };
    languages.into_iter().flatten().filter_map(read).collect()
}

/// Where the versions of the file at `path` are kept.
fn history_dir(path: &Path) -> Option<PathBuf> {
    let name = format!("{:016x}", hash_of(path));
    Some(eframe::storage_dir(APP_NAME)?.join("history").join(name))
}

/// Keep `bytes`, what was just saved to `path`, as a version to come back to.
fn keep_version(path: &Path, bytes: &[u8]) {
    let Some(dir) = history_dir(path) else {
        return;
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let seconds = now.map_or(0, |since| since.as_secs());
    // The history is a comfort: failing to write it must not get in the way of saving.
    if std::fs::create_dir_all(&dir).is_err()
        || std::fs::write(dir.join(format!("{seconds:012}.txt")), bytes).is_err()
    {
        return;
    }
    let mut kept: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    kept.sort();
    for old in kept.iter().rev().skip(MAX_VERSIONS) {
        let _ = std::fs::remove_file(old);
    }
}

/// The versions kept of the file at `path`, latest first, each with how old it is.
fn versions_of(path: &Path) -> Vec<(String, PathBuf)> {
    let Some(dir) = history_dir(path) else {
        return Vec::new();
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let now = now.map_or(0, |since| since.as_secs());
    let mut kept: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    kept.sort();
    kept.reverse();
    let labelled = kept.into_iter().filter_map(|file| {
        let saved: u64 = file.file_stem()?.to_str()?.parse().ok()?;
        let age = now.saturating_sub(saved);
        let label = match age {
            0..60 => "a moment ago".to_owned(),
            60..3600 => format!("{} min ago", age / 60),
            3600..86_400 => format!("{} h ago", age / 3600),
            _ => format!("{} days ago", age / 86_400),
        };
        Some((label, file))
    });
    labelled.collect()
}

/// A list to pick from, with a field to narrow it when `typed` is given. `None` while it
/// is open; then the row picked, or `None` when it was closed without picking.
fn list_dialog(
    ctx: &Context,
    title: &str,
    rows: &[String],
    selected: &mut usize,
    typed: Option<&mut String>,
) -> Option<Option<usize>> {
    list_dialog_as(ctx, title, rows, selected, typed, false)
}

/// A list to pick from. `drawn`: its rows are a drawing in letters, shown wide and in
/// letters of one width so that it lines up.
fn list_dialog_as(
    ctx: &Context,
    title: &str,
    rows: &[String],
    selected: &mut usize,
    typed: Option<&mut String>,
    drawn: bool,
) -> Option<Option<usize>> {
    let (up, down, enter) = ctx.input_mut(|input| {
        (
            input.consume_key(Modifiers::NONE, Key::ArrowUp),
            input.consume_key(Modifiers::NONE, Key::ArrowDown),
            input.consume_key(Modifiers::NONE, Key::Enter),
        )
    });
    let last = rows.len().saturating_sub(1);
    *selected = (*selected + usize::from(down)).min(last);
    *selected = selected.saturating_sub(usize::from(up));
    let mut chosen = (enter && !rows.is_empty()).then_some(*selected);
    let response = Modal::new(Id::new(title)).show(ctx, |ui| {
        ui.set_width(if drawn { 860.0 } else { 520.0 });
        ui.heading(title);
        ui.add_space(6.0);
        if let Some(typed) = typed {
            let field = ui.add(
                TextEdit::singleline(typed)
                    .margin(crate::widgets::FIELD_MARGIN)
                    .hint_text("Type to narrow the list, or a new name")
                    .desired_width(f32::INFINITY),
            );
            field.request_focus();
            if field.changed() {
                *selected = 0;
            }
            ui.add_space(6.0);
        }
        ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            for (index, row) in rows.iter().enumerate() {
                let text = match drawn {
                    true => RichText::new(row.as_str()).monospace().size(12.5),
                    false => RichText::new(row.as_str()),
                };
                let button = Button::selectable(index == *selected, text)
                    .frame_when_inactive(index == *selected)
                    .stroke(Stroke::NONE)
                    .wrap_mode(egui::TextWrapMode::Truncate)
                    .min_size(vec2(ui.available_width(), 28.0));
                if ui.add(button).clicked() {
                    chosen = Some(index);
                }
            }
        });
    });
    match chosen {
        Some(row) => Some(Some(row)),
        None if response.should_close() => Some(None),
        None => None,
    }
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
    pinned: bool,
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
    let (glyph, tip) = if pinned {
        (icon::PUSH_PIN, "Pinned: click to unpin")
    } else {
        (icon::X, "Close")
    };
    let close = ui
        .put(close_rect, widgets::icon_widget(glyph, 13.0))
        .on_hover_text(tip);
    let response = response.on_hover_text(title);
    (
        response.clicked(),
        close.clicked() || response.middle_clicked(),
        response,
    )
}

/// The text of a file that is not open, for a language server to change. One that is not
/// there yet is empty: the server wants it made (code moved to a new file). `None` when it
/// is there but is not text that can be read.
fn text_on_disk(path: &Path) -> Option<Decoded> {
    match std::fs::read(path) {
        Ok(bytes) => godcode_core::decode(&bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Some(Decoded {
            text: String::new(),
            ending: LineEnding::Lf,
            bom: false,
        }),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakpoints_stay_on_their_code_when_lines_come_and_go() {
        let mut lines = vec![2, 5, 9];
        let mut conditions = std::collections::HashMap::from([(5, "i > 3".to_owned())]);
        // Two lines typed under line 3: what is below goes down, what is above stays.
        assert!(move_stops(&mut lines, &mut conditions, 3, 2));
        assert_eq!(lines, [2, 7, 11]);
        assert_eq!(conditions.get(&7).map(String::as_str), Some("i > 3"));
        // Lines 5 to 8 erased: the breakpoint that was among them waits where they were.
        assert!(move_stops(&mut lines, &mut conditions, 4, -4));
        assert_eq!(lines, [2, 4, 7]);
        // Nothing below the edit: nothing moves.
        assert!(!move_stops(&mut lines, &mut conditions, 20, 3));
    }

    #[test]
    fn the_tests_of_a_project_are_found_file_by_file() {
        let root = std::env::temp_dir().join(format!("godcode-tests-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).expect("folder");
        std::fs::write(
            root.join("src/sum.rs"),
            "fn add() {}\n\n#[test]\nfn adds() {}\n\n#[test]\nfn breaks() {}\n",
        )
        .expect("file");
        std::fs::write(root.join("test_app.py"), "def test_runs():\n    pass\n").expect("file");
        std::fs::write(root.join("notes.md"), "# no test here\n").expect("file");
        let listed = ["src/sum.rs", "test_app.py", "notes.md", "gone.rs"].map(str::to_owned);
        let found = tests_under(&root, &listed);
        let names = |file: &FileTests| file.1.iter().map(|test| test.1.clone()).collect::<Vec<_>>();
        // The files that have tests, with them; a file without, or that is gone, not at all.
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(
            (found[0].0.as_str(), names(&found[0])),
            ("src/sum.rs", vec!["adds".to_owned(), "breaks".to_owned()])
        );
        assert_eq!(found[0].1[0].0, 3);
        assert_eq!(names(&found[1]), ["test_runs"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_font_file_is_told_from_another() {
        assert!(widgets::is_font(&[0, 1, 0, 0, 9, 9]));
        assert!(widgets::is_font(b"OTTO...") && widgets::is_font(b"ttcf..."));
        assert!(!widgets::is_font(b"%PDF-1.7") && !widgets::is_font(&[]));
        // A font of this system, when there is one, is one.
        if let Ok(bytes) = std::fs::read("C:/Windows/Fonts/consola.ttf") {
            assert!(widgets::is_font(&bytes));
            // And code is drawn with it without a hitch.
            let ctx = Context::default();
            widgets::install_fonts(&ctx, Some(bytes));
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.label(RichText::new("let code = 1;").monospace());
            });
        }
    }

    #[test]
    fn a_typed_address_is_made_whole() {
        let whole = |typed: &str| whole_address(typed);
        assert_eq!(
            whole(" localhost:5173 ").as_deref(),
            Some("http://localhost:5173")
        );
        assert_eq!(
            whole("https://example.org/a").as_deref(),
            Some("https://example.org/a")
        );
        assert_eq!(
            whole("file:///C:/site/index.html").as_deref(),
            Some("file:///C:/site/index.html")
        );
        assert_eq!(whole("  "), None);
    }

    #[test]
    fn folded_blocks_are_written_and_read_back() {
        let mut folds = std::collections::HashMap::new();
        folds.insert(PathBuf::from("C:/code/a b.rs"), vec![3, 12]);
        let text = folds_text(&folds);
        assert_eq!(text, "C:/code/a b.rs\t3,12");
        assert_eq!(folds_of(&text), folds);
        // What is not a file and its lines is left out.
        let read = folds_of("no tab here\nb.rs\t4,x,9\n");
        assert_eq!(read.len(), 1);
        assert_eq!(read[&PathBuf::from("b.rs")], [4, 9]);
    }

    #[test]
    fn a_file_a_server_changes_may_not_exist_yet() {
        let folder = std::env::temp_dir().join(format!("godcode-edits-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("folder");
        let missing = text_on_disk(&folder.join("new.ts")).expect("an empty text");
        assert_eq!((missing.text.as_str(), missing.bom), ("", false));
        std::fs::write(
            folder.join("old.ts"),
            "let a;
",
        )
        .expect("file");
        let found = text_on_disk(&folder.join("old.ts")).expect("its text");
        assert_eq!(
            found.text,
            "let a;
"
        );
        // Not text: left alone rather than overwritten.
        std::fs::write(folder.join("data.bin"), [0xff, 0xfe, 0x00, 0xd8]).expect("file");
        assert!(text_on_disk(&folder.join("data.bin")).is_none());
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn shortcuts_are_read_as_written() {
        let both = Modifiers::COMMAND | Modifiers::SHIFT;
        assert_eq!(parse_shortcut("Ctrl+Shift+K"), Some((both, Key::K)));
        assert_eq!(
            parse_shortcut("f5"),
            Key::from_name("f5").map(|key| (Modifiers::NONE, key))
        );
        assert_eq!(
            parse_shortcut("Alt + ArrowUp"),
            Some((Modifiers::ALT, Key::ArrowUp))
        );
        assert_eq!(parse_shortcut("Ctrl+"), None);
    }

    #[test]
    fn a_file_in_two_editors_has_two_views() {
        let ctx = Context::default();
        let decoded = Decoded {
            text: "one\ntwo\nthree".to_owned(),
            ending: LineEnding::Lf,
            bom: false,
        };
        let mut tab = Tab::new(7, "a.txt".to_owned(), None, decoded);
        tab.select(&ctx, 4..7);
        let first = tab.editor_id();

        // The second view starts where the first is, then goes its own way.
        tab.swap_views(&ctx);
        assert!(tab.in_second && tab.editor_id() != first);
        assert_eq!((tab.selection.clone(), tab.view.scroll_to), (4..7, Some(4)));
        tab.select(&ctx, 8..13);
        tab.swap_views(&ctx);
        assert_eq!((tab.editor_id(), tab.selection.clone()), (first, 4..7));

        // The text is shortened in the first: the second no longer points past its end.
        tab.text.truncate(5);
        tab.touched();
        tab.select(&ctx, 4..5);
        tab.swap_views(&ctx);
        assert_eq!(tab.selection, 5..5);
        let kept = egui::TextEdit::load_state(&ctx, tab.editor_id())
            .and_then(|state| state.cursor.char_range())
            .map(|range| (range.primary.index.0, range.secondary.index.0));
        assert_eq!(kept, Some((5, 5)));
        // Nothing changed since: the first comes back as it was left.
        tab.swap_views(&ctx);
        assert_eq!((tab.in_second, tab.selection.clone()), (false, 4..5));
    }

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
