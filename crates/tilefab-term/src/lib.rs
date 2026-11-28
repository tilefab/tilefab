//! Terminal side: crossterm backend, double-buffered terminal, inline viewport,
//! initialization and restoration.
//!
//! Depends only on [`tilefab_core`]: it draws any [`Widget`](tilefab_core::Widget) through
//! [`Frame`], without knowing the widget library.

// The entire public API is documented; the lint catches omissions.
#![warn(missing_docs)]

pub mod backend;
mod default_terminal;
pub mod init;
mod key;
pub mod terminal;
pub mod viewport;

#[cfg(test)]
mod inline_tests;
#[cfg(test)]
mod sim;

pub use backend::{Backend, ClearType};
pub use default_terminal::{DefaultTerminal, init_fullscreen, init_inline, init_with_options};
pub use init::{TerminalGuard, install_fatal_panic_hook, install_panic_hook, restore};
pub use key::{is_ctrl_c, poll_key};
pub use terminal::{Frame, MAX_PENDING_LINES, MAX_SCREEN_HEIGHT, MAX_SCREEN_WIDTH, Terminal};
pub use viewport::{ScrollMode, TerminalOptions, Viewport};

// The terminal can move to another thread or task with a `Send` writer.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Terminal<std::io::Stdout>>();
    assert_send::<Terminal<Vec<u8>>>();
};
