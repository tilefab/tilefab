//! Rendering a `List` or a titled `Block` allocates nothing: a warm frame that draws them
//! stays allocation-free (see `terminal_alloc.rs` for the `Terminal` side).
//!
//! A separate test binary, because the counting allocator is global to the whole binary:
//! it holds this single test, so no other test thread allocates during the measurement.
//! Building a widget allocates (its items, its title spans), and rendering consumes it:
//! the widgets are built before the measurement, only their rendering is measured.

use std::alloc::System;

use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use tilefab::{
    Alignment, Block, Buffer, Line, List, ListItem, ListState, Modifier, Rect, StatefulWidget,
    Style, Widget,
};

// `stats_alloc` holds the `unsafe` code, which this project forbids in its own sources.
#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

const RENDERS: usize = 100;

/// A titled, bordered block (the title alignment comes from the block, not the line).
fn titled_block() -> Block<'static> {
    Block::bordered()
        .title(Line::from("tasks"))
        .title_alignment(Alignment::Center)
}

/// A list with a block, a highlight symbol and multi-line items, wider than the area.
fn list() -> List<'static> {
    List::new([
        ListItem::new("first"),
        ListItem::new("second\nwith two lines"),
        ListItem::new("third"),
        ListItem::new("fourth"),
    ])
    .block(titled_block())
    .highlight_symbol("> ")
    .highlight_style(Style::new().add_modifier(Modifier::BOLD))
}

#[test]
fn rendering_a_list_or_a_titled_block_allocates_nothing() {
    let mut buf = Buffer::empty(Rect::new(0, 0, 30, 6));
    // Warm-up: anything lazily initialized is set up before measuring.
    list().render(buf.area(), &mut buf, &mut ListState::with_selected(Some(1)));
    titled_block().render(buf.area(), &mut buf);

    let lists: Vec<List<'static>> = (0..RENDERS).map(|_| list()).collect();
    let blocks: Vec<Block<'static>> = (0..RENDERS).map(|_| titled_block()).collect();
    let mut list_states: Vec<ListState> = (0..RENDERS)
        .map(|i| ListState::with_selected(Some(i % 4)))
        .collect();
    let area = buf.area();

    let region = Region::new(GLOBAL);
    for ((list, block), state) in lists.into_iter().zip(blocks).zip(&mut list_states) {
        list.render(area, &mut buf, state);
        block.render(area, &mut buf);
    }
    let stats = region.change();
    assert_eq!(
        (stats.allocations, stats.reallocations),
        (0, 0),
        "list and titled block rendering: {stats:?}"
    );
}
