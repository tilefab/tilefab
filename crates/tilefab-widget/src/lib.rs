//! A one-line widget, [`LineWidget`], composed from a short description string and lists of
//! values, and laid out again at every draw. It draws in a [`Buffer`](tilefab_core::Buffer)
//! without I/O, so it is tested without a terminal.
//!
//! # Composition
//!
//! The *composition* lists the components in display order, separated by exactly one space:
//!
//! | Token | Component | Parameter |
//! |---|---|---|
//! | `S` | symbol of the `symbols` list (one grapheme, 1 or 2 columns) | `S3`: fixed index; `S`: dynamic |
//! | `B` | barrier of the `barriers` list (any text) | index, required |
//! | `D` | date: `D0` `YYYY-MM-DD`, `D1` `YY-MM-DD`, `D2` `MM-DD` | format, required |
//! | `T` | time: `T0` `HH:MM:SS.SSS`, `T1` `MM:SS.SSS`, `T2` `SS.SSS`, `T3` `HH:MM:SS`, `T4` `HH:MM`, `T5` `MM:SS` | format, required |
//! | `P` | progress number and `%`, rounded down | size 3–9 (`P3` by default) |
//! | `K` | keyword of the `keywords` list, aligned in the width of the longest | `K3`: fixed; `K`: dynamic |
//! | `N` | name of the `names` list, as wide as the entry shown | `N14`: fixed; `N`: dynamic |
//! | `L` | log text | `L40`: fixed size; `L`: elastic |
//! | `V` | value, with optional unit (`U`) and total (`/`): `V5U/` | size 1–999 (5 by default) |
//! | `I` | illustration of the progress with two characters, chosen by its look: `B` bar `█░`, `L` line `━─`, `D` dots `●·`, `S` squares `■□`, `O` circles `●○`, `R` diamonds `◆◇`, `G` segments `▰▱`, `H` shaded blocks `▓░`, `V` vertical bars `┃│`, `A` ASCII `#-`, `C` chevrons `>·` | `I20`: fixed size; `I`: elastic; then the letter of the look (`I20L`), else the characters given to the builder or the bar |
//!
//! `D`, `T`, `P`, `K`, `L`, `V` and `I` appear at most once; `S`, `B` and `N` can be repeated.
//!
//! A token may end with a *paint*, laid over the styles given to the builder for that
//! component only: `.`, then at least one of, in this order, a foreground color, `:` and a
//! background color, `*` for bold. Colors: `K` black, `R` red, `G` green, `Y` yellow, `B` blue,
//! `M` magenta, `C` cyan, `W` white (ANSI color 7, a light gray), followed by `+` for the bright
//! version (`K+` dark gray, `W+` bright white). `I` and `V` take a second part after `/`, with
//! the same rule, for their todo cells and their units. For instance `B1.K+` (a dark gray
//! barrier), `N.W+:B*` (a bright white bold name on blue), `I20L.G/K+` (a green line on a dark
//! gray track).
//! The library has no default colors: they come from the entries and the `*_style` methods.
//! [`LineWidgetBuilder`] shows a paint and a look at work, and
//! [`build`](LineWidgetBuilder::build) the errors of a composition.
//! The dynamic components are changed by the setters (`set_symbol`, `set_keyword`,
//! `set_name`, `set_progress`, `set_value`, `set_time`, `set_log`); the repeatable ones are
//! addressed by their *rank* among the dynamic components of the same letter. `P` and `I`
//! share the progress; `D` and `T` share the [`WallTime`], given by the caller (the crate has
//! no clock and no time-zone dependency).
//!
//! # Layout
//!
//! The widget draws on the first row of the area, whose width is the available width. `L`,
//! `I` and `N` are *flexible*. When everything fits, each component has its nominal width and
//! the extra columns go to the elastic `L`, otherwise to the elastic `I`. Otherwise `L` and `I`
//! shrink together, in proportion, down to 2 columns; then the `N` do the same; then the last
//! component is dropped, and the layout starts again. A truncated text ends with `…`; a
//! barrier is never hidden on its own, so a line cut short ends with one.
//!
//! # Example
//!
//! ```
//! use tilefab_core::{Buffer, Color, Rect, Style, Widget};
//! use tilefab_widget::{Entry, LineWidget, Rank, WallTime};
//!
//! let mut widget = LineWidget::builder("B0 T2 B1 B2 S B2 L")
//!     .barriers([Entry::new("["), Entry::new("]"), Entry::new(" "), Entry::new("-")])
//!     .symbols([
//!         Entry::styled("🔴", Style::new().fg(Color::Red)),
//!         Entry::styled("🟢", Style::new().fg(Color::Green)),
//!     ])
//!     .target_width(100)
//!     .build()?;
//! // Problems that do not prevent drawing: to write in the application's log.
//! assert!(widget.warnings().is_empty());
//!
//! let now = WallTime { year: 2026, month: 11, day: 25, hour: 9, minute: 30, second: 5, millis: 123 };
//! widget.set_time(now)?;
//! widget.set_log("Downloading crate");
//!
//! let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
//! (&widget).render(buf.area(), &mut buf);
//! assert_eq!(buf.lines()[0].trim_end(), "[05.123] 🔴 Downloading crate");
//!
//! // Narrower: the log is truncated, then dropped; the barrier before it stays.
//! widget.set_symbol(Rank(0), 1)?;
//! let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
//! (&widget).render(buf.area(), &mut buf);
//! assert_eq!(buf.lines()[0], "[05.123] 🟢 Downloa…");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Errors and warnings
//!
//! [`LineWidgetBuilder::build`] returns a [`BuildError`] for a composition, list or entry
//! that cannot be drawn; a widget wider than the window is never an error.
//! [`LineWidget::warnings`] lists the [`Warning`]s (for now: a nominal width above the
//! target width). A setter returns an [`UpdateError`] for an invalid call and keeps the
//! previous value; `set_log` cannot fail.
//!
//! An area of width 0 or 1, or of height 0, never causes a panic: the widget draws what fits.

// The entire public API is documented; the lint catches omissions.
#![warn(missing_docs)]

mod builder;
mod composition;
mod entry;
mod format;
mod layout;
#[cfg(test)]
mod tests;
mod time;
mod widget;

pub use builder::{
    BuildError, DEFAULT_TARGET_WIDTH, LineWidgetBuilder, ListName, MAX_ENTRIES, MAX_NAMES,
    MAX_VALUE_DECIMALS, TokenProblem, Warning,
};
pub use entry::{Entry, EntryProblem};
pub use time::{TimeField, WallTime};
pub use widget::{LineWidget, Rank, UpdateError, ValueShape};

// The widget can move to another thread or task, and be shared between threads.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<LineWidget>();
};
