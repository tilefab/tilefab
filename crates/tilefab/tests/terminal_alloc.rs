//! `Terminal` reuses its cell buffers: once warmed up, a frame allocates nothing. The one
//! exception is a symbol longer than a cell holds inline (22 bytes, some emoji sequences):
//! writing it allocates once, but its copies by the frame do not.
//!
//! A separate test binary, because the counting allocator is global to the whole binary:
//! it holds this single test, so no other test thread allocates during the measurement.

use std::alloc::System;

use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use tilefab::{Backend, Position, ScrollMode, Size, Style, Terminal, TerminalOptions, Viewport};

// `stats_alloc` holds the `unsafe` code, which this project forbids in its own sources.
#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

/// One frame: a line inserted above an inline viewport of `height` rows, then the viewport
/// drawn. Text goes through `Buffer::set_string`, which does not allocate, so any
/// allocation comes from `Terminal`.
fn frame(terminal: &mut Terminal<Vec<u8>>, i: usize, lines: usize) {
    frame_with(terminal, i, lines, None);
}

/// [`frame`], with `long` written once in the viewport when given.
fn frame_with(terminal: &mut Terminal<Vec<u8>>, i: usize, lines: usize, long: Option<&str>) {
    const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
    let digit = DIGITS[i % 10];
    for _ in 0..lines {
        terminal.insert_before(1, |buf| {
            buf.set_string(0, 0, "log ", Style::new());
            buf.set_string(4, 0, digit, Style::new());
        });
    }
    terminal
        .draw(|frame| {
            let area = frame.area();
            for y in area.top()..area.bottom() {
                frame.buffer_mut().set_string(0, y, digit, Style::new());
            }
            if let Some(long) = long {
                frame.buffer_mut().set_string(4, area.y, long, Style::new());
            }
            frame.set_cursor_position((1, area.bottom() - 1));
        })
        .expect("draw");
    // Clear the output but keep its memory, as a real terminal writer would.
    terminal.backend_mut().writer_mut().clear();
}

#[test]
fn steady_state_frames_allocate_nothing() {
    // Every scroll mode; one line per frame, then a batch larger than the screen (applied
    // in several chunks).
    for mode in [ScrollMode::Screen, ScrollMode::Region, ScrollMode::Fallback] {
        for lines in [1, 30] {
            let backend = Backend::with_fixed(Vec::new(), Size::new(80, 20), Position::new(0, 0));
            let mut terminal = Terminal::with_options(
                backend,
                TerminalOptions::default()
                    .viewport(Viewport::Inline(6))
                    .scroll_mode(mode),
            )
            .expect("in-memory terminal");
            // Warm-up: buffers and the output grow to their final size.
            for i in 0..20 {
                frame(&mut terminal, i, lines);
            }
            let region = Region::new(GLOBAL);
            for i in 0..100 {
                frame(&mut terminal, i, lines);
            }
            let stats = region.change();
            assert_eq!(
                (stats.allocations, stats.reallocations),
                (0, 0),
                "mode {mode:?}, {lines} line(s) per frame: {stats:?}"
            );
        }
    }

    // Fullscreen: double buffering, no allocation either.
    let backend = Backend::with_fixed(Vec::new(), Size::new(80, 20), Position::new(0, 0));
    let mut terminal = Terminal::new(backend).expect("in-memory terminal");
    for i in 0..20 {
        frame(&mut terminal, i, 0);
    }
    let region = Region::new(GLOBAL);
    for i in 0..100 {
        frame(&mut terminal, i, 0);
    }
    let stats = region.change();
    assert_eq!(
        (stats.allocations, stats.reallocations),
        (0, 0),
        "fullscreen: {stats:?}"
    );

    // A symbol longer than a cell holds inline: one allocation per frame, to write it; the
    // copies of its cell (into `previous`, the target and the model of the inline commit)
    // share it.
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
    assert!(family.len() > 22, "not a long symbol");
    for viewport in [Viewport::Inline(6), Viewport::Fullscreen] {
        let backend = Backend::with_fixed(Vec::new(), Size::new(80, 20), Position::new(0, 0));
        let mut terminal =
            Terminal::with_options(backend, TerminalOptions::default().viewport(viewport))
                .expect("in-memory terminal");
        for i in 0..20 {
            frame_with(&mut terminal, i, 1, Some(family));
        }
        let region = Region::new(GLOBAL);
        for i in 0..100 {
            frame_with(&mut terminal, i, 1, Some(family));
        }
        let stats = region.change();
        assert_eq!(
            (stats.allocations, stats.reallocations),
            (100, 0),
            "{viewport:?}, a long symbol per frame: {stats:?}"
        );
    }
}
