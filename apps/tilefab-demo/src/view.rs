//! How a task is drawn in the viewport: the widget, variant and border chosen with `select`.
//!
//! The selection applies to the tasks started afterwards: each task keeps the view it was
//! started with, so several views can be compared side by side.

/// The word after the variant that surrounds the widget with a border.
const BORDERED: &str = "bordered";

/// A widget and one of its variants.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// `ProgressLine` with `█`/`░` (the original look).
    #[default]
    LineBlocks,
    /// `ProgressLine` with `━`/`─`.
    LineThin,
    /// `ProgressLine` with `●`/`·`.
    LineDots,
    /// `ProgressBar` drawn with background colors.
    Bar,
}

/// How a task is drawn: a look, with or without a rounded border titled with the task name.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TaskView {
    pub look: Look,
    pub bordered: bool,
}

/// A look as `select` names it. For each widget, the first variant is its default.
struct LookSpec {
    widget: &'static str,
    variant: &'static str,
    look: Look,
}

const LOOKS: &[LookSpec] = &[
    LookSpec {
        widget: "progressline",
        variant: "blocks",
        look: Look::LineBlocks,
    },
    LookSpec {
        widget: "progressline",
        variant: "thin",
        look: Look::LineThin,
    },
    LookSpec {
        widget: "progressline",
        variant: "dots",
        look: Look::LineDots,
    },
    LookSpec {
        widget: "progressbar",
        variant: "plain",
        look: Look::Bar,
    },
];

/// Why `select` arguments name no view.
#[derive(Debug, PartialEq, Eq)]
pub enum ViewError {
    UnknownWidget(String),
    UnknownVariant {
        widget: String,
        variant: String,
    },
    /// A word after the variant other than `bordered`.
    UnknownOption(String),
    /// More than a widget, a variant and `bordered`.
    TooManyArguments,
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownWidget(widget) => write!(f, "unknown widget: \"{widget}\""),
            Self::UnknownVariant { widget, variant } => {
                write!(f, "unknown variant of {widget}: \"{variant}\"")
            }
            Self::UnknownOption(option) => {
                write!(f, "unknown option: \"{option}\" (only {BORDERED})")
            }
            Self::TooManyArguments => f.write_str("usage: select <widget> [variant] [bordered]"),
        }
    }
}

impl TaskView {
    /// `look` without a border.
    #[cfg(test)]
    pub const fn new(look: Look) -> Self {
        Self {
            look,
            bordered: false,
        }
    }

    /// The same view, with a border.
    #[cfg(test)]
    pub const fn with_border(self) -> Self {
        Self {
            bordered: true,
            ..self
        }
    }

    /// Parse `<widget> [variant] [bordered]` (without the variant: the widget's default one).
    pub fn parse(args: &str) -> Result<Self, ViewError> {
        let mut words = args.split_whitespace();
        let widget = words.next().unwrap_or_default();
        let mut specs = LOOKS.iter().filter(|spec| spec.widget == widget).peekable();
        let Some(default) = specs.peek().map(|spec| spec.look) else {
            return Err(ViewError::UnknownWidget(widget.to_string()));
        };
        let mut next = words.next();
        let look = match next {
            Some(variant) if variant != BORDERED => {
                next = words.next();
                specs
                    .find(|spec| spec.variant == variant)
                    .map(|spec| spec.look)
                    .ok_or_else(|| ViewError::UnknownVariant {
                        widget: widget.to_string(),
                        variant: variant.to_string(),
                    })?
            }
            _ => default,
        };
        let bordered = match next {
            None => false,
            Some(BORDERED) => true,
            Some(option) => return Err(ViewError::UnknownOption(option.to_string())),
        };
        if words.next().is_some() {
            return Err(ViewError::TooManyArguments);
        }
        Ok(Self { look, bordered })
    }

    fn spec(self) -> &'static LookSpec {
        LOOKS
            .iter()
            .find(|spec| spec.look == self.look)
            .expect("every look is listed in LOOKS")
    }

    /// `widget variant [bordered]`, as typed after `select`.
    pub fn name(self) -> String {
        let spec = self.spec();
        let border = if self.bordered { " bordered" } else { "" };
        format!("{} {}{border}", spec.widget, spec.variant)
    }

    /// True for the views drawn in the shared name column (`progressline` without a border:
    /// name, bar, percentage).
    pub fn uses_name_column(self) -> bool {
        !self.bordered
            && matches!(
                self.look,
                Look::LineBlocks | Look::LineThin | Look::LineDots
            )
    }

    /// Rows the task takes in the viewport.
    pub fn height(self) -> u16 {
        if self.bordered { 3 } else { 1 }
    }

    /// The widget names, in order, without duplicates.
    pub fn widgets() -> Vec<&'static str> {
        let mut widgets: Vec<&str> = LOOKS.iter().map(|spec| spec.widget).collect();
        widgets.dedup();
        widgets
    }

    /// The variants of `widget` (none for an unknown widget).
    pub fn variants(widget: &str) -> impl Iterator<Item = &'static str> {
        LOOKS
            .iter()
            .filter(move |spec| spec.widget == widget)
            .map(|spec| spec.variant)
    }

    /// The words that can follow `words` (the complete words typed after `select`).
    pub fn next_words(words: &[&str]) -> Vec<&'static str> {
        match words {
            [] => Self::widgets(),
            [widget] if Self::variants(widget).next().is_some() => {
                Self::variants(widget).chain([BORDERED]).collect()
            }
            [widget, variant] if Self::variants(widget).any(|v| v == *variant) => {
                vec![BORDERED]
            }
            _ => Vec::new(),
        }
    }

    /// `progressline [blocks|thin|dots], progressbar [plain], then [bordered]`.
    pub fn summary() -> String {
        let widgets: Vec<String> = Self::widgets()
            .into_iter()
            .map(|widget| {
                let variants: Vec<&str> = Self::variants(widget).collect();
                format!("{widget} [{}]", variants.join("|"))
            })
            .collect();
        format!("{}, then [{BORDERED}]", widgets.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_view_parses_from_its_name() {
        for spec in LOOKS {
            let view = TaskView::new(spec.look);
            assert_eq!(TaskView::parse(&view.name()), Ok(view));
            let view = view.with_border();
            assert_eq!(TaskView::parse(&view.name()), Ok(view));
        }
    }

    #[test]
    fn the_variant_defaults_to_the_first_one() {
        assert_eq!(
            TaskView::parse("progressline"),
            Ok(TaskView::new(Look::LineBlocks))
        );
        assert_eq!(
            TaskView::parse(" progressbar "),
            Ok(TaskView::new(Look::Bar))
        );
        assert_eq!(
            TaskView::parse("progressbar bordered"),
            Ok(TaskView::new(Look::Bar).with_border())
        );
        assert_eq!(TaskView::default(), TaskView::new(Look::LineBlocks));
    }

    #[test]
    fn parse_errors() {
        assert_eq!(
            TaskView::parse("table"),
            Err(ViewError::UnknownWidget("table".into()))
        );
        assert_eq!(
            TaskView::parse("progressbar dots"),
            Err(ViewError::UnknownVariant {
                widget: "progressbar".into(),
                variant: "dots".into()
            })
        );
        assert_eq!(
            TaskView::parse("progressline dots x"),
            Err(ViewError::UnknownOption("x".into()))
        );
        assert_eq!(
            TaskView::parse("progressline dots bordered x"),
            Err(ViewError::TooManyArguments)
        );
        assert_eq!(
            TaskView::parse("progressline bordered bordered"),
            Err(ViewError::TooManyArguments)
        );
    }

    #[test]
    fn the_border_adds_two_rows_and_leaves_the_name_column() {
        let line = TaskView::new(Look::LineThin);
        assert_eq!((line.height(), line.uses_name_column()), (1, true));
        let line = line.with_border();
        assert_eq!((line.height(), line.uses_name_column()), (3, false));
        assert!(!TaskView::new(Look::Bar).uses_name_column());
    }

    #[test]
    fn next_words_follow_the_syntax() {
        const NONE: [&str; 0] = [];
        assert_eq!(TaskView::next_words(&[]), ["progressline", "progressbar"]);
        assert_eq!(
            TaskView::next_words(&["progressbar"]),
            ["plain", "bordered"]
        );
        assert_eq!(
            TaskView::next_words(&["progressline", "dots"]),
            ["bordered"]
        );
        assert_eq!(TaskView::next_words(&["table"]), NONE);
        assert_eq!(TaskView::next_words(&["progressline", "bordered"]), NONE);
        assert_eq!(
            TaskView::next_words(&["progressline", "dots", "bordered"]),
            NONE
        );
    }

    #[test]
    fn summary_lists_widgets_variants_and_the_border() {
        assert_eq!(
            TaskView::summary(),
            "progressline [blocks|thin|dots], progressbar [plain], then [bordered]"
        );
    }
}
