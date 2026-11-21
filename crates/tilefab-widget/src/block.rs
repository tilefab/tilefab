//! `Block`: borders, title, and padding around content.

use std::ops::BitOr;

use tilefab_core::buffer::Buffer;
use tilefab_core::layout::Rect;
use tilefab_core::style::Style;
use tilefab_core::text::{Alignment, Line};
use tilefab_core::widget::Widget;

/// Sides with a border (combinable with `|`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Borders(u8);

impl Borders {
    /// No borders.
    pub const NONE: Self = Self(0);
    /// Top border.
    pub const TOP: Self = Self(1);
    /// Right border.
    pub const RIGHT: Self = Self(2);
    /// Bottom border.
    pub const BOTTOM: Self = Self(4);
    /// Left border.
    pub const LEFT: Self = Self(8);
    /// All four borders.
    pub const ALL: Self = Self(15);

    /// True if all sides in `other` are present.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// True if no borders are enabled.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for Borders {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// Border line style.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BorderType {
    /// `┌─┐│└┘`
    #[default]
    Plain,
    /// `╭─╮│╰╯`
    Rounded,
    /// `╔═╗║╚╝`
    Double,
    /// `┏━┓┃┗┛`
    Thick,
}

/// The six symbols of a border frame.
struct BorderSet {
    horizontal: &'static str,
    vertical: &'static str,
    top_left: &'static str,
    top_right: &'static str,
    bottom_left: &'static str,
    bottom_right: &'static str,
}

impl BorderType {
    fn set(self) -> BorderSet {
        let [
            horizontal,
            vertical,
            top_left,
            top_right,
            bottom_left,
            bottom_right,
        ] = match self {
            Self::Plain => ["─", "│", "┌", "┐", "└", "┘"],
            Self::Rounded => ["─", "│", "╭", "╮", "╰", "╯"],
            Self::Double => ["═", "║", "╔", "╗", "╚", "╝"],
            Self::Thick => ["━", "┃", "┏", "┓", "┗", "┛"],
        };
        BorderSet {
            horizontal,
            vertical,
            top_left,
            top_right,
            bottom_left,
            bottom_right,
        }
    }
}

/// Padding between borders and content.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Padding {
    /// Left padding in cells.
    pub left: u16,
    /// Right padding in cells.
    pub right: u16,
    /// Top padding in cells.
    pub top: u16,
    /// Bottom padding in cells.
    pub bottom: u16,
}

impl Padding {
    /// Padding on each side.
    #[must_use]
    pub const fn new(left: u16, right: u16, top: u16, bottom: u16) -> Self {
        Self {
            left,
            right,
            top,
            bottom,
        }
    }

    /// Equal padding on all four sides.
    #[must_use]
    pub const fn uniform(value: u16) -> Self {
        Self::new(value, value, value, value)
    }

    /// Left and right padding only.
    #[must_use]
    pub const fn horizontal(value: u16) -> Self {
        Self::new(value, value, 0, 0)
    }

    /// Top and bottom padding only.
    #[must_use]
    pub const fn vertical(value: u16) -> Self {
        Self::new(0, 0, value, value)
    }
}

/// A frame: background, borders, title (on the top row), and padding.
///
/// ```
/// use tilefab_core::{Buffer, Rect, Widget};
/// use tilefab_widget::{Block, Borders};
///
/// let mut buf = Buffer::empty(Rect::new(0, 0, 7, 3));
/// Block::new().borders(Borders::ALL).title("hi").render(buf.area(), &mut buf);
/// assert_eq!(buf.lines(), ["┌hi───┐", "│     │", "└─────┘"]);
/// ```
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Block<'a> {
    title: Option<Line<'a>>,
    title_alignment: Alignment,
    borders: Borders,
    border_type: BorderType,
    border_style: Style,
    style: Style,
    padding: Padding,
}

impl<'a> Block<'a> {
    /// Block without borders or a title.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Full frame (all four borders).
    #[must_use]
    pub fn bordered() -> Self {
        Self::new().borders(Borders::ALL)
    }

    /// Title displayed on the top row, between corners. If the block has no top
    /// border, the title still occupies the first row.
    #[must_use]
    pub fn title<T: Into<Line<'a>>>(mut self, title: T) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Title alignment (unless the title's `Line` has its own).
    #[must_use]
    pub fn title_alignment(mut self, alignment: Alignment) -> Self {
        self.title_alignment = alignment;
        self
    }

    /// Choose which sides have a border.
    #[must_use]
    pub fn borders(mut self, borders: Borders) -> Self {
        self.borders = borders;
        self
    }

    /// Choose the border line style.
    #[must_use]
    pub fn border_type(mut self, border_type: BorderType) -> Self {
        self.border_type = border_type;
        self
    }

    /// Border style, applied over `style`.
    #[must_use]
    pub fn border_style<S: Into<Style>>(mut self, style: S) -> Self {
        self.border_style = style.into();
        self
    }

    /// Style for the entire block area (including the background).
    #[must_use]
    pub fn style<S: Into<Style>>(mut self, style: S) -> Self {
        self.style = style.into();
        self
    }

    /// Add padding between the border and content.
    #[must_use]
    pub fn padding(mut self, padding: Padding) -> Self {
        self.padding = padding;
        self
    }

    fn has_title_row(&self) -> bool {
        self.borders.contains(Borders::TOP) || self.title.is_some()
    }

    /// Cells occupied on the left and right by borders and padding.
    pub(crate) fn horizontal_space(&self) -> u16 {
        let borders = u16::from(self.borders.contains(Borders::LEFT))
            + u16::from(self.borders.contains(Borders::RIGHT));
        borders
            .saturating_add(self.padding.left)
            .saturating_add(self.padding.right)
    }

    /// Cells occupied on the top and bottom by borders, title, and padding.
    pub(crate) fn vertical_space(&self) -> u16 {
        let borders =
            u16::from(self.has_title_row()) + u16::from(self.borders.contains(Borders::BOTTOM));
        borders
            .saturating_add(self.padding.top)
            .saturating_add(self.padding.bottom)
    }

    /// Area available for content: `area` minus borders, title, and padding.
    /// Empty (size 0) if the block leaves no space.
    #[must_use]
    pub fn inner(&self, area: Rect) -> Rect {
        // Saturated like `horizontal_space`/`vertical_space`: a padding of `u16::MAX` must
        // give an empty area, not an overflow.
        let left =
            u16::from(self.borders.contains(Borders::LEFT)).saturating_add(self.padding.left);
        let top = u16::from(self.has_title_row()).saturating_add(self.padding.top);
        let (hs, vs) = (self.horizontal_space(), self.vertical_space());
        if area.width <= hs || area.height <= vs {
            return Rect::new(area.x, area.y, 0, 0);
        }
        Rect::new(
            area.x.saturating_add(left),
            area.y.saturating_add(top),
            area.width - hs,
            area.height - vs,
        )
    }

    /// Draw without consuming the block (used by widgets containing it).
    pub(crate) fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let area = buf.area().intersection(area);
        if area.is_empty() {
            return;
        }
        buf.set_style(area, self.style);

        let set = self.border_type.set();
        let style = self.border_style;
        let (left, right) = (area.left(), area.right() - 1);
        let (top, bottom) = (area.top(), area.bottom() - 1);

        // Draw full-length sides first, then corners on top.
        if self.borders.contains(Borders::TOP) {
            for x in left..=right {
                buf.set_string(x, top, set.horizontal, style);
            }
        }
        if self.borders.contains(Borders::BOTTOM) {
            for x in left..=right {
                buf.set_string(x, bottom, set.horizontal, style);
            }
        }
        if self.borders.contains(Borders::LEFT) {
            for y in top..=bottom {
                buf.set_string(left, y, set.vertical, style);
            }
        }
        if self.borders.contains(Borders::RIGHT) {
            for y in top..=bottom {
                buf.set_string(right, y, set.vertical, style);
            }
        }
        // A corner exists only if both sides meeting there exist.
        if self.borders.contains(Borders::TOP | Borders::LEFT) {
            buf.set_string(left, top, set.top_left, style);
        }
        if self.borders.contains(Borders::TOP | Borders::RIGHT) {
            buf.set_string(right, top, set.top_right, style);
        }
        if self.borders.contains(Borders::BOTTOM | Borders::LEFT) {
            buf.set_string(left, bottom, set.bottom_left, style);
        }
        if self.borders.contains(Borders::BOTTOM | Borders::RIGHT) {
            buf.set_string(right, bottom, set.bottom_right, style);
        }

        // Title: between the left and right border columns.
        if let Some(title) = &self.title {
            let first = left + u16::from(self.borders.contains(Borders::LEFT));
            let end = area.right() - u16::from(self.borders.contains(Borders::RIGHT));
            if first < end {
                let mut title = title.clone();
                title.alignment.get_or_insert(self.title_alignment);
                title.render_to(Rect::new(first, top, end - first, 1), buf);
            }
        }
    }
}

impl Widget for Block<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.render_ref(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tilefab_core::style::{Color, Modifier};
    use tilefab_core::text::Stylize;

    fn render(block: Block, width: u16, height: u16) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        block.render(buf.area(), &mut buf);
        buf
    }

    #[test]
    fn plain_rounded_double_and_thick_frames() {
        let cases = [
            (BorderType::Plain, ["┌──┐", "│  │", "└──┘"]),
            (BorderType::Rounded, ["╭──╮", "│  │", "╰──╯"]),
            (BorderType::Double, ["╔══╗", "║  ║", "╚══╝"]),
            (BorderType::Thick, ["┏━━┓", "┃  ┃", "┗━━┛"]),
        ];
        for (border_type, expected) in cases {
            let block = Block::bordered().border_type(border_type);
            assert_eq!(render(block, 4, 3), Buffer::with_lines(expected));
        }
    }

    #[test]
    fn partial_borders_only_draw_corners_where_two_sides_meet() {
        assert_eq!(
            render(Block::new().borders(Borders::TOP), 4, 2),
            Buffer::with_lines(["────", "    "])
        );
        assert_eq!(
            render(Block::new().borders(Borders::LEFT | Borders::BOTTOM), 3, 2),
            Buffer::with_lines(["│  ", "└──"])
        );
        assert_eq!(
            render(Block::new().borders(Borders::RIGHT), 3, 2),
            Buffer::with_lines(["  │", "  │"])
        );
        assert_eq!(
            render(Block::new(), 3, 2),
            Buffer::empty(Rect::new(0, 0, 3, 2))
        );
        assert!(Borders::NONE.is_empty());
    }

    #[test]
    fn inner_removes_borders_title_and_padding() {
        let area = Rect::new(1, 1, 10, 6);
        assert_eq!(Block::bordered().inner(area), Rect::new(2, 2, 8, 4));
        assert_eq!(Block::new().inner(area), area);
        assert_eq!(
            Block::new().borders(Borders::LEFT).inner(area),
            Rect::new(2, 1, 9, 6)
        );
        // A title reserves the first row even without a top border.
        assert_eq!(Block::new().title("t").inner(area), Rect::new(1, 2, 10, 5));
        assert_eq!(
            Block::bordered()
                .padding(Padding::new(2, 1, 0, 1))
                .inner(area),
            Rect::new(4, 2, 5, 3)
        );
        assert_eq!(
            Block::bordered()
                .padding(Padding::uniform(1))
                .inner(Rect::new(0, 0, 7, 5)),
            Rect::new(2, 2, 3, 1)
        );
        assert_eq!(Padding::horizontal(2), Padding::new(2, 2, 0, 0));
        assert_eq!(Padding::vertical(2), Padding::new(0, 0, 2, 2));
    }

    #[test]
    fn inner_is_empty_when_the_block_leaves_no_room() {
        for (w, h) in [(0, 0), (1, 1), (2, 2), (2, 5), (5, 2), (1, 9)] {
            assert!(
                Block::bordered().inner(Rect::new(0, 0, w, h)).is_empty(),
                "{w}x{h}"
            );
        }
        assert!(!Block::bordered().inner(Rect::new(0, 0, 3, 3)).is_empty());
        assert!(
            Block::bordered()
                .padding(Padding::uniform(5))
                .inner(Rect::new(0, 0, 9, 9))
                .is_empty()
        );
    }

    #[test]
    fn maximal_padding_gives_an_empty_area_without_overflow() {
        let area = Rect::new(0, 0, 20, 10);
        let block = Block::bordered()
            .title("t")
            .padding(Padding::uniform(u16::MAX));
        assert!(block.inner(area).is_empty());
        // Only on the left and top sides, which `inner` adds to the area position.
        let block = Block::bordered()
            .title("t")
            .padding(Padding::new(u16::MAX, 0, u16::MAX, 0));
        assert!(block.inner(area).is_empty());
        // Rendering such a block, alone or inside a widget, must not panic either.
        render(block.clone(), 20, 10);
        let mut buf = Buffer::empty(area);
        crate::paragraph::Paragraph::new("text")
            .block(block.clone())
            .render(area, &mut buf);
        crate::gauge::Gauge::default()
            .block(block.clone())
            .render(area, &mut buf);
        crate::gauge::LineGauge::default()
            .block(block.clone())
            .render(area, &mut buf);
        let mut state = crate::list::ListState::default();
        tilefab_core::widget::StatefulWidget::render(
            crate::list::List::new(["item"]).block(block),
            area,
            &mut buf,
            &mut state,
        );
    }

    #[test]
    fn title_alignment_and_truncation() {
        let framed = |b: Block| render(b, 8, 2).lines()[0].clone();
        assert_eq!(framed(Block::bordered().title("ab")), "┌ab────┐");
        assert_eq!(
            framed(
                Block::bordered()
                    .title("ab")
                    .title_alignment(Alignment::Center)
            ),
            "┌──ab──┐"
        );
        assert_eq!(
            framed(
                Block::bordered()
                    .title("ab")
                    .title_alignment(Alignment::Right)
            ),
            "┌────ab┐"
        );
        // The title's `Line` keeps its own alignment.
        assert_eq!(
            framed(Block::bordered().title(Line::from("ab").right_aligned())),
            "┌────ab┐"
        );
        // Too long: truncated, preserving the corners.
        assert_eq!(framed(Block::bordered().title("abcdefghij")), "┌abcdef┐");
    }

    #[test]
    fn title_without_top_border_uses_the_first_row() {
        assert_eq!(
            render(Block::new().title("t"), 4, 2),
            Buffer::with_lines(["t   ", "    "])
        );
    }

    #[test]
    fn styles_block_borders_and_title() {
        let block = Block::bordered()
            .style(Style::new().bg(Color::Blue))
            .border_style(Color::Red)
            .title("t".bold());
        let buf = render(block, 5, 3);
        let corner = buf.get(0, 0);
        assert_eq!((corner.fg, corner.bg), (Color::Red, Color::Blue));
        assert_eq!(
            buf.get(2, 1).bg,
            Color::Blue,
            "background on the whole inside"
        );
        assert_eq!(buf.get(2, 1).fg, Color::Reset);
        assert!(
            buf.get(1, 0).modifier.contains(Modifier::BOLD),
            "titre en gras"
        );
    }

    #[test]
    fn tiny_areas_do_not_panic() {
        for w in 0..4 {
            for h in 0..4 {
                let block = Block::bordered()
                    .title("titre")
                    .padding(Padding::uniform(1));
                let _ = render(block, w, h);
            }
        }
        // 1 × 1: all sides occupy the same cell; the last corner (bottom-right) wins.
        assert_eq!(render(Block::bordered(), 1, 1).lines(), ["┘"]);
    }

    #[test]
    fn area_larger_than_the_buffer_is_clipped() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        Block::bordered().render(Rect::new(0, 0, 10, 10), &mut buf);
        assert_eq!(buf.lines(), ["┌──┐", "└──┘"]);
    }
}
