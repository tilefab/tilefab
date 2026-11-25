//! `LineWidget` construction, setters and drawing with an arbitrary composition, lists, setter
//! calls and widths: `build` and the setters never panic, nor does the drawing; the buffer
//! invariants hold, and nothing is written outside the first row of the area.

#![no_main]

#[path = "common.rs"]
mod common;

use arbitrary::Arbitrary;
use common::{Piece, check_buffer, text};
use libfuzzer_sys::fuzz_target;
use tilefab_core::{Alignment, Buffer, Cell, Rect, Widget};
use tilefab_widget::{Entry, LineWidget, WallTime};

/// Tokens of the grammar, valid or nearly so, that random bytes would rarely produce.
const TOKENS: &[&str] = &[
    "S", "S0", "S9", "B0", "B1", "B9", "D0", "D2", "T0", "T5", "P", "P5", "P9", "K", "K1", "N",
    "N0", "N14", "L", "L2", "L40", "V", "V5U/", "VU", "V/", "V999", "I", "I2", "I20", "X", "s",
    "P1", "L1", "N100", "",
];

/// One part of the composition.
#[derive(Debug, Arbitrary)]
enum Part {
    Token(u8),
    Raw(String),
}

/// A setter call.
#[derive(Debug, Arbitrary)]
enum Call {
    Symbol(u8, u8),
    Keyword(u8),
    Name(u8, u8),
    Progress(f64),
    Value(f64, Option<u8>, Option<(f64, Option<u8>)>),
    Time(u16, u8, u8, u8, u8, u8, u16),
    Log(Vec<Piece>),
}

#[derive(Debug, Arbitrary)]
struct Input {
    composition: Vec<Part>,
    /// Separator between the parts: one space, or arbitrary text.
    separator: Option<String>,
    symbols: Option<Vec<Vec<Piece>>>,
    barriers: Option<Vec<Vec<Piece>>>,
    keywords: Option<Vec<Vec<Piece>>>,
    names: Option<Vec<Vec<Piece>>>,
    units: Option<Vec<Vec<Piece>>>,
    illustration: Option<(Vec<Piece>, Vec<Piece>)>,
    alignment: u8,
    decimals: u8,
    calls: Vec<Call>,
    widths: Vec<u8>,
}

/// A cell that the widget never writes: shows what was left untouched.
const MARK: &str = "※";

fn entries(list: Option<&Vec<Vec<Piece>>>) -> Option<Vec<Entry>> {
    list.map(|list| list.iter().map(|pieces| Entry::new(text(pieces))).collect())
}

fn composition(input: &Input) -> String {
    let separator = input.separator.as_deref().unwrap_or(" ");
    let parts: Vec<String> = input
        .composition
        .iter()
        .map(|part| match part {
            Part::Token(index) => TOKENS[usize::from(*index) % TOKENS.len()].to_string(),
            Part::Raw(raw) => raw.clone(),
        })
        .collect();
    parts.join(separator)
}

fn call(widget: &mut LineWidget, call: &Call) {
    // Errors are expected: only panics are bugs.
    let _ = match call {
        Call::Symbol(rank, index) => widget.set_symbol(usize::from(*rank), usize::from(*index)),
        Call::Keyword(index) => widget.set_keyword(usize::from(*index)),
        Call::Name(rank, index) => widget.set_name(usize::from(*rank), usize::from(*index)),
        Call::Progress(value) => widget.set_progress(*value),
        Call::Value(value, unit, total) => widget.set_value(
            *value,
            unit.map(usize::from),
            total.map(|(total, unit)| (total, unit.map(usize::from))),
        ),
        Call::Time(year, month, day, hour, minute, second, millis) => widget.set_time(WallTime {
            year: *year,
            month: *month,
            day: *day,
            hour: *hour,
            minute: *minute,
            second: *second,
            millis: *millis,
        }),
        Call::Log(pieces) => {
            widget.set_log(&text(pieces));
            Ok(())
        }
    };
}

fuzz_target!(|input: Input| {
    let mut builder = LineWidget::builder(composition(&input))
        .keyword_alignment(match input.alignment % 3 {
            0 => Alignment::Left,
            1 => Alignment::Center,
            _ => Alignment::Right,
        })
        .value_decimals(input.decimals % 12);
    if let Some(list) = entries(input.symbols.as_ref()) {
        builder = builder.symbols(list);
    }
    if let Some(list) = entries(input.barriers.as_ref()) {
        builder = builder.barriers(list);
    }
    if let Some(list) = entries(input.keywords.as_ref()) {
        builder = builder.keywords(list);
    }
    if let Some(list) = entries(input.names.as_ref()) {
        builder = builder.names(list);
    }
    if let Some(list) = entries(input.units.as_ref()) {
        builder = builder.units(list);
    }
    if let Some((done, todo)) = &input.illustration {
        builder = builder.illustration(Entry::new(text(done)), Entry::new(text(todo)));
    }
    let Ok(mut widget) = builder.build() else {
        return;
    };
    let _ = widget.warnings();

    for (i, step) in input.calls.iter().enumerate() {
        call(&mut widget, step);
        let width = u16::from(input.widths.get(i).copied().unwrap_or(80));
        // The widget's area (two rows) inside a larger, marked buffer: only the first row of
        // the area may change.
        let area = Rect::new(2, 1, width, 2);
        let mut buf = Buffer::filled(Rect::new(0, 0, width + 4, 4), Cell::new(MARK));
        (&widget).render(area, &mut buf);
        check_buffer(&buf);
        for y in buf.area().top()..buf.area().bottom() {
            for x in buf.area().left()..buf.area().right() {
                let row = y == area.y && x >= area.left() && x < area.right();
                if !row {
                    assert_eq!(
                        buf.get(x, y).symbol(),
                        MARK,
                        "written outside the first row of the area at ({x}, {y})"
                    );
                }
            }
        }
    }
});
