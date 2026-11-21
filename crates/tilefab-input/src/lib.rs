//! Prompt line editor: readline-style editing, with a horizontally scrolling line.
//!
//! [`LineEditor`] turns key events into edits of a single line (Unicode-aware: the cursor
//! moves by grapheme), with a kill ring, undo / redo and a table of key bindings
//! ([`Keymap`], Emacs defaults). It draws the prompt and the line into a
//! [`tilefab_core::Buffer`] and returns the cursor cell, so that it works with any
//! terminal layer.
//!
//! This crate does no terminal input/output: it consumes `crossterm` key events and
//! draws into a buffer. It depends neither on the terminal (`tilefab-cli`) nor on the
//! widget library (`tilefab-widget`).

// The entire public API is documented; the lint catches omissions.
#![warn(missing_docs)]

mod buffer;
mod completion;
mod editor;
mod hint;
mod history;
mod keymap;
mod kill_ring;
mod render;
mod search;
mod undo;

pub use completion::{Candidate, Completer, Completions};
pub use editor::{EditorConfig, EditorEvent, LineEditor};
pub use hint::{Hinter, HistoryHinter};
pub use history::History;
pub use keymap::{Command, KeyBinding, Keymap};
pub use render::EditorStyles;
