//! CLI demo: logs scroll above an inline viewport whose height follows the number of
//! tasks (one line per task + the prompt), built on the small `tui` library.

mod app;
mod headless;
mod logging;
mod tasks;
mod ui;

use std::io::{BufWriter, IsTerminal, Stdout, Write, stdin, stdout};
use std::time::Duration;

use color_eyre::Result;
use crossterm::event::{self, Event};
use tilefab::{
    Backend, Line, Terminal, TerminalGuard, TerminalOptions, Viewport, install_panic_hook,
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

    install_panic_hook();
    // The guard enables raw mode; its `Drop` disables it, even if the loop fails.
    let guard = TerminalGuard::raw()?;
    // A single-line viewport when idle (the prompt); it grows with the tasks.
    let started = Terminal::with_options(
        Backend::new(BufWriter::new(stdout())),
        TerminalOptions {
            viewport: Viewport::Inline(1),
            ..TerminalOptions::default()
        },
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
    #[cfg(unix)]
    spawn_signal_listener(tx.clone())?;
    let dropped_logs = logging::init_channel(tx.clone());

    let result = run(&mut terminal, tx, rx, &dropped_logs).await;

    // Cleanup runs even if the loop failed: clear the viewport and move the cursor to
    // its start. The shell resumes just below the logs, which remain in scrollback.
    let cleanup = terminal.clear().map_err(Into::into);
    result.and(cleanup)
}

/// Dedicated keyboard reader thread: reading blocks, so it should not run
/// inside a tokio task. The thread stops when the channel closes
/// (the UI loop ends) or reading fails.
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
    std::thread::spawn(move || {
        loop {
            match event::poll(KEYBOARD_POLL) {
                Ok(true) => {}
                Ok(false) if tx.is_closed() => break,
                Ok(false) => continue,
                Err(_) => break,
            }
            let app_event = match event::read() {
                Ok(Event::Key(key)) => AppEvent::Key(key),
                Ok(Event::Resize(_, _)) => AppEvent::Resize,
                Ok(_) => continue, // mouse, focus, paste: ignored
                Err(_) => break,
            };
            if tx.blocking_send(app_event).is_err() {
                break;
            }
        }
    });
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

/// Turn termination signals into [`AppEvent::Quit`], so that a `kill <pid>` goes
/// through the normal cleanup (viewport cleared, raw mode disabled by the guard)
/// instead of leaving the shell in raw mode. In raw mode Ctrl+C arrives as a key,
/// so `SIGINT` only comes from another process. The handlers are registered here,
/// before the task starts, so that a registration error stops the program.
#[cfg(unix)]
fn spawn_signal_listener(tx: Sender<AppEvent>) -> Result<()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut hangup = signal(SignalKind::hangup())?;
    tokio::spawn(async move {
        tokio::select! {
            _ = terminate.recv() => {}
            _ = interrupt.recv() => {}
            _ = hangup.recv() => {}
        }
        // Waits for room if needed: the request to quit must not be lost.
        let _ = tx.send(AppEvent::Quit).await;
    });
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
) -> Result<()> {
    let mut app = App::new(tx);
    terminal.draw(|frame| ui::draw(frame, &app))?;

    while let Some(event) = rx.recv().await {
        app.handle_event(event);
        while let Ok(event) = rx.try_recv() {
            app.handle_event(event);
        }

        report_dropped_logs(&mut app, dropped_logs);
        queue_output(terminal, &mut app);
        terminal.set_height(ui::viewport_height(&app));
        terminal.draw(|frame| ui::draw(frame, &app))?;
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
    // in `app.output` until the screen has columns again.
    if app.output.is_empty() || terminal.area().width == 0 {
        return;
    }
    // The width `insert_before` gives its buffer (the screen width known at the last
    // `draw`), not the current terminal width: after a resize they differ until the
    // next `draw`, and wrapping at another width would truncate the paragraph.
    let width = terminal.area().width;
    let (paragraph, height) = ui::wrapped_lines(std::mem::take(&mut app.output), width);
    terminal.insert_before(height, |buf| {
        use tilefab::Widget;
        paragraph.render(buf.area, buf);
    });
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
            TerminalOptions {
                viewport: Viewport::Inline(1),
                ..TerminalOptions::default()
            },
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

    #[tokio::test]
    async fn the_burst_that_quits_is_written_before_leaving() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let backend = Backend::with_fixed(Vec::new(), Size::new(30, 10), Position::new(0, 0));
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(1),
                ..TerminalOptions::default()
            },
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

        run(&mut terminal, tx, rx, &DroppedLogs::default())
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
