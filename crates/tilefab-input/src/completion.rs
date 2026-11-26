//! Completion: the [`Completer`] trait, the candidates, and the menu drawn below the prompt.

use std::collections::HashSet;

use tilefab_core::buffer::grapheme_width;
use tilefab_core::{Buffer, Rect, Style};
use unicode_segmentation::UnicodeSegmentation;

use crate::buffer;
use crate::render::EditorStyles;
use crate::undo::Snapshot;

/// Source of completions, called synchronously on Tab: it must be fast.
///
/// `Send`, so that a [`LineEditor`](crate::LineEditor) holding one can move to another
/// thread or task (`tokio::spawn`).
pub trait Completer: Send {
    /// Candidates for the text before `cursor` (a byte offset in `line`, on a grapheme
    /// boundary).
    fn complete(&self, line: &str, cursor: usize) -> Completions;
}

/// What a [`Completer`] proposes: candidates that replace `line[start..cursor]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Completions {
    /// Byte offset where the replaced text starts.
    pub start: usize,
    /// The candidates, in the order to show them.
    pub candidates: Vec<Candidate>,
}

/// One completion candidate.
///
/// ```
/// use tilefab_input::Candidate;
///
/// let candidate = Candidate::new("start").description("start a task").append_space();
/// assert_eq!(candidate.replacement, "start");
/// assert!(candidate.append_space);
/// ```
///
/// New fields may come in minor versions, so it cannot be written as a struct literal outside
/// this crate: build it with [`Candidate::new`] and the setters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Candidate {
    /// The text that replaces `line[start..cursor]`.
    pub replacement: String,
    /// Shown in the menu instead of `replacement`.
    pub display: Option<String>,
    /// Shown beside it when there is room.
    pub description: Option<String>,
    /// Add a space after a unique completion.
    pub append_space: bool,
}

impl Candidate {
    /// A candidate that inserts `replacement`.
    #[must_use]
    pub fn new(replacement: impl Into<String>) -> Self {
        Self {
            replacement: replacement.into(),
            ..Self::default()
        }
    }

    /// Show `display` in the menu instead of the replacement.
    #[must_use]
    pub fn display(mut self, display: impl Into<String>) -> Self {
        self.display = Some(display.into());
        self
    }

    /// Show `description` beside the candidate.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Add a space after a unique completion.
    #[must_use]
    pub fn append_space(mut self) -> Self {
        self.append_space = true;
        self
    }

    /// The text shown in the menu.
    fn label(&self) -> &str {
        self.display.as_deref().unwrap_or(&self.replacement)
    }
}

/// Check what a completer returned, so that a careless one cannot break the invariants
/// of the line: `start` must be a grapheme boundary before `cursor`; control characters
/// are removed; empty replacements are dropped; duplicates are merged (the first is kept).
pub(crate) fn sanitize(
    completions: Completions,
    line: &str,
    cursor: usize,
) -> Option<(usize, Vec<Candidate>)> {
    let start = completions.start;
    if start > cursor || !buffer::is_boundary(line, start) {
        return None;
    }
    let clean = |text: &str| -> String { text.chars().filter(|c| !c.is_control()).collect() };
    let mut seen = HashSet::new();
    let candidates: Vec<Candidate> = completions
        .candidates
        .into_iter()
        .filter_map(|candidate| {
            let replacement = clean(&candidate.replacement);
            if replacement.is_empty() || !seen.insert(replacement.clone()) {
                return None;
            }
            Some(Candidate {
                replacement,
                display: candidate.display.as_deref().map(clean),
                description: candidate.description.as_deref().map(clean),
                append_space: candidate.append_space,
            })
        })
        .collect();
    (!candidates.is_empty()).then_some((start, candidates))
}

/// The longest common prefix of the replacements, by whole graphemes (case-sensitive).
pub(crate) fn common_prefix(candidates: &[Candidate]) -> &str {
    let Some((first, others)) = candidates.split_first() else {
        return "";
    };
    let first = first.replacement.as_str();
    let mut end = 0;
    for (start, grapheme) in first.grapheme_indices(true) {
        let shared = others.iter().all(|other| {
            other
                .replacement
                .get(start..)
                .and_then(|rest| rest.graphemes(true).next())
                == Some(grapheme)
        });
        if !shared {
            break;
        }
        end = start + grapheme.len();
    }
    &first[..end]
}

/// The completion menu while it is open.
#[derive(Debug)]
pub(crate) struct Menu {
    /// Start of the replaced text in the original line.
    pub(crate) start: usize,
    /// The sanitized candidates.
    pub(crate) candidates: Vec<Candidate>,
    /// The line before Tab, restored by `Esc`.
    pub(crate) original: Snapshot,
    /// Index of the selected candidate.
    pub(crate) selected: usize,
    /// First visible row, kept between renders.
    pub(crate) top: usize,
    /// Rows of the last layout (candidates per column), for ←/→.
    pub(crate) rows: usize,
}

impl Menu {
    /// The original line with the selected candidate in place, and the cursor after it.
    pub(crate) fn preview(&self) -> (String, usize) {
        let line = &self.original.value;
        let replacement = &self.candidates[self.selected].replacement;
        let value = format!(
            "{}{}{}",
            &line[..self.start],
            replacement,
            &line[self.original.cursor..]
        );
        (value, self.start + replacement.len())
    }

    /// Select the next (`forward`) or previous candidate, wrapping around.
    pub(crate) fn step(&mut self, forward: bool) {
        let len = self.candidates.len();
        self.selected = if forward {
            (self.selected + 1) % len
        } else {
            (self.selected + len - 1) % len
        };
    }

    /// Select the candidate on the same row in the next (`forward`) or previous column,
    /// wrapping around to the other end of the row.
    pub(crate) fn step_column(&mut self, forward: bool) {
        let len = self.candidates.len();
        let rows = self.rows.max(1);
        let row = self.selected % rows;
        self.selected = if forward {
            if self.selected + rows < len {
                self.selected + rows
            } else {
                row
            }
        } else if self.selected >= rows {
            self.selected - rows
        } else {
            // The last column that has this row.
            row + rows * ((len - 1 - row) / rows)
        };
    }
}

/// How the candidates are laid out in a given width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MenuLayout {
    /// Number of columns.
    pub(crate) columns: usize,
    /// Number of rows of the whole menu (candidates per column).
    pub(crate) rows: usize,
    /// Width of the labels, in columns.
    pub(crate) label_width: usize,
    /// A single column `label  description`.
    pub(crate) described: bool,
}

/// Columns between two menu columns, and between a label and its description.
const GAP: usize = 2;

/// The layout of `candidates` in `width` columns: columns sized to the longest label,
/// filled top to bottom; a single column with descriptions when every candidate has one
/// and the width leaves room for them.
pub(crate) fn layout(candidates: &[Candidate], width: usize) -> MenuLayout {
    let label_width = candidates
        .iter()
        .map(|c| text_width(c.label()))
        .max()
        .unwrap_or(0)
        .max(1);
    let described = candidates.iter().all(|c| c.description.is_some()) && label_width + GAP < width;
    if described {
        return MenuLayout {
            columns: 1,
            rows: candidates.len(),
            label_width,
            described,
        };
    }
    let columns = ((width + GAP) / (label_width + GAP)).max(1);
    MenuLayout {
        columns,
        rows: candidates.len().div_ceil(columns),
        label_width,
        described,
    }
}

/// Draw `menu` in `area` (at most `area.height` rows), scrolling so that the selected
/// candidate is visible. When candidates are hidden, the last row ends with `(+N more)`.
pub(crate) fn draw_menu(menu: &mut Menu, area: Rect, buf: &mut Buffer, styles: &EditorStyles) {
    let width = usize::from(area.width);
    let layout = layout(&menu.candidates, width);
    menu.rows = layout.rows;
    let visible = layout.rows.min(usize::from(area.height));
    // An area 0 columns wide only gets clipped writes: no need to stop for it.
    if visible == 0 {
        return;
    }
    let selected_row = menu.selected % layout.rows;
    // Scroll so that the selected row is visible.
    menu.top = menu.top.min(selected_row);
    if selected_row >= menu.top + visible {
        menu.top = selected_row + 1 - visible;
    }
    menu.top = menu.top.min(layout.rows - visible);

    let column_width = layout.label_width + GAP;
    let mut shown = 0;
    for (y, row) in (area.y..).zip(menu.top..menu.top + visible) {
        for column in 0..layout.columns {
            let index = column * layout.rows + row;
            let Some(candidate) = menu.candidates.get(index) else {
                break;
            };
            shown += 1;
            let x = column * column_width;
            if x >= width {
                break;
            }
            // The columns always fit (see `layout`): only a single column wider than the area
            // is cut, at `x` = 0.
            let room = layout.label_width.min(width);
            let style = if index == menu.selected {
                styles.menu_selected
            } else {
                Style::new()
            };
            // The selection is shown over the whole label width.
            let padded = format!("{:<pad$}", candidate.label(), pad = room);
            let _ = buf.set_stringn(area.x + to_u16(x), y, &padded, room, style);
            if layout.described
                && let Some(description) = &candidate.description
            {
                // `layout` describes only when `label_width + GAP < width`.
                let x = layout.label_width + GAP;
                let _ = buf.set_stringn(
                    area.x + to_u16(x),
                    y,
                    description,
                    width - x,
                    styles.menu_description,
                );
            }
        }
    }
    let hidden = menu.candidates.len() - shown;
    if hidden > 0 {
        let counter = format!(" (+{hidden} more)");
        let counter_width = text_width(&counter).min(width);
        let x = area.x + to_u16(width - counter_width);
        let y = area.y + to_u16(visible - 1);
        let _ = buf.set_stringn(x, y, &counter, counter_width, styles.menu_description);
    }
}

/// Number of rows `menu` needs in `width` columns, at most `max_rows`.
pub(crate) fn rows_needed(menu: &Menu, width: u16, max_rows: u16) -> u16 {
    let rows = layout(&menu.candidates, usize::from(width)).rows;
    to_u16(rows).min(max_rows)
}

/// Display width of `text`.
fn text_width(text: &str) -> usize {
    text.graphemes(true).map(grapheme_width).sum()
}

/// A count that fits a buffer coordinate.
fn to_u16(count: usize) -> u16 {
    u16::try_from(count).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates(names: &[&str]) -> Vec<Candidate> {
        names.iter().map(|n| Candidate::new(*n)).collect()
    }

    fn menu(names: &[&str], selected: usize) -> Menu {
        Menu {
            start: 0,
            candidates: candidates(names),
            original: Snapshot {
                value: String::new(),
                cursor: 0,
            },
            selected,
            top: 0,
            rows: names.len(),
        }
    }

    fn draw(menu: &mut Menu, width: u16, height: u16) -> Vec<String> {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        let area = buf.area();
        draw_menu(menu, area, &mut buf, &EditorStyles::default());
        buf.lines()
            .into_iter()
            .map(|l| l.trim_end().to_string())
            .collect()
    }

    #[test]
    fn the_common_prefix_is_made_of_whole_graphemes() {
        assert_eq!(
            common_prefix(&candidates(&["start", "stop", "status"])),
            "st"
        );
        assert_eq!(
            common_prefix(&candidates(&["e\u{301}a", "e\u{301}b"])),
            "e\u{301}"
        );
        // "e" alone is not a prefix of "é" written with a combining accent.
        assert_eq!(common_prefix(&candidates(&["e\u{301}", "ex"])), "");
        assert_eq!(common_prefix(&candidates(&["Ab", "ab"])), "");
        assert_eq!(common_prefix(&candidates(&["one"])), "one");
        assert_eq!(common_prefix(&[]), "");
    }

    #[test]
    fn bad_candidates_are_sanitized() {
        let completions = Completions {
            start: 0,
            candidates: vec![
                Candidate::new("a\u{7}b").description("x\ny"),
                Candidate::new(""),
                Candidate::new("\u{1b}"),
                Candidate::new("ab"),
                Candidate::new("c"),
            ],
        };
        let (start, clean) = sanitize(completions, "a", 1).unwrap();
        assert_eq!(start, 0);
        assert_eq!(
            clean,
            [Candidate::new("ab").description("xy"), Candidate::new("c")]
        );
        // A start after the cursor or inside a grapheme is refused.
        let bad = |start| Completions {
            start,
            candidates: candidates(&["x"]),
        };
        assert_eq!(sanitize(bad(2), "ab", 1), None);
        assert_eq!(sanitize(bad(1), "e\u{301}", 3), None);
        assert_eq!(sanitize(Completions::default(), "", 0), None);
    }

    #[test]
    fn the_menu_shows_the_display_text_of_a_candidate() {
        let candidate = Candidate::new("x").display("shown");
        assert_eq!(candidate.label(), "shown");
        let mut m = menu(&[], 0);
        m.candidates = vec![candidate, Candidate::new("plain")];
        assert_eq!(draw(&mut m, 20, 2)[0], "shown  plain");
    }

    #[test]
    fn a_completion_may_start_at_the_cursor() {
        let completions = Completions {
            start: 3,
            candidates: candidates(&["x"]),
        };
        assert_eq!(
            sanitize(completions, "ab ", 3),
            Some((3, candidates(&["x"])))
        );
    }

    #[test]
    fn left_and_right_move_along_the_row_and_wrap_around() {
        // 7 candidates in 3 rows: columns [0 1 2] [3 4 5] [6].
        let step = |selected, forward| {
            let mut m = menu(&["a", "b", "c", "d", "e", "f", "g"], selected);
            m.rows = 3;
            m.step_column(forward);
            m.selected
        };
        let forward: Vec<usize> = (0..7).map(|i| step(i, true)).collect();
        assert_eq!(forward, [3, 4, 5, 6, 1, 2, 0]);
        let backward: Vec<usize> = (0..7).map(|i| step(i, false)).collect();
        assert_eq!(backward, [6, 4, 5, 0, 1, 2, 3]);
    }

    #[test]
    fn descriptions_need_room_after_the_labels() {
        let described = |width| {
            let candidates = vec![
                Candidate::new("abcd").description("first"),
                Candidate::new("ab").description("second"),
            ];
            layout(&candidates, width).described
        };
        // Label width 4 + gap 2: a description needs at least 7 columns.
        assert!(!described(6));
        assert!(described(7));
    }

    #[test]
    fn the_scroll_of_the_menu_stays_within_its_rows() {
        // One column of 5 rows, 3 visible; a scroll left past the end is brought back.
        let mut m = menu(&["alpha", "bravo", "charlie", "delta", "echo"], 4);
        m.top = 4;
        // Wide enough for one column only, and for the counter of the hidden rows after it.
        let rows = draw(&mut m, 15, 3);
        assert_eq!(&rows[..2], ["charlie", "delta"]);
        assert!(rows[2].starts_with("echo"), "{rows:?}");
        assert_eq!(m.top, 2);
    }

    #[test]
    fn descriptions_stop_at_the_right_edge_of_the_area() {
        let mut m = menu(&[], 0);
        m.candidates = vec![Candidate::new("ab").description("a long description")];
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        draw_menu(
            &mut m,
            Rect::new(0, 0, 10, 1),
            &mut buf,
            &EditorStyles::default(),
        );
        assert_eq!(buf.lines()[0], format!("{:<20}", "ab  a long"));
    }

    #[test]
    fn a_menu_in_no_columns_draws_nothing_and_does_not_panic() {
        let mut m = menu(&["alpha", "bravo"], 1);
        assert_eq!(draw(&mut m, 0, 2), ["", ""]);
    }

    #[test]
    fn candidates_are_laid_out_in_columns_top_to_bottom() {
        let mut m = menu(&["a1", "a2", "a3", "a4", "a5"], 0);
        // Label width 2 + gap 2: 3 columns in 10 columns, 2 rows.
        assert_eq!(
            layout(&m.candidates, 10),
            MenuLayout {
                columns: 3,
                rows: 2,
                label_width: 2,
                described: false
            }
        );
        assert_eq!(draw(&mut m, 10, 5), ["a1  a3  a5", "a2  a4", "", "", ""]);
        assert_eq!(m.rows, 2);
    }

    #[test]
    fn descriptions_use_a_single_column_when_there_is_room() {
        let mut m = menu(&[], 0);
        m.candidates = vec![
            Candidate::new("start").description("start a task"),
            Candidate::new("list").description("list the tasks"),
        ];
        assert_eq!(
            draw(&mut m, 20, 2),
            ["start  start a task", "list   list the task"]
        );
        // Too narrow for descriptions: columns without them.
        assert_eq!(draw(&mut m, 7, 2), ["start", "list"]);
    }

    #[test]
    fn the_menu_scrolls_with_the_selection_and_counts_hidden_candidates() {
        let names: Vec<String> = (0..8).map(|i| format!("candidate{i}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        // One column (labels of 10 columns in 21), 3 visible rows.
        let mut m = menu(&names, 0);
        let rows = draw(&mut m, 21, 3);
        assert_eq!(rows, ["candidate0", "candidate1", "candidate2  (+5 more)"]);
        m.selected = 5;
        let rows = draw(&mut m, 21, 3);
        assert_eq!(rows, ["candidate3", "candidate4", "candidate5  (+5 more)"]);
        assert_eq!(m.top, 3);
        m.selected = 0;
        let _ = draw(&mut m, 21, 3);
        assert_eq!(m.top, 0);
    }

    #[test]
    fn the_selection_wraps_around_rows_and_columns() {
        let mut m = menu(&["a", "b", "c", "d", "e"], 0);
        m.rows = 2;
        m.step(false);
        assert_eq!(m.selected, 4);
        m.step(true);
        assert_eq!(m.selected, 0);
        // Columns: [a b] [c d] [e]: → from "b" goes to "d", then back to "b".
        m.selected = 1;
        m.step_column(true);
        assert_eq!(m.selected, 3);
        m.step_column(true);
        assert_eq!(m.selected, 1);
        // ← from "a" goes to the last column of its row: "e".
        m.selected = 0;
        m.step_column(false);
        assert_eq!(m.selected, 4);
    }

    #[test]
    fn the_preview_replaces_the_completed_word() {
        let mut m = menu(&["start", "stop"], 1);
        m.original = Snapshot {
            value: "x st y".to_string(),
            cursor: 4,
        };
        m.start = 2;
        assert_eq!(m.preview(), ("x stop y".to_string(), 6));
    }
}
