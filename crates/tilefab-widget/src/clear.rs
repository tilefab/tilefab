//! `Clear`: reset an area (unstyled spaces).

use tilefab_core::buffer::Buffer;
use tilefab_core::layout::Rect;
use tilefab_core::widget::Widget;

/// Clear the area: each cell becomes an unstyled space again. Useful for drawing
/// a floating window over existing content.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Clear;

impl Widget for Clear {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = buf.area.intersection(area);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf.get_mut(x, y).reset();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tilefab_core::style::{Color, Style};

    #[test]
    fn clears_symbols_and_styles_only_inside_the_area() {
        let mut buf = Buffer::with_lines(["abcd", "efgh", "ijkl"]);
        buf.set_style(buf.area, Style::new().fg(Color::Red));
        Clear.render(Rect::new(1, 1, 2, 5), &mut buf);
        assert_eq!(buf.lines(), ["abcd", "e  h", "i  l"]);
        assert_eq!(buf.get(1, 1).fg, Color::Reset);
        assert_eq!(buf.get(0, 1).fg, Color::Red);
    }

    #[test]
    fn empty_or_outside_areas_are_ignored() {
        let mut buf = Buffer::with_lines(["ab"]);
        Clear.render(Rect::new(0, 0, 0, 1), &mut buf);
        Clear.render(Rect::new(9, 9, 2, 2), &mut buf);
        assert_eq!(buf.lines(), ["ab"]);
    }
}
