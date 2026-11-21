//! `Gauge` (a multi-row bar with a centered label) and `LineGauge` (a single-row bar).
//!
//! In both cases, the ratio is an `f64` between 0 and 1, clamped to that range (an
//! out-of-range value or `NaN` never causes a panic). Bar symbols must occupy
//! **one column**.

use super::Block;
use tilefab_core::buffer::Buffer;
use tilefab_core::convert::saturating_u16;
use tilefab_core::layout::Rect;
use tilefab_core::style::Style;
use tilefab_core::text::{Line, Span};
use tilefab_core::widget::Widget;

fn clamp_ratio(ratio: f64) -> f64 {
    if ratio.is_nan() {
        0.0
    } else {
        ratio.clamp(0.0, 1.0)
    }
}

/// Default label: the rounded percentage.
/// Number of cells out of `width` filled at `ratio`, rounded by `round`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the ratio is clamped to 0..=1, so the rounded value lies in 0..=width"
)]
fn scale(width: u16, ratio: f64, round: fn(f64) -> f64) -> u16 {
    round(f64::from(width) * ratio) as u16
}

/// Percentage label of `ratio`, such as `50%`.
fn percent_label(ratio: f64) -> String {
    format!("{:.0}%", ratio * 100.0)
}

/// A progress bar filling its entire area, with a centered label.
///
/// ```
/// use tilefab_core::{Buffer, Rect, Widget};
/// use tilefab_widget::Gauge;
///
/// let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
/// Gauge::default().ratio(0.5).label("50").filled_symbol("#").unfilled_symbol(".")
///     .render(buf.area(), &mut buf);
/// assert_eq!(buf.lines(), ["####50...."]);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Gauge<'a> {
    block: Option<Block<'a>>,
    ratio: f64,
    label: Option<Span<'a>>,
    style: Style,
    filled_style: Style,
    unfilled_style: Style,
    filled_symbol: &'a str,
    unfilled_symbol: &'a str,
}

impl Default for Gauge<'_> {
    fn default() -> Self {
        Self {
            block: None,
            ratio: 0.0,
            label: None,
            style: Style::new(),
            filled_style: Style::new(),
            unfilled_style: Style::new(),
            filled_symbol: "█",
            unfilled_symbol: " ",
        }
    }
}

impl<'a> Gauge<'a> {
    /// Surround the gauge with a block.
    #[must_use]
    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    /// Ratio between 0 and 1 (clamped to this range).
    #[must_use]
    pub fn ratio(mut self, ratio: f64) -> Self {
        self.ratio = clamp_ratio(ratio);
        self
    }

    /// Ratio as a percentage (capped at 100).
    #[must_use]
    pub fn percent(self, percent: u16) -> Self {
        self.ratio(f64::from(percent.min(100)) / 100.0)
    }

    /// Label centered on the middle row. Default: the percentage. Its style overlays
    /// the bar's style: it must remain readable over the filled portion.
    #[must_use]
    pub fn label<T: Into<Span<'a>>>(mut self, label: T) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Style for the entire area.
    #[must_use]
    pub fn style<S: Into<Style>>(mut self, style: S) -> Self {
        self.style = style.into();
        self
    }

    /// Style for the filled portion.
    #[must_use]
    pub fn filled_style<S: Into<Style>>(mut self, style: S) -> Self {
        self.filled_style = style.into();
        self
    }

    /// Style for the empty portion.
    #[must_use]
    pub fn unfilled_style<S: Into<Style>>(mut self, style: S) -> Self {
        self.unfilled_style = style.into();
        self
    }

    /// Symbol for the filled portion.
    #[must_use]
    pub fn filled_symbol(mut self, symbol: &'a str) -> Self {
        self.filled_symbol = symbol;
        self
    }

    /// Symbol for the empty portion.
    #[must_use]
    pub fn unfilled_symbol(mut self, symbol: &'a str) -> Self {
        self.unfilled_symbol = symbol;
        self
    }
}

impl Widget for Gauge<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = buf.area().intersection(area);
        if area.is_empty() {
            return;
        }
        buf.set_style(area, self.style);
        let inner = match &self.block {
            Some(block) => {
                block.render_ref(area, buf);
                block.inner(area)
            }
            None => area,
        };
        if inner.is_empty() {
            return;
        }

        // Filled columns: round to the nearest integer.
        let filled = scale(inner.width, self.ratio, f64::round);
        for y in inner.top()..inner.bottom() {
            for dx in 0..inner.width {
                let (symbol, style) = if dx < filled {
                    (self.filled_symbol, self.filled_style)
                } else {
                    (self.unfilled_symbol, self.unfilled_style)
                };
                buf.set_string(inner.x + dx, y, symbol, style);
            }
        }

        // Label: centered, truncated to the area width.
        let label = self
            .label
            .unwrap_or_else(|| Span::raw(percent_label(self.ratio)));
        let width = inner.width as usize;
        let offset = width.saturating_sub(label.width()) / 2;
        // The limit `width - offset` only matters when `offset` is 0: a centered label fits.
        // So `width + offset` would be equivalent (`cargo mutants` sees it survive).
        let row = inner.y + inner.height / 2;
        buf.set_stringn(
            inner.x + saturating_u16(offset),
            row,
            &label.content,
            width - offset,
            label.style,
        );
    }
}

/// A single-row bar: label, one space, then a bar filling the remainder.
///
/// The label is always followed by an empty column, even when empty: `label("")` produces
/// a bar starting in the second column.
///
/// ```
/// use tilefab_core::{Buffer, Rect, Widget};
/// use tilefab_widget::LineGauge;
///
/// let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
/// LineGauge::default().ratio(0.5).label("ab").filled_symbol("#").unfilled_symbol(".")
///     .render(buf.area(), &mut buf);
/// assert_eq!(buf.lines(), ["ab ###...."]);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct LineGauge<'a> {
    block: Option<Block<'a>>,
    ratio: f64,
    label: Option<Line<'a>>,
    style: Style,
    filled_style: Style,
    unfilled_style: Style,
    filled_symbol: &'a str,
    unfilled_symbol: &'a str,
}

impl Default for LineGauge<'_> {
    fn default() -> Self {
        Self {
            block: None,
            ratio: 0.0,
            label: None,
            style: Style::new(),
            filled_style: Style::new(),
            unfilled_style: Style::new(),
            filled_symbol: "─",
            unfilled_symbol: "─",
        }
    }
}

impl<'a> LineGauge<'a> {
    /// Surround the gauge with a block.
    #[must_use]
    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    /// Ratio between 0 and 1 (clamped to this range).
    #[must_use]
    pub fn ratio(mut self, ratio: f64) -> Self {
        self.ratio = clamp_ratio(ratio);
        self
    }

    /// Ratio as a percentage (capped at 100).
    #[must_use]
    pub fn percent(self, percent: u16) -> Self {
        self.ratio(f64::from(percent.min(100)) / 100.0)
    }

    /// Label to the left of the bar. Default: the percentage.
    #[must_use]
    pub fn label<T: Into<Line<'a>>>(mut self, label: T) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Style for the entire area.
    #[must_use]
    pub fn style<S: Into<Style>>(mut self, style: S) -> Self {
        self.style = style.into();
        self
    }

    /// Style for the filled portion.
    #[must_use]
    pub fn filled_style<S: Into<Style>>(mut self, style: S) -> Self {
        self.filled_style = style.into();
        self
    }

    /// Style for the empty portion.
    #[must_use]
    pub fn unfilled_style<S: Into<Style>>(mut self, style: S) -> Self {
        self.unfilled_style = style.into();
        self
    }

    /// Symbol for the filled portion.
    #[must_use]
    pub fn filled_symbol(mut self, symbol: &'a str) -> Self {
        self.filled_symbol = symbol;
        self
    }

    /// Symbol for the empty portion.
    #[must_use]
    pub fn unfilled_symbol(mut self, symbol: &'a str) -> Self {
        self.unfilled_symbol = symbol;
        self
    }
}

impl Widget for LineGauge<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = buf.area().intersection(area);
        if area.is_empty() {
            return;
        }
        buf.set_style(area, self.style);
        let inner = match &self.block {
            Some(block) => {
                block.render_ref(area, buf);
                block.inner(area)
            }
            None => area,
        };
        if inner.is_empty() {
            return;
        }

        let label = self
            .label
            .unwrap_or_else(|| Line::from(percent_label(self.ratio)))
            .left_aligned();
        let label_width = saturating_u16(label.width()).min(inner.width);
        label.render_to(Rect::new(inner.x, inner.y, inner.width, 1), buf);

        // The bar starts one column after the label and fills the rest of the row.
        let start = inner.x.saturating_add(label_width).saturating_add(1);
        if start >= inner.right() {
            return;
        }
        let bar_width = inner.right() - start;
        // Filled columns: round down (like ratatui); 100% fills everything.
        let filled = scale(bar_width, self.ratio, f64::floor);
        for dx in 0..bar_width {
            let (symbol, style) = if dx < filled {
                (self.filled_symbol, self.filled_style)
            } else {
                (self.unfilled_symbol, self.unfilled_style)
            };
            buf.set_string(start + dx, inner.y, symbol, style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tilefab_core::style::Color;
    use tilefab_core::text::Stylize;

    fn render(widget: impl Widget, width: u16, height: u16) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        widget.render(buf.area(), &mut buf);
        buf
    }

    fn gauge(ratio: f64) -> Gauge<'static> {
        Gauge::default()
            .ratio(ratio)
            .filled_symbol("#")
            .unfilled_symbol(".")
    }

    fn line_gauge(ratio: f64) -> LineGauge<'static> {
        LineGauge::default()
            .ratio(ratio)
            .label("")
            .filled_symbol("#")
            .unfilled_symbol(".")
    }

    // --------------------------------------------------------------------- Gauge

    #[test]
    fn gauge_fills_proportionally_with_a_centered_label() {
        assert_eq!(render(gauge(0.0), 10, 1).lines(), ["....0%...."]);
        // 5 filled columns; the “50%” label (3 columns) overwrites columns 3 to 5.
        assert_eq!(render(gauge(0.5), 10, 1).lines(), ["###50%...."]);
        assert_eq!(render(gauge(1.0), 10, 1).lines(), ["###100%###"]);
    }

    #[test]
    fn gauge_custom_label_and_multiple_rows() {
        let buf = render(gauge(0.5).label("ab"), 6, 3);
        // The label is centered on the middle row (columns 2 and 3).
        assert_eq!(buf.lines(), ["###...", "##ab..", "###..."]);
    }

    #[test]
    fn gauge_ratio_is_clamped() {
        assert_eq!(render(gauge(-3.0).label(""), 4, 1).lines(), ["...."]);
        assert_eq!(render(gauge(7.0).label(""), 4, 1).lines(), ["####"]);
        assert_eq!(render(gauge(f64::NAN).label(""), 4, 1).lines(), ["...."]);
        assert_eq!(
            render(
                Gauge::default().percent(250).filled_symbol("#").label(""),
                4,
                1
            )
            .lines(),
            ["####"]
        );
    }

    #[test]
    fn gauge_percent_is_a_fraction_of_100() {
        let half = Gauge::default()
            .percent(50)
            .filled_symbol("#")
            .unfilled_symbol(".")
            .label("");
        assert_eq!(render(half, 4, 1).lines(), ["##.."]);
    }

    #[test]
    fn gauge_styles_apply_to_each_part() {
        let buf = render(
            gauge(0.5)
                .label("")
                .style(Style::new().bg(Color::Black))
                .filled_style(Color::Cyan)
                .unfilled_style(Color::DarkGray),
            4,
            1,
        );
        assert_eq!(buf.get(0, 0).fg, Color::Cyan);
        assert_eq!(buf.get(1, 0).fg, Color::Cyan);
        assert_eq!(buf.get(2, 0).fg, Color::DarkGray);
        assert!(buf.content().iter().all(|c| c.bg == Color::Black));
    }

    #[test]
    fn gauge_label_style_is_patched_over_the_bar() {
        let buf = render(gauge(1.0).label("x".red()).filled_style(Color::Cyan), 3, 1);
        let c = buf.get(1, 0);
        assert_eq!((c.symbol(), c.fg), ("x", Color::Red));
        assert_eq!(buf.get(0, 0).fg, Color::Cyan);
    }

    #[test]
    fn gauge_long_label_is_truncated() {
        assert_eq!(render(gauge(0.0).label("abcdef"), 4, 1).lines(), ["abcd"]);
    }

    #[test]
    fn gauge_with_block() {
        let buf = render(gauge(1.0).label("").block(Block::bordered()), 6, 3);
        assert_eq!(buf.lines(), ["┌────┐", "│####│", "└────┘"]);
    }

    #[test]
    fn gauge_tiny_areas() {
        for w in 0..4 {
            for h in 0..4 {
                let _ = render(gauge(0.5).block(Block::bordered()), w, h);
                let _ = render(gauge(0.5), w, h);
            }
        }
        assert_eq!(render(gauge(1.0).label(""), 1, 1).lines(), ["#"]);
    }

    // ----------------------------------------------------------------- LineGauge

    #[test]
    fn line_gauge_label_then_a_space_then_the_bar() {
        let g = LineGauge::default()
            .ratio(0.5)
            .label("ab")
            .filled_symbol("#")
            .unfilled_symbol(".");
        assert_eq!(render(g, 10, 1).lines(), ["ab ###...."]);
        // Empty label: the bar still starts in the second column.
        assert_eq!(render(line_gauge(0.5), 9, 1).lines(), [" ####...."]);
    }

    #[test]
    fn line_gauge_default_label_is_the_percentage() {
        let g = LineGauge::default()
            .ratio(0.25)
            .filled_symbol("#")
            .unfilled_symbol(".");
        assert_eq!(render(g, 12, 1).lines(), ["25% ##......"]);
    }

    #[test]
    fn line_gauge_rounds_down_but_100_percent_is_full() {
        // 7 bar columns at 50%: 3.5 → 3 filled.
        assert_eq!(render(line_gauge(0.5), 8, 1).lines(), [" ###...."]);
        assert_eq!(render(line_gauge(1.0), 8, 1).lines(), [" #######"]);
        assert_eq!(render(line_gauge(0.99), 8, 1).lines(), [" ######."]);
        assert_eq!(render(line_gauge(0.0), 8, 1).lines(), [" ......."]);
    }

    #[test]
    fn line_gauge_ratio_is_clamped() {
        assert_eq!(render(line_gauge(-1.0), 5, 1).lines(), [" ...."]);
        assert_eq!(render(line_gauge(2.0), 5, 1).lines(), [" ####"]);
        assert_eq!(render(line_gauge(f64::NAN), 5, 1).lines(), [" ...."]);
        assert_eq!(
            render(
                LineGauge::default()
                    .percent(999)
                    .label("")
                    .filled_symbol("#"),
                5,
                1
            )
            .lines(),
            [" ####"]
        );
    }

    #[test]
    fn line_gauge_percent_is_a_fraction_of_100() {
        let half = LineGauge::default()
            .percent(50)
            .label("")
            .filled_symbol("#")
            .unfilled_symbol(".");
        assert_eq!(render(half, 5, 1).lines(), [" ##.."]);
    }

    #[test]
    fn line_gauge_style_covers_the_whole_area() {
        let buf = render(line_gauge(0.5).style(Style::new().bg(Color::Black)), 5, 2);
        assert!(buf.content().iter().all(|c| c.bg == Color::Black));
    }

    #[test]
    fn line_gauge_styles_label_and_bar() {
        let g = LineGauge::default()
            .ratio(0.5)
            .label("ab".bold())
            .filled_symbol("#")
            .unfilled_symbol(".")
            .filled_style(Color::Cyan)
            .unfilled_style(Color::DarkGray);
        let buf = render(g, 9, 1);
        assert!(
            buf.get(0, 0)
                .modifier
                .contains(tilefab_core::style::Modifier::BOLD)
        );
        assert_eq!(buf.get(3, 0).fg, Color::Cyan);
        assert_eq!(buf.get(8, 0).fg, Color::DarkGray);
        assert_eq!(buf.get(2, 0).symbol(), " ", "separator column");
    }

    #[test]
    fn line_gauge_label_fills_all_the_width_so_no_bar() {
        assert_eq!(
            render(line_gauge(1.0).label("abcdef"), 5, 1).lines(),
            ["abcde"]
        );
        assert_eq!(
            render(line_gauge(1.0).label("abcd"), 5, 1).lines(),
            ["abcd "]
        );
    }

    #[test]
    fn line_gauge_is_drawn_on_the_first_row_with_block() {
        let buf = render(line_gauge(1.0).block(Block::bordered()), 7, 3);
        assert_eq!(buf.lines(), ["┌─────┐", "│ ####│", "└─────┘"]);
        let buf = render(line_gauge(1.0), 4, 2);
        assert_eq!(buf.lines(), [" ###", "    "]);
    }

    #[test]
    fn line_gauge_tiny_areas() {
        for w in 0..4 {
            for h in 0..4 {
                let _ = render(line_gauge(0.5).block(Block::bordered()), w, h);
                let _ = render(LineGauge::default().ratio(0.5), w, h);
            }
        }
        assert_eq!(render(line_gauge(1.0), 1, 1).lines(), [" "]);
    }
}
