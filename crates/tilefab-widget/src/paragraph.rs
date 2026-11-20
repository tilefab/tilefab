//! `Paragraph`: styled text with line wrapping, scrolling, and an optional block.
//!
//! # Wrapping algorithm (greedy)
//!
//! Each text line wraps independently, using display width (an emoji counts as 2).
//! Split the line into *words* (runs of non-whitespace characters) and *spaces*, then fill
//! the current visual line:
//!
//! 1. add a word if it fits in the remaining space;
//! 2. otherwise, finish the visual line and start the word on the next one;
//! 3. split a word wider than an entire line **at character boundaries** (graphemes),
//!    starting on a fresh line;
//! 4. a space that does not fit ends the line; with `trim: false`, it starts the next
//!    line; with `trim: true`, it is removed;
//! 5. with `trim: true`, remove spaces at the start of each visual line (including the first)
//!    and at the end of wrapped lines;
//! 6. omit a grapheme wider than the entire area (an ideograph in a one-column area);
//!    an empty line stays empty.
//!
//! Differences from ratatui: non-breaking space (U+00A0) does not separate words; an oversized
//! word never fills the end of the previous line; tabs and other control characters
//! are ignored.

use unicode_segmentation::UnicodeSegmentation;

use super::Block;
use tilefab_core::buffer::{Buffer, grapheme_width};
use tilefab_core::convert::saturating_u16;
use tilefab_core::layout::Rect;
use tilefab_core::style::Style;
use tilefab_core::text::{Alignment, Line, Text};
use tilefab_core::widget::Widget;

/// Wrapping mode for overly long lines.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Wrap {
    /// Remove leading spaces from visual lines and trailing spaces from wrapped lines.
    pub trim: bool,
}

/// A displayable grapheme (width ≥ 1) with its style.
///
/// 32 bytes: the width is stored on a `u8` (a grapheme is at most 2 columns wide), which
/// matters for a very long line, where every grapheme of the line is kept at once.
#[derive(Debug, Clone, Copy)]
struct Glyph<'t> {
    symbol: &'t str,
    style: Style,
    /// Display width in columns, 1 or 2 (see [`Glyph::width`]).
    columns: u8,
}

impl Glyph<'_> {
    /// Display width in columns.
    fn width(&self) -> usize {
        usize::from(self.columns)
    }

    /// Whitespace separates words; non-breaking space is excluded.
    fn is_space(&self) -> bool {
        self.symbol
            .chars()
            .all(|c| c.is_whitespace() && c != '\u{a0}')
    }
}

/// A paragraph of text.
///
/// ```
/// use tilefab_core::{Buffer, Rect, Widget};
/// use tilefab_widget::{Paragraph, Wrap};
///
/// let mut buf = Buffer::empty(Rect::new(0, 0, 6, 3));
/// Paragraph::new("one two three")
///     .wrap(Wrap { trim: true })
///     .render(buf.area, &mut buf);
/// assert_eq!(buf.lines(), ["one   ", "two   ", "three "]);
/// ```
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Paragraph<'a> {
    text: Text<'a>,
    block: Option<Block<'a>>,
    style: Style,
    wrap: Option<Wrap>,
    /// Scroll offset `(rows, columns)`.
    scroll: (u16, u16),
    alignment: Option<Alignment>,
}

impl<'a> Paragraph<'a> {
    /// Paragraph displaying `text`.
    pub fn new<T: Into<Text<'a>>>(text: T) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    /// Surround the paragraph with a block.
    #[must_use]
    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    /// Style for the entire paragraph area (including the background).
    #[must_use]
    pub fn style<S: Into<Style>>(mut self, style: S) -> Self {
        self.style = style.into();
        self
    }

    /// Enable wrapping for overly long lines. Without wrapping, they are truncated on the right.
    #[must_use]
    pub fn wrap(mut self, wrap: Wrap) -> Self {
        self.wrap = Some(wrap);
        self
    }

    /// Scroll the text: `(y, x)` = number of hidden visual lines and columns.
    #[must_use]
    pub fn scroll(mut self, offset: (u16, u16)) -> Self {
        self.scroll = offset;
        self
    }

    /// Alignment for lines without their own.
    #[must_use]
    pub fn alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = Some(alignment);
        self
    }

    /// Align left.
    #[must_use]
    pub fn left_aligned(self) -> Self {
        self.alignment(Alignment::Left)
    }

    /// Center.
    #[must_use]
    pub fn centered(self) -> Self {
        self.alignment(Alignment::Center)
    }

    /// Align right.
    #[must_use]
    pub fn right_aligned(self) -> Self {
        self.alignment(Alignment::Right)
    }

    /// Number of lines required to display all text in an area of `width`
    /// columns, including block borders. Use this to reserve space before
    /// `insert_before`. Returns 0 if the area is too narrow to contain anything.
    #[must_use]
    pub fn line_count(&self, width: u16) -> usize {
        let (horizontal, vertical) = self
            .block
            .as_ref()
            .map_or((0, 0), |b| (b.horizontal_space(), b.vertical_space()));
        if width <= horizontal {
            return 0;
        }
        let width = usize::from(width - horizontal);
        let mut wrapper = Wrapper::default();
        let rows: usize = self
            .text
            .lines
            .iter()
            .map(|line| wrapper.visual_lines(line, self.wrap, width).count())
            .sum();
        rows + usize::from(vertical)
    }
}

impl Widget for Paragraph<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = buf.area.intersection(area);
        if area.is_empty() {
            return;
        }
        buf.set_style(area, self.style);
        let text_area = match &self.block {
            Some(block) => {
                block.render_ref(area, buf);
                block.inner(area)
            }
            None => area,
        };
        if text_area.is_empty() {
            return;
        }
        buf.set_style(text_area, self.text.style);

        let (scroll_y, scroll_x) = (usize::from(self.scroll.0), usize::from(self.scroll.1));
        let end = scroll_y + usize::from(text_area.height);
        let width = usize::from(text_area.width);
        // Visual lines are produced one text line at a time, in buffers reused for the whole
        // paragraph, drawn as soon as they are produced, and not produced at all past the
        // area: lines above it are still wrapped (to know how many rows they take), lines
        // below it are not.
        let mut wrapper = Wrapper::default();
        let mut row = 0;
        for line in &self.text.lines {
            if row >= end {
                break;
            }
            let alignment = line.alignment.or(self.alignment).or(self.text.alignment);
            for glyphs in wrapper.visual_lines(line, self.wrap, width) {
                if (scroll_y..end).contains(&row) {
                    let y = text_area.y + saturating_u16(row - scroll_y);
                    let rect = Rect::new(text_area.x, y, text_area.width, 1);
                    draw_glyphs(glyphs, scroll_x, line.style, alignment, rect, buf);
                }
                row += 1;
            }
        }
    }
}

/// Buffers for wrapping the lines of one paragraph, reused from one line to the next:
/// once they have grown to the longest line, wrapping allocates nothing.
#[derive(Default)]
struct Wrapper<'t> {
    /// Graphemes of the current text line.
    glyphs: Vec<Glyph<'t>>,
    /// Its visual lines, one after the other, as indices into `glyphs` (8 bytes each,
    /// instead of copying 40-byte glyphs)…
    picked: Vec<usize>,
    /// …each ending at these indices of `picked`.
    ends: Vec<usize>,
}

impl<'t> Wrapper<'t> {
    /// The visual lines of `line` in an area of `width` columns: one without wrapping
    /// (the whole line, nothing copied), at least one with it.
    fn visual_lines(
        &mut self,
        line: &'t Line<'_>,
        wrap: Option<Wrap>,
        width: usize,
    ) -> impl Iterator<Item = Row<'_, 't>> {
        line_glyphs(line, &mut self.glyphs);
        let picked = match wrap {
            None => {
                self.ends.clear();
                self.ends.push(self.glyphs.len());
                None
            }
            Some(Wrap { trim }) => {
                wrap_glyphs_into(&self.glyphs, width, trim, &mut self.picked, &mut self.ends);
                Some(self.picked.as_slice())
            }
        };
        let glyphs = self.glyphs.as_slice();
        let starts = std::iter::once(0).chain(self.ends.iter().copied());
        starts
            .zip(self.ends.iter().copied())
            .map(move |(start, end)| Row {
                glyphs,
                picked: picked.map(|picked| &picked[start..end]),
            })
    }
}

/// One visual line: the graphemes of `glyphs` at the indices `picked`, in order, or all of
/// them when `picked` is `None` (no wrapping).
#[derive(Clone, Copy)]
struct Row<'w, 't> {
    glyphs: &'w [Glyph<'t>],
    picked: Option<&'w [usize]>,
}

impl<'w, 't> Row<'w, 't> {
    fn glyphs(self) -> impl Iterator<Item = Glyph<'t>> + use<'w, 't> {
        let len = self.picked.map_or(self.glyphs.len(), <[usize]>::len);
        (0..len).map(move |k| match self.picked {
            Some(picked) => self.glyphs[picked[k]],
            None => self.glyphs[k],
        })
    }
}

/// Draw one visual line in `row` (one row high), as `Line::render_to` would draw it with
/// the same spans, but straight from the measured graphemes: no intermediate `String`
/// or `Line`, and no second pass of grapheme segmentation.
fn draw_glyphs(
    visual: Row<'_, '_>,
    scroll_x: usize,
    style: Style,
    alignment: Option<Alignment>,
    row: Rect,
    buf: &mut Buffer,
) {
    let row = buf.area.intersection(row);
    if row.is_empty() {
        return;
    }
    buf.set_style(row, style);
    let available = usize::from(row.width);
    let width: usize = visible_glyphs(visual.glyphs(), scroll_x)
        .map(|g| g.width())
        .sum();
    let offset = if width >= available {
        0
    } else {
        match alignment.unwrap_or_default() {
            Alignment::Left => 0,
            Alignment::Center => (available - width) / 2,
            Alignment::Right => available - width,
        }
    };
    let mut x = row.x + saturating_u16(offset);
    // Only limits anything when `offset` is 0: an offset means the whole line fits.
    let mut remaining = available - offset;
    // Stop at the first grapheme that does not fit, like `Buffer::set_stringn`.
    for glyph in visible_glyphs(visual.glyphs(), scroll_x) {
        if glyph.width() > remaining {
            break;
        }
        buf.write_grapheme(x, row.y, glyph.symbol, glyph.width(), glyph.style);
        x += saturating_u16(glyph.width());
        remaining -= glyph.width();
    }
}

/// Displayable graphemes of a line (zero-width characters are omitted), into `glyphs`
/// (emptied first).
fn line_glyphs<'t>(line: &'t Line<'_>, glyphs: &mut Vec<Glyph<'t>>) {
    glyphs.clear();
    // A grapheme takes at least one byte: reserve once rather than grow step by step (a
    // no-op when a longer line came before).
    glyphs.reserve(line.spans.iter().map(|span| span.content.len()).sum());
    for span in &line.spans {
        for symbol in span.content.graphemes(true) {
            let width = grapheme_width(symbol);
            if width > 0 {
                glyphs.push(Glyph {
                    symbol,
                    style: span.style,
                    // `grapheme_width` is at most 2: the conversion never saturates.
                    columns: u8::try_from(width).unwrap_or(u8::MAX),
                });
            }
        }
    }
}

/// Wrap a line to `width` columns (see the module header), into `out`: the indices of
/// the graphemes kept, visual line after visual line, the `i`-th ending at index `ends[i]`
/// of `out` (both emptied first). Always at least one visual line (empty if `glyphs` is
/// empty).
fn wrap_glyphs_into(
    glyphs: &[Glyph<'_>],
    width: usize,
    trim: bool,
    out: &mut Vec<usize>,
    ends: &mut Vec<usize>,
) {
    out.clear();
    ends.clear();
    // The visual lines never hold more graphemes than the line: reserve once.
    out.reserve(glyphs.len());
    // The current visual line is `out[start..]`, `current_width` columns wide.
    let mut start = 0;
    let mut current_width = 0;

    // Finish the current visual line.
    let finish = |out: &mut Vec<usize>, ends: &mut Vec<usize>, start: &mut usize, w: &mut usize| {
        if trim {
            // Trailing spaces of this visual line only (`out[*start..]`).
            while out[*start..].last().is_some_and(|&j| glyphs[j].is_space()) {
                out.pop();
            }
        }
        ends.push(out.len());
        *start = out.len();
        *w = 0;
    };

    let mut i = 0;
    while i < glyphs.len() {
        // A run: consecutive spaces or non-spaces, `glyphs[run_start..i]` once measured.
        let run_start = i;
        let space = glyphs[i].is_space();
        let mut run_width = 0;
        while i < glyphs.len() && glyphs[i].is_space() == space {
            run_width += glyphs[i].width();
            i += 1;
        }
        let run = run_start..i;

        if space {
            for j in run {
                let glyph = &glyphs[j];
                let current_is_empty = out.len() == start;
                if trim && current_is_empty {
                    continue; // leading space
                }
                if current_width + glyph.width() > width {
                    if trim || current_is_empty {
                        continue; // the space is the wrap point (or cannot fit)
                    }
                    finish(out, ends, &mut start, &mut current_width);
                }
                out.push(j);
                current_width += glyph.width();
            }
        } else if current_width + run_width <= width {
            out.extend(run);
            current_width += run_width;
        } else {
            if out.len() > start {
                finish(out, ends, &mut start, &mut current_width);
            }
            if run_width <= width {
                out.extend(run);
                current_width = run_width;
            } else {
                // Word wider than a line: wrap at character boundaries.
                for j in run {
                    let glyph = &glyphs[j];
                    if glyph.width() > width {
                        continue; // will never fit
                    }
                    if current_width + glyph.width() > width {
                        finish(out, ends, &mut start, &mut current_width);
                    }
                    out.push(j);
                    current_width += glyph.width();
                }
            }
        }
    }
    finish(out, ends, &mut start, &mut current_width);
}

/// [`wrap_glyphs_into`] with one `Vec` of glyphs per visual line, for the tests.
#[cfg(test)]
fn wrap_glyphs<'t>(glyphs: &[Glyph<'t>], width: usize, trim: bool) -> Vec<Vec<Glyph<'t>>> {
    let (mut out, mut ends) = (Vec::new(), Vec::new());
    wrap_glyphs_into(glyphs, width, trim, &mut out, &mut ends);
    let starts = std::iter::once(0).chain(ends.iter().copied());
    starts
        .zip(ends.iter().copied())
        .map(|(s, e)| out[s..e].iter().map(|&j| glyphs[j]).collect())
        .collect()
}

/// The graphemes left once the first `columns` columns are hidden. A wide character split
/// in half leaves a space for its still-visible half.
fn visible_glyphs<'t>(
    glyphs: impl Iterator<Item = Glyph<'t>>,
    columns: usize,
) -> impl Iterator<Item = Glyph<'t>> {
    let mut skipped = 0;
    glyphs.filter_map(move |glyph| {
        if skipped >= columns {
            Some(glyph)
        } else if skipped + glyph.width() <= columns {
            skipped += glyph.width();
            None
        } else {
            skipped = columns;
            Some(Glyph {
                symbol: " ",
                style: glyph.style,
                columns: 1,
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::super::Borders;
    use super::*;
    use tilefab_core::convert::saturating_u16;
    use tilefab_core::style::{Color, Modifier};
    use tilefab_core::text::{Span, Stylize};

    const TRIM: Wrap = Wrap { trim: true };
    const KEEP: Wrap = Wrap { trim: false };

    fn render(paragraph: Paragraph, width: u16, height: u16) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        paragraph.render(buf.area, &mut buf);
        buf
    }

    /// Wrapped text, one string per visual line (trailing spaces removed for comparison).
    fn wrapped(text: &str, width: u16, wrap: Wrap) -> Vec<String> {
        let paragraph = Paragraph::new(text).wrap(wrap);
        let count = paragraph.line_count(width);
        render(paragraph, width, saturating_u16(count))
            .lines()
            .into_iter()
            .map(|l| l.trim_end().to_string())
            .collect()
    }

    #[test]
    fn without_wrap_long_lines_are_truncated() {
        assert_eq!(
            render(Paragraph::new("abcdef\nxy"), 4, 3),
            Buffer::with_lines(["abcd", "xy  ", "    "])
        );
    }

    #[test]
    fn wraps_at_word_boundaries() {
        assert_eq!(
            wrapped("hello world foo", 8, KEEP),
            ["hello", "world", "foo"]
        );
        assert_eq!(
            wrapped("hello world foo", 8, TRIM),
            ["hello", "world", "foo"]
        );
        assert_eq!(wrapped("un deux trois", 6, TRIM), ["un", "deux", "trois"]);
        // Everything fits: a single line.
        assert_eq!(wrapped("a b c", 20, TRIM), ["a b c"]);
    }

    #[test]
    fn trim_controls_the_space_at_the_break() {
        // “hello world” fills exactly 11 columns; the next space is the wrap point.
        assert_eq!(
            wrapped("hello world foo", 11, KEEP),
            ["hello world", " foo"]
        );
        assert_eq!(wrapped("hello world foo", 11, TRIM), ["hello world", "foo"]);
    }

    #[test]
    fn trim_removes_leading_whitespace_of_every_line_keep_preserves_it() {
        assert_eq!(wrapped("  ab cd", 10, TRIM), ["ab cd"]);
        assert_eq!(wrapped("  ab cd", 10, KEEP), ["  ab cd"]);
        assert_eq!(wrapped("    ab", 4, TRIM), ["ab"]);
    }

    #[test]
    fn long_words_are_split_by_character_on_a_fresh_line() {
        assert_eq!(wrapped("abcdefghij", 4, TRIM), ["abcd", "efgh", "ij"]);
        assert_eq!(wrapped("ab cdefghij", 4, TRIM), ["ab", "cdef", "ghij"]);
        assert_eq!(
            wrapped("abcdefghij kl", 4, TRIM),
            ["abcd", "efgh", "ij", "kl"]
        );
        assert_eq!(wrapped("abcdefgh", 4, KEEP), ["abcd", "efgh"]);
    }

    #[test]
    fn wide_characters_wrap_by_display_width() {
        assert_eq!(wrapped("日本語", 5, TRIM), ["日本", "語"]);
        assert_eq!(wrapped("a日本 b", 4, TRIM), ["a日", "本 b"]);
        // A grapheme wider than the area is omitted, without an infinite loop.
        assert_eq!(wrapped("a日b", 1, TRIM), ["a", "b"]);
        assert_eq!(wrapped("👍🏽🇫🇷", 2, TRIM), ["👍🏽", "🇫🇷"]);
    }

    #[test]
    fn empty_lines_are_preserved() {
        assert_eq!(wrapped("a\n\nb", 5, TRIM), ["a", "", "b"]);
        assert_eq!(wrapped("", 5, TRIM), [""]);
        assert_eq!(wrapped("a\n", 5, KEEP), ["a", ""]);
        assert_eq!(wrapped("   ", 5, TRIM), [""]);
    }

    #[test]
    fn non_breaking_space_does_not_split_words() {
        assert_eq!(wrapped("a\u{a0}b c", 3, TRIM), ["a\u{a0}b", "c"]);
    }

    #[test]
    fn line_count_matches_the_rendered_height() {
        for text in [
            "",
            "a",
            "abc def ghi jkl",
            "abcdefghij",
            "a\n\nb c d",
            "日本語 日本語",
            "  x  y  ",
        ] {
            for width in 1..12 {
                for wrap in [KEEP, TRIM] {
                    let paragraph = Paragraph::new(text).wrap(wrap);
                    let expected = {
                        let n = paragraph.line_count(width);
                        // Every counted line must exist: one fewer loses visible text.
                        let full = render(paragraph.clone(), width, saturating_u16(n));
                        let one_less = render(
                            paragraph.clone(),
                            width,
                            saturating_u16(n).saturating_sub(1),
                        );
                        (n, full, one_less)
                    };
                    let (n, full, _) = expected;
                    assert_eq!(
                        n,
                        wrapped(text, width, wrap).len(),
                        "{text:?} largeur {width}"
                    );
                    assert_eq!(full.area.height as usize, n);
                }
            }
        }
    }

    #[test]
    fn line_count_without_wrap_is_the_number_of_lines() {
        assert_eq!(Paragraph::new("a\nb\nc").line_count(1), 3);
        assert_eq!(Paragraph::new("abcdef").line_count(2), 1);
    }

    #[test]
    fn line_count_edge_cases() {
        assert_eq!(Paragraph::new("abc").wrap(TRIM).line_count(0), 0);
        let framed = Paragraph::new("abc def")
            .wrap(TRIM)
            .block(Block::bordered());
        // 3-column interior: “abc”, “def”, plus the two border rows.
        assert_eq!(framed.line_count(5), 4);
        // Too narrow to fit a column of text between the borders.
        assert_eq!(framed.line_count(2), 0);
    }

    #[test]
    fn alignment_applies_to_each_wrapped_line() {
        let p = Paragraph::new("ab cd").wrap(TRIM).centered();
        assert_eq!(render(p, 4, 2), Buffer::with_lines([" ab ", " cd "]));
        let p = Paragraph::new("ab cd").wrap(TRIM).right_aligned();
        assert_eq!(render(p, 4, 2), Buffer::with_lines(["  ab", "  cd"]));
        // Without trim, the trailing space is part of the line and shifts its centering.
        let p = Paragraph::new("ab cd").wrap(KEEP).centered();
        assert_eq!(render(p, 4, 2), Buffer::with_lines(["ab  ", " cd "]));
    }

    #[test]
    fn a_line_alignment_overrides_the_paragraph_one() {
        let text = Text::from(vec![Line::from("a"), Line::from("b").right_aligned()]);
        let p = Paragraph::new(text).centered();
        assert_eq!(render(p, 3, 2), Buffer::with_lines([" a ", "  b"]));
    }

    #[test]
    fn scroll_skips_lines_and_columns() {
        let p = Paragraph::new("l1\nl2\nl3\nl4").scroll((2, 0));
        assert_eq!(render(p, 2, 2), Buffer::with_lines(["l3", "l4"]));
        let p = Paragraph::new("abcdef").scroll((0, 2));
        assert_eq!(render(p, 4, 1), Buffer::with_lines(["cdef"]));
        // Scrolling beyond the text: nothing, without panicking.
        let p = Paragraph::new("abc").scroll((9, 9));
        assert_eq!(render(p, 3, 1), Buffer::with_lines(["   "]));
        let p = Paragraph::new("abc").scroll((0, 9));
        assert_eq!(render(p, 3, 1), Buffer::with_lines(["   "]));
    }

    #[test]
    fn scrolling_through_a_wide_character_leaves_a_blank() {
        let p = Paragraph::new("日本").scroll((0, 1));
        assert_eq!(render(p, 4, 1), Buffer::with_lines([" 本 "]));
        let p = Paragraph::new("日本").scroll((0, 2));
        assert_eq!(render(p, 4, 1), Buffer::with_lines(["本  "]));
    }

    #[test]
    fn scroll_counts_wrapped_lines() {
        let p = Paragraph::new("aa bb cc").wrap(TRIM).scroll((1, 0));
        assert_eq!(render(p, 2, 3), Buffer::with_lines(["bb", "cc", "  "]));
    }

    #[test]
    fn spans_keep_their_style_across_wrapped_lines() {
        let text = Line::from(vec!["aa ".into(), "bb".fg(Color::Red), " cc".bold()]);
        let buf = render(Paragraph::new(text).wrap(TRIM), 2, 3);
        assert_eq!(buf.lines(), ["aa", "bb", "cc"]);
        assert_eq!(buf.get(0, 0).fg, Color::Reset);
        assert_eq!(buf.get(0, 1).fg, Color::Red);
        assert!(buf.get(0, 2).modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn paragraph_style_and_line_style_cover_their_area() {
        let text = Text::from(vec![Line::from("a"), Line::from("b").bg(Color::Green)]);
        let buf = render(
            Paragraph::new(text).style(Style::new().fg(Color::Cyan)),
            3,
            2,
        );
        assert!(buf.content.iter().all(|c| c.fg == Color::Cyan));
        assert_eq!(
            buf.get(2, 1).bg,
            Color::Green,
            "the line style covers the row"
        );
        assert_eq!(buf.get(2, 0).bg, Color::Reset);
    }

    #[test]
    fn block_frames_the_text() {
        let p = Paragraph::new("abc def")
            .wrap(TRIM)
            .block(Block::bordered().title("t"));
        assert_eq!(
            render(p, 5, 4),
            Buffer::with_lines(["┌t──┐", "│abc│", "│def│", "└───┘"])
        );
        let p = Paragraph::new("abc").block(Block::new().borders(Borders::LEFT));
        assert_eq!(render(p, 4, 1), Buffer::with_lines(["│abc"]));
    }

    #[test]
    fn tiny_areas_never_panic() {
        let long = "very long word with accents été 日本語 and emoji 👍🏽 end";
        for w in 0..5 {
            for h in 0..4 {
                for wrap in [None, Some(KEEP), Some(TRIM)] {
                    let mut p = Paragraph::new(long).block(Block::bordered()).centered();
                    if let Some(wrap) = wrap {
                        p = p.wrap(wrap);
                    }
                    let _ = render(p.clone(), w, h);
                    let _ = render(p.clone().scroll((1, 1)), w, h);
                    let _ = p.line_count(w);
                }
            }
        }
    }

    #[test]
    fn width_one_area_shows_one_column() {
        assert_eq!(
            render(Paragraph::new("abc").wrap(TRIM), 1, 3),
            Buffer::with_lines(["a", "b", "c"])
        );
        assert_eq!(
            render(Paragraph::new("abc"), 1, 1),
            Buffer::with_lines(["a"])
        );
    }

    #[test]
    fn a_glyph_takes_32_bytes() {
        assert_eq!(std::mem::size_of::<Glyph<'_>>(), 32);
    }

    #[test]
    fn left_aligned_overrides_an_earlier_alignment() {
        let paragraph = Paragraph::new("ab").centered().left_aligned();
        assert_eq!(render(paragraph, 6, 1), Buffer::with_lines(["ab    "]));
    }

    #[test]
    fn zero_width_characters_are_never_written() {
        // A styled tab before a plain character: its style must not reach any cell.
        let line = Line::from(vec![
            Span::styled("\t", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw("x"),
        ]);
        let buf = render(Paragraph::new(line), 3, 1);
        assert_eq!(buf.lines(), ["x  "]);
        assert!(buf.content.iter().all(|c| c.modifier.is_empty()), "{buf:?}");
        // At the very end of a full row: nothing is written past its right edge.
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 2));
        Paragraph::new("abc\t").render(Rect::new(0, 0, 3, 1), &mut buf);
        assert_eq!(buf.lines(), ["abc", "   "]);
    }

    #[test]
    fn rendering_is_clipped_to_the_buffer() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        Paragraph::new("abcdef").render(Rect::new(1, 0, 20, 5), &mut buf);
        assert_eq!(buf, Buffer::with_lines([" ab"]));
    }

    /// The rendering before the wrapping optimization, kept verbatim as the reference
    /// that the optimized one must match cell for cell.
    mod reference {
        use super::super::{Block, Paragraph, Wrap};
        use tilefab_core::buffer::{Buffer, grapheme_width};
        use tilefab_core::layout::Rect;
        use tilefab_core::style::Style;
        use tilefab_core::text::{Alignment, Line, Span};
        use unicode_segmentation::UnicodeSegmentation;

        /// The glyph as it was (a `usize` width).
        #[derive(Debug, Clone, Copy)]
        struct Glyph<'t> {
            symbol: &'t str,
            style: Style,
            width: usize,
        }

        impl Glyph<'_> {
            /// Whitespace separates words; non-breaking space is excluded.
            fn is_space(&self) -> bool {
                self.symbol
                    .chars()
                    .all(|c| c.is_whitespace() && c != '\u{a0}')
            }
        }

        /// A line as it will be drawn.
        struct Visual<'t> {
            glyphs: Vec<Glyph<'t>>,
            style: Style,
            alignment: Option<Alignment>,
        }

        /// Lines to draw in an area of `width` columns.
        fn visual_lines<'p>(paragraph: &'p Paragraph<'_>, width: usize) -> Vec<Visual<'p>> {
            let mut visuals = Vec::new();
            for line in &paragraph.text.lines {
                let glyphs = line_glyphs(line);
                let alignment = line
                    .alignment
                    .or(paragraph.alignment)
                    .or(paragraph.text.alignment);
                let rows = match paragraph.wrap {
                    None => vec![glyphs],
                    Some(Wrap { trim }) => wrap_glyphs(&glyphs, width, trim),
                };
                visuals.extend(rows.into_iter().map(|glyphs| Visual {
                    glyphs,
                    style: line.style,
                    alignment,
                }));
            }
            visuals
        }

        /// `Paragraph::line_count`, as it was.
        pub fn line_count(paragraph: &Paragraph<'_>, width: u16) -> usize {
            let (horizontal, vertical) = paragraph
                .block
                .as_ref()
                .map_or((0, 0), |b| (b.horizontal_space(), b.vertical_space()));
            if width <= horizontal {
                return 0;
            }
            visual_lines(paragraph, (width - horizontal) as usize).len() + vertical as usize
        }

        /// `Paragraph::render`, as it was.
        pub fn render(paragraph: &Paragraph<'_>, area: Rect, buf: &mut Buffer) {
            let area = buf.area.intersection(area);
            if area.is_empty() {
                return;
            }
            buf.set_style(area, paragraph.style);
            let text_area = match &paragraph.block {
                Some(block) => {
                    block.render_ref(area, buf);
                    Block::inner(block, area)
                }
                None => area,
            };
            if text_area.is_empty() {
                return;
            }
            buf.set_style(text_area, paragraph.text.style);
            let (scroll_y, scroll_x) = paragraph.scroll;
            let visuals = visual_lines(paragraph, text_area.width as usize);
            let rows = visuals
                .into_iter()
                .skip(scroll_y as usize)
                .take(text_area.height as usize);
            for (row, visual) in (0..text_area.height).zip(rows) {
                let glyphs = skip_columns(&visual.glyphs, scroll_x as usize);
                let line = Line {
                    spans: to_spans(&glyphs),
                    style: visual.style,
                    alignment: visual.alignment,
                };
                let rect = Rect::new(text_area.x, text_area.y + row, text_area.width, 1);
                line.render_to(rect, buf);
            }
        }

        /// Displayable graphemes of a line (zero-width characters are omitted).
        fn line_glyphs<'t>(line: &'t Line<'_>) -> Vec<Glyph<'t>> {
            let mut glyphs = Vec::new();
            for span in &line.spans {
                for symbol in span.content.graphemes(true) {
                    let width = grapheme_width(symbol);
                    if width > 0 {
                        glyphs.push(Glyph {
                            symbol,
                            style: span.style,
                            width,
                        });
                    }
                }
            }
            glyphs
        }

        /// Wrap a line to `width` columns (see the module header). Always return at least
        /// one line (empty if `glyphs` is empty).
        fn wrap_glyphs<'t>(glyphs: &[Glyph<'t>], width: usize, trim: bool) -> Vec<Vec<Glyph<'t>>> {
            let mut lines: Vec<Vec<Glyph<'t>>> = Vec::new();
            let mut current: Vec<Glyph<'t>> = Vec::new();
            let mut current_width = 0;

            // Finish the current visual line.
            let finish =
                |lines: &mut Vec<Vec<Glyph<'t>>>, current: &mut Vec<Glyph<'t>>, w: &mut usize| {
                    if trim {
                        while current.last().is_some_and(Glyph::is_space) {
                            current.pop();
                        }
                    }
                    lines.push(std::mem::take(current));
                    *w = 0;
                };

            let mut i = 0;
            while i < glyphs.len() {
                // A run: consecutive spaces or non-spaces.
                let space = glyphs[i].is_space();
                let mut end = i;
                let mut run_width = 0;
                while end < glyphs.len() && glyphs[end].is_space() == space {
                    run_width += glyphs[end].width;
                    end += 1;
                }
                let run = &glyphs[i..end];
                i = end;

                if space {
                    for glyph in run {
                        if trim && current.is_empty() {
                            continue; // leading space
                        }
                        if current_width + glyph.width > width {
                            if trim || current.is_empty() {
                                continue; // the space is the wrap point (or cannot fit)
                            }
                            finish(&mut lines, &mut current, &mut current_width);
                        }
                        current.push(*glyph);
                        current_width += glyph.width;
                    }
                } else if current_width + run_width <= width {
                    current.extend_from_slice(run);
                    current_width += run_width;
                } else {
                    if !current.is_empty() {
                        finish(&mut lines, &mut current, &mut current_width);
                    }
                    if run_width <= width {
                        current.extend_from_slice(run);
                        current_width = run_width;
                    } else {
                        // Word wider than a line: wrap at character boundaries.
                        for glyph in run {
                            if glyph.width > width {
                                continue; // will never fit
                            }
                            if current_width + glyph.width > width {
                                finish(&mut lines, &mut current, &mut current_width);
                            }
                            current.push(*glyph);
                            current_width += glyph.width;
                        }
                    }
                }
            }
            finish(&mut lines, &mut current, &mut current_width);
            lines
        }

        /// Hide the first `columns` columns. A wide character split in half leaves a space
        /// for its still-visible half.
        fn skip_columns<'t>(glyphs: &[Glyph<'t>], columns: usize) -> Vec<Glyph<'t>> {
            let mut skipped = 0;
            let mut kept = Vec::with_capacity(glyphs.len());
            for glyph in glyphs {
                if skipped >= columns {
                    kept.push(*glyph);
                } else if skipped + glyph.width <= columns {
                    skipped += glyph.width;
                } else {
                    kept.push(Glyph {
                        symbol: " ",
                        style: glyph.style,
                        width: 1,
                    });
                    skipped = columns;
                }
            }
            kept
        }

        /// Group consecutive graphemes with the same style into spans.
        fn to_spans(glyphs: &[Glyph<'_>]) -> Vec<Span<'static>> {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for glyph in glyphs {
                match spans.last_mut() {
                    Some(last) if last.style == glyph.style => {
                        last.content.to_mut().push_str(glyph.symbol);
                    }
                    _ => spans.push(Span::styled(glyph.symbol.to_string(), glyph.style)),
                }
            }
            spans
        }
    }

    /// Styles used by the generated paragraphs, so that spans can differ in style.
    fn styles() -> [Style; 3] {
        [
            Style::new(),
            Style::new().fg(Color::Red),
            Style::new().bg(Color::Blue).add_modifier(Modifier::BOLD),
        ]
    }
    const ALIGNMENTS: [Option<Alignment>; 4] = [
        None,
        Some(Alignment::Left),
        Some(Alignment::Center),
        Some(Alignment::Right),
    ];

    /// A random paragraph: up to 4 lines of up to 3 spans each, with styles, alignments,
    /// wrapping (none, kept spaces, trimmed), scrolling and an optional border.
    fn paragraph() -> impl proptest::strategy::Strategy<Value = Paragraph<'static>> {
        use proptest::collection::vec;
        use proptest::prelude::*;
        let span = (vec(0..SYMBOLS.len(), 0..10), 0..3usize).prop_map(|(symbols, style)| {
            let content: String = symbols.iter().map(|&i| SYMBOLS[i]).collect();
            Span::styled(content, styles()[style])
        });
        let line = (vec(span, 0..4), 0..3usize, 0..ALIGNMENTS.len()).prop_map(
            |(spans, style, alignment)| Line {
                spans,
                style: styles()[style],
                alignment: ALIGNMENTS[alignment],
            },
        );
        (
            vec(line, 0..5),
            0..3usize,
            (0u16..6, 0u16..8),
            0..ALIGNMENTS.len(),
            any::<bool>(),
        )
            .prop_map(|(lines, wrap, scroll, alignment, bordered)| {
                let mut paragraph = Paragraph::new(Text::from(lines)).scroll(scroll);
                if let Some(alignment) = ALIGNMENTS[alignment] {
                    paragraph = paragraph.alignment(alignment);
                }
                if wrap > 0 {
                    paragraph = paragraph.wrap(Wrap { trim: wrap == 2 });
                }
                if bordered {
                    paragraph = paragraph.block(Block::bordered());
                }
                paragraph
            })
    }

    proptest::proptest! {
        #[test]
        fn optimized_rendering_matches_the_reference(
            paragraph in paragraph(),
            (x, y, width, height) in (0u16..3, 0u16..3, 0u16..16, 0u16..9),
        ) {
            // A buffer larger than the area, so that writing outside it would show.
            let area = Rect::new(x, y, width, height);
            let mut expected = Buffer::empty(Rect::new(0, 0, 20, 12));
            let mut actual = expected.clone();
            reference::render(&paragraph, area, &mut expected);
            paragraph.clone().render(area, &mut actual);
            proptest::prop_assert_eq!(actual, expected);
            proptest::prop_assert_eq!(
                paragraph.line_count(width),
                reference::line_count(&paragraph, width)
            );
        }
    }

    // --- properties (proptest) of `wrap_glyphs` ---

    /// Symbols mixing narrow and wide characters, breaking and non-breaking spaces, and
    /// a tab (zero width: never displayed).
    const SYMBOLS: [&str; 7] = ["a", "b", "é", "漢", " ", "\u{a0}", "\t"];

    /// Glyphs as `line_glyphs` makes them: zero-width symbols (the tab) are left out.
    fn glyphs(indices: &[usize]) -> Vec<Glyph<'static>> {
        indices
            .iter()
            .map(|&i| Glyph {
                symbol: SYMBOLS[i],
                style: Style::default(),
                columns: u8::try_from(grapheme_width(SYMBOLS[i])).unwrap_or(u8::MAX),
            })
            .filter(|glyph| glyph.width() > 0)
            .collect()
    }

    fn line_width(line: &[Glyph<'_>]) -> usize {
        line.iter().map(Glyph::width).sum()
    }

    proptest::proptest! {
        #[test]
        fn wrapped_lines_fit_and_lose_no_word_character(
            indices in proptest::collection::vec(0..SYMBOLS.len(), 0..40),
            width in 0usize..12,
            trim: bool,
        ) {
            let input = glyphs(&indices);
            let lines = wrap_glyphs(&input, width, trim);
            proptest::prop_assert!(!lines.is_empty());
            for line in &lines {
                proptest::prop_assert!(line_width(line) <= width, "too wide: {:?}", line);
            }
            // Characters other than spaces are all kept, in order, except those wider
            // than a line, which can never fit.
            let words = |glyphs: &mut dyn Iterator<Item = &Glyph<'static>>| -> Vec<&str> {
                glyphs.filter(|g| !g.is_space()).map(|g| g.symbol).collect()
            };
            let expected = words(&mut input.iter().filter(|g| g.width() <= width));
            proptest::prop_assert_eq!(words(&mut lines.iter().flatten()), expected);
        }

        #[test]
        fn trimmed_lines_neither_start_nor_end_with_a_space(
            indices in proptest::collection::vec(0..SYMBOLS.len(), 0..40),
            width in 0usize..12,
        ) {
            for line in wrap_glyphs(&glyphs(&indices), width, true) {
                proptest::prop_assert!(!line.first().is_some_and(Glyph::is_space), "{:?}", line);
                proptest::prop_assert!(!line.last().is_some_and(Glyph::is_space), "{:?}", line);
            }
        }

        #[test]
        fn untrimmed_wrapping_keeps_every_glyph_that_fits(
            indices in proptest::collection::vec(0..SYMBOLS.len(), 0..40),
            width in 1usize..12,
        ) {
            // Without trimming, spaces are kept too (they are the wrap points): only
            // glyphs wider than a line disappear.
            let input = glyphs(&indices);
            let kept: Vec<&str> = wrap_glyphs(&input, width, false)
                .iter()
                .flatten()
                .map(|g| g.symbol)
                .collect();
            let expected: Vec<&str> =
                input.iter().filter(|g| g.width() <= width).map(|g| g.symbol).collect();
            proptest::prop_assert_eq!(kept, expected);
        }
    }
}
