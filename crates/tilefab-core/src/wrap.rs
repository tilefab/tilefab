//! Wrapping of styled lines at a given width: a line wider than the area is cut into several
//! rows, on grapheme boundaries, with the width rule of the buffer ([`grapheme_width`]), so
//! that nothing is lost.
//!
//! The terminal uses it for the lines inserted above an inline viewport
//! (`Terminal::insert_lines` in `tilefab-term`); an application that manages its own rows
//! can call it directly.
//!
//! ```
//! use tilefab_core::Line;
//! use tilefab_core::wrap::wrap_lines;
//!
//! let rows = wrap_lines([Line::from("abcdef"), Line::from("gh")], 4);
//! let texts: Vec<String> = rows
//!     .iter()
//!     .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect())
//!     .collect();
//! assert_eq!(texts, ["abcd", "ef", "gh"]);
//! ```

use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::{Buffer, grapheme_width};
use crate::layout::Rect;
use crate::text::{Line, Span};

/// Shown instead of a grapheme wider than the width (a wide character in a 1-column area):
/// the replacement character, one column wide.
pub const TOO_WIDE: &str = "\u{FFFD}";

/// [`wrap_lines_into`] into a new list.
#[must_use]
pub fn wrap_lines<'a>(lines: impl IntoIterator<Item = Line<'a>>, width: u16) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    wrap_lines_into(lines, width, &mut rows);
    rows
}

/// Cut `lines` into rows at most `width` columns wide, keeping the style of each span and
/// line. An empty line stays one empty row; a grapheme wider than `width` (a wide character
/// in a 1-column area) cannot be shown and is replaced with [`TOO_WIDE`], so that the loss
/// is visible. With `width` 0, no row.
///
/// The rows are appended to `rows`: a caller that wraps often can keep that list (and the
/// one it takes `lines` from, with `drain`) from one call to the next, with its capacity.
pub fn wrap_lines_into<'a>(
    lines: impl IntoIterator<Item = Line<'a>>,
    width: u16,
    rows: &mut Vec<Line<'static>>,
) {
    debug_assert_eq!(grapheme_width(TOO_WIDE), 1);
    let width = usize::from(width);
    if width == 0 {
        return;
    }
    for line in lines {
        let empty_row = || Line {
            spans: Vec::new(),
            style: line.style,
            alignment: line.alignment,
        };
        let mut row = empty_row();
        let mut used = 0;
        for span in &line.spans {
            let mut piece = String::new();
            for grapheme in span.content.graphemes(true) {
                let (grapheme, columns) = match grapheme_width(grapheme) {
                    columns if columns > width => (TOO_WIDE, 1),
                    columns => (grapheme, columns),
                };
                if used + columns > width {
                    if !piece.is_empty() {
                        row.spans
                            .push(Span::styled(std::mem::take(&mut piece), span.style));
                    }
                    rows.push(std::mem::replace(&mut row, empty_row()));
                    used = 0;
                }
                piece.push_str(grapheme);
                used += columns;
            }
            if !piece.is_empty() {
                row.spans.push(Span::styled(piece, span.style));
            }
        }
        rows.push(row);
    }
}

/// Draw `rows` (from [`wrap_lines`]) one per row of `buf`, from its top; the rows that do not
/// fit are not drawn.
pub fn render_rows(rows: &[Line<'_>], buf: &mut Buffer) {
    let area = buf.area();
    for (y, row) in (area.top()..area.bottom()).zip(rows) {
        row.render_to(Rect::new(area.x, y, area.width, 1), buf);
    }
}

#[cfg(test)]
mod tests {
    use crate::{Color, Style, Stylize};

    use super::*;

    fn texts(rows: &[Line<'_>]) -> Vec<String> {
        rows.iter()
            .map(|row| row.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn wrapped_lines_counts_visual_lines() {
        let rows = wrap_lines([Line::from("abcdef"), Line::from("gh")], 4);
        // “abcdef” wrapped to 4 columns = 2 lines, plus “gh” = 3.
        assert_eq!(texts(&rows), ["abcd", "ef", "gh"]);
    }

    #[test]
    fn empty_lines_stay_and_zero_width_gives_nothing() {
        let rows = wrap_lines([Line::from(""), Line::from("a")], 3);
        assert_eq!(texts(&rows), ["", "a"]);
        assert_eq!(wrap_lines([Line::from("abc")], 0), []);
    }

    #[test]
    fn wide_characters_are_never_split() {
        let rows = wrap_lines([Line::from("a日本語")], 4);
        assert_eq!(texts(&rows), ["a日", "本語"]);
        // Too wide for one column: replaced, on its own row like any other grapheme.
        assert_eq!(
            texts(&wrap_lines([Line::from("a日b")], 1)),
            ["a", TOO_WIDE, "b"]
        );
        assert_eq!(texts(&wrap_lines([Line::from("日")], 1)), [TOO_WIDE]);
    }

    #[test]
    fn styles_follow_the_cut() {
        let red = Style::new().fg(Color::Red);
        let line = Line::from(vec![Span::raw("[WARN] "), Span::styled("abcdef", red)]).bold();
        let rows = wrap_lines([line.clone()], 9);
        assert_eq!(texts(&rows), ["[WARN] ab", "cdef"]);
        assert_eq!(rows[0].spans[1].style, red);
        assert_eq!(rows[1].spans[0].style, red);
        assert_eq!(rows[1].style, line.style);
    }

    #[test]
    fn borrowed_lines_give_owned_rows() {
        let text = String::from("abc");
        let rows = wrap_lines([Line::from(text.as_str())], 2);
        drop(text);
        assert_eq!(texts(&rows), ["ab", "c"]);
    }

    #[test]
    fn render_writes_one_row_each() {
        let rows = wrap_lines([Line::from("abcdef")], 4);
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        render_rows(&rows, &mut buf);
        assert_eq!(buf, Buffer::with_lines(["abcd", "ef  "]));
    }
}
