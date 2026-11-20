//! Measure the cost of rendering: heap allocations and time per frame.
//!
//! Run: `cargo run --release --example cell_alloc`.
//!
//! A counting global allocator (`stats_alloc`) wraps the system one. Two scenarios render real widgets
//! into an in-memory backend (no terminal needed):
//!
//! - **inline**: like `tilefab`, a 6-row viewport on a 100-column screen (5 task gauges
//!   and a prompt), one log line inserted above it per frame;
//! - **fullscreen**: a 200 × 50 screen with a bordered block of wrapped text and a gauge;
//! - **paragraph**: a log viewer, the `Paragraph` widget alone: 200 lines of about 150
//!   columns, wrapped in a 120 × 40 buffer and scrolled to the middle (most lines are not
//!   visible);
//! - **terminal**: the inline scenario's terminal work alone (one inserted line, a 6 × 100
//!   viewport), with text written by `Buffer::set_string`, which does not allocate: what
//!   remains is the cost of `Terminal` itself.
//!
//! Each frame changes a little (gauge ratios, a counter), as in a real application.

use std::alloc::System;
use std::fmt::Write as _;
use std::hint::black_box;
use std::time::{Duration, Instant};

use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use tilefab::{
    Backend, Block, Constraint, Gauge, Layout, LineGauge, Paragraph, Position, Size, Style,
    Terminal, TerminalOptions, Viewport, Widget, Wrap,
};

// The system allocator, counting allocations: `stats_alloc` holds the `unsafe` code,
// which this project forbids in its own sources.
#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

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

fn inline() -> Report {
    let backend = Backend::with_fixed(Vec::new(), Size::new(100, 40), Position::new(0, 0));
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(6),
            ..TerminalOptions::default()
        },
    )
    .expect("in-memory terminal");
    measure(2_000, |i| {
        terminal.insert_before(1, |buf| {
            Paragraph::new(format!("[INFO] log line {i:06}")).render(buf.area, buf);
        });
        terminal
            .draw(|frame| {
                let rows = Layout::vertical([Constraint::Length(1); 6]).split(frame.area());
                for (task, row) in rows[..5].iter().enumerate() {
                    let ratio =
                        f64::from((i + 17 * u32::try_from(task).unwrap_or(0)) % 100) / 100.0;
                    frame.render_widget(
                        LineGauge::default()
                            .ratio(ratio)
                            .label(format!("task {task}")),
                        *row,
                    );
                }
                frame.render_widget(Paragraph::new(format!("> start task {i}")), rows[5]);
            })
            .expect("draw");
        // The written bytes are not the subject here: discard them.
        black_box(std::mem::take(terminal.backend_mut().writer_mut()));
    })
}

fn fullscreen() -> Report {
    let backend = Backend::with_fixed(Vec::new(), Size::new(200, 50), Position::new(0, 0));
    let mut terminal = Terminal::new(backend).expect("in-memory terminal");
    let text = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor \
                incididunt ut labore et dolore magna aliqua. été 漢字 ";
    let text = text.repeat(40);
    measure(500, |i| {
        terminal
            .draw(|frame| {
                let [body, gauge] = Layout::vertical([Constraint::Fill(1), Constraint::Length(3)])
                    .areas(frame.area());
                frame.render_widget(
                    Paragraph::new(format!("frame {i}\n{text}"))
                        .wrap(Wrap { trim: true })
                        .block(Block::bordered().title("text")),
                    body,
                );
                frame.render_widget(
                    Gauge::default()
                        .ratio(f64::from(i % 100) / 100.0)
                        .block(Block::bordered())
                        .style(Style::new()),
                    gauge,
                );
            })
            .expect("draw");
        black_box(std::mem::take(terminal.backend_mut().writer_mut()));
    })
}

fn paragraph() -> Report {
    let mut log = String::new();
    for i in 0..200 {
        // Writing to a `String` cannot fail.
        let _ = writeln!(
            log,
            "[INFO] {i:03} request served in {} ms by worker {} for user été-{} 日本 {}",
            i * 7 % 300,
            i % 8,
            i * 31 % 1000,
            "lorem ipsum dolor sit amet ".repeat(3)
        );
    }
    let mut buf = tilefab::Buffer::empty(tilefab::Rect::new(0, 0, 120, 40));
    measure(500, |i| {
        buf.reset();
        Paragraph::new(log.as_str())
            .wrap(Wrap { trim: true })
            .scroll((u16::try_from(150 + i % 10).unwrap_or(0), 0))
            .render(buf.area, &mut buf);
        black_box(&buf);
    })
}

fn terminal_only() -> Report {
    let backend = Backend::with_fixed(Vec::new(), Size::new(100, 40), Position::new(0, 0));
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(6),
            ..TerminalOptions::default()
        },
    )
    .expect("in-memory terminal");
    let digits = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
    measure(2_000, |i| {
        let digit = digits[(i % 10) as usize];
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
        ("inline", inline()),
        ("fullscreen", fullscreen()),
        ("paragraph", paragraph()),
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
