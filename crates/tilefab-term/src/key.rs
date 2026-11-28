//! Simple keyboard input for a loop that draws between key presses: [`poll_key`] waits for
//! the next key press with a timeout, and [`is_ctrl_c`] recognizes the usual way to quit.
//!
//! Raw mode turns the terminal's signals off: Ctrl+C no longer stops the program, it arrives
//! as a key, and the application must handle it.
//!
//! These helpers read crossterm's global input queue on the calling thread. An application
//! that reads keys on another thread, or needs pastes and resizes as events, uses
//! `crossterm::event::{poll, read}` directly: the guide of the `tilefab` crate shows how.

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// Wait at most `timeout` for a key press, and return it; `None` when the time is up.
///
/// Only presses (and repeats, a key held down) are returned: key releases, which some
/// terminals report, are skipped, so that each key counts once. The other events (paste,
/// resize, focus, mouse) are dropped: `Terminal::draw` checks the screen size by itself.
/// A timeout of zero checks without waiting.
///
/// ```no_run
/// use std::time::Duration;
/// use tilefab_term::{is_ctrl_c, poll_key};
///
/// loop {
///     // … draw a frame …
///     if let Some(key) = poll_key(Duration::from_millis(50))? {
///         if is_ctrl_c(&key) {
///             break;
///         }
///     }
/// }
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// Returns the error of reading the terminal input.
pub fn poll_key(timeout: Duration) -> io::Result<Option<KeyEvent>> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if !event::poll(remaining)? {
            return Ok(None);
        }
        if let Event::Key(key) = event::read()?
            && key.kind != KeyEventKind::Release
        {
            return Ok(Some(key));
        }
        if remaining.is_zero() {
            return Ok(None);
        }
    }
}

/// True for Ctrl+C, which raw mode delivers as a key instead of stopping the program.
#[must_use]
pub fn is_ctrl_c(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_c_is_recognized_with_or_without_other_modifiers() {
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        assert!(is_ctrl_c(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        assert!(is_ctrl_c(&key(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )));
        assert!(!is_ctrl_c(&key(KeyCode::Char('c'), KeyModifiers::NONE)));
        assert!(!is_ctrl_c(&key(KeyCode::Char('d'), KeyModifiers::CONTROL)));
    }
}
