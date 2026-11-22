//! Headless mode, used when stdin or stdout is not a terminal
//! (`echo "start a" | tilefab-demo`, redirection to a file…).
//!
//! No UI: commands are read line by line from stdin and logs are
//! written to stderr. The logic (commands, tasks) is the same as in
//! interactive mode; only the loop differs.

use std::io::{BufRead, Write};

use color_eyre::Result;
use tokio::sync::mpsc::{self, Sender};

use crate::app::{App, AppEvent};
use crate::logging;

/// Run the commands read from stdin until it ends (and the tasks finish) or `quit`.
///
/// `warning`, when given, is logged first: the reason why the interactive UI was not
/// used although stdin and stdout are terminals.
pub async fn run(warning: Option<&str>) -> Result<()> {
    logging::init_stderr();
    if let Some(warning) = warning {
        tracing::warn!("{warning}");
    }

    let (tx, mut rx) = mpsc::channel(crate::EVENT_QUEUE);
    spawn_stdin_thread(tx.clone());
    let mut app = App::new(tx);
    let mut input_closed = false;

    while let Some(event) = rx.recv().await {
        if matches!(event, AppEvent::InputClosed) {
            input_closed = true;
        }
        app.handle_event(event);
        // Lines added directly to `app.output` (task started or finished)
        // bypass tracing: print them here, otherwise they are never shown.
        flush_output(&mut app, &mut std::io::stderr().lock())?;
        // At the end of stdin, wait for all launched tasks to finish.
        if app.should_quit || (input_closed && app.tasks.is_empty()) {
            break;
        }
    }
    Ok(())
}

/// Write the pending lines of `app.output` as plain text, one per line,
/// and empty it.
fn flush_output(app: &mut App, out: &mut impl Write) -> std::io::Result<()> {
    for line in app.output.drain(..) {
        for span in &line.spans {
            out.write_all(span.content.as_bytes())?;
        }
        out.write_all(b"\n")?;
    }
    out.flush()
}

/// Read stdin line by line in a thread (blocking reads). The channel is bounded: when it
/// is full, reading waits, so a large input (`cat big_file | tilefab-demo`) is not loaded
/// into memory faster than it is processed.
fn spawn_stdin_thread(tx: Sender<AppEvent>) {
    std::thread::spawn(move || read_lines(std::io::stdin().lock(), &tx));
}

/// Send each line of `input` as [`AppEvent::Line`], then [`AppEvent::InputClosed`].
///
/// A line that is not valid UTF-8 does not stop reading: it is sent with the invalid bytes
/// replaced by `U+FFFD`, and a warning is logged. A read error ends the input, with a
/// warning too, instead of passing for a normal end of file. Blocking: run it in a thread.
fn read_lines(mut input: impl BufRead, tx: &Sender<AppEvent>) {
    let mut bytes = Vec::new();
    let mut number = 0_u64;
    loop {
        bytes.clear();
        match input.read_until(b'\n', &mut bytes) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                tracing::warn!("stdin read failed ({error}): no more commands are read");
                break;
            }
        }
        number += 1;
        // Strip the line ending like `BufRead::lines`: `\n` or `\r\n`.
        let end = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
        let end = end.strip_suffix(b"\r").unwrap_or(end);
        let line = String::from_utf8_lossy(end);
        if let std::borrow::Cow::Owned(_) = line {
            // Lossy conversion only allocates when it had to replace invalid bytes.
            tracing::warn!("stdin line {number} is not valid UTF-8: invalid bytes replaced");
        }
        let line = line.into_owned();
        if tx.blocking_send(AppEvent::Line(line)).is_err() {
            return;
        }
    }
    let _ = tx.blocking_send(AppEvent::InputClosed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Task;

    #[test]
    fn task_finished_line_is_flushed_and_output_emptied() {
        let (tx, _rx) = mpsc::channel(16);
        let mut app = App::new(tx);
        app.tasks.push(Task {
            id: 1,
            name: "a".into(),
            ratio: 0.5,
            view: crate::view::TaskView::default(),
        });
        app.handle_event(AppEvent::TaskDone(1));

        let mut out = Vec::new();
        flush_output(&mut app, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "[INFO] task \"a\" finished\n"
        );
        assert!(app.output.is_empty(), "output must be drained");
    }

    /// Lines `read_lines` sends for `input`, `None` standing for `InputClosed`.
    fn lines_read(input: &[u8]) -> Vec<Option<String>> {
        let (tx, mut rx) = mpsc::channel(16);
        read_lines(input, &tx);
        drop(tx);
        let mut lines = Vec::new();
        while let Ok(event) = rx.try_recv() {
            lines.push(match event {
                AppEvent::Line(line) => Some(line),
                AppEvent::InputClosed => None,
                _ => panic!("unexpected event"),
            });
        }
        lines
    }

    #[test]
    fn a_line_that_is_not_utf8_does_not_stop_reading() {
        assert_eq!(
            lines_read(b"start a\n\xff\nstart b\n"),
            [
                Some("start a".to_owned()),
                Some("\u{fffd}".to_owned()),
                Some("start b".to_owned()),
                None
            ]
        );
    }

    #[test]
    fn line_endings_are_stripped_like_lines_does() {
        assert_eq!(
            lines_read(b"a\r\nb\n\nc"),
            [
                Some("a".to_owned()),
                Some("b".to_owned()),
                Some(String::new()),
                Some("c".to_owned()),
                None
            ]
        );
        assert_eq!(lines_read(b""), [None]);
    }

    #[test]
    fn a_read_error_ends_the_input() {
        /// Yields one line, then fails.
        struct Failing(bool);
        impl std::io::Read for Failing {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if std::mem::replace(&mut self.0, true) {
                    return Err(std::io::Error::other("broken pipe"));
                }
                buf[..2].copy_from_slice(b"a\n");
                Ok(2)
            }
        }
        let (tx, mut rx) = mpsc::channel(16);
        read_lines(std::io::BufReader::new(Failing(false)), &tx);
        assert!(matches!(rx.try_recv(), Ok(AppEvent::Line(line)) if line == "a"));
        assert!(matches!(rx.try_recv(), Ok(AppEvent::InputClosed)));
    }

    #[tokio::test]
    async fn task_started_line_is_flushed() {
        let (tx, _rx) = mpsc::channel(16);
        let mut app = App::new(tx);
        app.handle_event(AppEvent::Line("start a".into()));

        let mut out = Vec::new();
        flush_output(&mut app, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.starts_with("[INFO] task \"a\" started"),
            "unexpected output: {text:?}"
        );
    }
}
