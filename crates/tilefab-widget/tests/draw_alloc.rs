//! A warm draw of a `LineWidget` allocates nothing: the layout is computed without a list,
//! and the short texts (date, time, progress) are formatted on the stack.
//!
//! A separate test binary, because the counting allocator is global to the whole binary: it
//! holds this single test, so no other test thread allocates during the measurement.

use std::alloc::System;

use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use tilefab_core::{Buffer, Rect, Widget};
use tilefab_widget::{Entry, LineWidget, WallTime};

// `stats_alloc` holds the `unsafe` code, which this project forbids in its own sources.
#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

const DRAWS: u16 = 100;

#[test]
fn a_warm_draw_allocates_nothing() {
    let mut widget = LineWidget::builder(
        "N.W+* B0 D0.C B1 T0 B0 S.:B B1 K.R B1 P5.* B1 V4U/.G/K+ B1 ID.C/K B1 L.:M",
    )
    .names([Entry::new("alpha"), Entry::new("界界")])
    .barriers([Entry::new("["), Entry::new(" ")])
    .symbols([Entry::new("🔴"), Entry::new("x")])
    .keywords([Entry::new("ok"), Entry::new("fail")])
    .units([Entry::new("KB")])
    .illustration(Entry::new("#"), Entry::new("."))
    .build()
    .unwrap();
    widget
        .set_time(WallTime {
            year: 2026,
            month: 11,
            day: 25,
            hour: 9,
            minute: 30,
            second: 1,
            millis: 2,
        })
        .unwrap();
    widget.set_progress(42.0).unwrap();
    widget
        .set_value(99.3, Some(0), Some((109.6, Some(0))))
        .unwrap();
    widget.set_log("Downloading a crate with a rather long name");
    let mut buf = Buffer::empty(Rect::new(0, 0, 200, 1));
    // Warm-up.
    (&widget).render(buf.area(), &mut buf);

    let region = Region::new(GLOBAL);
    // Every width from 0 to 99: nominal, shrunk, names shrunk, components dropped.
    for width in 0..DRAWS {
        (&widget).render(Rect::new(0, 0, width, 1), &mut buf);
    }
    let stats = region.change();
    assert_eq!(
        (stats.allocations, stats.reallocations),
        (0, 0),
        "line widget drawing: {stats:?}"
    );
}
