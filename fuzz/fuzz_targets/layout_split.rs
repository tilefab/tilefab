//! `Layout::split` with arbitrary constraints, margins, spacing and areas (up to the edges
//! of the `u16` plane): no panic, one rectangle per constraint, and, as documented, the
//! rectangles stay within the area and do not overlap.

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use tilefab_core::{Constraint, Direction, Layout, Rect};

#[derive(Debug, Arbitrary)]
enum AnyConstraint {
    Length(u16),
    Min(u16),
    Max(u16),
    Percentage(u16),
    Ratio(u32, u32),
    Fill(u16),
}

impl From<&AnyConstraint> for Constraint {
    fn from(constraint: &AnyConstraint) -> Self {
        match *constraint {
            AnyConstraint::Length(n) => Self::Length(n),
            AnyConstraint::Min(n) => Self::Min(n),
            AnyConstraint::Max(n) => Self::Max(n),
            AnyConstraint::Percentage(n) => Self::Percentage(n),
            AnyConstraint::Ratio(a, b) => Self::Ratio(a, b),
            AnyConstraint::Fill(n) => Self::Fill(n),
        }
    }
}

#[derive(Debug, Arbitrary)]
struct Input {
    vertical: bool,
    constraints: Vec<AnyConstraint>,
    horizontal_margin: u16,
    vertical_margin: u16,
    spacing: u16,
    area: (u16, u16, u16, u16),
}

fn within(rect: Rect, area: Rect) -> bool {
    rect.left() >= area.left()
        && rect.right() <= area.right()
        && rect.top() >= area.top()
        && rect.bottom() <= area.bottom()
}

fuzz_target!(|input: Input| {
    let constraints: Vec<Constraint> = input.constraints.iter().take(32).map(Into::into).collect();
    let direction = if input.vertical {
        Direction::Vertical
    } else {
        Direction::Horizontal
    };
    let layout = Layout::new(direction, constraints.iter().copied())
        .horizontal_margin(input.horizontal_margin)
        .vertical_margin(input.vertical_margin)
        .spacing(input.spacing);
    let (x, y, width, height) = input.area;
    let area = Rect::new(x, y, width, height);

    let rects = layout.split(area);
    assert_eq!(
        rects.len(),
        constraints.len(),
        "one rectangle per constraint"
    );
    for (i, rect) in rects.iter().enumerate() {
        if rect.is_empty() {
            continue;
        }
        assert!(
            within(*rect, area),
            "rectangle {i} {rect:?} outside {area:?}"
        );
        for (j, other) in rects.iter().enumerate().skip(i + 1) {
            assert!(
                rect.intersection(*other).is_empty(),
                "rectangles {i} {rect:?} and {j} {other:?} overlap"
            );
        }
    }
});
