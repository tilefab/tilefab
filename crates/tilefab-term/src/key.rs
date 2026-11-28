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
/// A timeout of zero checks without waiting; a timeout too long for the clock
/// (`Duration::MAX`) waits for the next key press without a limit.
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
    poll_key_with(timeout, event::poll, event::read)
}

/// [`poll_key`] with `poll` and `read` given (crossterm's, or a test reader).
///
/// A timeout too long for the clock (`Duration::MAX`, for instance) has no deadline: each
/// wait is that timeout again, so it waits for the next key press without a limit.
fn poll_key_with(
    timeout: Duration,
    mut poll: impl FnMut(Duration) -> io::Result<bool>,
    mut read: impl FnMut() -> io::Result<Event>,
) -> io::Result<Option<KeyEvent>> {
    // `Instant + Duration` panics when the sum does not fit: no deadline then.
    let deadline = Instant::now().checked_add(timeout);
    loop {
        let remaining = deadline.map_or(timeout, |deadline| {
            deadline.saturating_duration_since(Instant::now())
        });
        if !poll(remaining)? {
            return Ok(None);
        }
        if let Event::Key(key) = read()?
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
    use std::collections::VecDeque;

    use super::*;

    /// A test input: the events `poll` reports ready, then nothing (`poll` gives `false`).
    /// Returns the result of `poll_key_with` and the timeouts `poll` was given.
    fn run(timeout: Duration, events: Vec<Event>) -> (Option<KeyEvent>, Vec<Duration>) {
        let events = std::cell::RefCell::new(VecDeque::from(events));
        let mut waits = Vec::new();
        let key = poll_key_with(
            timeout,
            |wait| {
                waits.push(wait);
                Ok(!events.borrow().is_empty())
            },
            || {
                Ok(events
                    .borrow_mut()
                    .pop_front()
                    .expect("read only after a ready poll"))
            },
        )
        .unwrap();
        (key, waits)
    }

    fn press(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    fn release(c: char) -> Event {
        let mut key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        Event::Key(key)
    }

    #[test]
    fn a_press_is_returned_and_releases_and_other_events_are_skipped() {
        let (key, _) = run(
            Duration::from_secs(5),
            vec![
                release('a'),
                Event::FocusGained,
                Event::Resize(10, 5),
                press('b'),
            ],
        );
        assert_eq!(key.map(|key| key.code), Some(KeyCode::Char('b')));
    }

    #[test]
    fn no_event_before_the_timeout_gives_none() {
        let (key, waits) = run(Duration::from_millis(20), vec![]);
        assert_eq!(key, None);
        assert_eq!(waits.len(), 1);
        assert!(waits[0] <= Duration::from_millis(20));
    }

    #[test]
    fn a_zero_timeout_checks_once_without_waiting() {
        let (key, waits) = run(Duration::ZERO, vec![release('a'), press('b')]);
        assert_eq!(key, None, "a zero timeout reads one ready event only");
        assert_eq!(waits, [Duration::ZERO]);
    }

    #[test]
    fn a_timeout_too_long_for_the_clock_waits_without_a_deadline() {
        // `Instant::now() + Duration::MAX` would panic.
        let (key, waits) = run(Duration::MAX, vec![release('a'), press('b')]);
        assert_eq!(key.map(|key| key.code), Some(KeyCode::Char('b')));
        assert!(waits.iter().all(|&wait| wait == Duration::MAX), "{waits:?}");
    }

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
