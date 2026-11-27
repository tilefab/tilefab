//! Common base: cells, buffer, styles, geometry, styled text and the widget traits.
//!
//! The other crates of the workspace (`tilefab-term`, `tilefab-widget`, `tilefab-input`)
//! build on this one, which depends on none of them and does no input/output (no crossterm).

// The entire public API is documented; the lint catches omissions.
#![warn(missing_docs)]

pub mod buffer;
pub mod convert;
pub mod layout;
pub mod style;
pub mod text;
pub mod widget;

pub use buffer::{Buffer, Cell, grapheme_width, str_width};
pub use layout::{Constraint, Direction, Layout, Margin, Position, Rect, Size};
pub use style::{Color, Modifier, Style};
pub use text::{Alignment, Line, Span, Stylize, Text};
pub use widget::{StatefulWidget, Widget};
