//! Viewport (fullscreen or inline) and frame commit geometry.
//!
//! An **inline** viewport occupies only a band of lines on the normal screen, above which
//! log lines can be inserted and pushed into terminal scrollback.
//!
//! # The frame commit
//!
//! `Terminal::insert_before` and `Terminal::set_height` write nothing: they accumulate lines
//! to insert (`n`) and a target height (`h'`). `Terminal::draw` applies everything at once, in
//! a single synchronized update block, along with the viewport redraw. The terminal therefore
//! never displays an intermediate state.
//!
//! # Placement rule
//!
//! Treat the screen as a downward flow. The viewport occupies lines
//! `top .. top + h`. After the commit:
//!
//! 1. the `n` log lines are written starting at `top` (replacing the first
//!    lines of the old viewport);
//! 2. the new viewport, with `h'` lines, sits just below: it starts at `top + n`;
//! 3. if its bottom exceeds the screen (`top + n + h' > H`), **scroll** the area above
//!    the old viewport by the excess: logs move up by that amount, and the viewport bottom
//!    stays at the bottom of the screen. Otherwise, nothing scrolls;
//! 4. if the new viewport is shorter than the old one *and* logs have not filled all the
//!    freed space, clear the remaining lines below the viewport one by one.
//!
//! The usual cases: at the screen bottom, shrinking by 1 with 1 log line
//! does not scroll (the log fills the freed space); keeping the height with 2 lines scrolls by 2;
//! growing by 1 with 1 line scrolls by 2. Away from the bottom, nothing scrolls: the viewport moves down.

/// Where the terminal draws.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Viewport {
    /// The entire screen (usually the alternate screen).
    #[default]
    Fullscreen,
    /// A band of `n` lines at the cursor position, on the normal screen. `n` is capped at
    /// the terminal height and is at least 1.
    Inline(u16),
}

/// How to scroll the area above an inline viewport.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ScrollMode {
    /// Scroll region (DECSTBM): only lines above the viewport move. The
    /// viewport is untouched; only changed lines are rewritten.
    #[default]
    Region,
    /// Fallback without regions, for terminals that handle them poorly: clear the viewport,
    /// print the lines (the terminal scrolls automatically at the bottom), then redraw the entire
    /// viewport. Simpler, but the entire viewport is rewritten on each commit.
    Fallback,
}

/// Options for creating a `Terminal`.
///
/// New fields may come in minor versions, so it cannot be written as a struct literal outside
/// this crate: start from `TerminalOptions::default()` and chain the setters below (or assign the
/// public fields of a mutable value).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct TerminalOptions {
    /// Fullscreen or inline band.
    pub viewport: Viewport,
    /// How to scroll the area above an inline viewport.
    pub scroll_mode: ScrollMode,
}

/// Setters, to chain from [`TerminalOptions::default`].
///
/// ```
/// use tilefab_term::{ScrollMode, TerminalOptions, Viewport};
///
/// let options = TerminalOptions::default().viewport(Viewport::Inline(3)).scroll_mode(ScrollMode::Fallback);
/// assert_eq!(options.viewport, Viewport::Inline(3));
/// ```
impl TerminalOptions {
    /// Set [`viewport`](Self::viewport).
    #[must_use]
    pub const fn viewport(mut self, viewport: Viewport) -> Self {
        self.viewport = viewport;
        self
    }

    /// Set [`scroll_mode`](Self::scroll_mode).
    #[must_use]
    pub const fn scroll_mode(mut self, scroll_mode: ScrollMode) -> Self {
        self.scroll_mode = scroll_mode;
        self
    }
}

/// The result of a commit's placement calculation (see the module documentation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitPlan {
    /// Number of lines to scroll the area above the old viewport (0 = none).
    pub scroll: u16,
    /// First screen row of the new viewport.
    pub top: u16,
    /// Screen rows to clear below the new viewport (empty if none).
    pub erase: std::ops::Range<u16>,
}

/// Calculate the placement of a commit.
///
/// - `screen_height`: screen height;
/// - `top`, `height`: current viewport area;
/// - `lines`: number of log lines to insert;
/// - `new_height`: desired height.
///
/// Requires `lines + new_height <= screen_height` (otherwise the caller splits the batch) and
/// `top + height <= screen_height`. Returns `None` when these preconditions do not hold
/// (for instance on a screen with no rows): there is no consistent placement, and the
/// caller must not commit anything.
pub(crate) fn plan_commit(
    screen_height: u16,
    top: u16,
    height: u16,
    lines: u16,
    new_height: u16,
) -> Option<CommitPlan> {
    if u32::from(lines) + u32::from(new_height) > u32::from(screen_height)
        || u32::from(top) + u32::from(height) > u32::from(screen_height)
    {
        return None;
    }

    // The viewport starts below the inserted lines and may extend past the screen bottom:
    // the excess is the required scroll amount. Summed in `u32`: `top + lines + new_height`
    // can exceed `u16::MAX`, and a saturated sum would scroll one line too few.
    let end = u32::from(top) + u32::from(lines) + u32::from(new_height);
    let scroll_wide = end.saturating_sub(u32::from(screen_height));
    // With the preconditions, `scroll <= top` and `new_top <= screen_height - new_height`:
    // both fit in a `u16` (the fallbacks are never used).
    let scroll = u16::try_from(scroll_wide).unwrap_or(u16::MAX);
    let new_top =
        u16::try_from(u32::from(top) + u32::from(lines) - scroll_wide).unwrap_or(u16::MAX);

    let old_bottom = top + height;
    let new_bottom = new_top + new_height;
    // If the viewport shrinks more than the logs fill, stale lines remain below it.
    let erase = if new_bottom < old_bottom {
        new_bottom..old_bottom
    } else {
        0..0
    };
    Some(CommitPlan {
        scroll,
        top: new_top,
        erase,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // 10-line screen, 4-line viewport anchored at the bottom (rows 6 to 9).
    const H: u16 = 10;

    #[test]
    fn shrink_by_one_with_one_line_at_the_bottom_does_not_scroll() {
        let p = plan_commit(H, 6, 4, 1, 3).unwrap();
        assert_eq!(
            p,
            CommitPlan {
                scroll: 0,
                top: 7,
                erase: 0..0
            }
        );
    }

    #[test]
    fn shrink_by_two_with_one_line_erases_the_freed_row_below() {
        let p = plan_commit(H, 6, 4, 1, 2).unwrap();
        assert_eq!(p.scroll, 0);
        assert_eq!(p.top, 7);
        assert_eq!(p.erase, 9..10);
    }

    #[test]
    fn same_height_with_two_lines_scrolls_by_two() {
        let p = plan_commit(H, 6, 4, 2, 4).unwrap();
        assert_eq!((p.scroll, p.top, p.erase), (2, 6, 0..0));
    }

    #[test]
    fn grow_by_one_with_one_line_at_the_bottom_scrolls_by_two() {
        let p = plan_commit(H, 6, 4, 1, 5).unwrap();
        assert_eq!((p.scroll, p.top, p.erase), (2, 5, 0..0));
    }

    #[test]
    fn grow_alone_at_the_bottom_scrolls_by_the_growth() {
        let p = plan_commit(H, 6, 4, 0, 5).unwrap();
        assert_eq!((p.scroll, p.top), (1, 5));
    }

    #[test]
    fn grow_with_room_below_does_not_scroll() {
        let p = plan_commit(H, 2, 3, 0, 5).unwrap();
        assert_eq!((p.scroll, p.top, p.erase), (0, 2, 0..0));
    }

    #[test]
    fn lines_with_room_below_push_the_viewport_down_without_scrolling() {
        let p = plan_commit(H, 2, 3, 2, 3).unwrap();
        assert_eq!((p.scroll, p.top, p.erase), (0, 4, 0..0));
    }

    #[test]
    fn shrink_alone_with_room_below_keeps_the_top_and_erases_below() {
        let p = plan_commit(H, 2, 5, 0, 3).unwrap();
        assert_eq!((p.scroll, p.top, p.erase), (0, 2, 5..7));
    }

    #[test]
    fn nothing_to_do_keeps_everything() {
        let p = plan_commit(H, 6, 4, 0, 4).unwrap();
        assert_eq!((p.scroll, p.top, p.erase), (0, 6, 0..0));
    }

    #[test]
    fn scroll_never_exceeds_the_zone_above() {
        // For every valid combination, scrolling never exceeds the area above the viewport.
        for top in 0..=H {
            for height in 1..=(H - top).min(5) {
                for lines in 0..=H {
                    for new_height in 1..=H {
                        if lines + new_height > H {
                            continue;
                        }
                        let p = plan_commit(H, top, height, lines, new_height).unwrap();
                        assert!(p.scroll <= top, "{top} {height} {lines} {new_height}");
                        assert!(p.top + new_height <= H);
                        assert!(p.top >= lines.saturating_sub(p.scroll));
                    }
                }
            }
        }
    }

    #[test]
    fn arguments_outside_the_preconditions_give_no_plan() {
        // A screen with no rows: even a one-row viewport does not fit.
        assert_eq!(plan_commit(0, 0, 1, 0, 1), None);
        // Too many lines for the viewport, or an old viewport past the screen bottom.
        assert_eq!(plan_commit(H, 6, 4, H, 1), None);
        assert_eq!(plan_commit(H, 8, 4, 0, 4), None);
        // Sums beyond `u16::MAX` are not saturated into a false "fits".
        assert_eq!(plan_commit(u16::MAX, 0, 1, u16::MAX, 1), None);
        assert_eq!(plan_commit(u16::MAX, u16::MAX, 1, 0, 1), None);
    }

    #[test]
    fn huge_screen_sum_beyond_u16_scrolls_exactly_enough() {
        // Found by proptest: `top + lines + new_height` = 65 536. A saturated `u16` sum
        // scrolled one line too few and pushed the viewport past the screen bottom.
        let p = plan_commit(63_728, 1_964, 1, 13_884, 49_688).unwrap();
        assert_eq!(p.scroll, 1_808);
        assert_eq!(p.top, 63_728 - 49_688);
    }

    /// Valid `plan_commit` arguments over the whole `u16` range (the callers'
    /// preconditions): `(screen_height, top, height, lines, new_height)`.
    fn commit_args() -> impl proptest::strategy::Strategy<Value = (u16, u16, u16, u16, u16)> {
        use proptest::prelude::*;
        (1u16..=u16::MAX)
            .prop_flat_map(|screen| (Just(screen), 0..screen, 1..=screen))
            .prop_flat_map(|(screen, top, new_height)| {
                (
                    Just(screen),
                    Just(top),
                    1..=screen - top,
                    0..=screen - new_height,
                    Just(new_height),
                )
            })
    }

    proptest::proptest! {
        #[test]
        fn plan_scrolls_only_as_needed_and_keeps_everything_on_screen(
            (screen, top, height, lines, new_height) in commit_args()
        ) {
            let p = plan_commit(screen, top, height, lines, new_height).unwrap();
            let (screen, top, height, lines, new_height) = (
                u32::from(screen), u32::from(top), u32::from(height),
                u32::from(lines), u32::from(new_height),
            );
            let (scroll, new_top) = (u32::from(p.scroll), u32::from(p.top));
            // Never more scrolling than the zone above the old viewport holds…
            proptest::prop_assert!(scroll <= top);
            // …and none at all when the lines and the viewport fit below `top`.
            let fits = top + lines + new_height <= screen;
            proptest::prop_assert_eq!(scroll == 0, fits);
            // The new viewport is on screen, right below the inserted lines; after a scroll,
            // it is anchored at the bottom.
            proptest::prop_assert!(new_top + new_height <= screen);
            proptest::prop_assert_eq!(new_top, top + lines - scroll);
            if scroll > 0 {
                proptest::prop_assert_eq!(new_top + new_height, screen);
            }
            // Erased rows: exactly the stale rows between the new and the old bottom.
            let (new_bottom, old_bottom) = (new_top + new_height, top + height);
            if new_bottom < old_bottom {
                proptest::prop_assert_eq!(
                    (u32::from(p.erase.start), u32::from(p.erase.end)),
                    (new_bottom, old_bottom)
                );
            } else {
                proptest::prop_assert!(p.erase.is_empty());
            }
        }
    }
}
