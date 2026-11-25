//! A value of one of the lists of a [`LineWidget`](crate::LineWidget): a text and its style.

use std::fmt;

use tilefab_core::{Style, grapheme_width};
use unicode_segmentation::UnicodeSegmentation;

/// One entry of a list given to the builder (symbols, barriers, keywords, names, units, the
/// characters of the illustration): a text and the style it is always drawn with.
///
/// The text is checked by [`LineWidgetBuilder::build`](crate::LineWidgetBuilder::build): it
/// must not be empty nor contain a control character; a symbol must be a single grapheme
/// 1 or 2 columns wide, a character of the illustration exactly 1 column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The text shown.
    text: String,
    /// Its style.
    style: Style,
}

impl Entry {
    /// An entry drawn with [`Style::default()`].
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self::styled(text, Style::default())
    }

    /// An entry drawn with `style`.
    #[must_use]
    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    /// The text of the entry.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The style of the entry.
    #[must_use]
    pub fn style(&self) -> Style {
        self.style
    }

    /// Check the text of a list entry and return its width in columns.
    pub(crate) fn check_text(&self) -> Result<usize, EntryProblem> {
        if self.text.chars().any(char::is_control) {
            return Err(EntryProblem::ControlCharacter);
        }
        match text_width(&self.text) {
            0 => Err(EntryProblem::Empty),
            width => Ok(width),
        }
    }

    /// Check a symbol (one grapheme, 1 or 2 columns) and return its width.
    pub(crate) fn check_symbol(&self) -> Result<usize, EntryProblem> {
        let width = self.check_text()?;
        if self.text.graphemes(true).count() == 1 {
            Ok(width)
        } else {
            Err(EntryProblem::NotASymbol)
        }
    }

    /// Check a character of the illustration (one grapheme, exactly 1 column).
    pub(crate) fn check_one_column(&self) -> Result<(), EntryProblem> {
        match self.check_text()? {
            1 if self.text.graphemes(true).count() == 1 => Ok(()),
            _ => Err(EntryProblem::NotOneColumn),
        }
    }
}

/// Width of `text` in columns, with the rule of [`tilefab_core::Buffer`]: the sum of the
/// [`grapheme_width`] of its graphemes.
pub(crate) fn text_width(text: &str) -> usize {
    text.graphemes(true).map(grapheme_width).sum()
}

/// What is wrong with an [`Entry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntryProblem {
    /// The text is empty, or has no visible character.
    Empty,
    /// The text contains a control character (tab, line feed, escape…).
    ControlCharacter,
    /// A symbol must be a single grapheme 1 or 2 columns wide.
    NotASymbol,
    /// A character of the illustration must be a single grapheme exactly 1 column wide.
    NotOneColumn,
}

impl fmt::Display for EntryProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "is empty or has no visible character",
            Self::ControlCharacter => "contains a control character",
            Self::NotASymbol => "is not a single grapheme 1 or 2 columns wide",
            Self::NotOneColumn => "is not a single grapheme exactly 1 column wide",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tilefab_core::Color;

    #[test]
    fn constructors_keep_text_and_style() {
        let style = Style::new().fg(Color::Red);
        assert_eq!(Entry::new("a").style(), Style::default());
        let entry = Entry::styled("ab", style);
        assert_eq!((entry.text(), entry.style()), ("ab", style));
    }

    #[test]
    fn text_width_follows_the_buffer() {
        assert_eq!(text_width("abc"), 3);
        assert_eq!(text_width("界"), 2);
        assert_eq!(text_width("🔴"), grapheme_width("🔴"));
        assert_eq!(text_width("e\u{301}"), 1);
        assert_eq!(text_width(""), 0);
    }

    #[test]
    fn list_entries() {
        assert_eq!(Entry::new(" - ").check_text(), Ok(3));
        assert_eq!(Entry::new("").check_text(), Err(EntryProblem::Empty));
        assert_eq!(Entry::new("\u{301}").check_text(), Err(EntryProblem::Empty));
        assert_eq!(
            Entry::new("a\tb").check_text(),
            Err(EntryProblem::ControlCharacter)
        );
        assert_eq!(
            Entry::new("\u{1b}[31m").check_text(),
            Err(EntryProblem::ControlCharacter)
        );
    }

    #[test]
    fn symbols() {
        assert_eq!(Entry::new("x").check_symbol(), Ok(1));
        assert_eq!(Entry::new("🔴").check_symbol(), Ok(grapheme_width("🔴")));
        assert_eq!(Entry::new("✔︎").check_symbol(), Ok(grapheme_width("✔︎")));
        assert_eq!(
            Entry::new("ab").check_symbol(),
            Err(EntryProblem::NotASymbol)
        );
        assert_eq!(Entry::new("").check_symbol(), Err(EntryProblem::Empty));
    }

    #[test]
    fn characters_of_the_illustration() {
        assert_eq!(Entry::new("█").check_one_column(), Ok(()));
        assert_eq!(
            Entry::new("界").check_one_column(),
            Err(EntryProblem::NotOneColumn)
        );
        assert_eq!(
            Entry::new("ab").check_one_column(),
            Err(EntryProblem::NotOneColumn)
        );
        // One column wide, but two graphemes (a zero-width space after the letter).
        let two = Entry::new("a\u{200b}");
        assert_eq!(two.text().graphemes(true).count(), 2);
        assert_eq!(two.check_text(), Ok(1));
        assert_eq!(two.check_one_column(), Err(EntryProblem::NotOneColumn));
    }
}
