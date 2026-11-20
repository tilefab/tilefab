//! The widget traits: objects that draw themselves in an area of a [`Buffer`].
//!
//! Rendering consumes a widget (`render(self, …)`): build it, draw it,
//! discard it, every frame. Stateful widgets implement [`StatefulWidget`]:
//! the caller owns the state (selection, scrolling), which persists between frames.
//!
//! The traits live in the common base so that the terminal (`Frame::render_widget`)
//! and the widget library can both use them without depending on each other.
//! Text types (`&str`, `String`, [`Span`], [`Line`], [`Text`]) are widgets too.

use super::buffer::Buffer;
use super::layout::Rect;
use super::text::{Line, Span, Text};

/// A UI element that draws itself in `area`.
pub trait Widget {
    /// Draw the widget in `area` of buffer `buf`.
    fn render(self, area: Rect, buf: &mut Buffer);
}

/// A widget whose display depends on state retained between frames.
pub trait StatefulWidget {
    /// The type of state retained between frames.
    type State;
    /// Draw the widget in `area`, reading and updating `state`.
    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State);
}

/// A string draws on the area's first row, truncated on the right.
impl Widget for &str {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Span::raw(self).render(area, buf);
    }
}

impl Widget for String {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Span::raw(self).render(area, buf);
    }
}

/// A span draws on the area's first row; its style covers only its text.
impl Widget for Span<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Line::from(self).render_to(area, buf);
    }
}

impl Widget for Line<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.render_to(area, buf);
    }
}

impl Widget for Text<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.render_to(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Color;
    use crate::text::Stylize;

    #[test]
    fn str_string_span_line_text_are_widgets() {
        let area = Rect::new(0, 0, 4, 2);
        let mut buf = Buffer::empty(area);
        "abcdef".render(area, &mut buf);
        assert_eq!(buf, Buffer::with_lines(["abcd", "    "]));

        let mut buf = Buffer::empty(area);
        String::from("xy").render(Rect::new(1, 1, 3, 1), &mut buf);
        assert_eq!(buf, Buffer::with_lines(["    ", " xy "]));

        let mut buf = Buffer::empty(area);
        Text::from("a\nb").render(area, &mut buf);
        assert_eq!(buf, Buffer::with_lines(["a   ", "b   "]));

        let mut buf = Buffer::empty(area);
        Line::from("z").right_aligned().render(area, &mut buf);
        assert_eq!(buf, Buffer::with_lines(["   z", "    "]));
    }

    #[test]
    fn span_style_covers_only_its_text() {
        let area = Rect::new(0, 0, 4, 1);
        let mut buf = Buffer::empty(area);
        "ab".bg(Color::Blue).render(area, &mut buf);
        assert_eq!(buf.get(1, 0).bg, Color::Blue);
        assert_eq!(buf.get(2, 0).bg, Color::Reset);
        assert_eq!(
            (buf.get(0, 0).fg, buf.get(0, 0).bg),
            (Color::Reset, Color::Blue)
        );
    }

    #[test]
    fn rendering_in_an_empty_or_outside_area_does_nothing() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        "abc".render(Rect::new(0, 0, 0, 1), &mut buf);
        "abc".render(Rect::new(0, 0, 3, 0), &mut buf);
        "abc".render(Rect::new(5, 5, 3, 1), &mut buf);
        assert_eq!(buf, Buffer::empty(Rect::new(0, 0, 3, 1)));
    }
}
