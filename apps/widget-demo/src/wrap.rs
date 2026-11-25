//! Wrapping of the lines inserted above the viewport: a log line wider than the terminal is
//! cut into several rows, on grapheme boundaries, at the terminal width (measured with the
//! rule of the buffer, `tilefab::grapheme_width`), so that nothing is lost.

use tilefab::{Buffer, Line, Rect, Span, grapheme_width};
use unicode_segmentation::UnicodeSegmentation;

/// Cut `lines` into rows at most `width` columns wide, keeping the style of each span and
/// line. An empty line stays one empty row; a grapheme wider than `width` (a wide character
/// in a 1-column terminal) cannot be shown and is dropped. With `width` 0, no row.
pub fn wrap(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width);
    if width == 0 {
        return Vec::new();
    }
    let mut rows = Vec::new();
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
                let columns = grapheme_width(grapheme);
                if columns > width {
                    continue;
                }
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
    rows
}

/// Draw `rows` (from [`wrap`]) one per row of `buf`, from its top.
pub fn render(rows: &[Line<'_>], buf: &mut Buffer) {
    let area = buf.area();
    for (y, row) in (area.top()..area.bottom()).zip(rows) {
        row.render_to(Rect::new(area.x, y, area.width, 1), buf);
    }
}

#[cfg(test)]
mod tests {
    use tilefab::{Color, Style, Stylize};

    use super::*;

    fn texts(rows: &[Line<'_>]) -> Vec<String> {
        rows.iter()
            .map(|row| row.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn wrapped_lines_counts_visual_lines() {
        let rows = wrap(vec![Line::from("abcdef"), Line::from("gh")], 4);
        // “abcdef” wrapped to 4 columns = 2 lines, plus “gh” = 3.
        assert_eq!(texts(&rows), ["abcd", "ef", "gh"]);
    }

    #[test]
    fn empty_lines_stay_and_zero_width_gives_nothing() {
        let rows = wrap(vec![Line::from(""), Line::from("a")], 3);
        assert_eq!(texts(&rows), ["", "a"]);
        assert_eq!(wrap(vec![Line::from("abc")], 0), []);
    }

    #[test]
    fn wide_characters_are_never_split() {
        let rows = wrap(vec![Line::from("a日本語")], 4);
        assert_eq!(texts(&rows), ["a日", "本語"]);
        // Too wide for one column: dropped.
        assert_eq!(texts(&wrap(vec![Line::from("a日b")], 1)), ["a", "b"]);
    }

    #[test]
    fn styles_follow_the_cut() {
        let red = Style::new().fg(Color::Red);
        let line = Line::from(vec![Span::raw("[WARN] "), Span::styled("abcdef", red)]).bold();
        let rows = wrap(vec![line.clone()], 9);
        assert_eq!(texts(&rows), ["[WARN] ab", "cdef"]);
        assert_eq!(rows[0].spans[1].style, red);
        assert_eq!(rows[1].spans[0].style, red);
        assert_eq!(rows[1].style, line.style);
    }

    #[test]
    fn render_writes_one_row_each() {
        let rows = wrap(vec![Line::from("abcdef")], 4);
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        render(&rows, &mut buf);
        assert_eq!(buf, Buffer::with_lines(["abcd", "ef  "]));
    }
}
