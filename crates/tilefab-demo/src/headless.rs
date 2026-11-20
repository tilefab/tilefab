//! Headless mode, used when stdin or stdout is not a terminal
//! (`echo "start a" | tilefab`, redirection to a file…).
//!
//! No UI: commands are read line by line from stdin and logs are
//! written to stderr. The logic (commands, tasks) is the same as in
//! interactive mode; only the loop differs.

use std::io::{BufRead, Write};

use color_eyre::Result;
use tokio::sync::mpsc::{self, Sender};

use crate::app::{App, AppEvent};
use crate::logging;

pub async fn run() -> Result<()> {
    logging::init_stderr();

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
/// is full, reading waits, so a large input (`cat big_file | tilefab`) is not loaded
/// into memory faster than it is processed.
fn spawn_stdin_thread(tx: Sender<AppEvent>) {
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.blocking_send(AppEvent::Line(line)).is_err() {
                return;
            }
        }
        let _ = tx.blocking_send(AppEvent::InputClosed);
    });
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
