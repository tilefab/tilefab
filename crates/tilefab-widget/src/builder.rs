//! Construction of a [`LineWidget`]: the builder, its errors and its warnings.

use std::fmt;

use tilefab_core::{Alignment, Style};

pub use crate::composition::TokenProblem;
use crate::composition::{self, Look, Token};
use crate::entry::{Entry, EntryProblem};
use crate::widget::{Component, Description, LineWidget, Measured, Slot, Styles, ValueShape};

/// Most entries of the `symbols`, `barriers`, `keywords` and `units` lists (indices 0–9).
pub const MAX_ENTRIES: usize = 10;
/// Most entries of the `names` list (indices 0–99).
pub const MAX_NAMES: usize = 100;
/// Most decimals of the numbers of `V` ([`LineWidgetBuilder::value_decimals`]).
pub const MAX_VALUE_DECIMALS: u8 = 9;
/// Default target width of [`LineWidgetBuilder::target_width`].
pub const DEFAULT_TARGET_WIDTH: usize = 80;

/// Builder of a [`LineWidget`], made by [`LineWidget::builder`].
///
/// Every setting is checked by [`build`](Self::build), which reports the first problem.
/// A list given but not used by the composition is accepted and ignored.
#[derive(Debug, Clone)]
#[must_use]
pub struct LineWidgetBuilder {
    composition: String,
    symbols: Option<Vec<Entry>>,
    barriers: Option<Vec<Entry>>,
    keywords: Option<Vec<Entry>>,
    names: Option<Vec<Entry>>,
    units: Option<Vec<Entry>>,
    illustration: Option<(Entry, Entry)>,
    styles: Styles,
    keyword_alignment: Alignment,
    value_decimals: u8,
    target_width: usize,
}

impl LineWidget {
    /// Start building a widget from its composition, for instance `"B0 T2 B1 B2 S B2 L"`.
    pub fn builder(composition: impl Into<String>) -> LineWidgetBuilder {
        LineWidgetBuilder {
            composition: composition.into(),
            symbols: None,
            barriers: None,
            keywords: None,
            names: None,
            units: None,
            illustration: None,
            styles: Styles::default(),
            keyword_alignment: Alignment::Left,
            value_decimals: 1,
            target_width: DEFAULT_TARGET_WIDTH,
        }
    }
}

impl LineWidgetBuilder {
    /// The symbols of `S` (at most 10): each one a single grapheme, 1 or 2 columns wide.
    pub fn symbols(mut self, symbols: impl IntoIterator<Item = Entry>) -> Self {
        self.symbols = Some(symbols.into_iter().collect());
        self
    }

    /// The barriers of `B` (at most 10): any non-empty text.
    pub fn barriers(mut self, barriers: impl IntoIterator<Item = Entry>) -> Self {
        self.barriers = Some(barriers.into_iter().collect());
        self
    }

    /// The keywords of `K` (at most 10).
    pub fn keywords(mut self, keywords: impl IntoIterator<Item = Entry>) -> Self {
        self.keywords = Some(keywords.into_iter().collect());
        self
    }

    /// The names of `N` (at most 100).
    pub fn names(mut self, names: impl IntoIterator<Item = Entry>) -> Self {
        self.names = Some(names.into_iter().collect());
        self
    }

    /// The units of `V` with `U` (at most 10), glued to the numbers.
    pub fn units(mut self, units: impl IntoIterator<Item = Entry>) -> Self {
        self.units = Some(units.into_iter().collect());
        self
    }

    /// The two characters of `I`, each exactly 1 column wide: `done` fills the part of the
    /// width reached by the progress, `todo` the rest. Without them, `I` is a bar (`█`/`░`).
    ///
    /// When the composition gives a look to `I` (`IB`, `IL`, `I20C`…: the letters are listed in
    /// the crate documentation), the look picks the characters and only the styles of `done`
    /// and `todo` are kept.
    pub fn illustration(mut self, done: Entry, todo: Entry) -> Self {
        self.illustration = Some((done, todo));
        self
    }

    /// Style of the date (`D`).
    pub fn date_style(mut self, style: Style) -> Self {
        self.styles.date = style;
        self
    }

    /// Style of the time (`T`).
    pub fn time_style(mut self, style: Style) -> Self {
        self.styles.time = style;
        self
    }

    /// Style of the progress (`P`: the number and `%`).
    pub fn progress_style(mut self, style: Style) -> Self {
        self.styles.progress = style;
        self
    }

    /// Style of the log text (`L`).
    pub fn log_style(mut self, style: Style) -> Self {
        self.styles.log = style;
        self
    }

    /// Styles of the value (`V`): the numbers, and the `/` before the total. The units have
    /// the styles of their entries.
    pub fn value_styles(mut self, numbers: Style, slash: Style) -> Self {
        self.styles.value_numbers = numbers;
        self.styles.value_slash = slash;
        self
    }

    /// Alignment of the keyword (`K`) in the width of the longest keyword: left (default),
    /// right or centered (the extra space goes to the right of the word).
    pub fn keyword_alignment(mut self, alignment: Alignment) -> Self {
        self.keyword_alignment = alignment;
        self
    }

    /// Number of decimals of the numbers of `V`, 0–9 (1 by default).
    pub fn value_decimals(mut self, decimals: u8) -> Self {
        self.value_decimals = decimals;
        self
    }

    /// Width above which the nominal width of the widget gives a
    /// [`Warning::WiderThanTarget`] (80 by default).
    pub fn target_width(mut self, width: usize) -> Self {
        self.target_width = width;
        self
    }

    /// Check everything and build the widget.
    ///
    /// # Errors
    ///
    /// The first [`BuildError`] found: in the composition, then in the lists, then in the
    /// fixed indices, then in the illustration and the number of decimals.
    pub fn build(self) -> Result<LineWidget, BuildError> {
        let painted = composition::parse(&self.composition)?;
        let tokens: Vec<Token> = painted.iter().map(|p| p.token).collect();
        let uses = |f: fn(&Token) -> bool| tokens.iter().any(f);

        let mut description = Description {
            symbols: checked_list(
                ListName::Symbols,
                self.symbols,
                uses(|t| matches!(t, Token::Symbol(_))),
                MAX_ENTRIES,
                Entry::check_symbol,
            )?,
            barriers: checked_list(
                ListName::Barriers,
                self.barriers,
                uses(|t| matches!(t, Token::Barrier(_))),
                MAX_ENTRIES,
                Entry::check_text,
            )?,
            keywords: checked_list(
                ListName::Keywords,
                self.keywords,
                uses(|t| matches!(t, Token::Keyword(_))),
                MAX_ENTRIES,
                Entry::check_text,
            )?,
            names: checked_list(
                ListName::Names,
                self.names,
                uses(|t| matches!(t, Token::Name(_))),
                MAX_NAMES,
                Entry::check_text,
            )?,
            units: checked_list(
                ListName::Units,
                self.units,
                uses(|t| matches!(t, Token::Value { unit: true, .. })),
                MAX_ENTRIES,
                Entry::check_text,
            )?,
            styles: self.styles,
            keyword_alignment: self.keyword_alignment,
            value_decimals: self.value_decimals,
            ..Description::default()
        };
        let components = resolve(&tokens, &mut description)?;
        description.paints = painted.iter().map(|p| p.paint).collect();
        let look = tokens.iter().find_map(|t| match t {
            Token::Illustration { look, .. } => Some(*look),
            _ => None,
        });
        if let Some(look) = look {
            description.illustration = Some(checked_illustration(look, self.illustration)?);
        }
        if self.value_decimals > MAX_VALUE_DECIMALS {
            return Err(BuildError::DecimalsOutOfRange {
                decimals: self.value_decimals,
            });
        }

        let mut widget = LineWidget::new(components, description);
        let nominal = widget.nominal_width();
        if nominal > self.target_width {
            widget.warnings.push(Warning::WiderThanTarget {
                nominal,
                target: self.target_width,
            });
        }
        Ok(widget)
    }
}

/// Turn the tokens into components: check the fixed indices against the lists of
/// `description`, give the dynamic components their ranks and record the shape of `V`.
fn resolve(tokens: &[Token], description: &mut Description) -> Result<Vec<Component>, BuildError> {
    let mut components = Vec::with_capacity(tokens.len());
    for (index, &token) in tokens.iter().enumerate() {
        let fixed = |list: ListName, entries: &[Measured], i: u8| {
            if usize::from(i) < entries.len() {
                Ok(Slot::Fixed(i))
            } else {
                Err(BuildError::IndexOutOfList {
                    position: index + 1,
                    token: token.to_string(),
                    list,
                    len: entries.len(),
                })
            }
        };
        let dynamic = |count: &mut usize| {
            *count += 1;
            Slot::Dynamic(*count - 1)
        };
        components.push(match token {
            Token::Symbol(Some(i)) => {
                Component::Symbol(fixed(ListName::Symbols, &description.symbols, i)?)
            }
            Token::Symbol(None) => Component::Symbol(dynamic(&mut description.dynamic_symbols)),
            Token::Barrier(i) => {
                fixed(ListName::Barriers, &description.barriers, i)?;
                Component::Barrier(i)
            }
            Token::Keyword(Some(i)) => {
                Component::Keyword(fixed(ListName::Keywords, &description.keywords, i)?)
            }
            Token::Keyword(None) => {
                description.dynamic_keyword = true;
                Component::Keyword(Slot::Dynamic(0))
            }
            Token::Name(Some(i)) => Component::Name(fixed(ListName::Names, &description.names, i)?),
            Token::Name(None) => Component::Name(dynamic(&mut description.dynamic_names)),
            Token::Date(format) => Component::Date(format),
            Token::Time(format) => Component::Time(format),
            Token::Progress(size) => Component::Progress(size),
            Token::Log(size) => Component::Log(size),
            Token::Illustration { size, .. } => Component::Illustration(size),
            Token::Value { size, unit, total } => {
                description.value = Some(ValueShape { size, unit, total });
                Component::Value
            }
        });
    }
    Ok(components)
}

/// The two characters of `I`: those of its `look` with the styles of the builder's, else the
/// builder's, else the bar; each exactly 1 column wide.
fn checked_illustration(
    look: Option<Look>,
    illustration: Option<(Entry, Entry)>,
) -> Result<(Entry, Entry), BuildError> {
    let (done, todo) = match (look, illustration) {
        (None, Some(given)) => given,
        (look, given) => {
            let (done, todo) = look.unwrap_or(Look::Bar).characters();
            let (done_style, todo_style) = given
                .map(|(done, todo)| (done.style(), todo.style()))
                .unwrap_or_default();
            (
                Entry::styled(done, done_style),
                Entry::styled(todo, todo_style),
            )
        }
    };
    for (index, entry) in [&done, &todo].into_iter().enumerate() {
        entry
            .check_one_column()
            .map_err(|problem| BuildError::InvalidEntry {
                list: ListName::Illustration,
                index,
                problem,
            })?;
    }
    Ok((done, todo))
}

/// Check a list: required when used, not too long, every entry valid. An unused list is
/// dropped without being checked.
fn checked_list(
    list: ListName,
    entries: Option<Vec<Entry>>,
    used: bool,
    max: usize,
    check: fn(&Entry) -> Result<usize, EntryProblem>,
) -> Result<Vec<Measured>, BuildError> {
    if !used {
        return Ok(Vec::new());
    }
    let entries = entries.unwrap_or_default();
    if entries.is_empty() {
        return Err(BuildError::MissingList { list });
    }
    if entries.len() > max {
        return Err(BuildError::TooManyEntries {
            list,
            count: entries.len(),
            max,
        });
    }
    entries
        .into_iter()
        .enumerate()
        .map(|(index, entry)| match check(&entry) {
            Ok(width) => Ok(Measured { entry, width }),
            Err(problem) => Err(BuildError::InvalidEntry {
                list,
                index,
                problem,
            }),
        })
        .collect()
}

/// A list of values given to the builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ListName {
    /// [`LineWidgetBuilder::symbols`].
    Symbols,
    /// [`LineWidgetBuilder::barriers`].
    Barriers,
    /// [`LineWidgetBuilder::keywords`].
    Keywords,
    /// [`LineWidgetBuilder::names`].
    Names,
    /// [`LineWidgetBuilder::units`].
    Units,
    /// The two characters of [`LineWidgetBuilder::illustration`] (`done` is entry 0).
    Illustration,
}

impl fmt::Display for ListName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Symbols => "symbols",
            Self::Barriers => "barriers",
            Self::Keywords => "keywords",
            Self::Names => "names",
            Self::Units => "units",
            Self::Illustration => "illustration",
        })
    }
}

/// Why a [`LineWidget`] could not be built.
///
/// Positions of tokens count from 1, in composition order; indices of entries count from 0,
/// as in the composition.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BuildError {
    /// The composition is empty.
    EmptyComposition,
    /// A leading, trailing or double space, or whitespace other than U+0020, at byte
    /// `offset` of the composition.
    BadSpacing {
        /// Byte offset of the problem in the composition.
        offset: usize,
    },
    /// A token that the grammar does not accept.
    BadToken {
        /// Position of the token (from 1).
        position: usize,
        /// The text of the token.
        token: String,
        /// What is wrong with it.
        problem: TokenProblem,
    },
    /// A unique component (`D`, `T`, `P`, `K`, `L`, `V`, `I`) that appears a second time.
    RepeatedComponent {
        /// Position of the second occurrence (from 1).
        position: usize,
        /// The text of the token.
        token: String,
    },
    /// A list used by the composition that is missing or empty.
    MissingList {
        /// The list.
        list: ListName,
    },
    /// A list with more entries than its indices can reach.
    TooManyEntries {
        /// The list.
        list: ListName,
        /// Number of entries given.
        count: usize,
        /// Most entries accepted.
        max: usize,
    },
    /// A fixed index (`S3`, `B4`, `K2`, `N14`) beyond the end of its list.
    IndexOutOfList {
        /// Position of the token (from 1).
        position: usize,
        /// The text of the token.
        token: String,
        /// The list.
        list: ListName,
        /// Number of entries of the list.
        len: usize,
    },
    /// An entry that cannot be drawn.
    InvalidEntry {
        /// The list.
        list: ListName,
        /// Index of the entry (from 0).
        index: usize,
        /// What is wrong with it.
        problem: EntryProblem,
    },
    /// [`LineWidgetBuilder::value_decimals`] above 9.
    DecimalsOutOfRange {
        /// The number of decimals given.
        decimals: u8,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyComposition => f.write_str("empty composition"),
            Self::BadSpacing { offset } => write!(
                f,
                "bad spacing at byte {offset}: tokens are separated by exactly one space"
            ),
            Self::BadToken {
                position,
                token,
                problem,
            } => write!(f, "token {position} ({token:?}): {problem}"),
            Self::RepeatedComponent { position, token } => write!(
                f,
                "token {position} ({token:?}): component {} can appear only once",
                token.chars().next().unwrap_or(' ')
            ),
            Self::MissingList { list } => write!(
                f,
                "the {list} list is used by the composition but missing or empty"
            ),
            Self::TooManyEntries { list, count, max } => {
                write!(f, "the {list} list has {count} entries, at most {max}")
            }
            Self::IndexOutOfList {
                position,
                token,
                list,
                len,
            } => write!(
                f,
                "token {position} ({token:?}): index out of the {list} list (length {len})"
            ),
            Self::InvalidEntry {
                list,
                index,
                problem,
            } => write!(f, "entry {index} of the {list} list {problem}"),
            Self::DecimalsOutOfRange { decimals } => {
                write!(f, "{decimals} value decimals: at most {MAX_VALUE_DECIMALS}")
            }
        }
    }
}

impl std::error::Error for BuildError {}

/// A problem that does not prevent drawing, found by
/// [`LineWidgetBuilder::build`] and listed by [`LineWidget::warnings`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Warning {
    /// The nominal width of the widget is larger than its target width
    /// ([`LineWidgetBuilder::target_width`]). Nominal width: the fixed components, plus `L`
    /// and `I` at their nominal width (an elastic `L` counts as 2, an elastic `I` as 10),
    /// plus each `N` at its widest possible entry.
    WiderThanTarget {
        /// Nominal width, in columns.
        nominal: usize,
        /// Target width, in columns.
        target: usize,
    },
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WiderThanTarget { nominal, target } => write!(
                f,
                "the widget is {nominal} columns wide, more than its target width of {target}"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(texts: &[&str]) -> Vec<Entry> {
        texts.iter().copied().map(Entry::new).collect()
    }

    fn error(builder: LineWidgetBuilder) -> BuildError {
        builder.build().expect_err("the build should fail")
    }

    /// The example of the specification.
    fn example() -> LineWidgetBuilder {
        LineWidget::builder("B0 T2 B1 B2 S B2 L")
            .symbols(entries(&["🔴", "🟠", "🟡", "🟢"]))
            .barriers(entries(&["[", "]", " ", "-"]))
    }

    #[test]
    fn the_example_builds_without_warning() {
        let widget = example().build().unwrap();
        assert_eq!(widget.warnings(), []);
        assert_eq!(widget.components.len(), 7);
    }

    #[test]
    fn composition_errors_come_first() {
        assert_eq!(error(LineWidget::builder("")), BuildError::EmptyComposition);
        assert!(matches!(
            error(LineWidget::builder("S X")),
            BuildError::BadToken { position: 2, .. }
        ));
    }

    #[test]
    fn missing_or_empty_lists() {
        let cases = [
            ("S", ListName::Symbols),
            ("B0", ListName::Barriers),
            ("K", ListName::Keywords),
            ("N", ListName::Names),
            ("VU", ListName::Units),
        ];
        for (composition, list) in cases {
            assert_eq!(
                error(LineWidget::builder(composition)),
                BuildError::MissingList { list },
                "{composition}"
            );
        }
        assert_eq!(
            error(LineWidget::builder("S").symbols([])),
            BuildError::MissingList {
                list: ListName::Symbols
            }
        );
    }

    #[test]
    fn lists_not_used_are_not_checked() {
        let widget = LineWidget::builder("V5 P")
            .symbols(entries(&["", "ab"]))
            .names([])
            .units(entries(&["a"; 20]))
            .illustration(Entry::new("界"), Entry::new(""))
            .build();
        assert!(widget.is_ok());
    }

    #[test]
    fn too_many_entries() {
        let ten = entries(&["x"; 10]);
        let eleven = entries(&["x"; 11]);
        assert!(LineWidget::builder("S").symbols(ten).build().is_ok());
        assert_eq!(
            error(LineWidget::builder("B0").barriers(eleven)),
            BuildError::TooManyEntries {
                list: ListName::Barriers,
                count: 11,
                max: 10
            }
        );
        assert!(
            LineWidget::builder("N")
                .names(entries(&["n"; 100]))
                .build()
                .is_ok()
        );
        assert!(matches!(
            error(LineWidget::builder("N").names(entries(&["n"; 101]))),
            BuildError::TooManyEntries {
                list: ListName::Names,
                max: 100,
                ..
            }
        ));
    }

    #[test]
    fn fixed_index_out_of_its_list() {
        let cases = [
            (
                LineWidget::builder("S S2").symbols(entries(&["a", "b"])),
                2,
                ListName::Symbols,
            ),
            (
                LineWidget::builder("B1").barriers(entries(&["a"])),
                1,
                ListName::Barriers,
            ),
            (
                LineWidget::builder("K3").keywords(entries(&["a"])),
                1,
                ListName::Keywords,
            ),
            (
                LineWidget::builder("N N14").names(entries(&["a"])),
                2,
                ListName::Names,
            ),
        ];
        for (builder, position, list) in cases {
            assert!(matches!(
                error(builder),
                BuildError::IndexOutOfList { position: p, list: l, .. } if p == position && l == list
            ));
        }
        assert!(
            LineWidget::builder("N14")
                .names(entries(&["n"; 15]))
                .build()
                .is_ok()
        );
    }

    #[test]
    fn invalid_entries() {
        assert_eq!(
            error(LineWidget::builder("B0").barriers(entries(&["[", ""]))),
            BuildError::InvalidEntry {
                list: ListName::Barriers,
                index: 1,
                problem: EntryProblem::Empty
            }
        );
        assert_eq!(
            error(LineWidget::builder("N").names(entries(&["a\nb"]))),
            BuildError::InvalidEntry {
                list: ListName::Names,
                index: 0,
                problem: EntryProblem::ControlCharacter
            }
        );
        assert_eq!(
            error(LineWidget::builder("S").symbols(entries(&["ok"]))),
            BuildError::InvalidEntry {
                list: ListName::Symbols,
                index: 0,
                problem: EntryProblem::NotASymbol
            }
        );
        assert!(matches!(
            error(LineWidget::builder("K").keywords(entries(&["\u{7}"]))),
            BuildError::InvalidEntry {
                list: ListName::Keywords,
                ..
            }
        ));
        assert!(matches!(
            error(LineWidget::builder("VU").units(entries(&["\t"]))),
            BuildError::InvalidEntry {
                list: ListName::Units,
                ..
            }
        ));
    }

    #[test]
    fn illustration_characters() {
        assert_eq!(
            error(LineWidget::builder("I5").illustration(Entry::new("█"), Entry::new("界"))),
            BuildError::InvalidEntry {
                list: ListName::Illustration,
                index: 1,
                problem: EntryProblem::NotOneColumn
            }
        );
        assert!(
            LineWidget::builder("I")
                .illustration(Entry::new("━"), Entry::new("─"))
                .build()
                .is_ok()
        );
    }

    #[test]
    fn value_decimals_range() {
        assert!(LineWidget::builder("V").value_decimals(9).build().is_ok());
        assert_eq!(
            error(LineWidget::builder("V").value_decimals(10)),
            BuildError::DecimalsOutOfRange { decimals: 10 }
        );
    }

    #[test]
    fn fixed_widths() {
        // Each composition holds one component: its nominal width is the component's.
        let width = |composition: &str| {
            let widget = LineWidget::builder(composition)
                .symbols(entries(&["a", "界"]))
                .keywords(entries(&["ok", "fail", "x"]))
                .barriers(entries(&[" - "]))
                .units(entries(&["b", "KB", "MiB"]))
                .build()
                .unwrap();
            widget.nominal_width()
        };
        assert_eq!(width("S"), 2);
        assert_eq!(width("K"), 4);
        assert_eq!(width("K1"), 4);
        assert_eq!(width("B0"), 3);
        for (composition, expected) in [("D0", 10), ("D1", 8), ("D2", 5)] {
            assert_eq!(width(composition), expected, "{composition}");
        }
        for (composition, expected) in [
            ("T0", 12),
            ("T1", 9),
            ("T2", 6),
            ("T3", 8),
            ("T4", 5),
            ("T5", 5),
        ] {
            assert_eq!(width(composition), expected, "{composition}");
        }
        assert_eq!(width("P"), 4);
        assert_eq!(width("P9"), 10);
        assert_eq!(width("V"), 5);
        assert_eq!(width("V/"), 11);
        assert_eq!(width("V5"), 5);
        assert_eq!(width("V5U"), 8);
        assert_eq!(width("V5/"), 11);
        assert_eq!(width("V5U/"), 17);
    }

    #[test]
    fn warning_exactly_when_the_nominal_width_exceeds_the_target() {
        // B0 (1) + L elastic (2) + I elastic (10) + N widest (5) = 18.
        let builder = |target| {
            LineWidget::builder("B0 L I N")
                .barriers(entries(&["["]))
                .names(entries(&["ab", "abcde"]))
                .illustration(Entry::new("#"), Entry::new("."))
                .target_width(target)
        };
        assert_eq!(builder(18).build().unwrap().warnings(), []);
        assert_eq!(
            builder(17).build().unwrap().warnings(),
            [Warning::WiderThanTarget {
                nominal: 18,
                target: 17
            }]
        );
        // A fixed N counts its own entry, fixed L and I their size.
        let fixed = LineWidget::builder("N0 L5 I3")
            .names(entries(&["ab", "abcde"]))
            .illustration(Entry::new("#"), Entry::new("."))
            .target_width(0)
            .build()
            .unwrap();
        assert_eq!(
            fixed.warnings(),
            [Warning::WiderThanTarget {
                nominal: 10,
                target: 0
            }]
        );
    }

    #[test]
    fn the_default_target_width_is_80() {
        let widget = |size: u16| {
            LineWidget::builder(format!("L{size}"))
                .build()
                .unwrap()
                .warnings()
                .len()
        };
        assert_eq!(widget(80), 0);
        assert_eq!(widget(81), 1);
    }

    #[test]
    fn messages() {
        assert_eq!(
            BuildError::MissingList {
                list: ListName::Symbols
            }
            .to_string(),
            "the symbols list is used by the composition but missing or empty"
        );
        assert_eq!(
            error(LineWidget::builder("B3").barriers(entries(&["["]))).to_string(),
            "token 1 (\"B3\"): index out of the barriers list (length 1)"
        );
        assert_eq!(
            error(LineWidget::builder("B0").barriers(entries(&["a\tb"]))).to_string(),
            "entry 0 of the barriers list contains a control character"
        );
        assert_eq!(
            Warning::WiderThanTarget {
                nominal: 90,
                target: 80
            }
            .to_string(),
            "the widget is 90 columns wide, more than its target width of 80"
        );
    }
}
