//! The shortest way to start: [`init_inline`] (or [`init_fullscreen`]) sets the terminal up in
//! one call and returns a [`DefaultTerminal`], which puts everything back when dropped.
//!
//! These functions gather the steps an application otherwise writes itself: install the
//! restoring panic hook ([`install_panic_hook`](crate::install_panic_hook)), enable raw mode
//! (and the alternate screen) with a [`TerminalGuard`], wrap stdout in a buffered
//! [`Backend`], create the [`Terminal`], and on exit clear the viewport before the guard
//! restores the terminal. The building blocks stay public, for an application that needs
//! another writer or another order.

use std::io::{self, BufWriter, IsTerminal, Stdout};
use std::ops::{Deref, DerefMut};
use std::sync::Once;

use crate::backend::Backend;
use crate::init::{TerminalGuard, install_panic_hook};
use crate::terminal::Terminal;
use crate::viewport::{TerminalOptions, Viewport};

/// Installs the restoring panic hook once, whatever the number of `init_*` calls.
static PANIC_HOOK: Once = Once::new();

/// A [`Terminal`] on a buffered stdout, together with the [`TerminalGuard`] that set raw
/// mode up: made by [`init_inline`], [`init_fullscreen`] or [`init_with_options`].
///
/// It dereferences to the [`Terminal`]: call `draw`, `insert_line`, `set_height`… on it
/// directly. When it is dropped, it clears the viewport (inline: the log lines stay above,
/// and the shell resumes where the viewport was) and restores the terminal. To see a
/// clearing error, call [`DefaultTerminal::finish`] instead of letting it drop. During a
/// panic, the drop only restores the terminal: the viewport stays on screen above the panic
/// report.
#[derive(Debug)]
pub struct DefaultTerminal {
    // Declared before `guard`: fields drop in order, so the terminal (its `BufWriter`) is
    // flushed before the guard leaves raw mode.
    terminal: Terminal<BufWriter<Stdout>>,
    _guard: TerminalGuard,
    /// True once the viewport was cleared by `finish`: the drop then has nothing to do.
    finished: bool,
}

/// Set the terminal up for an inline viewport of `height` rows at the bottom of the normal
/// screen (raw mode, no alternate screen): log lines inserted above it stay in the
/// terminal's history. The height can change later with `set_height`.
///
/// Also installs, once, the panic hook that restores the terminal before the panic report:
/// call `color_eyre::install()` (or install another hook) before this function, so that it
/// runs after the restoration.
///
/// ```no_run
/// let mut terminal = tilefab_term::init_inline(1)?;
/// terminal.insert_line("hello, above the viewport");
/// terminal.draw(|frame| {
///     let area = frame.area();
///     frame.buffer_mut().set_string(area.x, area.y, "> ", Default::default());
/// })?;
/// // Dropping `terminal` clears the viewport and restores the terminal.
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// Returns an [`io::ErrorKind::Unsupported`] error when stdin or stdout is not a terminal
/// (redirected to a file or a pipe), an [`io::ErrorKind::AlreadyExists`] error when a
/// [`TerminalGuard`] is already alive, or the error of enabling raw mode, querying the cursor
/// position or writing to the terminal. The terminal is restored before returning an error.
pub fn init_inline(height: u16) -> io::Result<DefaultTerminal> {
    init_with_options(TerminalOptions::default().viewport(Viewport::Inline(height)))
}

/// Set the terminal up for a fullscreen application: raw mode, alternate screen, hidden
/// cursor. On exit, the original screen comes back unchanged. Also installs the restoring
/// panic hook, as [`init_inline`] does.
///
/// # Errors
///
/// The same as [`init_inline`].
pub fn init_fullscreen() -> io::Result<DefaultTerminal> {
    init_with_options(TerminalOptions::default().viewport(Viewport::Fullscreen))
}

/// [`init_inline`] or [`init_fullscreen`], depending on `options.viewport`, with the other
/// options (the [`ScrollMode`](crate::ScrollMode)) too.
///
/// # Errors
///
/// The same as [`init_inline`].
pub fn init_with_options(options: TerminalOptions) -> io::Result<DefaultTerminal> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "stdin and stdout must both be a terminal (not redirected to a file or a pipe)",
        ));
    }
    PANIC_HOOK.call_once(install_panic_hook);
    let guard = match options.viewport {
        Viewport::Inline(_) => TerminalGuard::raw()?,
        Viewport::Fullscreen => TerminalGuard::fullscreen()?,
    };
    // On error, `guard` is dropped here and restores the terminal.
    let terminal = Terminal::with_options(Backend::new(BufWriter::new(io::stdout())), options)?;
    Ok(DefaultTerminal {
        terminal,
        _guard: guard,
        finished: false,
    })
}

impl DefaultTerminal {
    /// Clear the viewport (writing the log lines still pending first) and restore the
    /// terminal, reporting a write error that a drop would ignore.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the terminal fails; the terminal is restored anyway.
    pub fn finish(mut self) -> io::Result<()> {
        self.finished = true;
        self.terminal.clear()
    }
}

impl Deref for DefaultTerminal {
    type Target = Terminal<BufWriter<Stdout>>;

    fn deref(&self) -> &Self::Target {
        &self.terminal
    }
}

impl DerefMut for DefaultTerminal {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.terminal
    }
}

impl Drop for DefaultTerminal {
    fn drop(&mut self) {
        // During a panic, the hook already restored the terminal and printed the report
        // below the viewport: clearing would move the cursor back up into the viewport, and
        // the shell would then write over the report.
        if !self.finished && !std::thread::panicking() {
            // A `Drop` cannot propagate errors: `finish` reports them.
            let _ = self.terminal.clear();
        }
    }
}
