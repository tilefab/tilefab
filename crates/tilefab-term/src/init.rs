//! Terminal setup and restoration: raw mode, alternate screen, cursor, bracketed paste.
//!
//! The [`TerminalGuard`] is a guard: it enables the required modes on creation and **undoes
//! everything in its `Drop`**, including on errors or early returns. For panics, which
//! do not always run `Drop` in time (the report is displayed *before* stack unwinding),
//! [`install_panic_hook`] (or [`install_fatal_panic_hook`], which also ends the process)
//! chains a hook that restores the terminal before displaying the report.
//!
//! Only **one guard can be alive at a time**: the terminal state it restores is global, so
//! the `Drop` of one guard would undo the modes another guard still relies on. Creating a
//! second guard while the first is alive returns an [`io::ErrorKind::AlreadyExists`] error.
//!
//! # Raw mode
//!
//! In raw mode, the terminal no longer waits for Enter, echoes typed keys,
//! or turns Ctrl+C into a signal: the application receives everything, key by
//! key. Raw mode must be disabled before returning control to the shell, or it
//! becomes unusable (use `stty sane` or `reset` to repair it).

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::cursor::Show;
use crossterm::event::DisableBracketedPaste;
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

/// True while the alternate screen is active: the panic hook needs to know whether to leave it.
static ALTERNATE_SCREEN: AtomicBool = AtomicBool::new(false);

/// True while a [`TerminalGuard`] is alive: at most one guard may exist at a time.
static GUARD_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Claim the single guard slot, or fail if another guard is alive.
fn claim_guard_slot() -> io::Result<()> {
    if GUARD_ACTIVE.swap(true, Ordering::SeqCst) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a TerminalGuard is already active: only one guard may exist at a time",
        ));
    }
    Ok(())
}

/// Free the guard slot so that a new guard can be created.
fn release_guard_slot() {
    GUARD_ACTIVE.store(false, Ordering::SeqCst);
}

/// Enable raw mode (and optionally the alternate screen); undo both on destruction.
///
/// At most one guard may be alive at a time (see the module documentation).
#[derive(Debug)]
pub struct TerminalGuard {
    _private: (),
}

impl TerminalGuard {
    /// Raw mode only (inline viewport: stay on the normal screen, with its scrollback).
    ///
    /// # Errors
    ///
    /// Returns an [`io::ErrorKind::AlreadyExists`] error if another guard is alive, or an
    /// error if raw mode cannot be enabled.
    pub fn raw() -> io::Result<Self> {
        claim_guard_slot()?;
        if let Err(e) = enable_raw_mode() {
            release_guard_slot();
            return Err(e);
        }
        Ok(Self { _private: () })
    }

    /// Raw mode, alternate screen, and hidden cursor (fullscreen application).
    ///
    /// The alternate screen is a second terminal screen: draw freely on it, then on exit
    /// the original screen (with its scrollback) is restored unchanged.
    ///
    /// # Errors
    ///
    /// Returns an [`io::ErrorKind::AlreadyExists`] error if another guard is alive, or an
    /// error if raw mode, the alternate screen or hiding the cursor fails; whatever was
    /// already enabled is undone by the guard's `Drop`.
    pub fn fullscreen() -> io::Result<Self> {
        // Create the guard first: if a later step fails, its `Drop` undoes
        // whatever was already enabled.
        let guard = Self::raw()?;
        // Set the flag before entering: a panic between the two would restore an
        // inactive alternate screen, which is harmless (the sequence is ignored).
        ALTERNATE_SCREEN.store(true, Ordering::SeqCst);
        execute!(io::stdout(), EnterAlternateScreen, crossterm::cursor::Hide)?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // A `Drop` cannot propagate errors: do our best.
        let _ = restore();
        release_guard_slot();
    }
}

/// Restore normal terminal state: visible cursor, primary screen, line mode.
///
/// Safe to call repeatedly or when nothing has been enabled. Used by
/// the guard's `Drop` and the panic hook.
///
/// # Errors
///
/// Returns the first error among the restore steps; every step is attempted anyway.
pub fn restore() -> io::Result<()> {
    restore_to(&mut io::stdout())
}

/// [`restore`], writing the escape sequences to `out` instead of stdout (testable without
/// touching the real terminal).
fn restore_to(out: &mut impl Write) -> io::Result<()> {
    // Attempt every step even if one fails; return the first error.
    let mut first_error = None;
    let mut keep = |result: io::Result<()>| {
        if let Err(e) = result {
            first_error.get_or_insert(e);
        }
    };
    keep(execute!(out, Show));
    // `ESC[?2004l`: turn bracketed paste off, in case the application turned it on (a
    // shell left with it on would see pasted text wrapped in `ESC[200~ … ESC[201~`).
    // Harmless when it was off. Its result is ignored: the legacy Windows console API
    // has no bracketed paste and reports an error.
    let _ = execute!(out, DisableBracketedPaste);
    if ALTERNATE_SCREEN.swap(false, Ordering::SeqCst) {
        keep(execute!(out, LeaveAlternateScreen));
    }
    keep(disable_raw_mode());
    keep(out.flush());
    first_error.map_or(Ok(()), Err)
}

/// Install a panic hook that restores the terminal **before** calling the previous hook.
///
/// Otherwise, the panic report would be written to the alternate screen (and lost on exit)
/// or in raw mode (staircase lines due to missing carriage returns). Call after
/// `color_eyre::install()` so its readable report is the “previous hook”.
///
/// The panic then goes on as usual: it unwinds (destructors run, `catch_unwind` can stop
/// it) and does not end the process by itself. After a panic that the application catches
/// or survives (one in a secondary thread or a `tokio::spawn` task), the terminal is back
/// in line mode: the application must quit, or set its modes again with a new guard once
/// the current one is dropped. To end the process on any panic, use
/// [`install_fatal_panic_hook`].
pub fn install_panic_hook() {
    install(false);
}

/// Like [`install_panic_hook`], but **every panic is fatal**: after the report, the hook
/// ends the process with exit code 101 (the code of an unwinding panic).
///
/// For applications whose UI must not outlive a panic anywhere: a panic in a
/// `tokio::spawn` task or in a secondary thread does not stop the process by itself, and
/// without this exit the UI would keep drawing on a terminal already put back in line mode.
/// Destructors (including `TerminalGuard`'s) do not run, and a `catch_unwind` cannot stop
/// the panic: buffered data that only a destructor would write (a `BufWriter` log file, for
/// instance) is lost.
pub fn install_fatal_panic_hook() {
    install(true);
}

/// Chain the restoring hook before the current one; with `fatal`, exit after the report.
fn install(fatal: bool) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let was_fullscreen = ALTERNATE_SCREEN.load(Ordering::SeqCst);
        let _ = restore();
        if !was_fullscreen {
            // Normal screen: start a new line so the report does not begin in the middle
            // of the viewport (the terminal is no longer in raw mode; `\n` is enough).
            let _ = writeln!(io::stdout());
        }
        previous(info);
        let _ = io::stdout().flush();
        if fatal {
            std::process::exit(101);
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_is_idempotent() {
        // Write to a buffer, not stdout: when `cargo test` runs in a terminal, stdout *is*
        // that terminal (libtest only captures `print!`), and the sequences would reach it.
        // Raw mode was never enabled here, so `disable_raw_mode` leaves the terminal alone.
        ALTERNATE_SCREEN.store(true, Ordering::SeqCst);
        let mut out = Vec::new();
        restore_to(&mut out).unwrap();
        assert_eq!(
            out, b"\x1b[?25h\x1b[?2004l\x1b[?1049l",
            "show the cursor, disable bracketed paste, leave the alternate screen"
        );
        assert!(!ALTERNATE_SCREEN.load(Ordering::SeqCst));

        // Second call: the alternate screen is already left; the cursor is shown again and
        // bracketed paste disabled again.
        out.clear();
        restore_to(&mut out).unwrap();
        assert_eq!(out, b"\x1b[?25h\x1b[?2004l");
        assert!(!ALTERNATE_SCREEN.load(Ordering::SeqCst));
    }

    #[test]
    fn only_one_guard_slot_can_be_claimed_at_a_time() {
        // Exercise the slot directly: creating a real guard would put the test
        // runner's terminal in raw mode.
        claim_guard_slot().expect("the first claim must succeed");
        let err = claim_guard_slot().expect_err("a second claim must fail");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        release_guard_slot();
        claim_guard_slot().expect("the slot must be free again after release");
        release_guard_slot();
    }
}
