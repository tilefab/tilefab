//! Widget library: gauges, blocks, lists, paragraphs, drawn in a [`Buffer`](tilefab_core::Buffer).
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
mod clear;
mod gauge;
mod list;
mod paragraph;

pub use block::{Block, BorderType, Borders, Padding};
pub use clear::Clear;
pub use gauge::{Gauge, LineGauge};
pub use list::{List, ListItem, ListState};
pub use paragraph::{Paragraph, Wrap};
