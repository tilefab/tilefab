//! Measure the cost of rendering: heap allocations and time per frame.
//!
//! Run: `cargo run --release --example cell_alloc`.
//!
//! A counting global allocator (`stats_alloc`) wraps the system one. The scenarios render real
//! widgets into an in-memory backend (no terminal needed):
//!
//! - **inline**: like `widget-demo`, a 6-row viewport on a 100-column screen (5 task lines drawn
//!   by `LineWidget`s and a prompt), one log line inserted above it per frame;
//! - **idle**: the same viewport redrawn without log lines, as the demo's ticker does most of
//!   the time;
//! - **fullscreen**: a 200 × 50 screen with rows of text and three line widgets;
//! - **line**: a `LineWidget` alone in a buffer whose width changes at every frame (0 to 119
//!   columns), so that it is laid out again each time: nominal widths, shrinking, components
//!   dropped;
//! - **terminal**: the inline scenario's terminal work alone (one inserted line, a 6 × 100
//!   viewport), with text written by `Buffer::set_string`, which does not allocate: what
//!   remains is the cost of `Terminal` itself.
//!
//! Each frame changes a little (progress, time, log text), as in a real application. The
//! widgets are built before the measurement: a warm frame that draws them allocates nothing,
//! so the scenarios show 0 allocations per frame. The rows are laid out with `Rect::rows`; a
//! `Layout` would not allocate either (see `widget_alloc.rs`).

use std::alloc::System;
use std::hint::black_box;
use std::time::{Duration, Instant};

use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use tilefab::{
    Backend, Buffer, Entry, LineWidget, Position, Rect, Size, Style, Terminal, TerminalOptions,
    Viewport, WallTime, Widget,
};

// The system allocator, counting allocations: `stats_alloc` holds the `unsafe` code,
// which this project forbids in its own sources.
#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

/// The digits written by the scenarios, to change the text without formatting it.
const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
/// The log texts the task lines go through.
const STEPS: [&str; 4] = [
    "resolving",
    "downloading crates",
    "compiling",
    "linking the binary",
];

/// Per-frame averages of one scenario.
struct Report {
    allocations: f64,
    kib: f64,
    time: Duration,
}

/// Run `frame` `frames` times after a warm-up, and return per-frame averages.
fn measure(frames: u32, mut frame: impl FnMut(u32)) -> Report {
    for i in 0..frames / 10 {
        frame(i); // warm-up: first allocations of growing buffers
    }
    let region = Region::new(GLOBAL);
    let start = Instant::now();
    for i in 0..frames {
        frame(i);
    }
    let time = start.elapsed() / frames;
    let stats = region.change();
    let n = f64::from(frames);
    #[expect(
        clippy::cast_precision_loss,
        reason = "averages of counters far below 2^52"
    )]
    Report {
        allocations: (stats.allocations + stats.reallocations) as f64 / n,
        // `bytes_allocated` already includes what reallocations add (`bytes_reallocated`
        // holds the same growth again: adding it would count it twice).
        kib: stats.bytes_allocated as f64 / n / 1024.0,
        time,
    }
}

/// A task line of the demo: `[05.123] 🟠 task 3 ██████░░░░  60% compiling`.
fn task_line(name: &str) -> LineWidget {
    LineWidget::builder("B0 T2 B1 B2 S B2 N B2 I B2 P B2 L")
        .barriers([Entry::new("["), Entry::new("]"), Entry::new(" ")])
        .symbols([Entry::new("🔴"), Entry::new("🟠"), Entry::new("🟢")])
        .names([Entry::new(name)])
        .illustration(Entry::new("█"), Entry::new("░"))
        .build()
        .expect("valid task line")
}

/// Change a task line for frame `i`: time, progress, symbol and log text.
fn animate(widget: &mut LineWidget, i: u32, offset: u32) {
    let step = (i + offset) % 100;
    let time = WallTime {
        year: 2026,
        month: 11,
        day: 25,
        hour: 12,
        minute: u8::try_from(i / 60_000 % 60).unwrap_or(0),
        second: u8::try_from(i / 1000 % 60).unwrap_or(0),
        millis: u16::try_from(i % 1000).unwrap_or(0),
    };
    widget.set_time(time).expect("valid time");
    widget.set_progress(f64::from(step)).expect("finite");
    widget
        .set_symbol(0, usize::try_from(step / 34).unwrap_or(0))
        .expect("valid symbol");
    widget.set_log(STEPS[usize::try_from(step / 25).unwrap_or(0)]);
}

/// The inline scenario, with one log line per frame when `with_log` (otherwise `idle`).
fn inline(with_log: bool) -> Report {
    let backend = Backend::with_fixed(Vec::new(), Size::new(100, 40), Position::new(0, 0));
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions::default().viewport(Viewport::Inline(6)),
    )
    .expect("in-memory terminal");
    let mut tasks: Vec<LineWidget> = (0..5).map(|n| task_line(DIGITS[n])).collect();
    measure(2_000, |i| {
        let digit = DIGITS[(i % 10) as usize];
        if with_log {
            terminal.insert_before(1, |buf| {
                buf.set_string(0, 0, "[INFO] log line ", Style::new());
                buf.set_string(16, 0, digit, Style::new());
            });
        }
        for (offset, task) in (0..).step_by(17).zip(&mut tasks) {
            animate(task, i, offset);
        }
        terminal
            .draw(|frame| {
                // One row per line: `Rect::rows` (a `Layout` would not allocate either).
                let area = frame.area();
                for (task, row) in tasks.iter().zip(area.rows()) {
                    frame.render_widget(task, row);
                }
                let prompt = area.rows().last().unwrap_or(area);
                frame
                    .buffer_mut()
                    .set_string(prompt.x, prompt.y, "> start task ", Style::new());
                frame
                    .buffer_mut()
                    .set_string(prompt.x + 13, prompt.y, digit, Style::new());
            })
            .expect("draw");
        // The written bytes are not the subject here: discard them, but keep the memory of
        // the output, as a real terminal writer would.
        black_box(terminal.backend_mut().writer_mut()).clear();
    })
}

fn fullscreen() -> Report {
    let backend = Backend::with_fixed(Vec::new(), Size::new(200, 50), Position::new(0, 0));
    let mut terminal = Terminal::new(backend).expect("in-memory terminal");
    let row = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor \
               incididunt ut labore et dolore magna aliqua. été 漢字 ";
    let mut widgets: Vec<LineWidget> = ["build", "download", "check"]
        .into_iter()
        .map(task_line)
        .collect();
    measure(500, |i| {
        for (offset, widget) in (0..).step_by(31).zip(&mut widgets) {
            animate(widget, i, offset);
        }
        terminal
            .draw(|frame| {
                // The text, then three rows for the widgets.
                let area = frame.area();
                let split = area.bottom().saturating_sub(3).max(area.y);
                let body = Rect::new(area.x, area.y, area.width, split - area.y);
                let lines = Rect::new(area.x, split, area.width, area.bottom() - split);
                for y in body.top()..body.bottom() {
                    frame.buffer_mut().set_string(body.x, y, row, Style::new());
                }
                for (widget, y) in widgets.iter().zip(lines.top()..) {
                    frame.render_widget(widget, Rect::new(lines.x, y, lines.width, 1));
                }
            })
            .expect("draw");
        black_box(terminal.backend_mut().writer_mut()).clear();
    })
}

fn line() -> Report {
    let mut widget = task_line("a task with a long name");
    let mut buf = Buffer::empty(Rect::new(0, 0, 120, 1));
    measure(5_000, |i| {
        animate(&mut widget, i, 0);
        let width = u16::try_from(i % 120).unwrap_or(0);
        (&widget).render(Rect::new(0, 0, width, 1), &mut buf);
        black_box(&buf);
    })
}

fn terminal_only() -> Report {
    let backend = Backend::with_fixed(Vec::new(), Size::new(100, 40), Position::new(0, 0));
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions::default().viewport(Viewport::Inline(6)),
    )
    .expect("in-memory terminal");
    measure(2_000, |i| {
        let digit = DIGITS[(i % 10) as usize];
        terminal.insert_before(1, |buf| {
            buf.set_string(0, 0, "[INFO] log line ", Style::new());
            buf.set_string(16, 0, digit, Style::new());
        });
        terminal
            .draw(|frame| {
                let area = frame.area();
                for y in area.top()..area.bottom() {
                    frame.buffer_mut().set_string(0, y, "task ", Style::new());
                    frame.buffer_mut().set_string(5, y, digit, Style::new());
                }
            })
            .expect("draw");
        // Clear the output but keep its memory, as a real terminal writer would.
        terminal.backend_mut().writer_mut().clear();
    })
}

fn main() {
    println!(
        "{:<12} {:>14} {:>12} {:>12}",
        "scenario", "allocs/frame", "KiB/frame", "µs/frame"
    );
    for (name, report) in [
        ("inline", inline(true)),
        ("idle", inline(false)),
        ("fullscreen", fullscreen()),
        ("line", line()),
        ("terminal", terminal_only()),
    ] {
        println!(
            "{:<12} {:>14.0} {:>12.1} {:>12.1}",
            name,
            report.allocations,
            report.kib,
            report.time.as_secs_f64() * 1e6
        );
    }
}
