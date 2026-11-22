//! Small TUI library built on crossterm alone, with an inline
//! viewport whose height can change from one frame to the next.
//!
//! Façade: re-exports the crates of the workspace under a single path, for applications.
//!
//! - [`tilefab_core`]: cells, buffer, styles, geometry, styled text, widget traits;
//! - [`tilefab_cli`]: crossterm backend, terminal, inline viewport;
//! - `tilefab_widget` (feature `widget`, on by default): gauges, blocks, lists, paragraphs;
//! - `tilefab_input` (feature `input`, on by default): prompt line editor (its `Command`
//!   is re-exported as `EditorCommand`).

// The entire public API is documented; the lint catches omissions.
#![warn(missing_docs)]

pub use tilefab_cli::{
    Backend, ClearType, Frame, MAX_PENDING_LINES, ScrollMode, Terminal, TerminalGuard,
    TerminalOptions, Viewport, install_fatal_panic_hook, install_panic_hook, restore, viewport,
};
pub use tilefab_core::{
    Alignment, Buffer, Cell, Color, Constraint, Direction, Layout, Line, Margin, Modifier,
    Position, Rect, Size, Span, StatefulWidget, Style, Stylize, Text, Widget, convert,
};
#[cfg(feature = "input")]
pub use tilefab_input::{
    Candidate, Command as EditorCommand, Completer, Completions, EditorConfig, EditorEvent,
    EditorStyles, Hinter, History, HistoryHinter, KeyBinding, Keymap, LineEditor,
};
#[cfg(feature = "widget")]
pub use tilefab_widget::{
    Block, BorderType, Borders, Clear, Gauge, LineGauge, List, ListItem, ListState, Padding,
    Paragraph, Wrap,
};
