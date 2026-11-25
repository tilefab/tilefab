//! The line widget: its description, resolved at construction.

use tilefab_core::{Alignment, Style};

use crate::builder::Warning;
use crate::entry::Entry;

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
}

impl LineWidget {
    /// The widget described by `components` and `description`, with no warning yet.
    pub(crate) fn new(components: Vec<Component>, description: Description) -> Self {
        let widest = |list: &[Measured]| list.iter().map(|m| m.width).max().unwrap_or(0);
        let unit_width = match description.value {
            Some(ValueShape { unit: true, .. }) => widest(&description.units),
            _ => 0,
        };
        Self {
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
        }
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
            Component::Date(format) => [10, 8, 5][usize::from(format)],
            Component::Time(format) => [12, 9, 6, 8, 5, 5][usize::from(format)],
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
