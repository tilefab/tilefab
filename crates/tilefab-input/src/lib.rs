//! Prompt input line: text editing, cursor and horizontal scrolling.
//!
//! For now a thin layer over the `tui-input` crate, so that the rest of the code does not
//! depend on its API and an in-house implementation can replace it later.
//!
//! Like the widgets, this module does no input/output: it consumes key events and
//! exposes what is needed to draw the line (value, cursor column, scroll offset).
//! It depends neither on the terminal (`tilefab-cli`) nor on the widget library
//! (`tilefab-widget`).

// The entire public API is documented; the lint catches omissions.
#![warn(missing_docs)]

mod buffer;
mod editor;
mod keymap;
mod kill_ring;
mod undo;

pub use editor::{EditorConfig, EditorEvent, LineEditor};
pub use keymap::Command;

use crossterm::event::Event;
use tui_input::backend::crossterm::EventHandler;

/// A single-line text input with shell-like editing keys.
///
/// Handled keys: printable characters, Backspace/Delete, arrows, Home/End,
/// Ctrl+A/E (line start/end), Ctrl+W (previous word), Ctrl+U (line start)…
///
/// ```
/// use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
/// use tilefab_input::InputLine;
///
/// let mut input = InputLine::default();
/// for c in "hi".chars() {
///     input.handle_event(&Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
/// }
/// assert_eq!(input.value(), "hi");
/// assert_eq!(input.visual_cursor(), 2);
/// assert_eq!(input.value_and_reset(), "hi");
/// assert_eq!(input.value(), "");
/// ```
#[derive(Debug, Default, Clone)]
pub struct InputLine {
    inner: tui_input::Input,
}

impl InputLine {
    /// The current text.
    #[must_use]
    pub fn value(&self) -> &str {
        self.inner.value()
    }

    /// Take the current text and clear the line.
    pub fn value_and_reset(&mut self) -> String {
        self.inner.value_and_reset()
    }

    /// Cursor position in display columns from the start of the text
    /// (wide characters count for two).
    #[must_use]
    pub fn visual_cursor(&self) -> usize {
        self.inner.visual_cursor()
    }

    /// Number of columns to shift the text left so that the cursor stays visible
    /// in a field `width` columns wide.
    #[must_use]
    pub fn visual_scroll(&self, width: usize) -> usize {
        self.inner.visual_scroll(width)
    }

    /// Apply a terminal event (editing key, cursor move). Returns `true` if the text
    /// or the cursor changed; events it does not handle are ignored.
    pub fn handle_event(&mut self, event: &Event) -> bool {
        self.inner.handle_event(event).is_some()
    }
}
