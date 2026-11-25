//! Widget library: blocks, paragraphs, lists, progress bars, drawn in a [`Buffer`](tilefab_core::Buffer).
//!
//! Widgets implement the [`Widget`](tilefab_core::Widget) and
//! [`StatefulWidget`](tilefab_core::StatefulWidget) traits of the common base
//! (`tilefab-core`). They do no input/output: they only draw in a buffer, so they can be
//! tested without a terminal.
//!
//! All widgets silently clip anything outside their area or the buffer: an
//! area of width 0 or 1 never causes a panic.

// The entire public API is documented; the lint catches omissions.
#![warn(missing_docs)]

mod block;
// The line widget, built phase by phase next to the old widgets.
mod builder;
mod clear;
mod composition;
mod entry;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the drawing, not written yet")
)]
mod format;
mod list;
mod paragraph;
mod progress;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the drawing, not written yet")
)]
mod time;
mod widget;

pub use block::{Block, BorderType, Borders, Padding};
pub use builder::{
    BuildError, DEFAULT_TARGET_WIDTH, LineWidgetBuilder, ListName, MAX_ENTRIES, MAX_NAMES,
    MAX_VALUE_DECIMALS, TokenProblem, Warning,
};
pub use clear::Clear;
pub use entry::{Entry, EntryProblem};
pub use list::{List, ListItem, ListState};
pub use paragraph::{Paragraph, Wrap};
pub use progress::{ProgressBar, ProgressLine};
pub use time::{TimeField, WallTime};
pub use widget::{LineWidget, UpdateError};
