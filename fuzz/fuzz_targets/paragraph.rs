//! `Paragraph` rendering (wrapping, trimming, scrolling, alignment, block) with arbitrary
//! text: no panic, nothing written outside the target area, the buffer invariants hold,
//! and `line_count` is enough: given one more row than it counts, the last text row (inside
//! the block, if any) stays blank.

#![no_main]

#[path = "common.rs"]
mod common;

use arbitrary::Arbitrary;
use common::{Piece, check_buffer, text};
use libfuzzer_sys::fuzz_target;
use tilefab_core::{Alignment, Buffer, Cell, Rect, Widget};
use tilefab_widget::{Block, Paragraph, Wrap};

#[derive(Debug, Arbitrary)]
struct Input {
    text: Vec<Piece>,
    width: u8,
    height: u8,
    /// `None`: no wrapping; `Some(trim)`: wrapping, trimmed or not.
    wrap: Option<bool>,
    scroll: (u8, u8),
    alignment: u8,
    block: bool,
}

/// A cell that no widget writes: shows what was left untouched.
const MARK: &str = "※";

fn paragraph<'a>(input: &Input, text: &'a str) -> Paragraph<'a> {
    let mut paragraph = Paragraph::new(text).alignment(match input.alignment % 3 {
        0 => Alignment::Left,
        1 => Alignment::Center,
        _ => Alignment::Right,
    });
    if let Some(trim) = input.wrap {
        paragraph = paragraph.wrap(Wrap { trim });
    }
    if input.block {
        paragraph = paragraph.block(Block::bordered());
    }
    paragraph
}

fuzz_target!(|input: Input| {
    let text = text(&input.text);
    let width = u16::from(input.width % 64);
    let height = u16::from(input.height % 32);

    // The paragraph's area inside a larger, marked buffer: the margin must stay marked.
    let area = Rect::new(2, 1, width, height);
    let mut buf = Buffer::filled(Rect::new(0, 0, width + 4, height + 2), Cell::new(MARK));
    paragraph(&input, &text)
        .scroll((u16::from(input.scroll.0), u16::from(input.scroll.1)))
        .render(area, &mut buf);
    check_buffer(&buf);
    for y in buf.area().top()..buf.area().bottom() {
        for x in buf.area().left()..buf.area().right() {
            let inside =
                x >= area.left() && x < area.right() && y >= area.top() && y < area.bottom();
            if !inside {
                assert_eq!(
                    buf.get(x, y).symbol(),
                    MARK,
                    "written outside the area at ({x}, {y})"
                );
            }
        }
    }

    // `line_count`, without scrolling: one more row than counted must stay blank.
    let count = paragraph(&input, &text).line_count(width);
    let Ok(rows) = u16::try_from(count + 1) else {
        return;
    };
    if width == 0 || rows > 512 {
        return;
    }
    let area = Rect::new(0, 0, width, rows);
    let mut buf = Buffer::empty(area);
    paragraph(&input, &text).render(area, &mut buf);
    check_buffer(&buf);
    // With a block, the frame fills the area: the blank row is the last one inside it.
    let (last, columns) = if input.block {
        if count == 0 {
            return;
        }
        (rows - 2, 1..width.saturating_sub(1))
    } else {
        (rows - 1, 0..width)
    };
    let blank = columns
        .into_iter()
        .all(|x| buf.get(x, last).symbol() == " ");
    assert!(
        blank,
        "line_count({width}) = {count}, but row {last} holds text: {:?}",
        buf.lines()
    );
});
