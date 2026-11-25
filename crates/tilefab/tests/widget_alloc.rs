//! Drawing `LineWidget`s in the frames of an inline `Terminal` allocates nothing: a warm frame
//! that updates and draws them stays allocation-free (see `terminal_alloc.rs` for the
//! `Terminal` side alone).
//!
//! A separate test binary, because the counting allocator is global to the whole binary:
//! it holds this single test, so no other test thread allocates during the measurement.
//! Building a widget allocates (its lists): the widgets are built before the measurement,
//! only their updates and their drawing are measured.

use std::alloc::System;

use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use tilefab::{
    Backend, Color, Entry, LineWidget, Position, Size, Style, Terminal, TerminalOptions, Viewport,
    WallTime,
};

// `stats_alloc` holds the `unsafe` code, which this project forbids in its own sources.
#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

const FRAMES: u16 = 100;
const STEPS: [&str; 3] = ["resolving", "compiling a crate", "linking"];

/// A task line with every kind of component.
fn task(name: &str) -> LineWidget {
    LineWidget::builder("B0 D1 B2 T0 B1 B2 S B2 K B2 N B2 I B2 P5 B2 V4U/ B2 L")
        .barriers([Entry::new("["), Entry::new("]"), Entry::new(" ")])
        .symbols([
            Entry::styled("🔴", Style::new().fg(Color::Red)),
            Entry::new("x"),
        ])
        .keywords([Entry::new("run"), Entry::new("done")])
        .names([Entry::new(name), Entry::new("界界")])
        .units([Entry::new("KB"), Entry::new("MB")])
        .illustration(Entry::new("█"), Entry::new("░"))
        .build()
        .expect("valid task line")
}

#[test]
fn drawing_line_widgets_in_frames_allocates_nothing() {
    let backend = Backend::with_fixed(Vec::new(), Size::new(120, 20), Position::new(0, 0));
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions::default().viewport(Viewport::Inline(3)),
    )
    .expect("in-memory terminal");
    let mut tasks = [task("alpha"), task("beta"), task("a much longer task name")];
    let frame = |i: u16, terminal: &mut Terminal<Vec<u8>>, tasks: &mut [LineWidget]| {
        for (offset, task) in (0..).step_by(7).zip(tasks.iter_mut()) {
            let step = (i + offset) % 100;
            let index = usize::from(step % 2);
            task.set_progress(f64::from(step)).unwrap();
            task.set_symbol(0, index).unwrap();
            task.set_keyword(index).unwrap();
            task.set_name(0, index).unwrap();
            task.set_value(f64::from(step) * 1.5, Some(index), Some((150.0, Some(1))))
                .unwrap();
            task.set_time(WallTime {
                year: 2026,
                month: 11,
                day: 25,
                hour: 8,
                minute: 0,
                second: u8::try_from(step % 60).unwrap(),
                millis: step * 7,
            })
            .unwrap();
            task.set_log(STEPS[usize::from(step % 3)]);
        }
        // The width changes too: the widgets are laid out again.
        terminal
            .backend_mut()
            .set_fixed_size(Size::new(40 + i % 80, 20));
        terminal
            .draw(|frame| {
                let area = frame.area();
                for (task, row) in tasks.iter().zip(area.rows()) {
                    frame.render_widget(task, row);
                }
            })
            .unwrap();
        // Keep the memory of the output, as a real terminal writer would.
        terminal.backend_mut().writer_mut().clear();
    };
    // Warm-up: every width and every entry once.
    for i in 0..FRAMES {
        frame(i, &mut terminal, &mut tasks);
    }

    let region = Region::new(GLOBAL);
    for i in 0..FRAMES {
        frame(i, &mut terminal, &mut tasks);
    }
    let stats = region.change();
    assert_eq!(
        (stats.allocations, stats.reallocations),
        (0, 0),
        "line widget frames: {stats:?}"
    );
}
