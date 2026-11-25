//! Formatting of the components: sanitizing of the log, truncation, numbers of `P` and `V`,
//! cells of `I`. Pure functions, tested without a buffer.

use std::fmt::{self, Write};

use tilefab_core::grapheme_width;
use unicode_segmentation::UnicodeSegmentation;

/// The character appended to a truncated text.
pub(crate) const ELLIPSIS: &str = "…";

/// Copy `text` into `out` (cleared first, its memory kept) with every control character
/// replaced by one space, and return the width of the result.
pub(crate) fn sanitize_into(text: &str, out: &mut String) -> usize {
    out.clear();
    out.extend(text.chars().map(|c| if c.is_control() { ' ' } else { c }));
    crate::entry::text_width(out)
}

/// How a text is drawn in a given width (see [`truncate`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Truncated<'a> {
    /// The part of the text drawn, a prefix on grapheme boundaries.
    pub(crate) prefix: &'a str,
    /// Whether `…` follows the prefix.
    pub(crate) ellipsis: bool,
    /// Columns left after the prefix (and `…`) to fill with spaces.
    pub(crate) pad: usize,
}

/// Fit `text`, `text_width` columns wide, in `width` columns:
///
/// 1. when it fits, it is kept whole and padded;
/// 2. otherwise, the longest prefix at most `width − 1` columns wide is kept, followed by `…`;
/// 3. when a wide character does not fit before `…`, its free column becomes a space
///    (`pad` is then 1).
///
/// With `width` 0 nothing is drawn.
pub(crate) fn truncate(text: &str, text_width: usize, width: usize) -> Truncated<'_> {
    if text_width <= width {
        return Truncated {
            prefix: text,
            ellipsis: false,
            pad: width - text_width,
        };
    }
    if width == 0 {
        return Truncated {
            prefix: "",
            ellipsis: false,
            pad: 0,
        };
    }
    let (prefix, prefix_width) = prefix_within(text, width - 1);
    Truncated {
        prefix,
        ellipsis: true,
        pad: width - 1 - prefix_width,
    }
}

/// The longest prefix of `text`, cut on grapheme boundaries, at most `max` columns wide, and
/// its width. Stops at the first grapheme that does not fit, as `Buffer::set_stringn` does.
fn prefix_within(text: &str, max: usize) -> (&str, usize) {
    let mut width = 0;
    for (offset, grapheme) in text.grapheme_indices(true) {
        let next = width + grapheme_width(grapheme);
        if next > max {
            return (&text[..offset], width);
        }
        width = next;
    }
    (text, width)
}

/// `value × 10^decimals` rounded down, for a `value` in 0–100.
///
/// A product within a few ULPs of an integer counts as that integer: `0.29 × 100` is
/// `28.999999999999996` in floating point, and must give `29`, not `28`. Only a value of 100
/// or more gives `100 × 10^decimals`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the result is a non-negative integer of at most 10^11 (100 × 10^9)"
)]
fn floor_scaled(value: f64, decimals: u32) -> u64 {
    let scale = 10_f64.powi(i32::try_from(decimals).unwrap_or(0));
    let scaled = value * scale;
    let nearest = scaled.round();
    let floored = if (nearest - scaled).abs() <= scaled.abs() * 4.0 * f64::EPSILON {
        nearest
    } else {
        scaled.floor()
    };
    let full = 100.0 * scale;
    let floored = if value < 100.0 && floored >= full {
        full - 1.0
    } else {
        floored
    };
    floored.max(0.0) as u64
}

/// Write the text of `P` with size `size` (3–9): the progress `value` (already in 0–100)
/// rounded down to `size − 4` decimals (0 for `P3` and `P4`), right-justified in `size`
/// columns, then `%`. Always `size + 1` columns.
pub(crate) fn write_progress(out: &mut impl Write, value: f64, size: u8) -> fmt::Result {
    let decimals = u32::from(size.saturating_sub(4));
    let scaled = floor_scaled(value, decimals);
    let unit = 10_u64.pow(decimals);
    let (int, frac) = (scaled / unit, scaled % unit);
    let size = usize::from(size);
    if decimals == 0 {
        write!(out, "{int:>size$}%")
    } else {
        let decimals = decimals as usize;
        let int_size = size - decimals - 1;
        write!(out, "{int:>int_size$}.{frac:0decimals$}%")
    }
}

/// Write a number of `V` in at most `size` columns: with `decimals` decimals; when it does
/// not fit, with fewer decimals; when it does not fit with none, its first `size − 1`
/// columns and `…`. Return the width written.
pub(crate) fn write_number(out: &mut String, value: f64, size: usize, decimals: u8) -> usize {
    // No `-0.0`.
    let value = if value == 0.0 { 0.0 } else { value };
    for decimals in (0..=usize::from(decimals)).rev() {
        let mut count = Count(0);
        // Writing to a counter cannot fail.
        let _ = write!(count, "{value:.decimals$}");
        if count.0 <= size {
            let _ = write!(out, "{value:.decimals$}");
            return count.0;
        }
    }
    // Too long even without decimals: only ASCII digits and `-`, one column each.
    let mut prefix = Prefix {
        out,
        left: size.saturating_sub(1),
    };
    let _ = write!(prefix, "{value:.0}");
    out.push_str(ELLIPSIS);
    size.max(1)
}

/// Number of *done* cells of `I` in `width` columns for a progress `value` in 0–100:
/// `floor(value / 100 × width)`, all of them only at 100.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a width of at most 65535 columns, and a result in 0..width"
)]
pub(crate) fn done_cells(value: f64, width: usize) -> usize {
    if value >= 100.0 {
        return width;
    }
    let done = (value * width as f64 / 100.0).floor().max(0.0) as usize;
    done.min(width.saturating_sub(1))
}

/// A writer that only counts the bytes written.
struct Count(usize);

impl Write for Count {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0 += s.len();
        Ok(())
    }
}

/// A writer that keeps the first `left` bytes and drops the rest.
struct Prefix<'a> {
    out: &'a mut String,
    left: usize,
}

impl Write for Prefix<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // Only used for ASCII text: any byte is a character boundary.
        let kept = &s[..s.len().min(self.left)];
        self.out.push_str(kept);
        self.left -= kept.len();
        Ok(())
    }
}

/// A short ASCII text formatted on the stack, so that drawing allocates nothing: the date,
/// the time and the progress number. Text past the capacity is dropped.
pub(crate) struct StackText<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> StackText<N> {
    /// An empty text.
    pub(crate) fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    /// The text written so far.
    pub(crate) fn as_str(&self) -> &str {
        // Only whole `&str`s are copied, cut on a character boundary.
        std::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> Write for StackText<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut end = s.len().min(N - self.len);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        self.bytes[self.len..self.len + end].copy_from_slice(&s.as_bytes()[..end]);
        self.len += end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::text_width;
    use proptest::prelude::*;

    /// The text drawn by [`truncate`], padding included.
    fn fit(text: &str, width: usize) -> String {
        let t = truncate(text, text_width(text), width);
        let mut out = t.prefix.to_owned();
        if t.ellipsis {
            out.push_str(ELLIPSIS);
        }
        out.extend(std::iter::repeat_n(' ', t.pad));
        out
    }

    fn progress(value: f64, size: u8) -> String {
        let mut out = String::new();
        write_progress(&mut out, value, size).unwrap();
        out
    }

    fn number(value: f64, size: usize, decimals: u8) -> (String, usize) {
        let mut out = String::new();
        let width = write_number(&mut out, value, size, decimals);
        (out, width)
    }

    #[test]
    fn sanitize_replaces_each_control_character_by_one_space() {
        let mut out = String::from("old text");
        assert_eq!(sanitize_into("a\tb\r\nc\u{1b}[0m", &mut out), 10);
        assert_eq!(out, "a b  c [0m");
        assert_eq!(sanitize_into("界", &mut out), 2);
        assert_eq!(out, "界");
    }

    #[test]
    fn truncation_rules() {
        assert_eq!(fit("Downloading", 11), "Downloading");
        assert_eq!(fit("Downloading", 14), "Downloading   ");
        assert_eq!(fit("Downloading", 10), "Downloadi…");
        assert_eq!(fit("Downloading", 10).chars().count(), 10);
        assert_eq!(fit("Downloading", 2), "D…");
        assert_eq!(fit("Downloading", 1), "…");
        assert_eq!(fit("Downloading", 0), "");
        assert_eq!(fit("", 0), "");
    }

    #[test]
    fn a_wide_character_that_does_not_fit_before_the_ellipsis() {
        // "a界b": the cut at 3 columns keeps "a", then `…`, then a space where "界" ended.
        assert_eq!(fit("a界b", 3), "a… ");
        assert_eq!(text_width(&fit("a界b", 3)), 3);
        assert_eq!(fit("a界b", 4), "a界b");
        assert_eq!(fit("a界bc", 4), "a界…");
        assert_eq!(fit("界界界", 2), "… ");
        assert_eq!(fit("界界界", 4), "界… ");
    }

    #[test]
    fn progress_examples() {
        let cases = [
            (0.0, 3, "  0%"),
            (0.0, 4, "   0%"),
            (5.7, 3, "  5%"),
            (99.9, 3, " 99%"),
            (100.0, 3, "100%"),
            (99.96, 5, " 99.9%"),
            (99.95, 5, " 99.9%"),
            (99.99, 5, " 99.9%"),
            (99.99, 6, " 99.99%"),
            (100.0, 5, "100.0%"),
            (100.0, 6, "100.00%"),
            (100.0, 9, "100.00000%"),
            (0.29, 6, "  0.29%"),
            (0.3, 5, "  0.3%"),
            (12.5, 4, "  12%"),
            (99.999_999_999_9, 9, " 99.99999%"),
        ];
        for (value, size, expected) in cases {
            let text = progress(value, size);
            assert_eq!(text, expected, "{value} in P{size}");
            assert_eq!(text.len(), usize::from(size) + 1);
        }
    }

    #[test]
    fn just_below_a_step_stays_below() {
        assert_eq!(progress(49.999_999, 5), " 49.9%");
        assert_eq!(progress(99.999_999_999_999_9, 3), " 99%");
        assert_eq!(progress(f64::from(1) - 1e-12, 4), "   0%");
    }

    #[test]
    fn numbers_of_v() {
        assert_eq!(number(99.3, 5, 1), ("99.3".into(), 4));
        assert_eq!(number(99.3, 2, 1), ("99".into(), 2));
        assert_eq!(number(109.6, 4, 2), ("110".into(), 3));
        assert_eq!(number(1234.6, 4, 1), ("1235".into(), 4));
        assert_eq!(number(12345.0, 4, 1), ("123…".into(), 4));
        assert_eq!(number(0.0, 2, 1), ("0".into(), 1));
        assert_eq!(number(0.0, 3, 1), ("0.0".into(), 3));
        assert_eq!(number(-0.0, 3, 1), ("0.0".into(), 3));
        assert_eq!(number(3.25, 9, 3), ("3.250".into(), 5));
        assert_eq!(number(7.0, 1, 0), ("7".into(), 1));
        assert_eq!(number(12.0, 1, 0), ("…".into(), 1));
        assert_eq!(number(12.0, 2, 0), ("12".into(), 2));
        assert_eq!(number(123.0, 2, 0), ("1…".into(), 2));
        assert_eq!(number(1e300, 5, 1), ("1000…".into(), 5));
    }

    #[test]
    fn negative_numbers_count_the_minus_sign() {
        assert_eq!(number(-5.5, 4, 1), ("-5.5".into(), 4));
        assert_eq!(number(-5.5, 3, 1), ("-6".into(), 2));
        assert_eq!(number(-55.0, 2, 1), ("-…".into(), 2));
    }

    #[test]
    fn illustration_cells() {
        assert_eq!(done_cells(0.0, 10), 0);
        assert_eq!(done_cells(9.99, 10), 0);
        assert_eq!(done_cells(10.0, 10), 1);
        assert_eq!(done_cells(55.0, 10), 5);
        assert_eq!(done_cells(99.99, 10), 9);
        assert_eq!(done_cells(99.999_999_999_999_99, 999), 998);
        assert_eq!(done_cells(100.0, 10), 10);
        assert_eq!(done_cells(50.0, 0), 0);
        assert_eq!(done_cells(100.0, 2), 2);
    }

    #[test]
    fn stack_text_keeps_what_fits() {
        let mut text = StackText::<4>::new();
        write!(text, "ab").unwrap();
        write!(text, "c界").unwrap();
        assert_eq!(text.as_str(), "abc");
    }

    proptest! {
        #[test]
        fn truncation_fills_exactly_the_width(text in "\\PC{0,12}", width in 0usize..16) {
            let full = text_width(&text);
            let t = truncate(&text, full, width);
            let total = text_width(t.prefix) + usize::from(t.ellipsis) + t.pad;
            prop_assert_eq!(total, width);
            prop_assert_eq!(t.ellipsis, full > width && width > 0);
            if t.ellipsis {
                prop_assert!(t.pad <= 1);
            }
        }

        #[test]
        fn progress_is_always_size_plus_one_and_never_above_the_value(
            value in 0.0f64..=100.0,
            size in 3u8..=9,
        ) {
            let text = progress(value, size);
            prop_assert_eq!(text.len(), usize::from(size) + 1);
            let shown: f64 = text.trim().trim_end_matches('%').parse().unwrap();
            prop_assert!(shown <= value + 1e-9);
            if value < 100.0 {
                prop_assert!(shown < 100.0);
            }
        }

        #[test]
        fn numbers_never_exceed_their_size(value in any::<f64>().prop_filter("finite", |v| v.is_finite()), size in 1usize..12, decimals in 0u8..=9) {
            let (text, width) = number(value, size, decimals);
            prop_assert!(width <= size);
            prop_assert_eq!(text_width(&text), width);
        }
    }
}
