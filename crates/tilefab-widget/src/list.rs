//! `List`: a list of items of one or more lines, with a selected item.
//!
//! The caller owns the state ([`ListState`]): selection and scrolling persist
//! between frames. [`StatefulWidget::render`] adjusts scrolling to keep the selected
//! item visible, then stores it in the state.

use super::Block;
use tilefab_core::buffer::Buffer;
use tilefab_core::convert::saturating_u16;
use tilefab_core::layout::Rect;
use tilefab_core::style::Style;
use tilefab_core::text::{Span, Text};
use tilefab_core::widget::StatefulWidget;

/// A list item: text of one or more lines.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ListItem<'a> {
    content: Text<'a>,
    style: Style,
}

impl<'a> ListItem<'a> {
    /// Item displaying `content`.
    pub fn new<T: Into<Text<'a>>>(content: T) -> Self {
        Self {
            content: content.into(),
            style: Style::new(),
        }
    }

    /// Item style.
    #[must_use]
    pub fn style<S: Into<Style>>(mut self, style: S) -> Self {
        self.style = style.into();
        self
    }

    /// Number of lines in the item (at least 1: an empty item still occupies one line).
    #[must_use]
    pub fn height(&self) -> usize {
        self.content.height().max(1)
    }
}

impl<'a, T: Into<Text<'a>>> From<T> for ListItem<'a> {
    fn from(content: T) -> Self {
        Self::new(content)
    }
}

/// List state: selected item and scroll offset.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ListState {
    selected: Option<usize>,
    offset: usize,
}

impl ListState {
    /// State with an initial selection.
    #[must_use]
    pub fn with_selected(selected: Option<usize>) -> Self {
        Self {
            selected,
            offset: 0,
        }
    }

    /// Index of the selected item, if any.
    #[must_use]
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Select an item, or none with `None`.
    pub fn select(&mut self, index: Option<usize>) {
        self.selected = index;
        if index.is_none() {
            self.offset = 0;
        }
    }

    /// Index of the first displayed item.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Mutable access to the index of the first displayed item.
    pub fn offset_mut(&mut self) -> &mut usize {
        &mut self.offset
    }

    /// Select the next item (the first if none is selected), without passing the last.
    pub fn select_next(&mut self, len: usize) {
        if len == 0 {
            self.select(None);
            return;
        }
        self.selected = Some(self.selected.map_or(0, |i| (i + 1).min(len - 1)));
    }

    /// Select the previous item (the last if none is selected), without going below 0.
    pub fn select_previous(&mut self, len: usize) {
        if len == 0 {
            self.select(None);
            return;
        }
        self.selected = Some(self.selected.map_or(len - 1, |i| i.saturating_sub(1)));
    }
}

/// A list of items.
///
/// ```
/// use tilefab_core::{Buffer, Rect, StatefulWidget};
/// use tilefab_widget::{List, ListState};
///
/// let mut buf = Buffer::empty(Rect::new(0, 0, 6, 3));
/// let mut state = ListState::with_selected(Some(1));
/// List::new(["one", "two", "three"]).highlight_symbol("> ")
///     .render(buf.area(), &mut buf, &mut state);
/// assert_eq!(buf.lines(), ["  one ", "> two ", "  three"].map(|l| l.chars().take(6).collect::<String>()));
/// ```
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct List<'a> {
    items: Vec<ListItem<'a>>,
    block: Option<Block<'a>>,
    style: Style,
    highlight_style: Style,
    highlight_symbol: &'a str,
}

impl<'a> List<'a> {
    /// List of the given items.
    pub fn new<T: Into<ListItem<'a>>>(items: impl IntoIterator<Item = T>) -> Self {
        Self {
            items: items.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    /// Surround the list with a block.
    #[must_use]
    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    /// Style for the entire area.
    #[must_use]
    pub fn style<S: Into<Style>>(mut self, style: S) -> Self {
        self.style = style.into();
        self
    }

    /// Selected item style, applied across its full width.
    #[must_use]
    pub fn highlight_style<S: Into<Style>>(mut self, style: S) -> Self {
        self.highlight_style = style.into();
        self
    }

    /// Text displayed before the selected item; other items receive the same width
    /// of spaces to keep them aligned. (As soon as a selection exists.)
    #[must_use]
    pub fn highlight_symbol(mut self, symbol: &'a str) -> Self {
        self.highlight_symbol = symbol;
        self
    }

    /// Number of items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True if the list has no items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl StatefulWidget for List<'_> {
    type State = ListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut ListState) {
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
        if inner.is_empty() || self.items.is_empty() {
            return;
        }

        // Clamp an out-of-range selection to the last item.
        if let Some(selected) = state.selected {
            state.selected = Some(selected.min(self.items.len() - 1));
        }
        state.offset = state.offset.min(self.items.len() - 1);
        let height = inner.height as usize;

        // Adjust scrolling so the selected item is fully visible
        // (or, if taller than the area, so its start is visible).
        if let Some(selected) = state.selected {
            state.offset = scroll_offset(
                |index| self.items[index].height(),
                state.offset,
                selected,
                height,
            );
        }

        // Nothing allocated per frame: a borrowed `Span` measures the symbol.
        let symbol_width = if state.selected.is_some() {
            saturating_u16(Span::raw(self.highlight_symbol).width())
        } else {
            0
        };
        let prefix_width = symbol_width.min(inner.width);

        let mut y = inner.y;
        for (index, item) in self.items.iter().enumerate().skip(state.offset) {
            if y >= inner.bottom() {
                break;
            }
            let item_height = saturating_u16(item.height()).min(inner.bottom() - y);
            let item_area = Rect::new(inner.x, y, inner.width, item_height);
            let selected = state.selected == Some(index);

            buf.set_style(item_area, item.style);
            if selected {
                buf.set_style(item_area, self.highlight_style);
            }
            // The symbol is displayed only on the item's first line; other items get as
            // many spaces, written one by one rather than from an allocated string.
            if selected {
                buf.set_stringn(
                    inner.x,
                    y,
                    self.highlight_symbol,
                    inner.width as usize,
                    Style::new(),
                );
            } else {
                for x in inner.x..inner.x + prefix_width {
                    buf.set_string(x, y, " ", Style::new());
                }
            }
            let text_x = inner.x.saturating_add(prefix_width);
            let text_area = Rect::new(text_x, y, inner.right().saturating_sub(text_x), item_height);
            item.content.render_to(text_area, buf);
            y += item_height;
        }
    }
}

/// First item to display so that `selected` is fully visible in `area_height` rows.
///
/// Returns the smallest offset, no lower than `min(offset, selected)`, such that items
/// `offset..=selected` fit in the area; if the selected item alone is taller than the
/// area, returns `selected` (its beginning is shown). The current offset is kept
/// whenever it already works, so the list does not scroll needlessly.
///
/// Single pass from `selected` upwards, stopping at the first item that no longer
/// fits: O(number of visible items), instead of re-summing the heights for every
/// candidate offset. This is equivalent because the total height
/// of `offset..=selected` can only decrease when `offset` increases.
fn scroll_offset(
    height_of: impl Fn(usize) -> usize,
    offset: usize,
    selected: usize,
    area_height: usize,
) -> usize {
    let lowest = offset.min(selected);
    let mut start = selected;
    let mut used = height_of(selected);
    while start > lowest {
        let total = used.saturating_add(height_of(start - 1));
        if total > area_height {
            break;
        }
        used = total;
        start -= 1;
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;
    use tilefab_core::style::{Color, Modifier};

    /// The previous algorithm, kept as the reference: move the
    /// offset down one item at a time, re-summing the heights each time.
    fn scroll_offset_reference(
        heights: &[usize],
        offset: usize,
        selected: usize,
        area: usize,
    ) -> usize {
        let mut offset = offset.min(selected);
        while offset < selected && heights[offset..=selected].iter().sum::<usize>() > area {
            offset += 1;
        }
        offset
    }

    #[test]
    fn scroll_offset_matches_the_reference_exhaustively() {
        // Every list of 1 to 5 items with heights 0 to 3 (0 = empty item), every
        // offset and selection, and areas of 0 to 7 rows.
        for len in 1..=5u32 {
            for code in 0..4usize.pow(len) {
                let heights: Vec<usize> = (0..len).map(|i| code / 4usize.pow(i) % 4).collect();
                for offset in 0..heights.len() {
                    for selected in 0..heights.len() {
                        for area in 0..=7 {
                            assert_eq!(
                                scroll_offset(|i| heights[i], offset, selected, area),
                                scroll_offset_reference(&heights, offset, selected, area),
                                "heights {heights:?}, offset {offset}, selected {selected}, area {area}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn scroll_offset_is_linear_on_long_lists() {
        // Jump from the top to the last of a million items: the previous algorithm
        // re-summed up to a million heights for each of a million offsets.
        let len = 1_000_000;
        let offset = scroll_offset(|_| 1, 0, len - 1, 10);
        assert_eq!(offset, len - 10);
    }

    fn render(list: List, state: &mut ListState, width: u16, height: u16) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        list.render(buf.area(), &mut buf, state);
        buf
    }

    #[test]
    fn renders_items_top_to_bottom() {
        let mut state = ListState::default();
        let buf = render(List::new(["un", "deux"]), &mut state, 5, 3);
        assert_eq!(buf, Buffer::with_lines(["un   ", "deux ", "     "]));
    }

    #[test]
    fn highlight_symbol_marks_the_selected_item_and_indents_the_others() {
        let mut state = ListState::with_selected(Some(1));
        let list = List::new(["un", "deux", "trois"]).highlight_symbol("> ");
        let buf = render(list, &mut state, 8, 3);
        assert_eq!(
            buf,
            Buffer::with_lines(["  un    ", "> deux  ", "  trois "])
        );
    }

    #[test]
    fn no_selection_means_no_indent() {
        let mut state = ListState::default();
        let list = List::new(["un"]).highlight_symbol("> ");
        assert_eq!(render(list, &mut state, 4, 1), Buffer::with_lines(["un  "]));
    }

    #[test]
    fn highlight_style_covers_the_whole_row_of_the_selected_item() {
        let mut state = ListState::with_selected(Some(0));
        let list = List::new(["a", "b"])
            .highlight_style(Style::new().bg(Color::Blue).add_modifier(Modifier::BOLD));
        let buf = render(list, &mut state, 4, 2);
        assert!((0..4).all(|x| buf.get(x, 0).bg == Color::Blue));
        assert!((0..4).all(|x| buf.get(x, 1).bg == Color::Reset));
        assert!(buf.get(0, 0).modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn item_style_and_list_style() {
        let mut state = ListState::default();
        let list = List::new([ListItem::new("a").style(Color::Red), ListItem::new("b")])
            .style(Color::Green);
        let buf = render(list, &mut state, 2, 2);
        assert_eq!(buf.get(0, 0).fg, Color::Red);
        assert_eq!(buf.get(0, 1).fg, Color::Green);
    }

    #[test]
    fn scrolls_to_keep_the_selection_visible() {
        let items = ["a", "b", "c", "d", "e"];
        let mut state = ListState::with_selected(Some(0));
        assert_eq!(
            render(List::new(items), &mut state, 1, 2).lines(),
            ["a", "b"]
        );

        state.select(Some(3));
        assert_eq!(
            render(List::new(items), &mut state, 1, 2).lines(),
            ["c", "d"]
        );
        assert_eq!(state.offset(), 2);

        // Moving up one item does not scroll while the item remains visible.
        state.select(Some(2));
        assert_eq!(
            render(List::new(items), &mut state, 1, 2).lines(),
            ["c", "d"]
        );

        state.select(Some(1));
        assert_eq!(
            render(List::new(items), &mut state, 1, 2).lines(),
            ["b", "c"]
        );
        assert_eq!(state.offset(), 1);
    }

    #[test]
    fn multi_line_items_are_kept_whole_when_scrolling() {
        let items = [
            ListItem::new("a1\na2"),
            ListItem::new("b1\nb2"),
            ListItem::new("c1\nc2"),
        ];
        let mut state = ListState::with_selected(Some(2));
        let list = List::new(items).highlight_symbol(">");
        let buf = render(list, &mut state, 3, 4);
        assert_eq!(buf.lines(), [" b1", " b2", ">c1", " c2"]);
        assert_eq!(state.offset(), 1);
    }

    #[test]
    fn selected_item_taller_than_the_area_shows_its_beginning() {
        let mut state = ListState::with_selected(Some(1));
        let items = [ListItem::new("a"), ListItem::new("x1\nx2\nx3")];
        let buf = render(List::new(items), &mut state, 2, 2);
        assert_eq!(buf.lines(), ["x1", "x2"]);
    }

    #[test]
    fn out_of_range_selection_is_clamped_and_empty_lists_are_fine() {
        let mut state = ListState::with_selected(Some(99));
        render(List::new(["a", "b"]), &mut state, 3, 3);
        assert_eq!(state.selected(), Some(1));

        let mut state = ListState::with_selected(Some(2));
        let buf = render(List::new(Vec::<&str>::new()), &mut state, 3, 2);
        assert_eq!(buf, Buffer::empty(Rect::new(0, 0, 3, 2)));
        assert!(List::new(Vec::<&str>::new()).is_empty());
        assert!(!List::new(["a"]).is_empty());
        assert_eq!(List::new(["a"]).len(), 1);
        assert_eq!(List::new(["a", "b", "c"]).len(), 3);
    }

    #[test]
    fn out_of_range_offset_without_selection_shows_the_last_item() {
        let mut state = ListState::default();
        *state.offset_mut() = 10;
        assert_eq!(state.offset(), 10, "offset_mut writes the offset");
        let buf = render(List::new(["a", "b", "c"]), &mut state, 2, 2);
        assert_eq!(state.offset(), 2);
        assert_eq!(buf.lines(), ["c ", "  "]);
    }

    #[test]
    fn indent_of_unselected_items_overwrites_what_was_there() {
        let mut buf = Buffer::with_lines(["xxxxx", "xxxxx"]);
        let list = List::new(["a", "b"]).highlight_symbol("> ");
        list.render(buf.area(), &mut buf, &mut ListState::with_selected(Some(1)));
        assert_eq!(buf.lines(), ["  axx", "> bxx"]);
    }

    #[test]
    fn last_item_is_cut_at_the_bottom_of_the_block() {
        let mut state = ListState::default();
        let list = List::new(["a", "b\nc\nd"]).block(Block::bordered());
        let buf = render(list, &mut state, 4, 4);
        assert_eq!(buf.lines(), ["┌──┐", "│a │", "│b │", "└──┘"]);
    }

    #[test]
    fn select_next_and_previous_stop_at_the_ends() {
        let mut state = ListState::default();
        state.select_next(3);
        assert_eq!(state.selected(), Some(0));
        state.select_next(3);
        state.select_next(3);
        state.select_next(3);
        assert_eq!(state.selected(), Some(2));
        state.select_previous(3);
        assert_eq!(state.selected(), Some(1));
        state.select_previous(3);
        state.select_previous(3);
        assert_eq!(state.selected(), Some(0));

        let mut state = ListState::default();
        state.select_previous(3);
        assert_eq!(state.selected(), Some(2));
        state.select_next(0);
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn block_and_tiny_areas() {
        let mut state = ListState::with_selected(Some(0));
        let buf = render(List::new(["ab"]).block(Block::bordered()), &mut state, 6, 3);
        assert_eq!(buf.lines(), ["┌────┐", "│ab  │", "└────┘"]);
        for w in 0..3 {
            for h in 0..3 {
                let list = List::new(["abc", "日本語"])
                    .highlight_symbol(">> ")
                    .block(Block::bordered());
                render(list, &mut state, w, h);
                render(List::new(["abc"]).highlight_symbol(">> "), &mut state, w, h);
            }
        }
    }

    #[test]
    fn long_items_are_truncated_and_wide_symbols_measured_in_columns() {
        let mut state = ListState::with_selected(Some(0));
        let buf = render(
            List::new(["abcdefgh"]).highlight_symbol("日 "),
            &mut state,
            6,
            1,
        );
        assert_eq!(buf, Buffer::with_lines(["日 abc"]));
    }
}
