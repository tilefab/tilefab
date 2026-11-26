//! Inline viewport frame commit tests on a simulated terminal (`sim.rs`).
//!
//! Each test drives a real `Terminal` with a `Vec<u8>` backend, then replays the emitted bytes
//! in the simulator: check what the user *would see* and what was written to achieve it
//! (scrolling, rewritten lines, synchronized blocks).

use std::collections::BTreeSet;

use super::sim::{BlockStats, Sim};
use super::{Backend, ScrollMode, Terminal, TerminalOptions, Viewport};
use tilefab_core::convert::saturating_u16;
use tilefab_core::{Position, Rect, Size, Style};

const W: u16 = 30;
const H: u16 = 10;

/// An inline terminal and its simulator.
struct Rig {
    term: Terminal<Vec<u8>>,
    sim: Sim,
    /// Bytes emitted during the last `pump` (the last commit, after `commit`).
    last: Vec<u8>,
}

impl Rig {
    /// An `H`-row screen prefilled with `prefill` text lines (`avant0`, `avant1`…), then
    /// an inline viewport of `height` rows created at the cursor position.
    ///
    /// `ScrollMode::Region` assumes xterm's scrollback (a region whose top is row 0 saves its
    /// lines): it runs on [`Sim::xterm`]; the other modes on the strict simulator.
    fn new(prefill: usize, height: u16, mode: ScrollMode) -> Self {
        let mut sim = if mode == ScrollMode::Region {
            Sim::xterm(W, H)
        } else {
            Sim::new(W, H)
        };
        for i in 0..prefill {
            sim.feed_str(&format!("avant{i}\r\n"));
        }
        // Prefilling simulates the shell: it does not count as application output.
        sim.outside_sync.clear();
        let (x, y) = sim.cursor;
        let backend = Backend::with_fixed(Vec::new(), Size::new(W, H), Position::new(x, y));
        let term = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(height),
                scroll_mode: mode,
            },
        )
        .unwrap();
        let mut rig = Self {
            term,
            sim,
            last: Vec::new(),
        };
        rig.pump();
        rig
    }

    /// Send bytes emitted since the last call to the simulator.
    fn pump(&mut self) {
        let bytes = std::mem::take(self.term.backend_mut().writer_mut());
        self.sim.feed(&bytes);
        self.last = bytes;
    }

    /// A full commit: height = `view.len()`, `logs` inserted above, `view` drawn
    /// (the last line is the prompt, where the cursor is placed). Return statistics for the
    /// resulting synchronized block: there must be **exactly one**.
    fn commit(&mut self, logs: &[&str], view: &[&str]) -> BlockStats {
        let before = self.sim.blocks.len();
        self.term.set_height(saturating_u16(view.len()));
        for log in logs {
            self.term.insert_before(1, |buf| {
                buf.set_string(0, 0, log, Style::new());
            });
        }
        self.term
            .draw(|frame| {
                let area = frame.area();
                for (i, text) in view.iter().enumerate() {
                    frame.buffer_mut().set_string(
                        area.x,
                        area.y + saturating_u16(i),
                        text,
                        Style::new(),
                    );
                }
                frame.set_cursor_position((2, area.bottom() - 1));
            })
            .unwrap();
        self.pump();
        assert_eq!(
            self.sim.blocks.len(),
            before + 1,
            "a commit must fit in a single synchronized block"
        );
        assert!(
            self.sim.outside_sync.is_empty(),
            "bytes emitted outside a synchronized block: {:?}",
            String::from_utf8_lossy(&self.sim.outside_sync)
        );
        assert!(
            self.sim.out_of_range.is_empty(),
            "sequences beyond the screen: {:?}",
            self.sim.out_of_range
        );
        self.sim.blocks.last().unwrap().clone()
    }

    fn screen(&self) -> Vec<String> {
        self.sim.screen_lines()
    }
}

fn set(rows: &[u16]) -> BTreeSet<u16> {
    rows.iter().copied().collect()
}

/// The viewport displays `view` in the last screen rows.
fn assert_bottom(rig: &Rig, view: &[&str]) {
    let screen = rig.screen();
    let n = view.len();
    assert_eq!(&screen[H as usize - n..], view, "screen: {screen:#?}");
}

// --- initialization -------------------------------------------------------------------------

#[test]
fn init_at_the_top_reserves_lines_without_scrolling() {
    let rig = Rig::new(0, 3, ScrollMode::Region);
    assert_eq!(rig.term.area(), Rect::new(0, 0, W, 3));
    assert_eq!(rig.sim.scrollback, Vec::<String>::new());
}

#[test]
fn init_clears_exactly_the_reserved_rows() {
    // A screen full of old text, the cursor moved to `row`: only the `height` reserved
    // rows are cleared, the text above and below is kept.
    for row in [0u16, 2] {
        let mut sim = Sim::new(W, H);
        let old: Vec<String> = (0..H).map(|i| format!("old{i}")).collect();
        sim.feed_str(&old.join("\r\n"));
        sim.feed_str(&format!("\x1b[{};1H", row + 1));
        sim.outside_sync.clear();
        let backend = Backend::with_fixed(Vec::new(), Size::new(W, H), Position::new(0, row));
        let mut term = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(3),
                scroll_mode: ScrollMode::Region,
            },
        )
        .unwrap();
        sim.feed(&std::mem::take(term.backend_mut().writer_mut()));
        let expected: Vec<String> = (0..H)
            .map(|i| {
                if (row..row + 3).contains(&i) {
                    String::new()
                } else {
                    format!("old{i}")
                }
            })
            .collect();
        assert_eq!(sim.screen_lines(), expected, "cursor on row {row}");
    }
}

#[test]
fn init_at_the_bottom_scrolls_to_make_room() {
    // 10 text lines: the cursor is on the last screen row.
    let rig = Rig::new(12, 3, ScrollMode::Region);
    assert_eq!(rig.term.area(), Rect::new(0, 7, W, 3));
    // The preceding text is intact, only shifted upward.
    assert_eq!(rig.screen()[6], "avant11");
}

#[test]
fn init_after_a_prompt_starts_on_the_next_line() {
    let mut sim = Sim::new(W, H);
    sim.feed_str("avant0\r\n$ ls");
    let (x, y) = sim.cursor;
    let backend = Backend::with_fixed(Vec::new(), Size::new(W, H), Position::new(x, y));
    let term = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(2),
            scroll_mode: ScrollMode::Region,
        },
    )
    .unwrap();
    assert_eq!(term.area(), Rect::new(0, 2, W, 2));
}

// --- cases of the placement rule (`viewport.rs`) ---------------------------------------------

/// Screen full of logs, 4-row viewport (3 tasks + prompt) anchored at the bottom: rows 6 to 9.
fn at_bottom(mode: ScrollMode) -> Rig {
    let mut rig = Rig::new(15, 4, mode);
    rig.commit(&[], &["alpha 50%", "beta 20%", "gamma 5%", ">"]);
    assert_bottom(&rig, &["alpha 50%", "beta 20%", "gamma 5%", ">"]);
    rig
}

#[test]
fn shrink_by_one_with_one_line_does_not_scroll_and_keeps_the_prompt() {
    let mut rig = at_bottom(ScrollMode::Region);
    let stats = rig.commit(&["[INFO] alpha finished"], &["beta 20%", "gamma 5%", ">"]);
    assert_eq!(stats.scrolled, 0, "no scrolling expected");
    assert!(!stats.erased_screen);
    // Only the freed line is written: the log line replaces “alpha”.
    assert_eq!(stats.rows_written, set(&[6]));
    assert!(stats.rows_erased.is_empty());
    let screen = rig.screen();
    assert_eq!(screen[6], "[INFO] alpha finished");
    assert_bottom(&rig, &["beta 20%", "gamma 5%", ">"]);
    // No empty line between the last log and the viewport; the previous log is above it.
    assert_eq!(screen[5], "avant14");
    assert_eq!((rig.sim.cursor, rig.sim.cursor_visible), ((2, 9), true));
}

#[test]
fn shrink_by_two_with_one_line_erases_the_freed_row_below_the_viewport() {
    let mut rig = at_bottom(ScrollMode::Region);
    let stats = rig.commit(&["[INFO] fin"], &["gamma 5%", ">"]);
    assert_eq!(stats.scrolled, 0);
    // Case `d > n`: the viewport moves down one row; the last screen row is cleared.
    assert_eq!(stats.rows_erased, set(&[9]));
    let screen = rig.screen();
    assert_eq!(&screen[6..], ["[INFO] fin", "gamma 5%", ">", ""]);
    assert!(!stats.erased_screen);
}

#[test]
fn same_height_with_two_lines_scrolls_by_two_and_leaves_the_viewport_alone() {
    let mut rig = at_bottom(ScrollMode::Region);
    let stats = rig.commit(
        &["log one", "log two"],
        &["alpha 50%", "beta 20%", "gamma 5%", ">"],
    );
    assert_eq!(stats.scrolled, 2);
    assert_eq!(stats.rows_written, set(&[4, 5]), "only the two log lines");
    let screen = rig.screen();
    assert_eq!(&screen[4..6], ["log one", "log two"]);
    assert_bottom(&rig, &["alpha 50%", "beta 20%", "gamma 5%", ">"]);
    assert_eq!(screen[3], "avant14");
}

#[test]
fn grow_by_one_with_one_line_at_the_bottom_scrolls_by_two() {
    let mut rig = at_bottom(ScrollMode::Region);
    let stats = rig.commit(
        &["[INFO] delta started"],
        &["alpha 50%", "beta 20%", "gamma 5%", "delta 0%", ">"],
    );
    assert_eq!(stats.scrolled, 2);
    let screen = rig.screen();
    assert_eq!(screen[4], "[INFO] delta started");
    assert_bottom(
        &rig,
        &["alpha 50%", "beta 20%", "gamma 5%", "delta 0%", ">"],
    );
    assert_eq!(screen[3], "avant14");
    // The prompt stays on the same screen row.
    assert!(!stats.rows_written.contains(&9));
}

#[test]
fn grow_alone_at_the_bottom_scrolls_by_the_growth() {
    let mut rig = at_bottom(ScrollMode::Region);
    let stats = rig.commit(&[], &["a", "b", "c", "d", ">"]);
    assert_eq!(stats.scrolled, 1);
    let screen = rig.screen();
    assert_eq!(screen[5], "a");
    assert_eq!(screen[4], "avant14");
}

#[test]
fn grow_with_room_below_draws_downwards_without_scrolling() {
    let mut rig = Rig::new(2, 2, ScrollMode::Region);
    rig.commit(&[], &["a", ">"]);
    let stats = rig.commit(&[], &["a", "b", "c", ">"]);
    assert_eq!(stats.scrolled, 0);
    assert_eq!(&rig.screen()[..6], ["avant0", "avant1", "a", "b", "c", ">"]);
    // The viewport top has not moved, so “a” is not rewritten.
    assert!(!stats.rows_written.contains(&2));
}

#[test]
fn lines_with_room_below_push_the_viewport_down() {
    let mut rig = Rig::new(2, 2, ScrollMode::Region);
    rig.commit(&[], &["a", ">"]);
    let stats = rig.commit(&["log"], &["a", ">"]);
    assert_eq!(stats.scrolled, 0);
    assert_eq!(&rig.screen()[..5], ["avant0", "avant1", "log", "a", ">"]);
}

#[test]
fn nothing_changed_emits_only_the_sync_frame() {
    let mut rig = at_bottom(ScrollMode::Region);
    let stats = rig.commit(&[], &["alpha 50%", "beta 20%", "gamma 5%", ">"]);
    assert_eq!(stats, BlockStats::default());
}

#[test]
fn an_unchanged_frame_writes_nothing_but_the_sync_markers() {
    // Viewport at the bottom (row 6) and at the top (row 0, too high for a scroll region),
    // in both modes: no scrolling, so nothing to clear or rewrite, and no cursor move.
    let view = ["alpha 50%", "beta 20%", "gamma 5%", ">"];
    for mode in [ScrollMode::Region, ScrollMode::Fallback] {
        for prefill in [15, 0] {
            let mut rig = Rig::new(prefill, 4, mode);
            rig.commit(&[], &view);
            rig.commit(&[], &view);
            assert_eq!(
                String::from_utf8_lossy(&rig.last),
                "\x1b[?2026h\x1b[?2026l",
                "mode {mode:?}, prefill {prefill}"
            );
        }
    }
}

/// Whether `bytes` set a scroll region (`CSI top ; bottom r`, DECSTBM).
fn sets_scroll_region(bytes: &[u8]) -> bool {
    bytes.windows(2).enumerate().any(|(i, w)| {
        w == b"\x1b[" && {
            let rest = &bytes[i + 2..];
            let digits = rest
                .iter()
                .take_while(|b| b.is_ascii_digit() || **b == b';')
                .count();
            digits > 0 && rest.get(digits) == Some(&b'r')
        }
    })
}

#[test]
fn fallback_mode_never_sets_a_scroll_region() {
    // Viewport at row 7: high enough for a region, which Region mode does use.
    for (mode, expected) in [(ScrollMode::Region, true), (ScrollMode::Fallback, false)] {
        let mut rig = Rig::new(15, 3, mode);
        let stats = rig.commit(&["log"], &["a", "b", ">"]);
        assert!(stats.scrolled > 0, "the commit must scroll");
        assert_eq!(sets_scroll_region(&rig.last), expected, "mode {mode:?}");
    }
}

#[test]
fn cursor_is_put_back_after_lines_applied_in_chunks() {
    // The prompt stays at the same place, so only "the writes moved the cursor"
    // can trigger its repositioning: it must hold across several chunks.
    let view = ["alpha 50%", "beta 20%", "gamma 5%", ">"];
    for mode in [ScrollMode::Region, ScrollMode::Fallback] {
        let mut rig = at_bottom(mode);
        let logs: Vec<String> = (0..25).map(|i| format!("log {i:02}")).collect();
        let refs: Vec<&str> = logs.iter().map(String::as_str).collect();
        rig.commit(&refs, &view);
        assert_bottom(&rig, &view);
        assert_eq!(rig.sim.cursor, (2, H - 1), "mode {mode:?}");
    }
}

// --- sequences -------------------------------------------------------------------------------

/// Height cycle 1 → 5 → 1 with a log on each commit. Check at each step: a single block,
/// no screen clearing, no blank line above the viewport, and ultimately
/// every log exactly once, in order.
fn run_cycle(mode: ScrollMode, prefill: usize) {
    let mut rig = Rig::new(prefill, 1, mode);
    let heights: Vec<usize> = (1..=5).chain((1..=4).rev()).collect();
    let mut next_log = 0;
    for &h in &heights {
        let view: Vec<String> = (0..h)
            .map(|i| {
                if i == h - 1 {
                    ">".into()
                } else {
                    format!("t{i} {h}")
                }
            })
            .collect();
        let view_refs: Vec<&str> = view.iter().map(String::as_str).collect();
        let log = format!("log {next_log:02}");
        next_log += 1;
        let stats = rig.commit(&[&log], &view_refs);
        assert!(!stats.erased_screen, "ESC[J or ESC[2J emitted (height {h})");

        let screen = rig.screen();
        let top = rig.term.area().y as usize;
        assert_eq!(rig.term.area().height as usize, h);
        assert_eq!(
            &screen[top..top + h],
            &view_refs[..],
            "viewport (height {h})"
        );
        // Immediately above the viewport: the last log, never a blank line.
        assert_eq!(screen[top - 1], log, "line above the viewport (height {h})");
    }
    // Every log exactly once, in order.
    let logs: Vec<String> = rig
        .sim
        .all_lines()
        .into_iter()
        .filter(|l| l.starts_with("log "))
        .collect();
    let expected: Vec<String> = (0..next_log).map(|i| format!("log {i:02}")).collect();
    assert_eq!(logs, expected);
}

#[test]
fn height_cycle_with_logs_at_the_bottom_region_mode() {
    run_cycle(ScrollMode::Region, 20);
}

#[test]
fn height_cycle_with_logs_at_the_bottom_fallback_mode() {
    run_cycle(ScrollMode::Fallback, 20);
}

#[test]
fn height_cycle_from_the_top_of_the_screen_region_mode() {
    run_cycle(ScrollMode::Region, 0);
}

#[test]
fn height_cycle_from_the_top_of_the_screen_fallback_mode() {
    run_cycle(ScrollMode::Fallback, 0);
}

#[test]
fn height_cycle_with_logs_at_the_bottom_screen_mode() {
    run_cycle(ScrollMode::Screen, 20);
}

#[test]
fn height_cycle_from_the_top_of_the_screen_screen_mode() {
    run_cycle(ScrollMode::Screen, 0);
}

#[test]
fn screen_mode_keeps_every_line_of_a_commit_in_the_strict_scrollback() {
    let mut rig = at_bottom(ScrollMode::Screen);
    let stats = rig.commit(
        &["log one", "log two"],
        &["alpha 50%", "beta 20%", "gamma 5%", ">"],
    );
    // Two line feeds at the bottom, no scroll region, no erase.
    assert_eq!(stats.scrolled, 2);
    assert!(!sets_scroll_region(&rig.last));
    assert!(!stats.erased_screen && stats.rows_erased.is_empty());
    let screen = rig.screen();
    assert_eq!(&screen[4..6], ["log one", "log two"]);
    assert_bottom(&rig, &["alpha 50%", "beta 20%", "gamma 5%", ">"]);
    // The two rows that left the screen are in the scrollback, the viewport is not.
    assert_eq!(
        rig.sim.scrollback[rig.sim.scrollback.len() - 2..],
        ["avant9", "avant10"]
    );
    assert!(rig.sim.scrollback.iter().all(|l| !l.contains('%')));
}

#[test]
fn many_lines_at_once_are_applied_in_chunks_without_loss() {
    for mode in [ScrollMode::Region, ScrollMode::Fallback, ScrollMode::Screen] {
        let mut rig = Rig::new(15, 3, mode);
        let logs: Vec<String> = (0..25).map(|i| format!("log {i:02}")).collect();
        let refs: Vec<&str> = logs.iter().map(String::as_str).collect();
        let stats = rig.commit(&refs, &["a", "b", ">"]);
        assert!(!stats.erased_screen);
        let got: Vec<String> = rig
            .sim
            .all_lines()
            .into_iter()
            .filter(|l| l.starts_with("log "))
            .collect();
        assert_eq!(got, logs, "mode {mode:?}");
        assert_bottom(&rig, &["a", "b", ">"]);
        assert_eq!(rig.screen()[6], "log 24");
    }
}

#[test]
fn fallback_mode_gives_the_same_screen_as_region_mode() {
    let steps: [(&[&str], &[&str]); 4] = [
        (&["un"], &["a", "b", ">"]),
        (&["deux", "trois"], &["a", ">"]),
        (&[], &["a", "b", "c", "d", ">"]),
        (&["quatre"], &[">"]),
    ];
    let mut screens = Vec::new();
    for mode in [ScrollMode::Region, ScrollMode::Fallback, ScrollMode::Screen] {
        let mut rig = Rig::new(14, 2, mode);
        for (logs, view) in steps {
            rig.commit(logs, view);
        }
        screens.push(rig.sim.all_lines());
    }
    assert_eq!(screens[0], screens[1]);
    assert_eq!(screens[0], screens[2]);
}

#[test]
fn a_viewport_as_tall_as_the_screen_ignores_inserted_lines() {
    let mut rig = Rig::new(0, 1, ScrollMode::Region);
    let view: Vec<&str> = vec!["x"; H as usize];
    rig.commit(&["perdu"], &view);
    assert!(!rig.sim.all_lines().iter().any(|l| l == "perdu"));
    assert_eq!(rig.term.area().height, H);
}

#[test]
fn height_is_clamped_to_the_screen_and_to_at_least_one() {
    let mut rig = Rig::new(0, 2, ScrollMode::Region);
    rig.term.set_height(0);
    rig.term.draw(|_| {}).unwrap();
    assert_eq!(rig.term.area().height, 1);
    rig.term.set_height(99);
    rig.term.draw(|_| {}).unwrap();
    assert_eq!(rig.term.area().height, H);
}

#[test]
fn prompt_row_never_moves_during_a_cycle_at_the_bottom() {
    let mut rig = Rig::new(20, 1, ScrollMode::Region);
    rig.commit(&[], &[">"]);
    let mut views: Vec<Vec<&str>> = Vec::new();
    for h in [2usize, 3, 4, 5, 4, 3, 2, 1] {
        let mut v = vec!["task"; h - 1];
        v.push(">");
        views.push(v);
    }
    for (i, view) in views.iter().enumerate() {
        let log = format!("log {i}");
        rig.commit(&[&log], view);
        assert_eq!(rig.sim.cursor, (2, H - 1), "the prompt moved at step {i}");
        assert_eq!(rig.screen()[H as usize - 1], ">");
    }
}

// --- resizing and cleanup ---------------------------------------------------------------------

#[test]
fn resize_redraws_the_whole_viewport_in_one_sync_block() {
    let mut rig = Rig::new(3, 3, ScrollMode::Region);
    rig.commit(&[], &["a", "b", ">"]);
    // The terminal widens; the cursor stays on the prompt line (row 5).
    rig.sim.resize(40, H);
    rig.term.backend_mut().set_fixed_size(Size::new(40, H));
    rig.term.backend_mut().set_fixed_cursor(Position::new(2, 5));
    let stats = rig.commit(&[], &["a", "b", ">"]);
    assert!(stats.erased_screen, "ESC[J expected on resize");
    assert_eq!(rig.term.area(), Rect::new(0, 3, 40, 3));
    assert_eq!(&rig.screen()[3..6], ["a", "b", ">"]);
}

#[test]
fn resize_without_cursor_reply_keeps_the_viewport_row_when_the_screen_grows() {
    // Screen filled: the viewport occupies the last 3 rows.
    let mut rig = Rig::new(12, 3, ScrollMode::Region);
    rig.commit(&[], &["a", "b", ">"]);
    assert_eq!(rig.term.area(), Rect::new(0, H - 3, W, 3));
    // The terminal grows by 4 rows (added at the bottom) and does not reply to the query.
    rig.sim.resize(W, H + 4);
    rig.term.backend_mut().set_fixed_size(Size::new(W, H + 4));
    rig.term.backend_mut().fail_cursor_queries();
    let stats = rig.commit(&[], &["a", "b", ">"]);
    assert!(stats.erased_screen, "ESC[J expected on resize");
    assert_eq!(rig.term.area(), Rect::new(0, H - 3, W, 3));
    let screen = rig.screen();
    assert_eq!(&screen[H as usize - 3..H as usize], ["a", "b", ">"]);
    assert!(screen[H as usize..].iter().all(String::is_empty));
}

#[test]
fn resize_without_cursor_reply_keeps_a_floating_viewport_row() {
    // Nearly empty screen: the viewport floats at rows 1..4.
    let mut rig = Rig::new(1, 3, ScrollMode::Region);
    rig.commit(&[], &["a", "b", ">"]);
    assert_eq!(rig.term.area(), Rect::new(0, 1, W, 3));
    rig.sim.resize(40, H);
    rig.term.backend_mut().set_fixed_size(Size::new(40, H));
    rig.term.backend_mut().fail_cursor_queries();
    rig.commit(&[], &["a", "b", ">"]);
    assert_eq!(rig.term.area(), Rect::new(0, 1, 40, 3));
    assert_eq!(&rig.screen()[1..4], ["a", "b", ">"]);
}

#[test]
fn resize_without_cursor_reply_fits_the_viewport_on_a_smaller_screen() {
    let mut rig = Rig::new(12, 3, ScrollMode::Region);
    rig.commit(&[], &["a", "b", ">"]);
    rig.sim.resize(W, 5);
    rig.term.backend_mut().set_fixed_size(Size::new(W, 5));
    rig.term.backend_mut().fail_cursor_queries();
    rig.commit(&[], &["a", "b", ">"]);
    assert_eq!(rig.term.area(), Rect::new(0, 2, W, 3));
    assert_eq!(&rig.screen()[2..], ["a", "b", ">"]);
}

#[test]
fn clear_erases_only_the_viewport_and_leaves_the_cursor_at_its_top() {
    let mut rig = Rig::new(15, 3, ScrollMode::Region);
    rig.commit(&["log"], &["a", "b", ">"]);
    rig.term.clear().unwrap();
    rig.pump();
    let screen = rig.screen();
    assert_eq!(&screen[7..], ["", "", ""]);
    assert_eq!(screen[6], "log");
    assert_eq!(rig.sim.cursor, (0, 7));
    assert_eq!(rig.sim.outside_sync, b"");
}

#[test]
fn clear_writes_the_lines_still_pending_before_quitting() {
    let mut rig = Rig::new(0, 1, ScrollMode::Screen);
    // A viewport as tall as the screen: the lines wait.
    let view: Vec<&str> = vec!["x"; H as usize];
    rig.commit(&["first", "second"], &view);
    assert!(!rig.sim.all_lines().iter().any(|l| l == "first"));
    rig.term.clear().unwrap();
    rig.pump();
    let lines = rig.sim.all_lines();
    let first = lines
        .iter()
        .position(|l| l == "first")
        .expect("first line lost");
    assert_eq!(lines[first + 1], "second", "{lines:#?}");
    // The one-row viewport is cleared, the cursor at its start, below the lines.
    let screen = rig.screen();
    let row = usize::from(rig.sim.cursor.1);
    assert_eq!(screen[row], "", "{screen:#?}");
    assert_eq!(screen[row - 1], "second", "{screen:#?}");
    assert_eq!(rig.sim.cursor.0, 0);
    assert_eq!(rig.sim.outside_sync, b"");
    // The requested height is kept for the next frame.
    rig.commit(&[], &view);
    assert_eq!(rig.term.area().height, H);
}

#[test]
fn clear_without_pending_lines_writes_no_frame() {
    let mut rig = Rig::new(15, 3, ScrollMode::Screen);
    rig.commit(&["log"], &["a", "b", ">"]);
    let blocks = rig.sim.blocks.len();
    rig.term.clear().unwrap();
    rig.pump();
    // Only the clearing block.
    assert_eq!(rig.sim.blocks.len(), blocks + 1);
}

// --- random operation sequences ----------------------------------------------------------------

/// One step of a random session on an inline terminal.
#[derive(Debug, Clone)]
enum Op {
    /// The terminal reports a new size (0 rows or 0 columns included); `reply` is false
    /// when it does not answer the cursor position query that follows.
    Resize {
        width: u16,
        height: u16,
        reply: bool,
    },
    /// Queue that many one-row lines above the viewport.
    Insert(u8),
    /// Request a viewport height.
    SetHeight(u16),
    /// Draw a frame (applies the queued lines and the height).
    Draw,
}

fn op() -> impl proptest::strategy::Strategy<Value = Op> {
    use proptest::prelude::*;
    prop_oneof![
        1 => (0u16..=14, 0u16..=9, proptest::bool::weighted(0.8))
            .prop_map(|(width, height, reply)| Op::Resize { width, height, reply }),
        2 => (1u8..=3).prop_map(Op::Insert),
        1 => (1u16..=9).prop_map(Op::SetHeight),
        3 => Just(Op::Draw),
    ]
}

/// Run `ops` on a 14x9 inline terminal and its simulator, then restore a normal size and
/// draw. After every draw: no error, nothing outside a synchronized block, no sequence
/// beyond the screen, and (when the screen has rows and columns) a viewport on screen.
/// At the end, each inserted line appears at most once.
fn run_session(
    mode: ScrollMode,
    prefill: usize,
    ops: &[Op],
) -> Result<(), proptest::test_runner::TestCaseError> {
    use proptest::{prop_assert, prop_assert_eq};

    let (mut width, mut height) = (14u16, 9u16);
    let mut sim = Sim::new(width, height);
    for i in 0..prefill {
        sim.feed_str(&format!("avant{i}\r\n"));
    }
    sim.outside_sync.clear();
    let (x, y) = sim.cursor;
    let backend = Backend::with_fixed(Vec::new(), Size::new(width, height), Position::new(x, y));
    let mut term = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(2),
            scroll_mode: mode,
        },
    )
    .unwrap();
    // The initialization (rows reserved for the viewport) reaches the screen right away.
    let bytes = std::mem::take(term.backend_mut().writer_mut());
    sim.feed(&bytes);
    let mut labels = Vec::new();

    let final_ops = [
        Op::Resize {
            width: 14,
            height: 9,
            reply: true,
        },
        Op::SetHeight(1),
        Op::Draw,
        Op::Draw,
    ];
    for op in ops.iter().chain(&final_ops) {
        match *op {
            Op::Resize {
                width: w,
                height: h,
                reply,
            } => {
                (width, height) = (w, h);
                sim.resize(w, h);
                let backend = term.backend_mut();
                backend.set_fixed_size(Size::new(w, h));
                if reply {
                    // What a terminal reports: its cursor, within the reported size.
                    let (cx, cy) = sim.cursor;
                    backend.set_fixed_cursor(Position::new(
                        cx.min(w.saturating_sub(1)),
                        cy.min(h.saturating_sub(1)),
                    ));
                } else {
                    backend.fail_cursor_queries();
                }
            }
            Op::Insert(count) => {
                for _ in 0..count {
                    // Same length for every label: none is a substring of another.
                    let label = format!("#{:03}", labels.len());
                    term.insert_before(1, |buf| {
                        buf.set_string(0, 0, &label, Style::new());
                    });
                    labels.push(label);
                }
            }
            Op::SetHeight(h) => term.set_height(h),
            Op::Draw => {
                let drawn = term.draw(|frame| {
                    let area = frame.area();
                    for y in area.top()..area.bottom() {
                        frame.buffer_mut().set_string(area.x, y, "~", Style::new());
                    }
                    frame.set_cursor_position((area.x, area.bottom().saturating_sub(1)));
                });
                prop_assert!(drawn.is_ok(), "draw failed: {drawn:?}");
                let bytes = std::mem::take(term.backend_mut().writer_mut());
                sim.feed(&bytes);
                prop_assert!(
                    sim.outside_sync.is_empty(),
                    "bytes outside a synchronized block: {:?}",
                    String::from_utf8_lossy(&sim.outside_sync)
                );
                prop_assert!(
                    sim.out_of_range.is_empty(),
                    "sequences beyond the {width}x{height} screen: {:?}",
                    sim.out_of_range
                );
                if width > 0 && height > 0 {
                    let area = term.area();
                    prop_assert_eq!(area.width, width);
                    prop_assert!(area.height >= 1 && area.bottom() <= height, "{area:?}");
                }
            }
        }
    }

    let text = sim.all_lines().join("\n");
    for label in &labels {
        let count = text.matches(label.as_str()).count();
        prop_assert!(count <= 1, "{label} written {count} times:\n{text}");
    }
    Ok(())
}

proptest::proptest! {
    #[test]
    fn random_sessions_with_zero_sizes_keep_the_terminal_consistent(
        mode in proptest::sample::select(vec![
            ScrollMode::Screen,
            ScrollMode::Region,
            ScrollMode::Fallback,
        ]),
        prefill in 0usize..12,
        ops in proptest::collection::vec(op(), 1..40),
    ) {
        run_session(mode, prefill, &ops)?;
    }
}

// --- the scrollback ----------------------------------------------------------------------------

/// A change without resizing (a terminal that reflows its lines on resize may lose some).
fn scrollback_op() -> impl proptest::strategy::Strategy<Value = Op> {
    use proptest::prelude::*;
    prop_oneof![
        2 => (1u8..=4).prop_map(Op::Insert),
        1 => (1u16..=9).prop_map(Op::SetHeight),
        3 => Just(Op::Draw),
    ]
}

/// Run `ops` on a 14x9 inline terminal in `mode`, replayed on `sim`, then shrink the viewport
/// to one row and draw. Every inserted line must then be found exactly once and in order in
/// the scrollback and the screen, and no viewport row (drawn as `~`) may ever enter the
/// scrollback.
fn run_scrollback(
    mut sim: Sim,
    mode: ScrollMode,
    prefill: usize,
    ops: &[Op],
) -> Result<(), proptest::test_runner::TestCaseError> {
    use proptest::{prop_assert, prop_assert_eq};

    for i in 0..prefill {
        sim.feed_str(&format!("avant{i}\r\n"));
    }
    let (x, y) = sim.cursor;
    let backend = Backend::with_fixed(Vec::new(), Size::new(14, 9), Position::new(x, y));
    let mut term = Terminal::with_options(
        backend,
        TerminalOptions::default()
            .viewport(Viewport::Inline(2))
            .scroll_mode(mode),
    )
    .unwrap();
    sim.feed(&std::mem::take(term.backend_mut().writer_mut()));
    let mut labels = Vec::new();
    let final_ops = [Op::SetHeight(1), Op::Draw, Op::Draw];
    for op in ops.iter().chain(&final_ops) {
        match *op {
            Op::Insert(count) => {
                for _ in 0..count {
                    let label = format!("#{:03}", labels.len());
                    term.insert_before(1, |buf| {
                        buf.set_string(0, 0, &label, Style::new());
                    });
                    labels.push(label);
                }
            }
            Op::SetHeight(h) => term.set_height(h),
            Op::Draw => {
                term.draw(|frame| {
                    let area = frame.area();
                    for y in area.top()..area.bottom() {
                        frame.buffer_mut().set_string(area.x, y, "~", Style::new());
                    }
                })
                .unwrap();
                sim.feed(&std::mem::take(term.backend_mut().writer_mut()));
                prop_assert!(
                    !sim.scrollback.iter().any(|line| line.starts_with('~')),
                    "a viewport row entered the scrollback: {:?}",
                    sim.scrollback
                );
            }
            Op::Resize { .. } => unreachable!("no resize here"),
        }
    }
    let found: Vec<String> = sim
        .all_lines()
        .into_iter()
        .filter(|line| line.starts_with('#'))
        .collect();
    prop_assert_eq!(found, labels, "scrollback: {:?}", sim.scrollback);
    Ok(())
}

proptest::proptest! {
    #[test]
    fn every_inserted_line_reaches_the_scrollback_of_a_strict_terminal(
        prefill in 0usize..12,
        ops in proptest::collection::vec(scrollback_op(), 1..40),
    ) {
        run_scrollback(Sim::new(14, 9), ScrollMode::default(), prefill, &ops)?;
    }

    #[test]
    fn every_inserted_line_reaches_the_scrollback_of_xterm_in_every_mode(
        mode in proptest::sample::select(vec![
            ScrollMode::default(),
            ScrollMode::Region,
            ScrollMode::Fallback,
        ]),
        prefill in 0usize..12,
        ops in proptest::collection::vec(scrollback_op(), 1..40),
    ) {
        run_scrollback(Sim::xterm(14, 9), mode, prefill, &ops)?;
    }
}
