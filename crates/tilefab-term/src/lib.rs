//! Terminal side: crossterm backend, double-buffered terminal, inline viewport,
//! initialization and restoration.
//!
//! Depends only on [`tilefab_core`]: it draws any [`Widget`](tilefab_core::Widget) through
//! [`Frame`], without knowing the widget library.

// The entire public API is documented; the lint catches omissions.
#![warn(missing_docs)]

pub mod backend;
pub mod init;
pub mod terminal;
pub mod viewport;

#[cfg(test)]
mod inline_tests;
#[cfg(test)]
mod sim;

pub use backend::{Backend, ClearType};
pub use init::{TerminalGuard, install_fatal_panic_hook, install_panic_hook, restore};
pub use terminal::{Frame, MAX_PENDING_LINES, Terminal};
pub use viewport::{ScrollMode, TerminalOptions, Viewport};
