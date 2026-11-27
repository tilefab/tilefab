//! CLI demo: logs scroll above an inline viewport whose height follows the number of
//! tasks (one line per task, drawn by a `LineWidget`, + the prompt), built on the small
//! `tilefab` library.

mod app;
mod headless;
mod line;
mod logging;
mod tasks;
mod ui;
mod wrap;

use std::io::{BufWriter, IsTerminal, Stdout, Write, stdin, stdout};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use color_eyre::Result;
use tilefab::crossterm::event::{self, EnableBracketedPaste, Event};
use tilefab::crossterm::execute;
use tilefab::{
    Backend, Line, MAX_PENDING_LINES, ScrollMode, Terminal, TerminalGuard, TerminalOptions,
    Viewport, install_fatal_panic_hook,
};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tracing::Level;

use crate::app::{App, AppEvent};
use crate::logging::DroppedLogs;

/// Maximum number of tasks displayed at once.
pub const MAX_TASKS: usize = 5;
/// Capacity of the application event channel: far more than one iteration
/// of the UI loop accumulates, since it empties the queue each time. Each producer
/// decides what to do when it is full (see `logging`, `tasks`, and the producers below).
pub const EVENT_QUEUE: usize = 1024;
/// Tick interval that forces a redraw.
const TICK: Duration = Duration::from_millis(100);
/// Maximum keyboard thread wait before releasing the crossterm reader.
const KEYBOARD_POLL: Duration = Duration::from_millis(50);

/// The application terminal: buffered output sends each frame in a single `write`.
type AppTerminal = Terminal<BufWriter<Stdout>>;

#[tokio::main]
async fn main() -> Result<()> {
    // color-eyre installs its own panic hook (readable report); it must be installed BEFORE
    // the library's hook, which restores the terminal and then delegates to color-eyre.
    color_eyre::install()?;

    // Without a terminal on both sides, the UI cannot run: use headless mode.
    if !stdin().is_terminal() || !stdout().is_terminal() {
        return headless::run(None).await;
    }

    // Fatal: a panic in a task or a thread must not leave the UI running.
    install_fatal_panic_hook();
    // The guard enables raw mode; its `Drop` disables it, even if the loop fails.
    let guard = TerminalGuard::raw()?;
    // `ESC[?2004h`: bracketed paste. The terminal wraps pasted text in `ESC[200~ … ESC[201~`,
    // which crossterm reports as one `Event::Paste`: a multi-line paste is inserted on the
    // prompt line instead of running each line. Sent before the first frame, and turned
    // off by the guard's restoration. Without support (legacy Windows console), pasted
    // text arrives as keys.
    let _ = execute!(stdout(), EnableBracketedPaste);
    // A single-line viewport when idle (the prompt); it grows with the tasks.
    let (scroll_mode, scroll_warning) = scroll_mode(std::env::var_os("TILEFAB_SCROLL_MODE"));
    let started = Terminal::with_options(
        Backend::new(BufWriter::new(stdout())),
        TerminalOptions::default()
            .viewport(Viewport::Inline(1))
            .scroll_mode(scroll_mode),
    );
    let mut terminal: AppTerminal = match started {
        Ok(terminal) => terminal,
        // Typically a terminal that does not answer the cursor position query (`ESC[6n`):
        // the inline viewport cannot be placed, but the commands still work line by line.
        // Raw mode is disabled first, so that stdin is read as lines again.
        Err(error) => {
            drop(guard);
            let reason = format!("cannot start the inline UI ({error}): using headless mode");
            return headless::run(Some(&reason)).await;
        }
    };

    let (tx, rx) = mpsc::channel(EVENT_QUEUE);
    spawn_keyboard_thread(tx.clone());
    spawn_ticker(tx.clone());
    spawn_signal_listener(tx.clone())?;
    let dropped_logs = logging::init_channel(tx.clone());
    if let Some(warning) = scroll_warning {
        tracing::warn!("{warning}");
    }

    let history = app::history_path(|name| std::env::var_os(name));
    let result = run(&mut terminal, tx, rx, &dropped_logs, history).await;

    // Cleanup runs even if the loop failed: clear the viewport and move the cursor to
    // its start. The shell resumes just below the logs, which remain in scrollback.
    let cleanup: Result<()> = terminal.clear().map_err(Into::into);
    result.and(cleanup)?;
    // Quit on a signal: exit with its code, once the terminal is restored.
    if let Some(code) = SIGNALS.exit_code() {
        drop(terminal);
        drop(guard);
        std::process::exit(code);
    }
    Ok(())
}

/// The scroll mode named by `TILEFAB_SCROLL_MODE` (`screen`, the default, `region` or
/// `fallback`; see `tilefab::ScrollMode`), and a warning for a value it does not know.
fn scroll_mode(value: Option<std::ffi::OsString>) -> (ScrollMode, Option<String>) {
    let Some(value) = value else {
        return (ScrollMode::default(), None);
    };
    match value.to_str() {
        Some("screen") => (ScrollMode::Screen, None),
        Some("region") => (ScrollMode::Region, None),
        Some("fallback") => (ScrollMode::Fallback, None),
        _ => (
            ScrollMode::default(),
            Some(format!(
                "TILEFAB_SCROLL_MODE={}: unknown mode (screen, region or fallback), using screen",
                value.to_string_lossy().escape_debug()
            )),
        ),
    }
}

/// Dedicated keyboard reader thread: reading blocks, so it should not run
/// inside a tokio task. The thread stops when the channel closes
/// (the UI loop ends) or reading fails; a failure is sent as [`AppEvent::InputFailed`], so
/// that the application quits instead of running on without a keyboard.
///
/// Why `poll` + `read` instead of a blocking `read()`: crossterm
/// has a single global input reader, protected by a lock that `read()`
/// holds throughout its wait. Our inline terminal queries the cursor
/// position (`cursor::position()`, at startup and on resize), which
/// requires the same lock and times out after 2 seconds.
/// A short `poll` timeout releases the lock
/// regularly.
///
/// When the channel is full, the thread waits: keys stay in the terminal's buffer, none
/// is lost.
fn spawn_keyboard_thread(tx: Sender<AppEvent>) {
    std::thread::spawn(move || read_keys(|| event::poll(KEYBOARD_POLL), event::read, &tx));
}

/// The loop of the keyboard thread, with `poll` and `read` given (crossterm's, or a test
/// reader). Blocking: run it in a thread.
fn read_keys(
    mut poll: impl FnMut() -> std::io::Result<bool>,
    mut read: impl FnMut() -> std::io::Result<Event>,
    tx: &Sender<AppEvent>,
) {
    loop {
        let event = match poll() {
            Ok(true) => read(),
            Ok(false) if tx.is_closed() => return,
            Ok(false) => continue,
            Err(error) => Err(error),
        };
        let app_event = match event {
            Ok(Event::Key(key)) => AppEvent::Key(key),
            Ok(Event::Resize(_, _)) => AppEvent::Resize,
            Ok(Event::Paste(text)) => AppEvent::Paste(text),
            Ok(_) => continue, // mouse, focus: ignored
            Err(error) => {
                // Waits for room if needed: the request to quit must not be lost.
                let _ = tx.blocking_send(AppEvent::InputFailed(error.to_string()));
                return;
            }
        };
        if tx.blocking_send(app_event).is_err() {
            return;
        }
    }
}

/// Periodic redraw. A tick that finds the channel full is skipped: the queued events
/// cause a redraw anyway.
fn spawn_ticker(tx: Sender<AppEvent>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK);
        loop {
            interval.tick().await;
            match tx.try_send(AppEvent::Tick) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Closed(_)) => break,
            }
        }
    });
}

/// Termination signals received. The first one quits through the normal cleanup, and the
/// process then exits with its code; a next one ends the process at once.
struct Signals(AtomicI32);

impl Signals {
    /// Record a signal asking to exit with `code` (not 0): true for the first one.
    fn first(&self, code: i32) -> bool {
        self.0
            .compare_exchange(0, code, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// The exit code of the first signal, if one was received.
    fn exit_code(&self) -> Option<i32> {
        Some(self.0.load(Ordering::SeqCst)).filter(|&code| code != 0)
    }
}

static SIGNALS: Signals = Signals(AtomicI32::new(0));

/// How long a forced exit waits for the restoration of the terminal (see [`force_exit`]).
const FORCED_RESTORE_WAIT: Duration = Duration::from_millis(500);

/// A termination signal asking to exit with `code`: quit through the normal cleanup the
/// first time, end the process at once the next times.
fn on_signal(tx: &Sender<AppEvent>, code: i32) {
    if SIGNALS.first(code) {
        // In a task of its own: with a full queue (UI loop stuck), waiting for room here
        // would keep the next signal from being seen.
        let tx = tx.clone();
        tokio::spawn(async move {
            // Waits for room if needed: the request to quit must not be lost.
            let _ = tx.send(AppEvent::Quit).await;
        });
    } else {
        force_exit(code);
    }
}

/// A signal after the first one: the UI loop did not quit, stuck on a write to a terminal
/// that reads nothing, or waiting for the answer to a query. Restore what can be, and exit
/// with `code` at once.
fn force_exit(code: i32) -> ! {
    // Raw mode first: an `ioctl`, no write to stdout, which the stuck loop may hold.
    let _ = tilefab::crossterm::terminal::disable_raw_mode();
    // The rest of the restoration writes to stdout: in a thread, for a bounded time.
    let (done, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tilefab::restore();
        let _ = done.send(());
    });
    let _ = wait.recv_timeout(FORCED_RESTORE_WAIT);
    std::process::exit(code);
}

/// Turn termination signals into [`AppEvent::Quit`], so that a `kill <pid>` goes
/// through the normal cleanup (viewport cleared, raw mode disabled by the guard)
/// instead of leaving the shell in raw mode; the process then exits with the usual code,
/// 128 + the signal number. A second signal ends it at once (see [`force_exit`]). In raw
/// mode Ctrl+C arrives as a key, so `SIGINT` only comes from another process. The handlers
/// are registered here, before the task starts, so that a registration error stops the
/// program.
#[cfg(unix)]
fn spawn_signal_listener(tx: Sender<AppEvent>) -> Result<()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut hangup = signal(SignalKind::hangup())?;
    tokio::spawn(async move {
        loop {
            let kind = tokio::select! {
                Some(()) = terminate.recv() => SignalKind::terminate(),
                Some(()) = interrupt.recv() => SignalKind::interrupt(),
                Some(()) = hangup.recv() => SignalKind::hangup(),
                else => return,
            };
            on_signal(&tx, 128 + kind.as_raw_value());
        }
    });
    Ok(())
}

/// Windows variant of the signal listener: closing the console window, logging off and
/// shutting down (`CTRL_CLOSE_EVENT`, `CTRL_LOGOFF_EVENT`, `CTRL_SHUTDOWN_EVENT`) quit through
/// the normal cleanup, with the exit code Windows gives a process ended that way
/// (`STATUS_CONTROL_C_EXIT`); a second event ends it at once. Ctrl+C arrives as a key in
/// raw mode, as on Unix.
#[cfg(windows)]
fn spawn_signal_listener(tx: Sender<AppEvent>) -> Result<()> {
    use tokio::signal::windows::{ctrl_close, ctrl_logoff, ctrl_shutdown};

    const STATUS_CONTROL_C_EXIT: i32 = 0xC000_013A_u32.cast_signed();
    let mut close = ctrl_close()?;
    let mut logoff = ctrl_logoff()?;
    let mut shutdown = ctrl_shutdown()?;
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(()) = close.recv() => {}
                Some(()) = logoff.recv() => {}
                Some(()) = shutdown.recv() => {}
                else => return,
            }
            on_signal(&tx, STATUS_CONTROL_C_EXIT);
        }
    });
    Ok(())
}

/// Elsewhere, no signal to listen to.
#[cfg(not(any(unix, windows)))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as the Unix and Windows variants"
)]
fn spawn_signal_listener(_tx: Sender<AppEvent>) -> Result<()> {
    Ok(())
}

/// UI loop: wait for an event, process all queued events (to redraw
/// only once per burst), then apply the new lines above the viewport,
/// its new height, and its content **in a single frame**.
///
/// The burst that requests quitting is drawn too, before leaving: its lines (the echo of
/// `quit`, logs received in the same burst) are written above the viewport, which the
/// cleanup then clears, instead of being dropped.
async fn run<W: Write>(
    terminal: &mut Terminal<W>,
    tx: Sender<AppEvent>,
    mut rx: Receiver<AppEvent>,
    dropped_logs: &DroppedLogs,
    history: Option<PathBuf>,
) -> Result<()> {
    let mut app = App::new(tx);
    if let Some(path) = history {
        app.load_history(&path);
    }
    terminal.draw(|frame| ui::draw(frame, &mut app))?;

    while let Some(event) = rx.recv().await {
        app.handle_event(event);
        while let Ok(event) = rx.try_recv() {
            app.handle_event(event);
        }

        report_dropped_logs(&mut app, dropped_logs);
        queue_output(terminal, &mut app);
        terminal.set_height(ui::viewport_height(&app, terminal.area().width));
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        if app.should_quit {
            break;
        }
    }
    Ok(())
}

/// Add a warning line if log lines were dropped because the channel was full, so that
/// the loss is visible.
fn report_dropped_logs(app: &mut App, dropped_logs: &DroppedLogs) {
    let dropped = dropped_logs.take();
    if dropped > 0 {
        let message = format!("{dropped} log line(s) dropped: the event queue was full");
        app.output
            .push(Line::from(logging::format_line(Level::WARN, &message)));
    }
}

/// Queue pending lines; the next `draw` writes them along with the height
/// change, in the same synchronized update block.
fn queue_output<W: Write>(terminal: &mut Terminal<W>, app: &mut App) {
    // A zero-width screen (a pty without a size) cannot take inserted lines: keep them
    // in `app.output` until the screen has columns again, the newest ones only.
    if terminal.area().width == 0 {
        app.limit_output();
        return;
    }
    if app.dropped_output > 0 {
        let message = format!(
            "{} log line(s) dropped while the terminal had no columns",
            app.dropped_output
        );
        app.output
            .insert(0, Line::from(logging::format_line(Level::WARN, &message)));
        app.dropped_output = 0;
    }
    if app.output.is_empty() {
        return;
    }
    // The width `insert_before` gives its buffer (the screen width known at the last
    // `draw`), not the current terminal width: after a resize they differ until the
    // next `draw`, and wrapping at another width would truncate the lines.
    let width = terminal.area().width;
    let rows = wrap::wrap(std::mem::take(&mut app.output), width);
    // The terminal keeps at most MAX_PENDING_LINES rows of one insert, the first ones: pass
    // the newest (a narrow terminal can wrap the kept text into many more rows).
    let newest = &rows[rows.len().saturating_sub(MAX_PENDING_LINES)..];
    if newest.len() < rows.len() {
        tracing::warn!(
            "{} wrapped log row(s) dropped: at most {MAX_PENDING_LINES} are shown at once",
            rows.len() - newest.len()
        );
    }
    let height = u16::try_from(newest.len()).unwrap_or(u16::MAX);
    terminal.insert_before(height, |buf| wrap::render(newest, buf));
}

#[cfg(test)]
mod tests {
    use tilefab::{Line, Position, Size};

    use super::*;

    #[test]
    fn dropped_logs_are_reported_once() {
        let (tx, _rx) = mpsc::channel(16);
        let mut app = App::new(tx);
        let (log_tx, _log_rx) = mpsc::channel(1);
        let dropped = DroppedLogs::default();
        let mut writer = logging::LineWriter::new(log_tx, dropped.clone());
        writer.write_all(b"a\nb\nc\n").unwrap(); // capacity 1: two lines dropped

        report_dropped_logs(&mut app, &dropped);
        let lines: Vec<String> = app
            .output
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(
            lines,
            ["[WARN] 2 log line(s) dropped: the event queue was full"]
        );
        // Nothing new: nothing more reported.
        report_dropped_logs(&mut app, &dropped);
        assert_eq!(app.output.len(), 1);
    }

    #[test]
    fn output_is_kept_while_the_screen_has_no_columns() {
        let backend = Backend::with_fixed(Vec::new(), Size::new(0, 10), Position::new(0, 0));
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions::default().viewport(Viewport::Inline(1)),
        )
        .unwrap();
        terminal.draw(|_| {}).unwrap();
        let (tx, _rx) = mpsc::channel(16);
        let mut app = App::new(tx);
        app.output.push(Line::from("[INFO] kept"));

        // Width 0: nothing can be inserted, the line stays in `app.output`.
        queue_output(&mut terminal, &mut app);
        terminal.draw(|_| {}).unwrap();
        assert_eq!(app.output.len(), 1, "line dropped at width 0");

        // Columns again: after the draw that discovers them, the line is queued and written.
        terminal.backend_mut().set_fixed_size(Size::new(20, 10));
        terminal.draw(|_| {}).unwrap();
        std::mem::take(terminal.backend_mut().writer_mut());
        queue_output(&mut terminal, &mut app);
        assert!(
            app.output.is_empty(),
            "line not queued once the width is back"
        );
        terminal.draw(|_| {}).unwrap();
        let written =
            String::from_utf8(std::mem::take(terminal.backend_mut().writer_mut())).unwrap();
        assert!(written.contains("kept"), "line not written: {written:?}");
    }

    #[test]
    fn the_newest_rows_are_inserted_when_there_are_too_many() {
        let backend = Backend::with_fixed(Vec::new(), Size::new(10, 10), Position::new(0, 0));
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions::default().viewport(Viewport::Inline(1)),
        )
        .unwrap();
        terminal.draw(|_| {}).unwrap();
        let (tx, _rx) = mpsc::channel(16);
        let mut app = App::new(tx);
        for n in 0..MAX_PENDING_LINES + 5 {
            app.output.push(Line::from(format!("r{n:04}")));
        }
        std::mem::take(terminal.backend_mut().writer_mut());

        queue_output(&mut terminal, &mut app);
        terminal.draw(|_| {}).unwrap();
        let written =
            String::from_utf8(std::mem::take(terminal.backend_mut().writer_mut())).unwrap();
        let last = format!("r{:04}", MAX_PENDING_LINES + 4);
        assert!(written.contains(&last), "newest row lost");
        assert!(written.contains("r0005"), "a kept row lost");
        assert!(
            !written.contains("r0004"),
            "an old row kept instead of a new one"
        );
    }

    #[test]
    fn only_the_first_signal_quits_through_the_cleanup() {
        let signals = Signals(AtomicI32::new(0));
        assert_eq!(signals.exit_code(), None);
        assert!(signals.first(143), "the first signal");
        assert!(!signals.first(130), "a second signal");
        assert_eq!(signals.exit_code(), Some(143), "the code of the first one");
    }

    #[test]
    fn the_scroll_mode_comes_from_the_environment() {
        use std::ffi::OsString;
        let mode = |value: Option<&str>| scroll_mode(value.map(OsString::from));
        assert_eq!(mode(None), (ScrollMode::Screen, None));
        assert_eq!(mode(Some("region")), (ScrollMode::Region, None));
        assert_eq!(mode(Some("fallback")), (ScrollMode::Fallback, None));
        assert_eq!(mode(Some("screen")), (ScrollMode::Screen, None));
        let (fallback, warning) = mode(Some("Region\u{1b}"));
        assert_eq!(fallback, ScrollMode::Screen);
        assert_eq!(
            warning.as_deref(),
            Some(
                "TILEFAB_SCROLL_MODE=Region\\u{1b}: unknown mode (screen, region or fallback), \
                 using screen"
            )
        );
    }

    #[test]
    fn a_keyboard_failure_is_sent_once_and_stops_the_reader() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut polls = 0;
        read_keys(
            || {
                polls += 1;
                if polls < 3 {
                    Ok(true)
                } else {
                    Err(std::io::Error::other("tty gone"))
                }
            },
            || Ok(Event::FocusGained),
            &tx,
        );
        assert!(matches!(rx.try_recv(), Ok(AppEvent::InputFailed(error)) if error == "tty gone"));
        assert!(rx.try_recv().is_err(), "nothing after the failure");
        // A read error, too.
        read_keys(
            || Ok(true),
            || Err(std::io::Error::other("read failed")),
            &tx,
        );
        assert!(matches!(rx.try_recv(), Ok(AppEvent::InputFailed(_))));
    }

    #[test]
    fn a_keyboard_failure_is_logged_and_quits() {
        let backend = Backend::with_fixed(Vec::new(), Size::new(80, 10), Position::new(0, 0));
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions::default().viewport(Viewport::Inline(1)),
        )
        .unwrap();
        let (tx, rx) = mpsc::channel(16);
        tx.try_send(AppEvent::InputFailed("tty gone".into()))
            .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        // On this thread, so that the log is captured; `run` returns: the application quits.
        let logs = logging::capture(|| {
            runtime
                .block_on(run(&mut terminal, tx, rx, &DroppedLogs::default(), None))
                .unwrap();
        });
        assert_eq!(logs, ["[ERROR] keyboard input failed (tty gone): quitting"]);
    }

    #[test]
    fn output_kept_at_zero_width_is_bounded_and_the_loss_reported() {
        let backend = Backend::with_fixed(Vec::new(), Size::new(0, 10), Position::new(0, 0));
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions::default().viewport(Viewport::Inline(1)),
        )
        .unwrap();
        terminal.draw(|_| {}).unwrap();
        let (tx, _rx) = mpsc::channel(16);
        let mut app = App::new(tx);
        for i in 0..app::MAX_KEPT_LINES + 30 {
            app.output.push(Line::from(format!("[INFO] help {i}")));
            queue_output(&mut terminal, &mut app);
        }
        assert_eq!(app.output.len(), app::MAX_KEPT_LINES);
        assert_eq!(app.dropped_output, 30);
        // Long lines: the byte bound applies too.
        app.output.push(Line::from("x".repeat(app::MAX_KEPT_BYTES)));
        queue_output(&mut terminal, &mut app);
        assert_eq!(
            app.output.len(),
            1,
            "only the newest line fits the byte bound"
        );
        let dropped = app.dropped_output;
        assert_eq!(dropped, 30 + app::MAX_KEPT_LINES);
        // A short line instead, so that the warning is not pushed out of the terminal's queue.
        app.output = vec![Line::from("[INFO] last")];

        terminal.backend_mut().set_fixed_size(Size::new(80, 10));
        terminal.draw(|_| {}).unwrap();
        queue_output(&mut terminal, &mut app);
        terminal.draw(|_| {}).unwrap();
        let written =
            String::from_utf8(std::mem::take(terminal.backend_mut().writer_mut())).unwrap();
        // The diff skips the blank cells: look for the words.
        for word in ["[WARN]", &dropped.to_string(), "dropped", "columns"] {
            assert!(written.contains(word), "{word} missing: {written:?}");
        }
        assert_eq!(app.dropped_output, 0);
    }

    #[tokio::test]
    async fn the_burst_that_quits_is_written_before_leaving() {
        use tilefab::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let backend = Backend::with_fixed(Vec::new(), Size::new(30, 10), Position::new(0, 0));
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions::default().viewport(Viewport::Inline(1)),
        )
        .unwrap();
        let (tx, rx) = mpsc::channel(16);
        // One burst: a log line, then `quit` typed and submitted.
        let key = |code| AppEvent::Key(KeyEvent::new(code, KeyModifiers::NONE));
        tx.send(AppEvent::Log("[INFO] last-log".into()))
            .await
            .unwrap();
        for c in "quit".chars() {
            tx.send(key(KeyCode::Char(c))).await.unwrap();
        }
        tx.send(key(KeyCode::Enter)).await.unwrap();

        run(&mut terminal, tx, rx, &DroppedLogs::default(), None)
            .await
            .unwrap();
        let written =
            String::from_utf8(std::mem::take(terminal.backend_mut().writer_mut())).unwrap();
        // No space in the searched text: the diff skips unchanged (blank) cells.
        assert!(written.contains("last-log"), "log dropped: {written:?}");
        assert!(
            written.contains("quit"),
            "echo of quit dropped: {written:?}"
        );
        assert!(
            written.rfind("quit") > written.rfind("last-log"),
            "echo of quit not written after the log: {written:?}"
        );
    }
}
