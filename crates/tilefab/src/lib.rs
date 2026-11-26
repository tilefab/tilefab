// The crate documentation (the docs.rs front page) is README.md, also the crates.io page:
// one text for both, and its example is compiled as a doctest. That example needs the
// `widget` feature; without it, a short description stands in.
#![cfg_attr(feature = "widget", doc = include_str!("../README.md"))]
#![cfg_attr(
    not(feature = "widget"),
    doc = "Small TUI library on crossterm with a variable-height inline viewport (built \
           without the `widget` feature: see README.md for the full introduction)."
)]
// The entire public API is documented; the lint catches omissions.
#![warn(missing_docs)]

/// The crossterm version tilefab is built on, for key events (`LineEditor::handle_key`,
/// `KeyBinding`) and terminal queries: use it instead of a separate `crossterm` dependency,
/// so that both always match.
pub use crossterm;

pub use tilefab_core::{
    Alignment, Buffer, Cell, Color, Constraint, Direction, Layout, Line, Margin, Modifier,
    Position, Rect, Size, Span, StatefulWidget, Style, Stylize, Text, Widget, convert,
    grapheme_width,
};
#[cfg(feature = "input")]
pub use tilefab_input::{
    Candidate, Command as EditorCommand, Completer, Completions, EditorConfig, EditorEvent,
    EditorStyles, Hinter, History, HistoryHinter, KeyBinding, Keymap, LineEditor,
};
pub use tilefab_term::{
    Backend, ClearType, Frame, MAX_PENDING_LINES, ScrollMode, Terminal, TerminalGuard,
    TerminalOptions, Viewport, install_fatal_panic_hook, install_panic_hook, restore, viewport,
};
#[cfg(feature = "widget")]
pub use tilefab_widget::{
    BuildError, Entry, EntryProblem, LineWidget, LineWidgetBuilder, ListName, TimeField,
    TokenProblem, UpdateError, ValueShape, WallTime, Warning,
};
