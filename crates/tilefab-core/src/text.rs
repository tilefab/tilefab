//! Styled text: [`Span`] (a segment), [`Line`] (a line of spans), [`Text`] (multiple lines).
//!
//! ```
//! use tilefab_core::{Buffer, Color, Line, Rect, Span, Stylize};
//!
//! let line = Line::from(vec![Span::raw("name"), "45%".fg(Color::Cyan).bold()]).right_aligned();
//! let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
//! line.render_to(buf.area(), &mut buf);
//! assert_eq!(buf.lines(), ["   name45%"]);
//! assert_eq!(buf.get(7, 0).fg, Color::Cyan);
//! ```
//!
//! Widths are **display columns**: an emoji or ideograph counts as 2, a combining
//! accent as 0 (see `buffer::grapheme_width`). Control characters (including `\n`)
//! are not displayed: build a `Text` line by line, rather than putting line feeds
//! in a `Span`.

use std::borrow::Cow;

use unicode_segmentation::UnicodeSegmentation;

use super::buffer::{Buffer, grapheme_width};
use super::convert::saturating_u16;
use super::layout::Rect;
use super::style::{Color, Modifier, Style};

/// Horizontal alignment of a line within its area.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Alignment {
    /// Left-aligned.
    #[default]
    Left,
    /// Centered.
    Center,
    /// Right-aligned.
    Right,
}

/// Display width of a string, in columns.
fn str_width(s: &str) -> usize {
    s.graphemes(true).map(grapheme_width).sum()
}

// ----------------------------------------------------------------------------------- Span

/// A text segment with a single style.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Span<'a> {
    /// The span's text.
    pub content: Cow<'a, str>,
    /// Style applied to all content.
    pub style: Style,
}

impl<'a> Span<'a> {
    /// Unstyled span.
    pub fn raw<T: Into<Cow<'a, str>>>(content: T) -> Self {
        Self {
            content: content.into(),
            style: Style::new(),
        }
    }

    /// Span, line, or text with the given style.
    pub fn styled<T: Into<Cow<'a, str>>, S: Into<Style>>(content: T, style: S) -> Self {
        Self {
            content: content.into(),
            style: style.into(),
        }
    }

    /// Replace the span's style.
    #[must_use]
    pub fn style<S: Into<Style>>(mut self, style: S) -> Self {
        self.style = style.into();
        self
    }

    /// Display width in columns.
    #[must_use]
    pub fn width(&self) -> usize {
        str_width(&self.content)
    }
}

impl<'a> From<&'a str> for Span<'a> {
    fn from(s: &'a str) -> Self {
        Span::raw(s)
    }
}

impl From<String> for Span<'static> {
    fn from(s: String) -> Self {
        Span::raw(s)
    }
}

// ----------------------------------------------------------------------------------- Line

/// A line: spans, a shared style (applied to the entire line area, including
/// the background), and optional alignment.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Line<'a> {
    /// The line's spans, in order.
    pub spans: Vec<Span<'a>>,
    /// Style applied to all content.
    pub style: Style,
    /// `None`: use the containing `Text`'s alignment, otherwise align left.
    pub alignment: Option<Alignment>,
}

impl<'a> Line<'a> {
    /// Line containing a single unstyled span.
    pub fn raw<T: Into<Cow<'a, str>>>(content: T) -> Self {
        Self::from(Span::raw(content))
    }

    /// Span, line, or text with the given style.
    pub fn styled<T: Into<Cow<'a, str>>, S: Into<Style>>(content: T, style: S) -> Self {
        Self::from(Span::styled(content, style))
    }

    /// Replace the line's style.
    #[must_use]
    pub fn style<S: Into<Style>>(mut self, style: S) -> Self {
        self.style = style.into();
        self
    }

    /// Set the alignment.
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

    /// Append a span to the line.
    pub fn push_span<T: Into<Span<'a>>>(&mut self, span: T) {
        self.spans.push(span.into());
    }

    /// Total display width, in columns.
    pub fn width(&self) -> usize {
        self.spans.iter().map(Span::width).sum()
    }

    /// Draw the line on the **first row** of `area`.
    ///
    /// The line's style is applied to the entire row, then spans are written with
    /// their own styles on top. If the line is wider than the area, it is truncated
    /// on the right (regardless of alignment): a wide character that does not fit is
    /// omitted entirely, never split in half.
    pub fn render_to(&self, area: Rect, buf: &mut Buffer) {
        self.render_aligned(area, buf, None);
    }

    /// Like [`Line::render_to`], with `default` as the alignment when the line has none of
    /// its own (a `Block` title takes the block's title alignment this way, without
    /// cloning the line).
    pub fn render_to_with_default_alignment(
        &self,
        area: Rect,
        buf: &mut Buffer,
        default: Alignment,
    ) {
        self.render_aligned(area, buf, Some(default));
    }

    /// Like [`Line::render_to`], with a default alignment from the parent `Text`.
    fn render_aligned(&self, area: Rect, buf: &mut Buffer, default: Option<Alignment>) {
        let area = buf.area().intersection(area);
        if area.is_empty() {
            return;
        }
        buf.set_style(Rect::new(area.x, area.y, area.width, 1), self.style);

        let available = area.width as usize;
        let width = self.width();
        let offset = if width >= available {
            0
        } else {
            match self.alignment.or(default).unwrap_or_default() {
                Alignment::Left => 0,
                Alignment::Center => (available - width) / 2,
                Alignment::Right => available - width,
            }
        };
        let mut x = area.x + saturating_u16(offset);
        let mut remaining = available - offset;
        for span in &self.spans {
            let (next_x, _) = buf.set_stringn(x, area.y, &span.content, remaining, span.style);
            let written = (next_x - x) as usize;
            remaining -= written;
            x = next_x;
            // This span does not fit entirely: stop, otherwise a subsequent span
            // would be placed where the omitted character belongs.
            if written < span.width() {
                break;
            }
        }
    }
}

impl<'a> From<&'a str> for Line<'a> {
    fn from(s: &'a str) -> Self {
        Line::raw(s)
    }
}

impl From<String> for Line<'static> {
    fn from(s: String) -> Self {
        Line::raw(s)
    }
}

impl<'a> From<Span<'a>> for Line<'a> {
    fn from(span: Span<'a>) -> Self {
        Self {
            spans: vec![span],
            ..Self::default()
        }
    }
}

impl<'a> From<Vec<Span<'a>>> for Line<'a> {
    fn from(spans: Vec<Span<'a>>) -> Self {
        Self {
            spans,
            ..Self::default()
        }
    }
}

// ----------------------------------------------------------------------------------- Text

/// Multiple lines, with a default style and alignment for all of them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Text<'a> {
    /// The text's lines, in order.
    pub lines: Vec<Line<'a>>,
    /// Style applied to all content.
    pub style: Style,
    /// Alignment for lines without their own.
    pub alignment: Option<Alignment>,
}

impl<'a> Text<'a> {
    /// Split `content` at line feeds (`\n`, stripping a trailing `\r`). An empty string
    /// produces one empty line; `"a\n"` produces two lines (“a” and an empty line).
    pub fn raw<T: Into<Cow<'a, str>>>(content: T) -> Self {
        let lines = match content.into() {
            Cow::Borrowed(s) => s.split('\n').map(|l| Line::raw(trim_cr(l))).collect(),
            Cow::Owned(s) => s
                .split('\n')
                .map(|l| Line::raw(trim_cr(l).to_string()))
                .collect(),
        };
        Self {
            lines,
            ..Self::default()
        }
    }

    /// Span, line, or text with the given style.
    pub fn styled<T: Into<Cow<'a, str>>, S: Into<Style>>(content: T, style: S) -> Self {
        Self::raw(content).style(style)
    }

    /// Replace the text's style.
    #[must_use]
    pub fn style<S: Into<Style>>(mut self, style: S) -> Self {
        self.style = style.into();
        self
    }

    /// Set the alignment.
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

    /// Append a line to the text.
    pub fn push_line<T: Into<Line<'a>>>(&mut self, line: T) {
        self.lines.push(line.into());
    }

    /// Width of the widest line, in columns.
    pub fn width(&self) -> usize {
        self.lines.iter().map(Line::width).max().unwrap_or(0)
    }

    /// Number of lines.
    #[must_use]
    pub fn height(&self) -> usize {
        self.lines.len()
    }

    /// Draw one text line per row of `area`, truncating overflow
    /// (on the right and bottom).
    pub fn render_to(&self, area: Rect, buf: &mut Buffer) {
        let area = buf.area().intersection(area);
        if area.is_empty() {
            return;
        }
        buf.set_style(area, self.style);
        for (i, line) in (0..area.height).zip(&self.lines) {
            let row = Rect::new(area.x, area.y + i, area.width, 1);
            line.render_aligned(row, buf, self.alignment);
        }
    }
}

fn trim_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

impl<'a> From<&'a str> for Text<'a> {
    fn from(s: &'a str) -> Self {
        Text::raw(s)
    }
}

impl From<String> for Text<'static> {
    fn from(s: String) -> Self {
        Text::raw(s)
    }
}

impl<'a> From<Line<'a>> for Text<'a> {
    fn from(line: Line<'a>) -> Self {
        Self {
            lines: vec![line],
            ..Self::default()
        }
    }
}

impl<'a> From<Vec<Line<'a>>> for Text<'a> {
    fn from(lines: Vec<Line<'a>>) -> Self {
        Self {
            lines,
            ..Self::default()
        }
    }
}

// -------------------------------------------------------------------------------- Stylize

/// Style shortcuts: `"text".bold()`, `line.fg(Color::Cyan)`, `span.red().italic()`…
///
/// Implemented for `&str` and `String` (producing a [`Span`]), [`Span`], [`Line`], and [`Text`].
/// On a `Line` or `Text`, the style patches the shared style (see `patch`); it does
/// not replace the spans' styles.
pub trait Stylize: Sized {
    /// Type returned by style methods (the same type, styled).
    type Output;

    /// Overlay `style` on the current style.
    fn patch_style(self, style: Style) -> Self::Output;

    /// Set the foreground color.
    fn fg(self, color: Color) -> Self::Output {
        self.patch_style(Style::new().fg(color))
    }

    /// Set the background color.
    fn bg(self, color: Color) -> Self::Output {
        self.patch_style(Style::new().bg(color))
    }

    /// Enable modifiers.
    fn add_modifier(self, modifier: Modifier) -> Self::Output {
        self.patch_style(Style::new().add_modifier(modifier))
    }

    /// Disable modifiers.
    fn remove_modifier(self, modifier: Modifier) -> Self::Output {
        self.patch_style(Style::new().remove_modifier(modifier))
    }

    /// Make bold.
    fn bold(self) -> Self::Output {
        self.add_modifier(Modifier::BOLD)
    }

    /// Dim.
    fn dim(self) -> Self::Output {
        self.add_modifier(Modifier::DIM)
    }

    /// Make italic.
    fn italic(self) -> Self::Output {
        self.add_modifier(Modifier::ITALIC)
    }

    /// Underline.
    fn underlined(self) -> Self::Output {
        self.add_modifier(Modifier::UNDERLINED)
    }

    /// Reverse colors.
    fn reversed(self) -> Self::Output {
        self.add_modifier(Modifier::REVERSED)
    }

    /// Cross out.
    fn crossed_out(self) -> Self::Output {
        self.add_modifier(Modifier::CROSSED_OUT)
    }

    /// Black text.
    fn black(self) -> Self::Output {
        self.fg(Color::Black)
    }

    /// Red text.
    fn red(self) -> Self::Output {
        self.fg(Color::Red)
    }

    /// Green text.
    fn green(self) -> Self::Output {
        self.fg(Color::Green)
    }

    /// Yellow text.
    fn yellow(self) -> Self::Output {
        self.fg(Color::Yellow)
    }

    /// Blue text.
    fn blue(self) -> Self::Output {
        self.fg(Color::Blue)
    }

    /// Magenta text.
    fn magenta(self) -> Self::Output {
        self.fg(Color::Magenta)
    }

    /// Cyan text.
    fn cyan(self) -> Self::Output {
        self.fg(Color::Cyan)
    }

    /// Light gray text.
    fn gray(self) -> Self::Output {
        self.fg(Color::Gray)
    }

    /// Dark gray text.
    fn dark_gray(self) -> Self::Output {
        self.fg(Color::DarkGray)
    }

    /// White text.
    fn white(self) -> Self::Output {
        self.fg(Color::White)
    }
}

impl<'a> Stylize for Span<'a> {
    type Output = Span<'a>;
    fn patch_style(mut self, style: Style) -> Self::Output {
        self.style = self.style.patch(style);
        self
    }
}

impl<'a> Stylize for Line<'a> {
    type Output = Line<'a>;
    fn patch_style(mut self, style: Style) -> Self::Output {
        self.style = self.style.patch(style);
        self
    }
}

impl<'a> Stylize for Text<'a> {
    type Output = Text<'a>;
    fn patch_style(mut self, style: Style) -> Self::Output {
        self.style = self.style.patch(style);
        self
    }
}

impl<'a> Stylize for &'a str {
    type Output = Span<'a>;
    fn patch_style(self, style: Style) -> Self::Output {
        Span::styled(self, style)
    }
}

impl Stylize for String {
    type Output = Span<'static>;
    fn patch_style(self, style: Style) -> Self::Output {
        Span::styled(self, style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_line(line: &Line, width: u16) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, 1));
        line.render_to(buf.area(), &mut buf);
        buf
    }

    #[test]
    fn widths_are_display_columns() {
        assert_eq!(Span::raw("été").width(), 3);
        assert_eq!(Span::raw("日本").width(), 4);
        assert_eq!(Span::raw("a😀").width(), 3);
        assert_eq!(Span::raw("👍🏽🇫🇷").width(), 4);
        assert_eq!(Span::raw("e\u{301}").width(), 1);
        assert_eq!(
            Span::raw("a\tb").width(),
            2,
            "control character: not displayed"
        );
        let line = Line::from(vec![Span::raw("ab"), Span::raw("日")]);
        assert_eq!(line.width(), 4);
    }

    #[test]
    fn multi_segment_line_keeps_each_style() {
        let line = Line::from(vec![
            Span::raw("ab"),
            Span::styled("cd", Color::Red),
            "ef".bold(),
        ]);
        let buf = render_line(&line, 8);
        assert_eq!(buf, {
            let mut expected = Buffer::with_lines(["abcdef  "]);
            expected.set_style(Rect::new(2, 0, 2, 1), Style::new().fg(Color::Red));
            expected.set_style(
                Rect::new(4, 0, 2, 1),
                Style::new().add_modifier(Modifier::BOLD),
            );
            expected
        });
    }

    #[test]
    fn line_style_covers_the_whole_row_and_spans_patch_over_it() {
        let line = Line::from(vec![Span::raw("ab"), "cd".fg(Color::Red)]).bg(Color::Blue);
        let buf = render_line(&line, 6);
        assert!(
            buf.content().iter().all(|c| c.bg == Color::Blue),
            "background on the whole row"
        );
        assert_eq!(buf.get(0, 0).fg, Color::Reset);
        assert_eq!(buf.get(2, 0).fg, Color::Red);
        assert_eq!(
            buf.get(2, 0).bg,
            Color::Blue,
            "the span style completes the line style"
        );
    }

    #[test]
    fn alignment_left_center_right() {
        let base = Line::from("abc");
        assert_eq!(render_line(&base, 9), Buffer::with_lines(["abc      "]));
        assert_eq!(
            render_line(&base.clone().centered(), 9),
            Buffer::with_lines(["   abc   "])
        );
        assert_eq!(
            render_line(&base.clone().right_aligned(), 9),
            Buffer::with_lines(["      abc"])
        );
        // Odd remainder: the extra column goes on the right.
        assert_eq!(
            render_line(&base.centered(), 6),
            Buffer::with_lines([" abc  "])
        );
    }

    #[test]
    fn alignment_uses_display_width_not_char_count() {
        let line = Line::from("日本").right_aligned();
        assert_eq!(render_line(&line, 6), Buffer::with_lines(["  日本"]));
    }

    #[test]
    fn long_line_is_truncated_on_the_right_whatever_the_alignment() {
        for alignment in [Alignment::Left, Alignment::Center, Alignment::Right] {
            let line = Line::from("abcdef").alignment(alignment);
            assert_eq!(render_line(&line, 4), Buffer::with_lines(["abcd"]));
        }
        // Exact width: nothing is lost.
        assert_eq!(
            render_line(&Line::from("abcd"), 4),
            Buffer::with_lines(["abcd"])
        );
    }

    #[test]
    fn truncation_never_splits_a_wide_character() {
        // 3 columns: “a” + “日” (2) fit; “本” is omitted entirely.
        let line = Line::from("a日本");
        assert_eq!(render_line(&line, 3), Buffer::with_lines(["a日"]));
        // 2 columns: “日” does not fit after “a”.
        assert_eq!(render_line(&line, 2), Buffer::with_lines(["a "]));
    }

    #[test]
    fn truncation_stops_at_the_first_segment_that_does_not_fit() {
        // The first span's “日” does not fit; the following “x” must not take its place.
        let line = Line::from(vec![Span::raw("a日"), Span::raw("x")]);
        assert_eq!(render_line(&line, 2), Buffer::with_lines(["a "]));
        // A span can be truncated in the middle; subsequent spans are discarded.
        let line = Line::from(vec![Span::raw("abc"), Span::raw("def")]);
        assert_eq!(render_line(&line, 5), Buffer::with_lines(["abcde"]));
    }

    #[test]
    fn rendering_is_clipped_to_the_buffer_and_ignores_empty_areas() {
        let line = Line::from("abcdef");
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        line.render_to(Rect::new(2, 0, 10, 5), &mut buf);
        assert_eq!(buf, Buffer::with_lines(["  ab"]));
        line.render_to(Rect::new(0, 0, 0, 1), &mut buf);
        line.render_to(Rect::new(0, 3, 4, 1), &mut buf);
        assert_eq!(buf, Buffer::with_lines(["  ab"]));
    }

    #[test]
    fn only_the_first_row_of_the_area_is_used_by_a_line() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 2));
        Line::from("abc").render_to(buf.area(), &mut buf);
        assert_eq!(buf, Buffer::with_lines(["abc", "   "]));
    }

    #[test]
    fn text_splits_lines_and_renders_one_per_row() {
        let text = Text::from("un\ndeux\r\ntrois");
        assert_eq!(text.height(), 3);
        assert_eq!(text.width(), 5);
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 3));
        text.render_to(buf.area(), &mut buf);
        assert_eq!(buf, Buffer::with_lines(["un   ", "deux ", "trois"]));
        // Area too short: the last lines are lost.
        let mut small = Buffer::empty(Rect::new(0, 0, 5, 2));
        text.render_to(small.area(), &mut small);
        assert_eq!(small, Buffer::with_lines(["un   ", "deux "]));
    }

    #[test]
    fn text_raw_edge_cases() {
        assert_eq!(Text::raw("").height(), 1);
        assert_eq!(Text::raw("a\n").height(), 2);
        assert_eq!(Text::from(String::from("x\ny")).height(), 2);
    }

    #[test]
    fn text_alignment_is_the_default_for_lines_without_their_own() {
        let mut text =
            Text::from(vec![Line::from("a"), Line::from("b").right_aligned()]).centered();
        text.push_line("cc");
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 3));
        text.render_to(buf.area(), &mut buf);
        assert_eq!(buf, Buffer::with_lines(["  a  ", "    b", " cc  "]));
    }

    #[test]
    fn text_style_covers_the_whole_area() {
        let text = Text::from("a").bg(Color::Green);
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 2));
        text.render_to(buf.area(), &mut buf);
        assert!(buf.content().iter().all(|c| c.bg == Color::Green));
    }

    #[test]
    fn stylize_on_str_string_span_line_and_text() {
        assert_eq!("x".bold().style, Style::new().add_modifier(Modifier::BOLD));
        assert_eq!(String::from("x").cyan().style.fg, Some(Color::Cyan));
        let span = Span::raw("x").red().italic().bg(Color::Blue);
        assert_eq!(span.style.fg, Some(Color::Red));
        assert_eq!(span.style.bg, Some(Color::Blue));
        assert!(span.style.add_modifier.contains(Modifier::ITALIC));
        assert!(
            Line::from("x")
                .dim()
                .style
                .add_modifier
                .contains(Modifier::DIM)
        );
        assert_eq!(Text::from("x").dark_gray().style.fg, Some(Color::DarkGray));
        // A second shortcut overrides the previous one for the same field.
        assert_eq!("x".red().green().style.fg, Some(Color::Green));
        assert!(
            !"x".bold()
                .remove_modifier(Modifier::BOLD)
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn conversions() {
        assert_eq!(Line::from("a").spans, vec![Span::raw("a")]);
        assert_eq!(Line::from(String::from("a")), Line::raw("a"));
        assert_eq!(Line::from(Span::raw("a")).width(), 1);
        assert_eq!(Text::from(Line::from("a")).lines.len(), 1);
        let owned: Span<'static> = Span::from(String::from("é"));
        assert_eq!(owned.width(), 1);
    }
}
