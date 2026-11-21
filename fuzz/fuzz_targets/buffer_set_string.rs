//! `Buffer::set_stringn` with arbitrary text, positions (in and out of the buffer) and width
//! limits, several writes over each other: no panic, the buffer invariants hold after each
//! write, and the returned position never passes the limit or the buffer edge.

#![no_main]

#[path = "common.rs"]
mod common;

use arbitrary::Arbitrary;
use common::{Piece, check_buffer, text};
use libfuzzer_sys::fuzz_target;
use tilefab_core::{Buffer, Rect, Style};

/// A small rectangle anywhere on the `u16` plane (wide enough to reach the edges, small
/// enough to keep each run fast).
#[derive(Debug, Arbitrary)]
struct SmallRect {
    x: u16,
    y: u16,
    width: u8,
    height: u8,
}

impl SmallRect {
    /// The rectangle, its sides limited to `max` cells.
    fn rect(&self, max: u16) -> Rect {
        Rect::new(
            self.x,
            self.y,
            u16::from(self.width) % (max + 1),
            u16::from(self.height) % (max + 1),
        )
    }
}

#[derive(Debug, Arbitrary)]
struct Write {
    /// Offset from the buffer's top-left corner (may land outside it).
    dx: i8,
    dy: i8,
    text: Vec<Piece>,
    max_width: Option<u8>,
}

#[derive(Debug, Arbitrary)]
struct Input {
    area: SmallRect,
    writes: Vec<Write>,
}

fuzz_target!(|input: Input| {
    let area = input.area.rect(40);
    let mut buf = Buffer::empty(area);
    for write in input.writes.iter().take(16) {
        let x = area.x.saturating_add_signed(i16::from(write.dx));
        let y = area.y.saturating_add_signed(i16::from(write.dy));
        let string = text(&write.text);
        let max_width = write.max_width.map_or(usize::MAX, usize::from);
        let (end_x, end_y) = buf.set_stringn(x, y, &string, max_width, Style::new());
        assert_eq!(end_y, y, "set_stringn moved to another row");
        assert!(end_x >= x, "set_stringn went backwards");
        let written = usize::from(end_x - x);
        assert!(
            written <= max_width,
            "{written} columns written, limit {max_width}"
        );
        if written > 0 {
            assert!(end_x <= area.right(), "written past the buffer edge");
        }
        check_buffer(&buf);
    }
});
