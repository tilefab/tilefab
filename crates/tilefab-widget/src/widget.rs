//! The line widget: its description, resolved at construction.

use std::fmt;

use tilefab_core::{Alignment, Style};

use crate::builder::{ListName, Warning};
use crate::entry::Entry;
use crate::format::{sanitize_into, write_number};
use crate::time::{TimeField, WallTime};

/// Nominal width of an elastic `I`.
pub(crate) const ELASTIC_ILLUSTRATION_WIDTH: usize = 10;
/// Smallest width of a flexible component (first character and `…`).
pub(crate) const MIN_FLEXIBLE_WIDTH: usize = 2;

/// Where a component takes its entry from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// Always this entry.
    Fixed(u8),
    /// The entry chosen by a setter for this rank (among the dynamic components of the same
    /// letter).
    Dynamic(usize),
}

/// One component of the widget, in composition order, with its list indices checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Component {
    /// `S`.
    Symbol(Slot),
    /// `B`, with its entry.
    Barrier(u8),
    /// `D`, with its format.
    Date(u8),
    /// `T`, with its format.
    Time(u8),
    /// `P`, with its size.
    Progress(u8),
    /// `K` (a dynamic `K` has rank 0: it is unique).
    Keyword(Slot),
    /// `N`.
    Name(Slot),
    /// `L`: `None` when elastic.
    Log(Option<u16>),
    /// `V`: its shape is in [`LineWidget::value`].
    Value,
    /// `I`: `None` when elastic.
    Illustration(Option<u16>),
}

/// An entry with its width, measured at construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Measured {
    /// The entry.
    pub(crate) entry: Entry,
    /// Its width in columns.
    pub(crate) width: usize,
}

/// The styles of the values that are not list entries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Styles {
    pub(crate) date: Style,
    pub(crate) time: Style,
    pub(crate) progress: Style,
    pub(crate) log: Style,
    pub(crate) value_numbers: Style,
    pub(crate) value_slash: Style,
}

/// The parameters of `V`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ValueShape {
    /// Most columns of each number.
    pub(crate) size: u16,
    /// Followed by a unit.
    pub(crate) unit: bool,
    /// Followed by a total.
    pub(crate) total: bool,
}

/// Everything the builder resolved, handed to [`LineWidget::new`].
#[derive(Debug, Default)]
pub(crate) struct Description {
    pub(crate) symbols: Vec<Measured>,
    pub(crate) barriers: Vec<Measured>,
    pub(crate) keywords: Vec<Measured>,
    pub(crate) names: Vec<Measured>,
    pub(crate) units: Vec<Measured>,
    pub(crate) illustration: Option<(Entry, Entry)>,
    pub(crate) styles: Styles,
    pub(crate) keyword_alignment: Alignment,
    pub(crate) value_decimals: u8,
    pub(crate) value: Option<ValueShape>,
    pub(crate) dynamic_symbols: usize,
    pub(crate) dynamic_names: usize,
    pub(crate) dynamic_keyword: bool,
}

/// A widget that always fits on one line, composed at construction from a composition string
/// and lists of values, and laid out again at every draw.
///
/// Built with [`LineWidget::builder`].
#[derive(Debug, Clone)]
#[expect(
    dead_code,
    reason = "the values are read by the drawing, not written yet"
)]
pub struct LineWidget {
    /// The components, in composition order.
    pub(crate) components: Vec<Component>,
    pub(crate) symbols: Vec<Measured>,
    pub(crate) barriers: Vec<Measured>,
    pub(crate) keywords: Vec<Measured>,
    pub(crate) names: Vec<Measured>,
    pub(crate) units: Vec<Measured>,
    /// The `done` and `todo` characters of `I`.
    pub(crate) illustration: Option<(Entry, Entry)>,
    pub(crate) styles: Styles,
    pub(crate) keyword_alignment: Alignment,
    pub(crate) value_decimals: u8,
    /// The shape of `V`, if the composition has one.
    pub(crate) value: Option<ValueShape>,
    /// Width of `S`: the widest symbol.
    pub(crate) symbol_width: usize,
    /// Width of `K`: the longest keyword.
    pub(crate) keyword_width: usize,
    /// Width of the widest name, for the nominal width of a dynamic `N`.
    pub(crate) widest_name: usize,
    /// Width of the longest unit (0 without `U`).
    pub(crate) unit_width: usize,
    pub(crate) dynamic_symbols: usize,
    pub(crate) dynamic_names: usize,
    pub(crate) dynamic_keyword: bool,
    /// The construction warnings.
    pub(crate) warnings: Vec<Warning>,
    /// The current values, changed by the setters.
    pub(crate) state: State,
}

/// The values that change after construction.
#[derive(Debug, Clone, Default)]
pub(crate) struct State {
    /// Entry shown by each dynamic `S`, by rank.
    pub(crate) symbols: Vec<u8>,
    /// Entry shown by the dynamic `K`.
    pub(crate) keyword: u8,
    /// Entry shown by each dynamic `N`, by rank.
    pub(crate) names: Vec<u8>,
    /// Progress of `P` and `I`, in 0–100.
    pub(crate) progress: f64,
    /// Date and time of `D` and `T`.
    pub(crate) time: WallTime,
    /// Text of `L`, sanitized (no control character).
    pub(crate) log: String,
    /// Width of [`State::log`].
    pub(crate) log_width: usize,
    /// Text of `V`, formatted by the setter.
    pub(crate) value: ValueText,
}

/// The parts of the text of `V`, each drawn with its own style.
#[derive(Debug, Clone, Default)]
pub(crate) struct ValueText {
    /// The value, formatted.
    pub(crate) number: String,
    /// Its unit (index in the `units` list).
    pub(crate) unit: Option<usize>,
    /// The total, formatted (empty without `/`).
    pub(crate) total: String,
    /// Its unit.
    pub(crate) total_unit: Option<usize>,
}

impl LineWidget {
    /// The widget described by `components` and `description`, with no warning yet.
    pub(crate) fn new(components: Vec<Component>, description: Description) -> Self {
        let widest = |list: &[Measured]| list.iter().map(|m| m.width).max().unwrap_or(0);
        let unit_width = match description.value {
            Some(ValueShape { unit: true, .. }) => widest(&description.units),
            _ => 0,
        };
        let mut widget = Self {
            components,
            symbol_width: widest(&description.symbols),
            keyword_width: widest(&description.keywords),
            widest_name: widest(&description.names),
            unit_width,
            symbols: description.symbols,
            barriers: description.barriers,
            keywords: description.keywords,
            names: description.names,
            units: description.units,
            illustration: description.illustration,
            styles: description.styles,
            keyword_alignment: description.keyword_alignment,
            value_decimals: description.value_decimals,
            value: description.value,
            dynamic_symbols: description.dynamic_symbols,
            dynamic_names: description.dynamic_names,
            dynamic_keyword: description.dynamic_keyword,
            warnings: Vec::new(),
            state: State {
                symbols: vec![0; description.dynamic_symbols],
                names: vec![0; description.dynamic_names],
                ..State::default()
            },
        };
        if let Some(shape) = widget.value {
            let unit = shape.unit.then_some(0);
            let total = shape.total.then_some((0.0, unit));
            widget.format_value(0.0, unit, total);
        }
        widget
    }

    /// The construction warnings, to write in the application's log.
    #[must_use]
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// Width of a component that never changes width, `None` for `L`, `I` and `N`.
    pub(crate) fn fixed_width(&self, component: Component) -> Option<usize> {
        Some(match component {
            Component::Symbol(_) => self.symbol_width,
            Component::Barrier(i) => self.barriers[usize::from(i)].width,
            Component::Date(format) => crate::time::date_width(format),
            Component::Time(format) => crate::time::time_width(format),
            Component::Progress(size) => usize::from(size) + 1,
            Component::Keyword(_) => self.keyword_width,
            Component::Value => {
                let shape = self.value?;
                let one = usize::from(shape.size) + self.unit_width;
                if shape.total { 2 * one + 1 } else { one }
            }
            Component::Name(_) | Component::Log(_) | Component::Illustration(_) => return None,
        })
    }

    /// Nominal width of the widget for the [`Warning::WiderThanTarget`] check.
    pub(crate) fn nominal_width(&self) -> usize {
        self.components
            .iter()
            .map(|&component| {
                self.fixed_width(component).unwrap_or(match component {
                    Component::Log(None) => MIN_FLEXIBLE_WIDTH,
                    Component::Illustration(None) => ELASTIC_ILLUSTRATION_WIDTH,
                    Component::Log(Some(size)) | Component::Illustration(Some(size)) => {
                        usize::from(size)
                    }
                    Component::Name(Slot::Fixed(i)) => self.names[usize::from(i)].width,
                    _ => self.widest_name,
                })
            })
            .sum()
    }
}

/// The setters: each one checks its arguments first and changes nothing on error.
impl LineWidget {
    /// Show entry `index` of the `symbols` list in the dynamic `S` of rank `rank` (ranks count
    /// the dynamic `S` only, from 0, in composition order).
    ///
    /// # Errors
    ///
    /// [`UpdateError::RankOutOfRange`] or [`UpdateError::IndexOutOfList`].
    pub fn set_symbol(&mut self, rank: usize, index: usize) -> Result<(), UpdateError> {
        let index = self.checked_index(ListName::Symbols, index)?;
        let slot = rank_slot(&mut self.state.symbols, 'S', rank)?;
        *slot = index;
        Ok(())
    }

    /// Show entry `index` of the `keywords` list in the dynamic `K`.
    ///
    /// # Errors
    ///
    /// [`UpdateError::RankOutOfRange`] when the composition has no dynamic `K`, or
    /// [`UpdateError::IndexOutOfList`].
    pub fn set_keyword(&mut self, index: usize) -> Result<(), UpdateError> {
        if !self.dynamic_keyword {
            return Err(UpdateError::RankOutOfRange {
                component: 'K',
                rank: 0,
                count: 0,
            });
        }
        self.state.keyword = self.checked_index(ListName::Keywords, index)?;
        Ok(())
    }

    /// Show entry `index` of the `names` list in the dynamic `N` of rank `rank` (ranks count
    /// the dynamic `N` only). The width of the name changes with it: the whole line is laid out
    /// again at the next draw.
    ///
    /// # Errors
    ///
    /// [`UpdateError::RankOutOfRange`] or [`UpdateError::IndexOutOfList`].
    pub fn set_name(&mut self, rank: usize, index: usize) -> Result<(), UpdateError> {
        let index = self.checked_index(ListName::Names, index)?;
        let slot = rank_slot(&mut self.state.names, 'N', rank)?;
        *slot = index;
        Ok(())
    }

    /// Set the progress shown by `P` and `I`, in percent; clamped to 0–100.
    ///
    /// # Errors
    ///
    /// [`UpdateError::NoComponent`] without `P` and `I`, [`UpdateError::NotFinite`].
    pub fn set_progress(&mut self, percent: f64) -> Result<(), UpdateError> {
        if !self.has(|c| matches!(c, Component::Progress(_) | Component::Illustration(_))) {
            return Err(UpdateError::NoComponent {
                component: "P or I",
            });
        }
        if !percent.is_finite() {
            return Err(UpdateError::NotFinite);
        }
        self.state.progress = percent.clamp(0.0, 100.0);
        Ok(())
    }

    /// Set the value of `V`: the number, its unit (index in the `units` list) and the total
    /// with its unit. The unit is given exactly when the composition has `U`, the total exactly
    /// when it has `/`. The widget never converts between units.
    ///
    /// # Errors
    ///
    /// [`UpdateError::NoComponent`] without `V`, [`UpdateError::NotFinite`],
    /// [`UpdateError::IndexOutOfList`] for a unit, [`UpdateError::UnitMismatch`] or
    /// [`UpdateError::TotalMismatch`] for an argument that does not match the composition.
    pub fn set_value(
        &mut self,
        value: f64,
        unit: Option<usize>,
        total: Option<(f64, Option<usize>)>,
    ) -> Result<(), UpdateError> {
        let shape = self
            .value
            .ok_or(UpdateError::NoComponent { component: "V" })?;
        if total.is_some() != shape.total {
            return Err(UpdateError::TotalMismatch {
                expected: shape.total,
            });
        }
        for unit in [Some(unit), total.map(|(_, unit)| unit)]
            .into_iter()
            .flatten()
        {
            match unit {
                Some(index) if shape.unit => {
                    self.checked_index(ListName::Units, index)?;
                }
                None if !shape.unit => {}
                _ => {
                    return Err(UpdateError::UnitMismatch {
                        expected: shape.unit,
                    });
                }
            }
        }
        if !value.is_finite() || total.is_some_and(|(total, _)| !total.is_finite()) {
            return Err(UpdateError::NotFinite);
        }
        self.format_value(value, unit, total);
        Ok(())
    }

    /// Set the date and time shown by `D` and `T`.
    ///
    /// # Errors
    ///
    /// [`UpdateError::NoComponent`] without `D` and `T`, [`UpdateError::InvalidTime`].
    pub fn set_time(&mut self, time: WallTime) -> Result<(), UpdateError> {
        if !self.has(|c| matches!(c, Component::Date(_) | Component::Time(_))) {
            return Err(UpdateError::NoComponent {
                component: "D or T",
            });
        }
        time.validate()
            .map_err(|field| UpdateError::InvalidTime { field })?;
        self.state.time = time;
        Ok(())
    }

    /// Set the text of `L`. Every control character is shown as one space. Never fails; the
    /// text is kept even when the composition has no `L`.
    pub fn set_log(&mut self, text: &str) {
        self.state.log_width = sanitize_into(text, &mut self.state.log);
    }

    /// Whether a component of the composition matches `f`.
    fn has(&self, f: impl Fn(Component) -> bool) -> bool {
        self.components.iter().any(|&c| f(c))
    }

    /// `index` as an entry of `list`, if it is one.
    fn checked_index(&self, list: ListName, index: usize) -> Result<u8, UpdateError> {
        let len = match list {
            ListName::Symbols => self.symbols.len(),
            ListName::Keywords => self.keywords.len(),
            ListName::Names => self.names.len(),
            ListName::Units => self.units.len(),
            ListName::Barriers => self.barriers.len(),
            ListName::Illustration => 2,
        };
        if index < len {
            // Lists hold at most 100 entries.
            Ok(u8::try_from(index).unwrap_or(u8::MAX))
        } else {
            Err(UpdateError::IndexOutOfList { list, index, len })
        }
    }

    /// Format the text of `V` (arguments already checked), reusing the memory of the
    /// previous text.
    fn format_value(
        &mut self,
        value: f64,
        unit: Option<usize>,
        total: Option<(f64, Option<usize>)>,
    ) {
        let Some(shape) = self.value else { return };
        let size = usize::from(shape.size);
        let text = &mut self.state.value;
        text.number.clear();
        write_number(&mut text.number, value, size, self.value_decimals);
        text.unit = unit;
        text.total.clear();
        text.total_unit = None;
        if let Some((total, unit)) = total {
            write_number(&mut text.total, total, size, self.value_decimals);
            text.total_unit = unit;
        }
    }
}

/// The entry of rank `rank` in `slots`, the dynamic `component`s.
fn rank_slot(slots: &mut [u8], component: char, rank: usize) -> Result<&mut u8, UpdateError> {
    let count = slots.len();
    slots.get_mut(rank).ok_or(UpdateError::RankOutOfRange {
        component,
        rank,
        count,
    })
}

/// Why a setter of [`LineWidget`] refused its arguments. The previous value is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpdateError {
    /// No dynamic component of this letter has this rank (`count` is their number).
    RankOutOfRange {
        /// The letter: `S`, `K` or `N`.
        component: char,
        /// The rank given.
        rank: usize,
        /// Number of dynamic components of this letter.
        count: usize,
    },
    /// An index beyond the end of its list.
    IndexOutOfList {
        /// The list.
        list: ListName,
        /// The index given.
        index: usize,
        /// Number of entries of the list.
        len: usize,
    },
    /// The composition has none of the components this setter changes.
    NoComponent {
        /// The components concerned, for instance `"P or I"`.
        component: &'static str,
    },
    /// A number that is infinite or not a number.
    NotFinite,
    /// A [`WallTime`] with a field out of its range.
    InvalidTime {
        /// The first field out of range.
        field: TimeField,
    },
    /// A unit given without `U` in the composition, or missing with `U`.
    UnitMismatch {
        /// Whether the composition expects a unit.
        expected: bool,
    },
    /// A total given without `/` in the composition, or missing with `/`.
    TotalMismatch {
        /// Whether the composition expects a total.
        expected: bool,
    },
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RankOutOfRange {
                component,
                rank,
                count,
            } => write!(
                f,
                "rank {rank} out of range: the composition has {count} dynamic {component}"
            ),
            Self::IndexOutOfList { list, index, len } => {
                write!(f, "index {index} out of the {list} list (length {len})")
            }
            Self::NoComponent { component } => {
                write!(f, "the composition has no {component}")
            }
            Self::NotFinite => f.write_str("the number is not finite"),
            Self::InvalidTime { field } => write!(f, "invalid time: {field} out of range"),
            Self::UnitMismatch { expected: true } => {
                f.write_str("the composition has U: a unit is required")
            }
            Self::UnitMismatch { expected: false } => {
                f.write_str("the composition has no U: no unit is accepted")
            }
            Self::TotalMismatch { expected: true } => {
                f.write_str("the composition has /: a total is required")
            }
            Self::TotalMismatch { expected: false } => {
                f.write_str("the composition has no /: no total is accepted")
            }
        }
    }
}

impl std::error::Error for UpdateError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(texts: &[&str]) -> Vec<Entry> {
        texts.iter().copied().map(Entry::new).collect()
    }

    fn full() -> LineWidget {
        LineWidget::builder("S S1 S K N N0 N D0 T0 P5 V4U/ I L")
            .symbols(entries(&["a", "b", "c"]))
            .keywords(entries(&["ok", "fail"]))
            .names(entries(&["alpha", "beta", "gamma"]))
            .units(entries(&["b", "KB"]))
            .illustration(Entry::new("#"), Entry::new("."))
            .build()
            .unwrap()
    }

    const NOON: WallTime = WallTime {
        year: 2026,
        month: 11,
        day: 25,
        hour: 12,
        minute: 0,
        second: 0,
        millis: 0,
    };

    #[test]
    fn initial_state() {
        let widget = full();
        assert_eq!(widget.state.symbols, [0, 0]);
        assert_eq!(widget.state.names, [0, 0]);
        assert_eq!(widget.state.keyword, 0);
        assert_eq!(widget.state.progress, 0.0);
        assert_eq!(widget.state.time, WallTime::default());
        assert_eq!((widget.state.log.as_str(), widget.state.log_width), ("", 0));
        let value = &widget.state.value;
        assert_eq!((value.number.as_str(), value.unit), ("0.0", Some(0)));
        assert_eq!((value.total.as_str(), value.total_unit), ("0.0", Some(0)));
    }

    #[test]
    fn symbols_by_rank() {
        let mut widget = full();
        widget.set_symbol(1, 2).unwrap();
        assert_eq!(widget.state.symbols, [0, 2]);
        assert_eq!(
            widget.set_symbol(2, 0),
            Err(UpdateError::RankOutOfRange {
                component: 'S',
                rank: 2,
                count: 2
            })
        );
        assert_eq!(
            widget.set_symbol(0, 3),
            Err(UpdateError::IndexOutOfList {
                list: ListName::Symbols,
                index: 3,
                len: 3
            })
        );
        assert_eq!(widget.state.symbols, [0, 2]);
    }

    #[test]
    fn keyword() {
        let mut widget = full();
        widget.set_keyword(1).unwrap();
        assert_eq!(widget.state.keyword, 1);
        assert!(matches!(
            widget.set_keyword(2),
            Err(UpdateError::IndexOutOfList { .. })
        ));
        assert_eq!(widget.state.keyword, 1);
        let mut fixed = LineWidget::builder("K1")
            .keywords(entries(&["a", "b"]))
            .build()
            .unwrap();
        assert_eq!(
            fixed.set_keyword(0),
            Err(UpdateError::RankOutOfRange {
                component: 'K',
                rank: 0,
                count: 0
            })
        );
    }

    #[test]
    fn names_by_rank() {
        let mut widget = full();
        widget.set_name(1, 2).unwrap();
        assert_eq!(widget.state.names, [0, 2]);
        assert!(matches!(
            widget.set_name(2, 0),
            Err(UpdateError::RankOutOfRange { component: 'N', .. })
        ));
        assert!(matches!(
            widget.set_name(0, 3),
            Err(UpdateError::IndexOutOfList {
                list: ListName::Names,
                ..
            })
        ));
        assert_eq!(widget.state.names, [0, 2]);
    }

    #[test]
    fn progress_is_clamped_and_must_be_finite() {
        let mut widget = full();
        widget.set_progress(42.5).unwrap();
        assert_eq!(widget.state.progress, 42.5);
        widget.set_progress(150.0).unwrap();
        assert_eq!(widget.state.progress, 100.0);
        widget.set_progress(-3.0).unwrap();
        assert_eq!(widget.state.progress, 0.0);
        widget.set_progress(12.0).unwrap();
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(widget.set_progress(bad), Err(UpdateError::NotFinite));
        }
        assert_eq!(widget.state.progress, 12.0);
        let mut no_progress = LineWidget::builder("L").build().unwrap();
        assert_eq!(
            no_progress.set_progress(1.0),
            Err(UpdateError::NoComponent {
                component: "P or I"
            })
        );
        // `I` alone also takes the progress.
        let mut illustration = LineWidget::builder("I")
            .illustration(Entry::new("#"), Entry::new("."))
            .build()
            .unwrap();
        assert_eq!(illustration.set_progress(1.0), Ok(()));
    }

    #[test]
    fn value_with_unit_and_total() {
        let mut widget = full();
        widget
            .set_value(99.3, Some(1), Some((109.6, Some(1))))
            .unwrap();
        let value = &widget.state.value;
        assert_eq!((value.number.as_str(), value.unit), ("99.3", Some(1)));
        assert_eq!((value.total.as_str(), value.total_unit), ("110", Some(1)));
    }

    #[test]
    fn value_arguments_must_match_the_composition() {
        let mut widget = full();
        widget
            .set_value(1.0, Some(0), Some((2.0, Some(0))))
            .unwrap();
        let errors = [
            (
                widget.set_value(5.0, None, Some((2.0, Some(0)))),
                UpdateError::UnitMismatch { expected: true },
            ),
            (
                widget.set_value(5.0, Some(0), Some((2.0, None))),
                UpdateError::UnitMismatch { expected: true },
            ),
            (
                widget.set_value(5.0, Some(0), None),
                UpdateError::TotalMismatch { expected: true },
            ),
            (
                widget.set_value(5.0, Some(2), Some((2.0, Some(0)))),
                UpdateError::IndexOutOfList {
                    list: ListName::Units,
                    index: 2,
                    len: 2,
                },
            ),
            (
                widget.set_value(f64::NAN, Some(0), Some((2.0, Some(0)))),
                UpdateError::NotFinite,
            ),
            (
                widget.set_value(5.0, Some(0), Some((f64::INFINITY, Some(0)))),
                UpdateError::NotFinite,
            ),
        ];
        for (result, error) in errors {
            assert_eq!(result, Err(error));
        }
        assert_eq!(widget.state.value.number, "1.0");
        assert_eq!(widget.state.value.total, "2.0");

        let mut plain = LineWidget::builder("V3").build().unwrap();
        assert_eq!(
            plain.set_value(1.0, Some(0), None),
            Err(UpdateError::UnitMismatch { expected: false })
        );
        assert_eq!(
            plain.set_value(1.0, None, Some((2.0, None))),
            Err(UpdateError::TotalMismatch { expected: false })
        );
        plain.set_value(-1.25, None, None).unwrap();
        assert_eq!(plain.state.value.number, "-1");
        let mut none = LineWidget::builder("L").build().unwrap();
        assert_eq!(
            none.set_value(1.0, None, None),
            Err(UpdateError::NoComponent { component: "V" })
        );
    }

    #[test]
    fn time_must_be_valid() {
        let mut widget = full();
        widget.set_time(NOON).unwrap();
        assert_eq!(widget.state.time, NOON);
        assert_eq!(
            widget.set_time(WallTime { hour: 24, ..NOON }),
            Err(UpdateError::InvalidTime {
                field: TimeField::Hour
            })
        );
        assert_eq!(widget.state.time, NOON);
        let mut none = LineWidget::builder("L").build().unwrap();
        assert_eq!(
            none.set_time(NOON),
            Err(UpdateError::NoComponent {
                component: "D or T"
            })
        );
    }

    #[test]
    fn log_is_sanitized_and_measured() {
        let mut widget = full();
        widget.set_log("a\tb界\n");
        assert_eq!(widget.state.log, "a b界 ");
        assert_eq!(widget.state.log_width, 6);
        let mut no_log = LineWidget::builder("P").build().unwrap();
        no_log.set_log("kept");
        assert_eq!(no_log.state.log, "kept");
    }

    #[test]
    fn messages() {
        assert_eq!(
            UpdateError::RankOutOfRange {
                component: 'S',
                rank: 3,
                count: 1
            }
            .to_string(),
            "rank 3 out of range: the composition has 1 dynamic S"
        );
        assert_eq!(
            UpdateError::InvalidTime {
                field: TimeField::Day
            }
            .to_string(),
            "invalid time: day out of range"
        );
        assert_eq!(
            UpdateError::NoComponent { component: "V" }.to_string(),
            "the composition has no V"
        );
    }
}
