//! Connecting `tracing` to the UI loop.
//!
//! Tracing events are formatted as text by `tracing-subscriber`,
//! then written to a [`LineWriter`] that splits the stream into lines and sends
//! each complete line to the application channel (`AppEvent::Log`).
//! The UI loop, which exclusively owns the terminal, then displays them.
//!
//! The channel is bounded, and an event can be logged from the UI loop
//! itself, its only consumer: waiting for room there would deadlock. So a line that finds
//! the channel full is dropped and counted in [`DroppedLogs`]; the UI loop reports the count.

use std::fmt;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, MakeWriter};
use tracing_subscriber::registry::LookupSpan;

use crate::app::AppEvent;

/// Number of log lines dropped because the channel was full (shared with the UI loop).
#[derive(Debug, Clone, Default)]
pub struct DroppedLogs(Arc<AtomicUsize>);

impl DroppedLogs {
    fn add(&self, n: usize) {
        self.0.fetch_add(n, Ordering::Relaxed);
    }

    /// The count since the last call, reset to 0.
    pub fn take(&self) -> usize {
        self.0.swap(0, Ordering::Relaxed)
    }
}

/// Install the global subscriber that sends logs to the UI channel. Return the counter of
/// lines dropped because the channel was full.
pub fn init_channel(tx: Sender<AppEvent>) -> DroppedLogs {
    let dropped = DroppedLogs::default();
    tracing_subscriber::fmt()
        .with_ansi(false) // no raw ANSI codes: the UI handles formatting
        .event_format(BracketFormat)
        .with_writer(ChannelMakeWriter {
            tx,
            dropped: dropped.clone(),
        })
        .init();
    dropped
}

/// Headless mode (no TTY): write logs directly to stderr.
pub fn init_stderr() {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .event_format(BracketFormat)
        .with_writer(std::io::stderr)
        .init();
}

/// Start of a log line: `[INFO] `.
pub fn level_prefix(level: Level) -> String {
    format!("[{level}] ")
}

/// A complete log line, in the same format as tracing lines (`[INFO] message`).
///
/// Used for lines produced directly by the application, bypassing tracing (task
/// started or finished): they arrive in the same frame as the accompanying viewport
/// height change, instead of the next frame.
pub fn format_line(level: Level, message: &str) -> String {
    format!("{}{message}", level_prefix(level))
}

/// Compact format: `[INFO] message field=value`.
pub struct BracketFormat;

impl<S, N> FormatEvent<S, N> for BracketFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        write!(writer, "{}", level_prefix(*event.metadata().level()))?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

#[cfg(test)]
std::thread_local! {
    /// Where `capture` collects the log lines of the current thread, if it is capturing.
    static CAPTURE: std::cell::RefCell<Option<Sender<AppEvent>>> =
        const { std::cell::RefCell::new(None) };
}

/// Writer factory of the test subscriber: the capture channel of the current thread, or
/// a closed channel (lines dropped) when that thread is not capturing.
#[cfg(test)]
struct CaptureMakeWriter;

#[cfg(test)]
impl<'a> MakeWriter<'a> for CaptureMakeWriter {
    type Writer = LineWriter;

    fn make_writer(&'a self) -> Self::Writer {
        let tx = CAPTURE.with(|c| c.borrow().clone()).unwrap_or_else(|| {
            let (tx, _rx) = tokio::sync::mpsc::channel(1);
            tx
        });
        LineWriter::new(tx, DroppedLogs::default())
    }
}

/// Run `f` and return the log lines it produced on the current thread, in the
/// application format (for tests of code that logs through tracing).
///
/// Uses a **global** test subscriber, installed once, rather than a thread-local one
/// (`with_default`): tests run in parallel, and a callsite first reached by a thread with
/// no subscriber would be cached as "never interested", so a thread-local subscriber
/// installed afterwards would miss its events. With a global subscriber always present,
/// every callsite stays enabled; each thread only collects its own lines.
#[cfg(test)]
pub fn capture(f: impl FnOnce()) -> Vec<String> {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .event_format(BracketFormat)
            .with_writer(CaptureMakeWriter)
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("no other global subscriber in tests");
    });
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    CAPTURE.with(|c| *c.borrow_mut() = Some(tx));
    f();
    CAPTURE.with(|c| *c.borrow_mut() = None);
    let mut lines = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AppEvent::Log(line) = event {
            lines.push(line);
        }
    }
    lines
}

/// Writer factory: tracing-subscriber requests one per event.
pub struct ChannelMakeWriter {
    tx: Sender<AppEvent>,
    dropped: DroppedLogs,
}

impl<'a> MakeWriter<'a> for ChannelMakeWriter {
    type Writer = LineWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LineWriter::new(self.tx.clone(), self.dropped.clone())
    }
}

/// Writer that accumulates bytes and sends each line ending in `\n`.
pub struct LineWriter {
    tx: Sender<AppEvent>,
    dropped: DroppedLogs,
    buf: Vec<u8>,
}

impl LineWriter {
    pub fn new(tx: Sender<AppEvent>, dropped: DroppedLogs) -> Self {
        Self {
            tx,
            dropped,
            buf: Vec::new(),
        }
    }

    /// Never waits (see the module header): a full channel drops the line and counts it.
    fn send(&self, bytes: &[u8]) {
        let line = String::from_utf8_lossy(bytes).into_owned();
        match self.tx.try_send(AppEvent::Log(line)) {
            // Closed: the UI has already stopped, nobody to show the line to.
            Ok(()) | Err(TrySendError::Closed(_)) => {}
            Err(TrySendError::Full(_)) => self.dropped.add(1),
        }
    }
}

impl io::Write for LineWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        // Send all complete lines and keep the remainder in the buffer.
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            self.send(&line[..pos]);
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LineWriter {
    /// An event normally ends with `\n`; otherwise,
    /// send the remainder anyway rather than losing it.
    fn drop(&mut self) {
        if !self.buf.is_empty() {
            self.send(&self.buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tokio::sync::mpsc::{self, Receiver};

    use super::*;

    /// Collect all lines sent through the channel.
    fn received(rx: &mut Receiver<AppEvent>) -> Vec<String> {
        let mut lines = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::Log(line) => lines.push(line),
                other => panic!("unexpected event: {other:?}"),
            }
        }
        lines
    }

    #[test]
    fn sends_one_event_per_complete_line() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut writer = LineWriter::new(tx, DroppedLogs::default());
        writer.write_all(b"un\ndeux\ntrois\n").unwrap();
        assert_eq!(received(&mut rx), ["un", "deux", "trois"]);
    }

    #[test]
    fn buffers_partial_lines_across_writes() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut writer = LineWriter::new(tx, DroppedLogs::default());
        writer.write_all(b"[INFO] ").unwrap();
        writer.write_all(b"bonjour").unwrap();
        assert_eq!(received(&mut rx), Vec::<String>::new());
        writer.write_all(b" monde\nsuite").unwrap();
        assert_eq!(received(&mut rx), ["[INFO] bonjour monde"]);
        writer.write_all(b"\n").unwrap();
        assert_eq!(received(&mut rx), ["suite"]);
    }

    #[test]
    fn keeps_utf8_split_across_writes() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut writer = LineWriter::new(tx, DroppedLogs::default());
        let bytes = "été 🚀\n".as_bytes();
        // Split inside the “é” (2 bytes) and the emoji (4 bytes).
        writer.write_all(&bytes[..1]).unwrap();
        writer.write_all(&bytes[1..6]).unwrap();
        writer.write_all(&bytes[6..]).unwrap();
        assert_eq!(received(&mut rx), ["été 🚀"]);
    }

    #[test]
    fn empty_lines_are_kept() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut writer = LineWriter::new(tx, DroppedLogs::default());
        writer.write_all(b"a\n\nb\n").unwrap();
        assert_eq!(received(&mut rx), ["a", "", "b"]);
    }

    #[test]
    fn flushes_unterminated_rest_on_drop() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut writer = LineWriter::new(tx, DroppedLogs::default());
        writer.write_all(b"line\nrest").unwrap();
        drop(writer);
        assert_eq!(received(&mut rx), ["line", "rest"]);
    }

    #[test]
    fn full_channel_drops_and_counts_lines_without_waiting() {
        let (tx, mut rx) = mpsc::channel(1);
        let dropped = DroppedLogs::default();
        let mut writer = LineWriter::new(tx, dropped.clone());
        // One place: the first line is queued, the next two are dropped (never waiting,
        // which would deadlock if the UI loop itself was logging).
        writer.write_all(b"kept\nlost 1\nlost 2\n").unwrap();
        assert_eq!(received(&mut rx), ["kept"]);
        assert_eq!(dropped.take(), 2);
        assert_eq!(dropped.take(), 0, "take resets the count");
    }

    #[test]
    fn closed_channel_is_not_counted_as_dropped() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let dropped = DroppedLogs::default();
        let mut writer = LineWriter::new(tx, dropped.clone());
        writer.write_all(b"nobody listens\n").unwrap();
        assert_eq!(dropped.take(), 0);
    }

    #[test]
    fn ignores_closed_channel() {
        let (tx, rx) = mpsc::channel(16);
        drop(rx);
        let mut writer = LineWriter::new(tx, DroppedLogs::default());
        // Must neither panic nor return an error.
        writer.write_all(b"perdu\n").unwrap();
    }

    #[test]
    fn format_line_matches_the_subscriber_format() {
        assert_eq!(format_line(Level::INFO, "bonjour"), "[INFO] bonjour");
        assert_eq!(format_line(Level::WARN, ""), "[WARN] ");
    }

    #[test]
    fn subscriber_formats_with_level_prefix() {
        let lines = capture(|| tracing::warn!(n = 3, "attention"));
        assert_eq!(lines, ["[WARN] attention n=3"]);
    }

    /// User input echoed in a log (unknown command) must not
    /// reach stderr as raw escape sequences. `tracing-subscriber` escapes
    /// control characters; this test catches a version that would stop doing so.
    #[test]
    fn subscriber_escapes_control_characters() {
        let word = "x\x1b[31mRED\x07";
        let lines = capture(|| {
            tracing::warn!("unknown command: \"{word}\"");
            tracing::warn!(word, "field");
        });
        assert_eq!(lines.len(), 2, "unexpected lines: {lines:?}");
        for line in &lines {
            assert!(
                !line.chars().any(char::is_control),
                "raw control character in log line: {line:?}"
            );
            assert!(
                line.contains("RED"),
                "input missing from log line: {line:?}"
            );
        }
    }
}
