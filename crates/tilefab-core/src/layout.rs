//! Geometry (`Position`, `Size`, `Rect`, `Margin`) and area splitting (`Layout`, `Constraint`).
//!
//! # Splitting algorithm (without a solver)
//!
//! No constraint solver: a simple deterministic algorithm runs in four steps along the
//! split axis (width for `horizontal`, height for `vertical`). Let `available` be the area
//! length (after subtracting margins) minus the spacing between elements.
//!
//! 1. **Each constraint's request**:
//!    `Length(n)`, `Min(n)`, and `Max(n)` request `n`; `Percentage(p)` and `Ratio(a, b)` request
//!    the corresponding share of `available`; `Fill(_)` requests nothing.
//!    Proportional shares are rounded using the largest remainder method: round
//!    each share down, then redistribute lost cells (up to the integer part of the exact
//!    sum) to shares with the largest fractional parts, breaking ties in favor of the
//!    first. Thus `[Percentage(50), Percentage(50)]` always fills the area exactly.
//! 2. **Allocation in order**: each constraint receives its request, limited by the remaining
//!    space. If the area is too small, the first constraints are served and the last ones
//!    are reduced (down to 0). No panics, no negative sizes.
//! 3. **Sharing the remainder**: leftover space goes to `Fill` constraints by weight (the same
//!    largest remainder method, so the sum is exact); without a nonzero-weight `Fill`, it goes
//!    to `Min` constraints equally. Otherwise the remainder stays empty **at the end of the
//!    area** (start-aligned behavior, `Flex::Start`).
//! 4. **Positions**: rectangles follow in order, separated by the spacing.
//!
//! Notes: `Max(n)` behaves like `Length(n)`, reduced when space is scarce
//! (it never grows); percentages use the available space after spacing, not the whole
//! area; when space is scarce, the last constraints are clipped regardless of type;
//! no `Flex` mode other than `Start`.

use super::convert::saturating_u16;

/// A position in a cell grid: column `x`, row `y`, zero-based.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position {
    /// Column (zero-based).
    pub x: u16,
    /// Row (zero-based).
    pub y: u16,
}

impl Position {
    /// Position at column `x`, row `y`.
    #[must_use]
    pub const fn new(x: u16, y: u16) -> Self {
        Self { x, y }
    }
}

impl From<(u16, u16)> for Position {
    fn from((x, y): (u16, u16)) -> Self {
        Self { x, y }
    }
}

/// A size in cells.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Size {
    /// Number of columns.
    pub width: u16,
    /// Number of rows.
    pub height: u16,
}

impl Size {
    /// Size of `width` columns by `height` rows.
    #[must_use]
    pub const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }
}

impl From<(u16, u16)> for Size {
    fn from((width, height): (u16, u16)) -> Self {
        Self { width, height }
    }
}

/// Inner margin: `horizontal` cells on the left **and** right, `vertical` on the top **and** bottom.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Margin {
    /// Cells removed on the left and right.
    pub horizontal: u16,
    /// Cells removed on the top and bottom.
    pub vertical: u16,
}

impl Margin {
    /// Margin of `horizontal` cells on each side, `vertical` on the top and bottom.
    #[must_use]
    pub const fn new(horizontal: u16, vertical: u16) -> Self {
        Self {
            horizontal,
            vertical,
        }
    }
}

/// A rectangle of cells: top-left corner `(x, y)` and dimensions.
///
/// Calculations saturate (no `u16` overflow, no panics): a rectangle
/// that would overflow is simply clipped.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rect {
    /// Column (zero-based).
    pub x: u16,
    /// Row (zero-based).
    pub y: u16,
    /// Number of columns.
    pub width: u16,
    /// Number of rows.
    pub height: u16,
}

impl Rect {
    /// Rectangle of `width` × `height` cells with its top-left corner at `(x, y)`.
    /// Clipped so the right and bottom edges do not exceed `u16::MAX`.
    #[must_use]
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        let width = if width > u16::MAX - x {
            u16::MAX - x
        } else {
            width
        };
        let height = if height > u16::MAX - y {
            u16::MAX - y
        } else {
            height
        };
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Number of cells.
    #[must_use]
    pub const fn area(self) -> u32 {
        self.width as u32 * self.height as u32
    }

    /// True if width or height is zero.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// First column of the rectangle.
    #[must_use]
    pub const fn left(self) -> u16 {
        self.x
    }

    /// First column *outside* the rectangle.
    #[must_use]
    pub const fn right(self) -> u16 {
        self.x.saturating_add(self.width)
    }

    /// First row of the rectangle.
    #[must_use]
    pub const fn top(self) -> u16 {
        self.y
    }

    /// First row *outside* the rectangle.
    #[must_use]
    pub const fn bottom(self) -> u16 {
        self.y.saturating_add(self.height)
    }

    /// The top-left corner.
    #[must_use]
    pub const fn as_position(self) -> Position {
        Position::new(self.x, self.y)
    }

    /// The dimensions, without the position.
    #[must_use]
    pub const fn as_size(self) -> Size {
        Size::new(self.width, self.height)
    }

    /// True if cell `(x, y)` is inside the rectangle.
    #[must_use]
    pub const fn contains(self, position: Position) -> bool {
        position.x >= self.left()
            && position.x < self.right()
            && position.y >= self.top()
            && position.y < self.bottom()
    }

    /// Intersection of two rectangles (empty, size 0, if they do not overlap).
    #[must_use]
    pub fn intersection(self, other: Rect) -> Rect {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = self.right().min(other.right());
        let y2 = self.bottom().min(other.bottom());
        Rect {
            x: x1,
            y: y1,
            width: x2.saturating_sub(x1),
            height: y2.saturating_sub(y1),
        }
    }

    /// Inner area, inset by `margin` cells on each edge. If the margins exceed
    /// the rectangle, the result is empty (size 0) and stays at the top-left corner.
    #[must_use]
    pub fn inner(self, margin: Margin) -> Rect {
        let double_h = margin.horizontal.saturating_mul(2);
        let double_v = margin.vertical.saturating_mul(2);
        if self.width < double_h || self.height < double_v {
            return Rect::new(self.x, self.y, 0, 0);
        }
        Rect::new(
            self.x.saturating_add(margin.horizontal),
            self.y.saturating_add(margin.vertical),
            self.width - double_h,
            self.height - double_v,
        )
    }

    /// The rectangle's rows, top to bottom: each is a one-row-high rectangle.
    pub fn rows(self) -> impl Iterator<Item = Rect> {
        (self.top()..self.bottom()).map(move |y| Rect::new(self.x, y, self.width, 1))
    }
}

impl From<Size> for Rect {
    fn from(size: Size) -> Self {
        Rect::new(0, 0, size.width, size.height)
    }
}

/// Constraint on an element's size along the split axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Constraint {
    /// Exactly `n` cells (fewer if space is scarce).
    Length(u16),
    /// At least `n` cells; also receives a share of the remainder if there is no `Fill`.
    Min(u16),
    /// At most `n` cells: requests `n`, like `Length`, and never grows.
    Max(u16),
    /// Percentage of the available space.
    Percentage(u16),
    /// Fraction `a / b` of the available space (`b = 0`: 0 cells).
    Ratio(u32, u32),
    /// Take a share of the remainder proportional to its weight.
    Fill(u16),
}

/// Split axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Elements side by side, left to right: split the width.
    Horizontal,
    /// Elements stacked top to bottom: split the height.
    Vertical,
}

/// Split an area into rectangles according to constraints (algorithm in the module header).
///
/// ```
/// use tilefab_core::{Constraint, Layout, Rect};
///
/// let area = Rect::new(0, 0, 20, 10);
/// let [top, bottom] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
/// assert_eq!(top, Rect::new(0, 0, 20, 9));
/// assert_eq!(bottom, Rect::new(0, 9, 20, 1));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    direction: Direction,
    constraints: Constraints,
    margin: Margin,
    spacing: u16,
}

/// Number of constraints a [`Layout`] keeps without allocating.
const INLINE: usize = 8;

/// The constraints of a [`Layout`]: up to [`INLINE`] kept inline, so that a layout built at
/// every frame (`Layout::vertical([…])`) allocates nothing.
#[derive(Clone)]
enum Constraints {
    Inline {
        items: [Constraint; INLINE],
        len: usize,
    },
    Heap(Vec<Constraint>),
}

impl Constraints {
    fn collect(constraints: impl IntoIterator<Item = Constraint>) -> Self {
        let mut constraints = constraints.into_iter();
        let mut items = [Constraint::Length(0); INLINE];
        for (len, slot) in items.iter_mut().enumerate() {
            match constraints.next() {
                Some(constraint) => *slot = constraint,
                None => return Self::Inline { items, len },
            }
        }
        match constraints.next() {
            None => Self::Inline { items, len: INLINE },
            Some(next) => {
                let mut all = items.to_vec();
                all.push(next);
                all.extend(constraints);
                Self::Heap(all)
            }
        }
    }

    fn as_slice(&self) -> &[Constraint] {
        match self {
            Self::Inline { items, len } => &items[..*len],
            Self::Heap(all) => all,
        }
    }
}

impl PartialEq for Constraints {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Eq for Constraints {}

impl std::fmt::Debug for Constraints {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_slice().fmt(f)
    }
}

impl Layout {
    /// Split in `direction`, one element per constraint.
    pub fn new(direction: Direction, constraints: impl IntoIterator<Item = Constraint>) -> Self {
        Self {
            direction,
            constraints: Constraints::collect(constraints),
            margin: Margin::default(),
            spacing: 0,
        }
    }

    /// Split into rows, top to bottom.
    pub fn vertical(constraints: impl IntoIterator<Item = Constraint>) -> Self {
        Self::new(Direction::Vertical, constraints)
    }

    /// Split into columns, left to right.
    pub fn horizontal(constraints: impl IntoIterator<Item = Constraint>) -> Self {
        Self::new(Direction::Horizontal, constraints)
    }

    /// Equal margin on all four sides of the split area.
    #[must_use]
    pub fn margin(mut self, margin: u16) -> Self {
        self.margin = Margin::new(margin, margin);
        self
    }

    /// Left and right margins of the split area.
    #[must_use]
    pub fn horizontal_margin(mut self, margin: u16) -> Self {
        self.margin.horizontal = margin;
        self
    }

    /// Top and bottom margins of the split area.
    #[must_use]
    pub fn vertical_margin(mut self, margin: u16) -> Self {
        self.margin.vertical = margin;
        self
    }

    /// Number of empty cells between consecutive elements. If there is not enough room,
    /// the spacing is reduced.
    #[must_use]
    pub fn spacing(mut self, spacing: u16) -> Self {
        self.spacing = spacing;
        self
    }

    /// Split `area`: one rectangle per constraint, in order. Rectangles do not
    /// overlap, stay within `area`, and may be empty (size 0).
    ///
    /// Allocates the returned `Vec` only; [`Layout::areas`] allocates nothing.
    #[must_use]
    pub fn split(&self, area: Rect) -> Vec<Rect> {
        let mut rects = vec![Rect::default(); self.constraints.as_slice().len()];
        self.split_into(area, &mut rects);
        rects
    }

    /// Like [`Layout::split`], with an array whose size is known at compile time:
    /// `let [a, b] = layout.areas(area);`. Allocates nothing, up to 8 constraints (the
    /// layout itself included), so it fits a frame drawn at every tick.
    ///
    /// # Panics
    ///
    /// If `N` is not the number of constraints.
    #[must_use]
    pub fn areas<const N: usize>(&self, area: Rect) -> [Rect; N] {
        let count = self.constraints.as_slice().len();
        assert!(
            count == N,
            "Layout::areas::<{N}> called with {count} constraints"
        );
        let mut rects = [Rect::default(); N];
        self.split_into(area, &mut rects);
        rects
    }

    /// Fill `rects`, one per constraint (as many), with the split of `area`.
    fn split_into(&self, area: Rect, rects: &mut [Rect]) {
        let constraints = self.constraints.as_slice();
        let area = area.inner(self.margin);
        let (start, length) = match self.direction {
            Direction::Horizontal => (area.x, area.width),
            Direction::Vertical => (area.y, area.height),
        };

        let gaps = constraints.len().saturating_sub(1);
        // Effective spacing: reduced if the `gaps` gaps do not fit in the area.
        let spacing = usize::from(length)
            .checked_div(gaps)
            .map_or(0, |per_gap| self.spacing.min(saturating_u16(per_gap)));
        let available = length - saturating_u16(gaps).wrapping_mul(spacing).min(length);
        // `gaps * spacing <= length <= u16::MAX`, so the u16 product cannot overflow;
        // the `min` above is just a guard.

        // The sizes on the stack, unless there are many constraints.
        let mut inline = [0u16; INLINE];
        let mut heap = Vec::new();
        let sizes: &mut [u16] = if constraints.len() <= INLINE {
            &mut inline[..constraints.len()]
        } else {
            heap.resize(constraints.len(), 0);
            &mut heap
        };
        solve(constraints, available, sizes);

        let mut position = start;
        for (rect, &size) in rects.iter_mut().zip(sizes.iter()) {
            *rect = match self.direction {
                Direction::Horizontal => Rect::new(position, area.y, size, area.height),
                Direction::Vertical => Rect::new(area.x, position, area.width, size),
            };
            position = position.saturating_add(size).saturating_add(spacing);
        }
    }
}

/// Write into `sizes` (one per constraint) the size in cells of each constraint for
/// `available` cells. See the module header. The sum of sizes never exceeds `available`.
/// Allocates nothing.
fn solve(constraints: &[Constraint], available: u16, sizes: &mut [u16]) {
    let avail = u64::from(available);

    // 1. Requests: proportional constraints share the available space by the largest
    // remainder method; a request above `u16::MAX` is capped, which changes nothing since
    // step 2 limits it to the remaining space.
    let share = |i: usize| match constraints[i] {
        // The product and quotient are exact when the result is an integer.
        Constraint::Percentage(p) => Some(exact_f64(avail * u64::from(p)) / 100.0),
        Constraint::Ratio(_, 0) => None,
        Constraint::Ratio(a, b) => Some(exact_f64(avail * u64::from(a)) / f64::from(b)),
        _ => None,
    };
    for (size, constraint) in sizes.iter_mut().zip(constraints) {
        *size = match *constraint {
            Constraint::Length(n) | Constraint::Min(n) | Constraint::Max(n) => n,
            _ => 0,
        };
    }
    largest_remainder(constraints.len(), share, |i, size| {
        sizes[i] = u16::try_from(size).unwrap_or(u16::MAX);
    });

    // 2. Allocate in order, limited by remaining space.
    let mut remaining = available;
    for size in sizes.iter_mut() {
        *size = (*size).min(remaining);
        remaining -= *size;
    }

    // 3. Share the remainder: among Fill constraints (by weight), otherwise Min constraints
    // (equally).
    if remaining > 0 {
        let has_fill = constraints
            .iter()
            .any(|c| matches!(c, Constraint::Fill(w) if *w > 0));
        let weight = |i: usize| match constraints[i] {
            Constraint::Fill(w) if has_fill && w > 0 => Some(f64::from(w)),
            Constraint::Min(_) if !has_fill => Some(1.0),
            _ => None,
        };
        let total: f64 = (0..constraints.len()).filter_map(weight).sum();
        if total > 0.0 {
            let extra = u64::from(remaining);
            // Force the sum to exactly `remaining` (the method distributes only the integer
            // part of the sum, which equals `remaining` up to floating-point error).
            largest_remainder(
                constraints.len(),
                |i| weight(i).map(|w| exact_f64(extra) * w / total),
                |i, more| {
                    // At most `remaining` in all, a `u16`: the fallback is never used.
                    let more = u16::try_from(more).unwrap_or(u16::MAX);
                    sizes[i] = sizes[i].saturating_add(more);
                },
            );
        }
    }
}

/// Convert an integer to `f64` without rounding.
///
/// Values reaching here are below 2^48 (`u16 * u32` at most), well under the 2^53 limit
/// where `f64` stops representing every integer.
#[expect(
    clippy::cast_precision_loss,
    reason = "callers pass values below 2^53, which f64 represents exactly"
)]
fn exact_f64(n: u64) -> f64 {
    debug_assert!(n < 1 << 53, "{n} is not exactly representable as f64");
    n as f64
}

/// Round the exact shares of the elements `0..len` that take part (`exact` gives `Some`) to
/// integers whose sum is the integer part of the exact sum: round each share down, then give
/// one more to the `missing` shares with the largest fractional parts (ties favor the first).
/// `give(i, share)` receives the rounded share of each element taking part.
///
/// Without storage: the shares are computed again rather than kept, and a share gets one
/// more when fewer than `missing` shares come before it in that order. Quadratic in `len`,
/// the number of constraints of a layout.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "shares are finite, non-negative and below 2^48: `floor` gives an exact u64"
)]
fn largest_remainder(
    len: usize,
    exact: impl Fn(usize) -> Option<f64>,
    mut give: impl FnMut(usize, u64),
) {
    let shares = || (0..len).filter_map(|i| exact(i).map(|share| (i, share)));
    // The small tolerance absorbs floating-point summation error (e.g. 0.1 + 0.2).
    let target = (shares().map(|(_, share)| share).sum::<f64>() + 1e-6).floor() as u64;
    let floors: u64 = shares().map(|(_, share)| share.floor() as u64).sum();
    let missing = target.saturating_sub(floors);
    for (i, share) in shares() {
        let fraction = share.fract();
        // Shares before this one: a larger fractional part, or the same one earlier.
        let before = shares()
            .filter(|&(j, other)| match other.fract().partial_cmp(&fraction) {
                Some(std::cmp::Ordering::Greater) => true,
                Some(std::cmp::Ordering::Equal) => j < i,
                _ => false,
            })
            .count();
        give(
            i,
            share.floor() as u64 + u64::from((before as u64) < missing),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Constraint::*;

    fn lens(constraints: &[Constraint], width: u16) -> Vec<u16> {
        Layout::horizontal(constraints.iter().copied())
            .split(Rect::new(0, 0, width, 1))
            .iter()
            .map(|r| r.width)
            .collect()
    }

    // ------------------------------------------------------------------------ geometry

    #[test]
    fn new_clamps_to_u16_range() {
        let r = Rect::new(u16::MAX - 1, u16::MAX - 2, 10, 10);
        assert_eq!((r.right(), r.bottom()), (u16::MAX, u16::MAX));
        assert_eq!((r.width, r.height), (1, 2));
    }

    #[test]
    fn edges_and_area() {
        let r = Rect::new(2, 3, 4, 5);
        assert_eq!((r.left(), r.right(), r.top(), r.bottom()), (2, 6, 3, 8));
        assert_eq!(r.area(), 20);
        assert!(!r.is_empty());
        assert!(Rect::new(0, 0, 0, 5).is_empty());
    }

    #[test]
    fn contains_excludes_right_and_bottom_edges() {
        let r = Rect::new(1, 1, 2, 2);
        assert!(r.contains(Position::new(1, 1)));
        assert!(r.contains(Position::new(2, 2)));
        assert!(!r.contains(Position::new(3, 1)));
        assert!(!r.contains(Position::new(1, 3)));
        assert!(!r.contains(Position::new(0, 1)));
    }

    #[test]
    fn intersection_overlap_and_disjoint() {
        let a = Rect::new(0, 0, 4, 4);
        let b = Rect::new(2, 2, 4, 4);
        assert_eq!(a.intersection(b), Rect::new(2, 2, 2, 2));
        let far = Rect::new(10, 10, 2, 2);
        assert!(a.intersection(far).is_empty());
        assert_eq!(a.intersection(a), a);
    }

    #[test]
    fn inner_shrinks_on_every_side() {
        let r = Rect::new(1, 2, 10, 6);
        assert_eq!(r.inner(Margin::new(2, 1)), Rect::new(3, 3, 6, 4));
        assert_eq!(r.inner(Margin::default()), r);
        // Margin consuming exactly everything: empty.
        assert!(r.inner(Margin::new(5, 0)).is_empty());
        assert_eq!(r.inner(Margin::new(5, 0)).height, 6);
    }

    #[test]
    fn inner_with_oversized_margin_is_empty_and_does_not_panic() {
        let r = Rect::new(4, 4, 3, 3);
        let inner = r.inner(Margin::new(2, 2));
        assert!(inner.is_empty());
        assert_eq!((inner.x, inner.y), (4, 4));
        assert!(r.inner(Margin::new(u16::MAX, u16::MAX)).is_empty());
    }

    #[test]
    fn rows_yields_one_rect_per_line() {
        let rows: Vec<Rect> = Rect::new(2, 5, 4, 3).rows().collect();
        assert_eq!(
            rows,
            [
                Rect::new(2, 5, 4, 1),
                Rect::new(2, 6, 4, 1),
                Rect::new(2, 7, 4, 1)
            ]
        );
        assert_eq!(Rect::new(0, 0, 5, 0).rows().count(), 0);
    }

    // ------------------------------------------------------------------------- layout

    #[test]
    fn fixed_sizes_follow_each_other_and_leave_the_rest_empty_at_the_end() {
        let rects = Layout::horizontal([Length(3), Length(4)]).split(Rect::new(10, 2, 20, 5));
        assert_eq!(rects, [Rect::new(10, 2, 3, 5), Rect::new(13, 2, 4, 5)]);
    }

    #[test]
    fn vertical_split_keeps_the_width() {
        let [a, b] = Layout::vertical([Length(2), Fill(1)]).areas(Rect::new(1, 1, 8, 10));
        assert_eq!(a, Rect::new(1, 1, 8, 2));
        assert_eq!(b, Rect::new(1, 3, 8, 8));
    }

    #[test]
    fn fill_takes_the_rest_and_shares_by_weight() {
        assert_eq!(lens(&[Length(2), Fill(1)], 10), [2, 8]);
        assert_eq!(lens(&[Fill(1), Fill(3)], 12), [3, 9]);
        assert_eq!(lens(&[Fill(1), Fill(1), Fill(1)], 10), [4, 3, 3]);
        // Typical prompt: a bar taking all but one row.
        assert_eq!(lens(&[Fill(1), Length(1)], 6), [5, 1]);
        // Zero weight: receives nothing while another Fill exists.
        assert_eq!(lens(&[Fill(0), Fill(1)], 7), [0, 7]);
    }

    #[test]
    fn fill_rounding_always_fills_the_area() {
        for width in 0..40u16 {
            for weights in [[1, 1, 1], [1, 2, 3], [5, 1, 1], [7, 7, 7]] {
                let fills = weights.map(Fill);
                let total: u16 = lens(&fills, width).iter().sum();
                assert_eq!(total, width, "poids {weights:?}, largeur {width}");
            }
        }
    }

    #[test]
    fn percentage_and_ratio() {
        assert_eq!(lens(&[Percentage(25), Percentage(75)], 20), [5, 15]);
        assert_eq!(lens(&[Ratio(1, 4), Ratio(3, 4)], 20), [5, 15]);
        // 11 / 2 = 5.5: a single share rounds down; Fill gets the remainder.
        assert_eq!(lens(&[Ratio(1, 2), Fill(1)], 11), [5, 6]);
        // Ratio with a zero denominator: no cells, no panic.
        assert_eq!(lens(&[Ratio(1, 0), Fill(1)], 5), [0, 5]);
    }

    #[test]
    fn proportional_rounding_sums_to_the_area_when_shares_add_up_to_100_percent() {
        for width in 0..60u16 {
            for set in [
                [Percentage(50), Percentage(50)],
                [Percentage(30), Percentage(70)],
                [Ratio(1, 3), Ratio(2, 3)],
                [Percentage(10), Percentage(90)],
            ] {
                let total: u16 = lens(&set, width).iter().sum();
                assert_eq!(total, width, "{set:?}, largeur {width}");
            }
        }
        // Three thirds of 10: the lost cell goes to the first (largest remainder, tied).
        assert_eq!(
            lens(&[Ratio(1, 3), Ratio(1, 3), Ratio(1, 3)], 10),
            [4, 3, 3]
        );
        assert_eq!(lens(&[Percentage(50), Percentage(50)], 5), [3, 2]);
    }

    #[test]
    fn partial_percentages_leave_the_rest_empty() {
        assert_eq!(lens(&[Percentage(30), Percentage(30)], 10), [3, 3]);
    }

    #[test]
    fn min_grows_only_when_there_is_no_fill() {
        assert_eq!(lens(&[Min(3), Length(2)], 10), [8, 2]);
        assert_eq!(lens(&[Min(3), Min(3)], 10), [5, 5]);
        assert_eq!(lens(&[Min(3), Fill(1)], 10), [3, 7]);
        assert_eq!(
            lens(&[Min(5), Length(2)], 4),
            [4, 0],
            "Min is also trimmed if the area is too small"
        );
    }

    #[test]
    fn max_is_never_grown() {
        assert_eq!(lens(&[Max(3), Fill(1)], 10), [3, 7]);
        assert_eq!(lens(&[Max(3)], 10), [3]);
        assert_eq!(lens(&[Max(30), Fill(1)], 10), [10, 0]);
    }

    #[test]
    fn too_small_areas_never_panic_and_saturate_at_zero() {
        assert_eq!(lens(&[Length(5), Length(5)], 7), [5, 2]);
        assert_eq!(lens(&[Length(5), Fill(1), Length(1)], 0), [0, 0, 0]);
        assert_eq!(lens(&[Percentage(100), Length(1)], 3), [3, 0]);
        assert_eq!(lens(&[Percentage(u16::MAX), Fill(1)], 3), [3, 0]);
        assert_eq!(Layout::vertical([]).split(Rect::new(0, 0, 4, 4)), []);
        // Empty area on the other axis: no crash; rectangles are empty.
        let rects = Layout::horizontal([Fill(1), Fill(1)]).split(Rect::new(0, 0, 10, 0));
        assert!(rects.iter().all(|r| r.is_empty()));
    }

    #[test]
    fn rects_never_overlap_and_stay_inside_the_area() {
        let pool = [
            Length(0),
            Length(3),
            Min(2),
            Max(4),
            Percentage(40),
            Ratio(1, 3),
            Fill(1),
            Fill(2),
        ];
        for width in [0u16, 1, 2, 5, 9, 20] {
            for a in pool {
                for b in pool {
                    for c in pool {
                        for spacing in [0, 1, 3] {
                            let area = Rect::new(3, 1, width, 2);
                            let rects = Layout::horizontal([a, b, c]).spacing(spacing).split(area);
                            assert_eq!(rects.len(), 3);
                            let mut edge = area.left();
                            for r in rects {
                                assert!(r.left() >= edge, "{a:?} {b:?} {c:?} largeur {width}");
                                assert!(r.right() <= area.right());
                                edge = r.right();
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn spacing_separates_elements_and_is_reduced_when_it_does_not_fit() {
        let rects = Layout::horizontal([Length(2), Length(2), Length(2)])
            .spacing(1)
            .split(Rect::new(0, 0, 20, 1));
        assert_eq!(rects.iter().map(|r| r.x).collect::<Vec<_>>(), [0, 3, 6]);
        // Fill constraints share what remains after spacing and fill the area.
        let rects = Layout::horizontal([Fill(1), Fill(1)])
            .spacing(2)
            .split(Rect::new(0, 0, 11, 1));
        assert_eq!((rects[0].width, rects[1].x, rects[1].width), (5, 7, 4));
        // Not enough room for spacing: reduced, never overflowing.
        let rects = Layout::horizontal([Fill(1), Fill(1), Fill(1)])
            .spacing(5)
            .split(Rect::new(0, 0, 4, 1));
        assert!(rects.iter().all(|r| r.right() <= 4));
        // A single element: no spacing.
        let rects = Layout::horizontal([Fill(1)])
            .spacing(5)
            .split(Rect::new(0, 0, 4, 1));
        assert_eq!(rects, [Rect::new(0, 0, 4, 1)]);
    }

    #[test]
    fn percentages_are_computed_on_the_space_left_after_spacing() {
        let rects = Layout::horizontal([Percentage(50), Percentage(50)])
            .spacing(2)
            .split(Rect::new(0, 0, 12, 1));
        assert_eq!((rects[0].width, rects[1].x, rects[1].width), (5, 7, 5));
    }

    #[test]
    fn margin_is_removed_before_splitting() {
        let rects = Layout::vertical([Fill(1), Length(1)])
            .margin(1)
            .split(Rect::new(0, 0, 10, 6));
        assert_eq!(rects, [Rect::new(1, 1, 8, 3), Rect::new(1, 4, 8, 1)]);

        let rects = Layout::horizontal([Fill(1)])
            .horizontal_margin(2)
            .vertical_margin(1)
            .split(Rect::new(0, 0, 10, 4));
        assert_eq!(rects, [Rect::new(2, 1, 6, 2)]);

        // Margin too large: everything is empty, without panicking.
        let rects = Layout::vertical([Fill(1), Fill(1)])
            .margin(10)
            .split(Rect::new(0, 0, 4, 4));
        assert!(rects.iter().all(|r| r.is_empty()));
    }

    #[test]
    fn areas_returns_a_fixed_size_array() {
        let [a, b, c] =
            Layout::horizontal([Length(1), Length(2), Fill(1)]).areas(Rect::new(0, 0, 6, 1));
        assert_eq!((a.width, b.width, c.width), (1, 2, 3));
    }

    #[test]
    #[should_panic(expected = "Layout::areas::<3> called with 2 constraints")]
    fn areas_panics_when_the_count_does_not_match() {
        let _: [Rect; 3] = Layout::horizontal([Fill(1), Fill(1)]).areas(Rect::new(0, 0, 6, 1));
    }

    #[test]
    fn near_the_u16_limit_nothing_overflows() {
        let area = Rect::new(u16::MAX - 5, 0, 5, 1);
        let rects = Layout::horizontal([Fill(1), Fill(1)]).split(area);
        assert_eq!(rects.iter().map(|r| r.width).sum::<u16>(), 5);
        let big = Layout::horizontal([Length(u16::MAX), Length(u16::MAX)]).split(Rect::new(
            0,
            0,
            u16::MAX,
            1,
        ));
        assert_eq!((big[0].width, big[1].width), (u16::MAX, 0));
    }

    // --- properties (proptest): any constraints, any area, sizes 0 included ---

    fn constraint() -> impl proptest::strategy::Strategy<Value = Constraint> {
        use proptest::prelude::*;
        prop_oneof![
            (0u16..300).prop_map(Length),
            (0u16..300).prop_map(Min),
            (0u16..300).prop_map(Max),
            (0u16..150).prop_map(Percentage),
            (0u32..10, 0u32..10).prop_map(|(a, b)| Ratio(a, b)),
            (0u16..4).prop_map(Fill),
        ]
    }

    /// The former solver, with `Vec`s: the reference the allocation-free one must agree
    /// with. Sizes (in cells) of each constraint for `available` cells. See the module header.
    /// The sum of sizes never exceeds `available`.
    fn solve_with_vecs(constraints: &[Constraint], available: u16) -> Vec<u16> {
        let avail = u64::from(available);

        // 1. Requests.
        let mut requests = vec![0u64; constraints.len()];
        let mut proportional: Vec<(usize, f64)> = Vec::new();
        for (i, constraint) in constraints.iter().enumerate() {
            match *constraint {
                Constraint::Length(n) | Constraint::Min(n) | Constraint::Max(n) => {
                    requests[i] = u64::from(n);
                }
                // The product and quotient are exact when the result is an integer.
                Constraint::Percentage(p) => {
                    proportional.push((i, exact_f64(avail * u64::from(p)) / 100.0));
                }
                Constraint::Ratio(_, 0) | Constraint::Fill(_) => {}
                Constraint::Ratio(a, b) => {
                    proportional.push((i, exact_f64(avail * u64::from(a)) / f64::from(b)));
                }
            }
        }
        let shares: Vec<f64> = proportional.iter().map(|&(_, exact)| exact).collect();
        for (&(i, _), size) in proportional
            .iter()
            .zip(largest_remainder_with_vecs(&shares))
        {
            requests[i] = size;
        }

        // 2. Allocate in order, limited by remaining space.
        let mut remaining = avail;
        let mut sizes: Vec<u64> = requests
            .iter()
            .map(|&request| {
                let size = request.min(remaining);
                remaining -= size;
                size
            })
            .collect();

        // 3. Share the remainder: among Fill constraints (by weight), otherwise Min constraints (equally).
        if remaining > 0 {
            let fills: Vec<usize> = (0..constraints.len())
                .filter(|&i| matches!(constraints[i], Constraint::Fill(w) if w > 0))
                .collect();
            let receivers = if fills.is_empty() {
                (0..constraints.len())
                    .filter(|&i| matches!(constraints[i], Constraint::Min(_)))
                    .collect()
            } else {
                fills
            };
            let weights: Vec<f64> = receivers
                .iter()
                .map(|&i| match constraints[i] {
                    Constraint::Fill(w) => f64::from(w),
                    _ => 1.0,
                })
                .collect();
            let total: f64 = weights.iter().sum();
            if total > 0.0 {
                let exact: Vec<f64> = weights
                    .iter()
                    .map(|w| exact_f64(remaining) * w / total)
                    .collect();
                // Force the sum to exactly `remaining` (the method distributes only
                // the integer part of the sum, which equals `remaining` up to floating-point error).
                for (&i, extra) in receivers.iter().zip(largest_remainder_with_vecs(&exact)) {
                    sizes[i] += extra;
                }
            }
        }

        // Each size is at most `available`, a `u16`: the fallback is never used.
        sizes
            .into_iter()
            .map(|size| u16::try_from(size).unwrap_or(u16::MAX))
            .collect()
    }

    /// Round exact shares to integers whose sum is the integer part of the exact sum:
    /// round each share down, then assign remaining cells to the shares with the
    /// largest fractional parts (ties favor the first).
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "shares are finite, non-negative and below 2^48: `floor` gives an exact u64"
    )]
    fn largest_remainder_with_vecs(exact: &[f64]) -> Vec<u64> {
        // The small tolerance absorbs floating-point summation error (e.g. 0.1 + 0.2).
        let target = (exact.iter().sum::<f64>() + 1e-6).floor() as u64;
        let mut sizes: Vec<u64> = exact.iter().map(|share| share.floor() as u64).collect();
        let mut missing = target.saturating_sub(sizes.iter().sum());

        let mut order: Vec<usize> = (0..exact.len()).collect();
        // Stable sort: equal remainders preserve the original order (favoring the first).
        order.sort_by(|&a, &b| {
            let (fa, fb) = (exact[a].fract(), exact[b].fract());
            fb.partial_cmp(&fa).unwrap_or(std::cmp::Ordering::Equal)
        });
        for i in order {
            if missing == 0 {
                break;
            }
            sizes[i] += 1;
            missing -= 1;
        }
        sizes
    }

    #[test]
    fn more_constraints_than_kept_inline() {
        let constraints = [Length(1); INLINE + 3];
        let rects = Layout::vertical(constraints).split(Rect::new(0, 0, 4, 20));
        assert_eq!(rects.len(), INLINE + 3);
        assert_eq!(rects[INLINE + 2], Rect::new(0, 10, 4, 1));
        assert_eq!(
            Layout::vertical(constraints),
            Layout::vertical(constraints.to_vec())
        );
    }

    proptest::proptest! {
        #[test]
        fn the_solver_agrees_with_the_former_one(
            constraints in proptest::collection::vec(constraint(), 0..12),
            available in 0u16..2000,
        ) {
            let mut sizes = vec![0; constraints.len()];
            solve(&constraints, available, &mut sizes);
            proptest::prop_assert_eq!(sizes, solve_with_vecs(&constraints, available));
        }

        #[test]
        fn split_stays_in_the_area_in_order_without_overlap(
            constraints in proptest::collection::vec(constraint(), 0..6),
            (x, y, width, height) in (0u16..1000, 0u16..1000, 0u16..500, 0u16..500),
            margin in 0u16..20,
            spacing in 0u16..20,
            vertical: bool,
        ) {
            let area = Rect::new(x, y, width, height);
            let layout = if vertical {
                Layout::vertical(constraints.clone())
            } else {
                Layout::horizontal(constraints.clone())
            };
            let rects = layout.margin(margin).spacing(spacing).split(area);
            let inner = area.inner(Margin::new(margin, margin));
            // (start, end) along the split axis, and the cross axis, which must not change.
            let along = |r: &Rect| if vertical { (r.y, r.bottom()) } else { (r.x, r.right()) };
            let across = |r: &Rect| if vertical { (r.x, r.width) } else { (r.y, r.height) };

            proptest::prop_assert_eq!(rects.len(), constraints.len());
            let (inner_start, inner_end) = along(&inner);
            let mut previous_end = inner_start;
            for rect in &rects {
                let (start, end) = along(rect);
                proptest::prop_assert!(start >= previous_end, "overlap or disorder: {:?}", rects);
                proptest::prop_assert!(end <= inner_end, "{:?} outside {:?}", rect, inner);
                proptest::prop_assert_eq!(across(rect), across(&inner));
                previous_end = end;
            }
            // With a weighted `Fill` or a `Min`, the remainder is given out: the area is
            // filled exactly (module header, step 3).
            let fills = constraints
                .iter()
                .any(|c| matches!(c, Fill(w) if *w > 0) || matches!(c, Min(_)));
            if fills {
                proptest::prop_assert_eq!(previous_end, inner_end, "area not filled: {:?}", rects);
            }
        }
    }
}
