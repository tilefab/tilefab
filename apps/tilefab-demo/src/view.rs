//! How a task is drawn in the viewport: the widget and variant chosen with `select`.
//!
//! The selection applies to the tasks started afterwards: each task keeps the view it was
//! started with, so several views can be compared side by side.

/// A widget and one of its variants, used to draw a task.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TaskView {
    /// `ProgressLine` with `█`/`░` (the original look).
    #[default]
    LineBlocks,
    /// `ProgressLine` with `━`/`─`.
    LineThin,
    /// `ProgressLine` with `●`/`·`.
    LineDots,
    /// One-row `ProgressBar`, name and percentage as its label.
    BarPlain,
    /// `ProgressBar` in a rounded `Block` titled with the name.
    BarBordered,
}

/// A view as `select` names it. For each widget, the first variant is its default.
struct ViewSpec {
    widget: &'static str,
    variant: &'static str,
    view: TaskView,
}

const VIEWS: &[ViewSpec] = &[
    ViewSpec {
        widget: "progressline",
        variant: "blocks",
        view: TaskView::LineBlocks,
    },
    ViewSpec {
        widget: "progressline",
        variant: "thin",
        view: TaskView::LineThin,
    },
    ViewSpec {
        widget: "progressline",
        variant: "dots",
        view: TaskView::LineDots,
    },
    ViewSpec {
        widget: "progressbar",
        variant: "plain",
        view: TaskView::BarPlain,
    },
    ViewSpec {
        widget: "progressbar",
        variant: "bordered",
        view: TaskView::BarBordered,
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
    /// More than a widget and a variant.
    TooManyArguments,
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownWidget(widget) => write!(f, "unknown widget: \"{widget}\""),
            Self::UnknownVariant { widget, variant } => {
                write!(f, "unknown variant of {widget}: \"{variant}\"")
            }
            Self::TooManyArguments => f.write_str("usage: select <widget> [variant]"),
        }
    }
}

impl TaskView {
    /// Parse `<widget> [variant]` (without the variant: the widget's default one).
    pub fn parse(args: &str) -> Result<Self, ViewError> {
        let mut words = args.split_whitespace();
        let widget = words.next().unwrap_or_default();
        let variant = words.next();
        if words.next().is_some() {
            return Err(ViewError::TooManyArguments);
        }
        let mut specs = VIEWS.iter().filter(|spec| spec.widget == widget).peekable();
        if specs.peek().is_none() {
            return Err(ViewError::UnknownWidget(widget.to_string()));
        }
        match variant {
            None => specs.next(),
            Some(variant) => specs.find(|spec| spec.variant == variant),
        }
        .map(|spec| spec.view)
        .ok_or_else(|| ViewError::UnknownVariant {
            widget: widget.to_string(),
            variant: variant.unwrap_or_default().to_string(),
        })
    }

    fn spec(self) -> &'static ViewSpec {
        VIEWS
            .iter()
            .find(|spec| spec.view == self)
            .expect("every view is listed in VIEWS")
    }

    /// `widget variant`, as typed after `select`.
    pub fn name(self) -> String {
        let spec = self.spec();
        format!("{} {}", spec.widget, spec.variant)
    }

    /// True for the `progressline` views (name column, bar, percentage).
    pub fn is_line(self) -> bool {
        matches!(self, Self::LineBlocks | Self::LineThin | Self::LineDots)
    }

    /// Rows the task takes in the viewport.
    pub fn height(self) -> u16 {
        match self {
            Self::LineBlocks | Self::LineThin | Self::LineDots | Self::BarPlain => 1,
            Self::BarBordered => 3,
        }
    }

    /// The widget names, in order, without duplicates.
    pub fn widgets() -> Vec<&'static str> {
        let mut widgets: Vec<&str> = VIEWS.iter().map(|spec| spec.widget).collect();
        widgets.dedup();
        widgets
    }

    /// The variants of `widget` (none for an unknown widget).
    pub fn variants(widget: &str) -> impl Iterator<Item = &'static str> {
        VIEWS
            .iter()
            .filter(move |spec| spec.widget == widget)
            .map(|spec| spec.variant)
    }

    /// `progressline [blocks|thin|dots], progressbar [plain|bordered]`.
    pub fn summary() -> String {
        let widgets: Vec<String> = Self::widgets()
            .into_iter()
            .map(|widget| {
                let variants: Vec<&str> = Self::variants(widget).collect();
                format!("{widget} [{}]", variants.join("|"))
            })
            .collect();
        widgets.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_view_parses_from_its_name() {
        for spec in VIEWS {
            assert_eq!(TaskView::parse(&spec.view.name()), Ok(spec.view));
        }
    }

    #[test]
    fn the_variant_defaults_to_the_first_one() {
        assert_eq!(TaskView::parse("progressline"), Ok(TaskView::LineBlocks));
        assert_eq!(TaskView::parse(" progressbar "), Ok(TaskView::BarPlain));
        assert_eq!(TaskView::default(), TaskView::LineBlocks);
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
            Err(ViewError::TooManyArguments)
        );
    }

    #[test]
    fn summary_lists_widgets_and_variants() {
        assert_eq!(
            TaskView::summary(),
            "progressline [blocks|thin|dots], progressbar [plain|bordered]"
        );
    }
}
