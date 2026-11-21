//! Drawing the prompt and the line into a [`Buffer`], with a persistent horizontal scroll.
//!
//! Columns are measured with [`grapheme_width`], the function the `Buffer` itself uses,
//! so the cursor never drifts from the drawn text.

use std::ops::Range;

use tilefab_core::buffer::grapheme_width;
use tilefab_core::{Buffer, Line, Modifier, Position, Rect, Style};
use unicode_segmentation::UnicodeSegmentation;

/// The symbol drawn where text is hidden by the horizontal scroll.
const INDICATOR: &str = "…";

/// Styles of the parts the editor draws on top of the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorStyles {
    /// The hint after the line (default: dim).
    pub hint: Style,
    /// The `…` indicators of hidden text (default: dim).
    pub indicator: Style,
    /// The matched part of the line during a search (default: underlined).
    pub search_match: Style,
    /// The selected candidate of the completion menu (default: reversed).
    pub menu_selected: Style,
    /// The descriptions and the `(+N more)` counter of the menu (default: dim).
    pub menu_description: Style,
}

impl Default for EditorStyles {
    fn default() -> Self {
        let dim = Style::new().add_modifier(Modifier::DIM);
        Self {
            hint: dim,
            indicator: dim,
            search_match: Style::new().add_modifier(Modifier::UNDERLINED),
            menu_selected: Style::new().add_modifier(Modifier::REVERSED),
            menu_description: dim,
        }
    }
}

/// What to draw on the prompt row.
pub(crate) struct LineView<'a> {
    /// The text of the line.
    pub(crate) text: &'a str,
    /// The cursor, in bytes.
    pub(crate) cursor: usize,
    /// Text drawn after the line, when the cursor is at its end.
    pub(crate) hint: Option<&'a str>,
    /// Byte range of the line drawn with the search match style.
    pub(crate) highlight: Option<Range<usize>>,
}

/// Draw `prompt` and `line` on the first row of `row` (non-empty, inside `buf`), update
/// the persistent `scroll` offset (in columns) and return the cursor cell.
pub(crate) fn draw_line(
    buf: &mut Buffer,
    row: Rect,
    prompt: &Line<'_>,
    line: &LineView<'_>,
    scroll: &mut usize,
    margin: u16,
    styles: &EditorStyles,
) -> Position {
    // The prompt never scrolls; when the row is too narrow, it is truncated first so
    // that the line keeps two columns.
    let prompt_width = prompt.width().min(usize::from(row.width.saturating_sub(2)));
    prompt.render_to(Rect::new(row.x, row.y, to_u16(prompt_width), 1), buf);
    let text_x = row.x + to_u16(prompt_width);
    let width = usize::from(row.width) - prompt_width;

    // Columns of each grapheme.
    let mut graphemes = Vec::new();
    let mut total = 0;
    let mut cursor_col = None;
    for (start, grapheme) in line.text.grapheme_indices(true) {
        if start == line.cursor {
            cursor_col = Some(total);
        }
        let columns = grapheme_width(grapheme);
        graphemes.push((start, grapheme, total, columns));
        total += columns;
    }
    let cursor_col = cursor_col.unwrap_or(total);

    *scroll = scroll_offset(*scroll, cursor_col, total, width, usize::from(margin));
    let offset = *scroll;

    for &(start, grapheme, col, columns) in &graphemes {
        // A grapheme cut by an edge is not drawn: its visible column stays blank (or
        // becomes an indicator).
        if columns == 0 || col < offset || col + columns > offset + width {
            continue;
        }
        let style = match &line.highlight {
            Some(range) if range.contains(&start) => styles.search_match,
            _ => Style::new(),
        };
        buf.write_grapheme(text_x + to_u16(col - offset), row.y, grapheme, style);
    }

    let cursor_x = cursor_col - offset;
    if let Some(hint) = line.hint
        && line.cursor == line.text.len()
        && total - offset < width
    {
        let x = total - offset;
        buf.set_stringn(text_x + to_u16(x), row.y, hint, width - x, styles.hint);
    }
    // Below 3 columns, an indicator would hide the only visible text.
    let indicators = width >= 3;
    if indicators && offset > 0 && cursor_x != 0 {
        buf.write_grapheme(text_x, row.y, INDICATOR, styles.indicator);
    }
    if indicators && total > offset + width && cursor_x + 1 != width {
        buf.write_grapheme(
            text_x + to_u16(width - 1),
            row.y,
            INDICATOR,
            styles.indicator,
        );
    }
    Position::new(text_x + to_u16(cursor_x), row.y)
}

/// The scroll offset (first visible column) for a line of `total` columns with the cursor
/// at column `cursor`, in a field `width` columns wide.
///
/// The previous offset is kept as long as it leaves `margin` columns of text visible on
/// each side of the cursor (less where the line ends, and at most `width / 4`): the text
/// does not jump when the cursor moves back. The column after the end of the line is
/// kept for the cursor.
fn scroll_offset(
    previous: usize,
    cursor: usize,
    total: usize,
    width: usize,
    margin: usize,
) -> usize {
    if total < width {
        return 0;
    }
    // At least one column of margin when there is room for it, so that the cursor is
    // never on an indicator.
    let margin = if width >= 3 {
        margin.min(width / 4).max(1)
    } else {
        0
    };
    let left = margin.min(cursor);
    let right = margin.min(total - cursor);
    let lowest = (cursor + right + 1).saturating_sub(width);
    let highest = (cursor - left).min(total + 1 - width);
    if lowest > highest {
        lowest
    } else {
        previous.clamp(lowest, highest)
    }
}

/// A column count that fits the buffer (always the case: the buffer is at most
/// `u16::MAX` wide).
fn to_u16(columns: usize) -> u16 {
    u16::try_from(columns).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draw `text` with the cursor at byte `cursor` in a `width`-column row with the
    /// prompt `"> "`, starting from `scroll`; return the row and the cursor column.
    fn draw(text: &str, cursor: usize, width: u16, scroll: &mut usize) -> (String, u16) {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, 1));
        let view = LineView {
            text,
            cursor,
            hint: None,
            highlight: None,
        };
        let styles = EditorStyles::default();
        let prompt = Line::from("> ");
        let area = buf.area();
        let position = draw_line(&mut buf, area, &prompt, &view, scroll, 4, &styles);
        assert_eq!(position.y, 0);
        (buf.lines().concat(), position.x)
    }

    /// The modifiers of the cell at `(x, 0)`.
    fn modifier(buf: &Buffer, x: u16) -> Modifier {
        buf.get(x, 0).style().add_modifier
    }

    #[test]
    fn a_line_that_fits_is_not_scrolled() {
        let mut scroll = 0;
        assert_eq!(
            draw("hello", 5, 10, &mut scroll),
            ("> hello   ".to_string(), 7)
        );
        assert_eq!(scroll, 0);
    }

    #[test]
    fn the_column_after_the_text_is_kept_for_the_cursor() {
        // 8 columns of text: "abcdefgh" + the cursor needs 9; the field has 8.
        let mut scroll = 0;
        let (row, x) = draw("abcdefgh", 8, 10, &mut scroll);
        assert_eq!(scroll, 1);
        assert_eq!((row.as_str(), x), ("> …cdefgh ", 9));
    }

    #[test]
    fn the_offset_is_kept_while_the_cursor_moves_back() {
        let text = "0123456789abcdefghij";
        let mut scroll = 0;
        let (row, _) = draw(text, 20, 12, &mut scroll);
        assert_eq!(row, "> …cdefghij ");
        assert_eq!(scroll, 11);
        // Moving left within the margin keeps the text in place.
        let (row, x) = draw(text, 15, 12, &mut scroll);
        assert_eq!((row.as_str(), x), ("> …cdefghij ", 6));
        // Close to the left edge: the text scrolls, keeping a margin of 2 columns
        // (width 10 / 4) on the left; the end of the line is visible again.
        let (row, x) = draw(text, 12, 12, &mut scroll);
        assert_eq!(scroll, 10);
        assert_eq!((row.as_str(), x), ("> …bcdefghij", 4));
    }

    #[test]
    fn indicators_on_both_sides_and_never_under_the_cursor() {
        let text = "0123456789abcdefghij";
        let mut scroll = 0;
        let (row, x) = draw(text, 10, 8, &mut scroll);
        assert!(row.starts_with("> …"), "{row}");
        assert!(row.ends_with('…'), "{row}");
        let cursor_cell = row.chars().nth(usize::from(x)).unwrap();
        assert_ne!(cursor_cell, '…');
    }

    #[test]
    fn a_wide_grapheme_cut_by_an_edge_is_never_drawn_half() {
        // Each "漢" is 2 columns: 10 of them are 20 columns.
        let text = "漢".repeat(10);
        let mut scroll = 0;
        for cursor in (0..=text.len()).step_by(3) {
            let (row, x) = draw(&text, cursor, 9, &mut scroll);
            // Every visible "漢" is whole: the row is 9 columns wide.
            let columns: usize = row.graphemes(true).map(grapheme_width).sum();
            assert_eq!(columns, 9, "{row:?}");
            assert!(usize::from(x) < 9);
        }
    }

    #[test]
    fn narrow_rows_truncate_the_prompt_first() {
        let mut scroll = 0;
        // 3 columns: the prompt keeps 1 column, the line 2.
        let (row, x) = draw("abc", 3, 3, &mut scroll);
        assert_eq!((row.as_str(), x), (">c ", 2));
        let (row, x) = draw("abc", 1, 1, &mut scroll);
        assert_eq!((row.as_str(), x), ("b", 0));
    }

    #[test]
    fn the_hint_is_drawn_after_the_line_and_truncated() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 1));
        let view = LineView {
            text: "ab",
            cursor: 2,
            hint: Some("cdefghij"),
            highlight: None,
        };
        let mut scroll = 0;
        let styles = EditorStyles::default();
        let area = buf.area();
        let position = draw_line(
            &mut buf,
            area,
            &Line::from("> "),
            &view,
            &mut scroll,
            4,
            &styles,
        );
        assert_eq!(buf.lines().concat(), "> abcdef");
        assert_eq!(position.x, 4);
        assert_eq!(modifier(&buf, 4), Modifier::DIM);
        assert_eq!(modifier(&buf, 3), Modifier::empty());
        assert_eq!(scroll, 0);
    }

    #[test]
    fn the_highlight_marks_the_matched_bytes() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        let view = LineView {
            text: "abcd",
            cursor: 1,
            hint: None,
            highlight: Some(1..3),
        };
        let styles = EditorStyles::default();
        let area = buf.area();
        let _ = draw_line(&mut buf, area, &Line::from(""), &view, &mut 0, 4, &styles);
        let underlined = Modifier::UNDERLINED;
        let modifiers: Vec<Modifier> = (0..4).map(|x| modifier(&buf, x)).collect();
        assert_eq!(
            modifiers,
            [Modifier::empty(), underlined, underlined, Modifier::empty()]
        );
    }

    #[test]
    fn scroll_offset_edge_cases() {
        assert_eq!(scroll_offset(5, 3, 3, 4, 4), 0);
        // Width 1: the cursor column is the only visible one.
        assert_eq!(scroll_offset(0, 7, 9, 1, 4), 7);
        // A previous offset out of range after a resize is clamped again.
        assert_eq!(scroll_offset(100, 10, 20, 8, 4), 8);
    }
}
