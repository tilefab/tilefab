//! Drawing tests of the line widget, with whole buffers.

use tilefab_core::{Alignment, Buffer, Color, Modifier, Rect, Style, Widget};

use crate::entry::text_width;
use crate::{Entry, LineWidget, WallTime};

fn entries(texts: &[&str]) -> Vec<Entry> {
    texts.iter().copied().map(Entry::new).collect()
}

/// Draw `widget` in a buffer one row high and `width` columns wide.
fn draw(widget: &LineWidget, width: u16) -> Buffer {
    let mut buf = Buffer::empty(Rect::new(0, 0, width, 1));
    widget.render(buf.area(), &mut buf);
    buf
}

/// Check that `widget` drawn in `width` columns gives `expected`, then spaces.
#[track_caller]
fn assert_row(widget: &LineWidget, width: u16, expected: &str) {
    let pad = usize::from(width) - text_width(expected);
    let mut expected_buf = Buffer::empty(Rect::new(0, 0, width, 1));
    expected_buf.set_string(
        0,
        0,
        &format!("{expected}{}", " ".repeat(pad)),
        Style::new(),
    );
    assert_eq!(draw(widget, width), expected_buf, "width {width}");
}

/// The example of the specification: `[05.123] 🔴 Downloading crate`.
fn example() -> LineWidget {
    let mut widget = LineWidget::builder("B0 T2 B1 B2 S B2 L")
        .symbols(entries(&["🔴", "🟠", "🟡", "🟢"]))
        .barriers(entries(&["[", "]", " ", "-"]))
        .build()
        .unwrap();
    widget
        .set_time(WallTime {
            year: 2026,
            month: 11,
            day: 25,
            hour: 10,
            minute: 4,
            second: 5,
            millis: 123,
        })
        .unwrap();
    widget.set_log("Downloading crate");
    widget
}

#[test]
fn the_example_at_several_widths() {
    let widget = example();
    let cases = [
        (40, "[05.123] 🔴 Downloading crate"),
        (29, "[05.123] 🔴 Downloading crate"),
        (28, "[05.123] 🔴 Downloading cra…"),
        (20, "[05.123] 🔴 Downloa…"),
        (14, "[05.123] 🔴 D…"),
        // `L` would get 1 column: it is dropped, the trailing barrier stays.
        (13, "[05.123] 🔴 "),
        (12, "[05.123] 🔴 "),
        (11, "[05.123] 🔴"),
        (10, "[05.123] "),
        (9, "[05.123] "),
        (8, "[05.123]"),
        (7, "[05.123"),
        (1, "["),
        (0, ""),
    ];
    for (width, expected) in cases {
        assert_row(&widget, width, expected);
    }
}

#[test]
fn the_symbol_changes_with_set_symbol() {
    let mut widget = example();
    widget.set_symbol(0, 3).unwrap();
    assert_row(&widget, 29, "[05.123] 🟢 Downloading crate");
}

#[test]
fn symbols_narrower_than_the_widest_are_left_justified() {
    let mut widget = LineWidget::builder("S B0")
        .symbols(entries(&["🔴", "x"]))
        .barriers(entries(&["|"]))
        .build()
        .unwrap();
    widget.set_symbol(0, 1).unwrap();
    assert_row(&widget, 5, "x |");
}

/// The six rows of the receiver table.
#[test]
fn receiver_of_the_extra_columns() {
    let build = |composition: &str| {
        let mut widget = LineWidget::builder(composition)
            .barriers(entries(&["|"]))
            .illustration(Entry::new("#"), Entry::new("."))
            .build()
            .unwrap();
        widget.set_log("ab");
        widget
    };
    // Elastic L, no I.
    assert_row(&build("L B0"), 10, "ab       |");
    // Elastic I, no L.
    assert_row(&build("I B0"), 10, ".........|");
    // Elastic L and elastic I: L takes the rest, I keeps 10.
    assert_row(&build("L I B0"), 20, "ab       ..........|");
    // Fixed L and elastic I.
    assert_row(&build("L3 I B0"), 20, "ab ................|");
    // Elastic L and fixed I.
    assert_row(&build("L I4 B0"), 20, "ab             ....|");
    // No elastic component: the natural width, the rest untouched.
    assert_row(&build("L3 I4 B0"), 20, "ab ....|");
}

#[test]
fn a_log_is_padded_only_when_something_follows() {
    let mut widget = LineWidget::builder("L").build().unwrap();
    widget.set_log("ab");
    let mut buf = Buffer::filled(Rect::new(0, 0, 5, 1), tilefab_core::Cell::new("x"));
    (&widget).render(buf.area(), &mut buf);
    assert_eq!(buf, Buffer::with_lines(["abxxx"]));
}

#[test]
fn cells_after_the_widget_are_untouched() {
    let widget = LineWidget::builder("B0")
        .barriers(entries(&["[]"]))
        .build()
        .unwrap();
    let mut buf = Buffer::filled(Rect::new(0, 0, 5, 1), tilefab_core::Cell::new("x"));
    (&widget).render(buf.area(), &mut buf);
    assert_eq!(buf, Buffer::with_lines(["[]xxx"]));
}

#[test]
fn progress_and_illustration_share_the_value() {
    let mut widget = LineWidget::builder("P I")
        .illustration(Entry::new("#"), Entry::new("."))
        .build()
        .unwrap();
    assert_row(&widget, 14, "  0%..........");
    widget.set_progress(55.0).unwrap();
    assert_row(&widget, 14, " 55%#####.....");
    widget.set_progress(100.0).unwrap();
    assert_row(&widget, 14, "100%##########");
    // The illustration shrinks with the width, down to 2.
    widget.set_progress(50.0).unwrap();
    assert_row(&widget, 8, " 50%##..");
    assert_row(&widget, 6, " 50%#.");
    assert_row(&widget, 5, " 50%");
}

#[test]
fn value_is_right_justified_as_a_whole() {
    let mut widget = LineWidget::builder("V5U/")
        .units(entries(&["b", "KB"]))
        .build()
        .unwrap();
    widget
        .set_value(99.3, Some(1), Some((109.6, Some(1))))
        .unwrap();
    assert_row(&widget, 15, " 99.3KB/109.6KB");
    widget
        .set_value(5.0, Some(0), Some((10.0, Some(0))))
        .unwrap();
    assert_row(&widget, 15, "     5.0b/10.0b");
    let mut plain = LineWidget::builder("V3 B0")
        .barriers(entries(&["|"]))
        .build()
        .unwrap();
    assert_row(&plain, 4, "0.0|");
    plain.set_value(7.25, None, None).unwrap();
    assert_row(&plain, 4, "7.2|");
}

#[test]
fn keyword_alignment() {
    let build = |alignment| {
        LineWidget::builder("K B0")
            .keywords(entries(&["ok", "fail", "ab"]))
            .barriers(entries(&["|"]))
            .keyword_alignment(alignment)
            .build()
            .unwrap()
    };
    assert_row(&build(Alignment::Left), 5, "ok  |");
    assert_row(&build(Alignment::Right), 5, "  ok|");
    assert_row(&build(Alignment::Center), 5, " ok |");
    let mut widget = LineWidget::builder("K B0")
        .keywords(entries(&["abc", "ab"]))
        .barriers(entries(&["|"]))
        .keyword_alignment(Alignment::Center)
        .build()
        .unwrap();
    widget.set_keyword(1).unwrap();
    // One free column: it goes to the right.
    assert_row(&widget, 4, "ab |");
}

#[test]
fn date_and_time() {
    let mut widget = LineWidget::builder("D0 B0 T0")
        .barriers(entries(&[" "]))
        .build()
        .unwrap();
    assert_row(&widget, 23, "0000-00-00 00:00:00.000");
    widget
        .set_time(WallTime {
            year: 2024,
            month: 2,
            day: 29,
            hour: 23,
            minute: 59,
            second: 58,
            millis: 7,
        })
        .unwrap();
    assert_row(&widget, 23, "2024-02-29 23:59:58.007");
}

#[test]
fn names_change_width_and_the_line_is_laid_out_again() {
    let mut widget = LineWidget::builder("N B0 L")
        .names(entries(&["alpha", "be"]))
        .barriers(entries(&["|"]))
        .build()
        .unwrap();
    widget.set_log("log");
    assert_row(&widget, 12, "alpha|log");
    widget.set_name(0, 1).unwrap();
    assert_row(&widget, 12, "be|log");
    widget.set_name(0, 0).unwrap();
    assert_row(&widget, 12, "alpha|log");
    // Narrower: L at 2 first, then the name shrinks.
    assert_row(&widget, 8, "alpha|l…");
    assert_row(&widget, 7, "alp…|l…");
    assert_row(&widget, 5, "a…|l…");
    assert_row(&widget, 4, "al…|");
}

#[test]
fn wide_characters_at_the_right_edge() {
    let widget = LineWidget::builder("B0 N")
        .names(entries(&["界界界"]))
        .barriers(entries(&["["]))
        .build()
        .unwrap();
    assert_row(&widget, 7, "[界界界");
    assert_row(&widget, 6, "[界界…");
    assert_row(&widget, 5, "[界… ");
    assert_row(&widget, 3, "[… ");
    let mut log = LineWidget::builder("L").build().unwrap();
    log.set_log("a🔴b");
    assert_row(&log, 4, "a🔴b");
    assert_row(&log, 3, "a… ");
}

#[test]
fn every_value_has_its_style() {
    let red = Style::new().fg(Color::Red);
    let green = Style::new().fg(Color::Green);
    let blue = Style::new().bg(Color::Blue);
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let mut widget = LineWidget::builder("S B0 K N D2 T4 P V2U/ I2 L")
        .symbols([Entry::styled("界", red), Entry::styled("x", green)])
        .barriers([Entry::styled("|", blue)])
        .keywords([Entry::styled("ok", bold), Entry::new("fail")])
        .names([Entry::styled("n", green)])
        .units([Entry::styled("u", blue)])
        .illustration(Entry::styled("#", red), Entry::styled(".", green))
        .date_style(red)
        .time_style(green)
        .progress_style(blue)
        .value_styles(bold, red)
        .log_style(green)
        .build()
        .unwrap();
    widget.set_symbol(0, 1).unwrap();
    widget.set_progress(50.0).unwrap();
    widget.set_log("z");
    // x_|ok  nMM-DDHH:MM  50%  0u/ 0u#.z
    let buf = draw(&widget, 40);
    let style_at = |x: u16| {
        let cell = buf.get(x, 0);
        (cell.fg, cell.bg, cell.modifier)
    };
    let expect = |x: u16, style: Style| {
        let mut cell = tilefab_core::Cell::default();
        cell.set_style(style);
        assert_eq!(style_at(x), (cell.fg, cell.bg, cell.modifier), "column {x}");
    };
    // S: "x" and its padding, in the style of the entry shown.
    expect(0, green);
    expect(1, green);
    expect(2, blue);
    // K: "ok" and its padding.
    for x in 3..7 {
        expect(x, bold);
    }
    expect(7, green);
    for x in 8..13 {
        expect(x, red);
    }
    for x in 13..18 {
        expect(x, green);
    }
    for x in 18..22 {
        expect(x, blue);
    }
    // V2U/: " 0.0" does not fit in 2: "0", unit, "/", "0", unit, right-justified in 7.
    for x in 22..24 {
        expect(x, bold);
    }
    expect(24, bold);
    expect(25, blue);
    expect(26, red);
    expect(27, bold);
    expect(28, blue);
    // I2 at 50 %: one done, one todo.
    expect(29, red);
    expect(30, green);
    expect(31, green);
    assert_eq!(buf.get(31, 0).symbol(), "z");
}

#[test]
fn small_or_empty_areas_do_not_panic() {
    let widget = example();
    for (width, height) in [(0, 1), (1, 1), (2, 0), (0, 0), (1, 3)] {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        (&widget).render(buf.area(), &mut buf);
    }
    // An area partly or wholly outside the buffer is clipped.
    let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
    (&widget).render(Rect::new(5, 0, 40, 1), &mut buf);
    (&widget).render(Rect::new(50, 5, 40, 1), &mut buf);
    // The widget draws on the first row of a taller area, at its position.
    let mut tall = Buffer::empty(Rect::new(0, 0, 12, 3));
    (&widget).render(Rect::new(0, 1, 12, 2), &mut tall);
    assert_eq!(
        tall,
        Buffer::with_lines(["            ", "[05.123] 🔴 ", "            "])
    );
}

#[test]
fn a_draw_is_the_same_after_an_invalid_setter_call() {
    let mut widget = LineWidget::builder("S N D0 T0 P V3U/ K I L")
        .symbols(entries(&["a", "b"]))
        .names(entries(&["one", "two"]))
        .units(entries(&["u"]))
        .keywords(entries(&["k"]))
        .illustration(Entry::new("#"), Entry::new("."))
        .build()
        .unwrap();
    widget.set_progress(30.0).unwrap();
    let before = draw(&widget, 60);
    assert!(widget.set_symbol(0, 2).is_err());
    assert!(widget.set_symbol(1, 0).is_err());
    assert!(widget.set_name(0, 9).is_err());
    assert!(widget.set_keyword(1).is_err());
    assert!(widget.set_progress(f64::NAN).is_err());
    assert!(
        widget
            .set_value(f64::INFINITY, Some(0), Some((1.0, Some(0))))
            .is_err()
    );
    assert!(widget.set_value(1.0, None, None).is_err());
    assert!(
        widget
            .set_time(WallTime {
                month: 2,
                day: 30,
                ..WallTime::default()
            })
            .is_err()
    );
    assert_eq!(draw(&widget, 60), before);
}
