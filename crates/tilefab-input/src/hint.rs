//! Hints (autosuggestions): text proposed after the line, accepted with → or End.

use crate::History;

/// Source of hints.
pub trait Hinter {
    /// The text to show **after** `line` (the cursor is at its end), or `None`.
    fn hint(&self, line: &str, history: &History) -> Option<String>;
}

/// The default hinter: the rest of the newest history entry that starts with the line
/// and is longer than it.
///
/// ```
/// use tilefab_input::{Hinter, History, HistoryHinter};
///
/// let mut history = History::new(10);
/// history.add("start alpha");
/// history.add("start beta");
/// assert_eq!(HistoryHinter.hint("start", &history).as_deref(), Some(" beta"));
/// assert_eq!(HistoryHinter.hint("start a", &history).as_deref(), Some("lpha"));
/// assert_eq!(HistoryHinter.hint("start beta", &history), None);
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct HistoryHinter;

impl Hinter for HistoryHinter {
    fn hint(&self, line: &str, history: &History) -> Option<String> {
        if line.is_empty() {
            return None;
        }
        history
            .iter()
            .rev()
            .find(|entry| entry.len() > line.len() && entry.starts_with(line))
            .map(|entry| entry[line.len()..].to_string())
    }
}
