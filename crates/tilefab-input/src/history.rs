//! The history of submitted lines.

use std::collections::VecDeque;

/// Default number of entries kept.
pub(crate) const DEFAULT_CAPACITY: usize = 1000;

/// Submitted lines, oldest first. Every line appears once, at its latest position.
#[derive(Debug)]
pub struct History {
    entries: VecDeque<String>,
    capacity: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl History {
    /// An empty history that keeps at most `capacity` entries.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity,
        }
    }

    /// Maximum number of entries kept.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if there is no entry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry at `index`, 0 being the oldest.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(String::as_str)
    }

    /// The entries, oldest first.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &str> + ExactSizeIterator {
        self.entries.iter().map(String::as_str)
    }

    /// Add `line` as the newest entry; an older identical entry is removed. Empty and
    /// whitespace-only lines, and lines with a control character, are not recorded.
    /// Return true if the line was recorded.
    pub fn add(&mut self, line: &str) -> bool {
        if line.trim().is_empty() || line.chars().any(char::is_control) || self.capacity == 0 {
            return false;
        }
        self.entries.retain(|entry| entry != line);
        self.entries.push_back(line.to_string());
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(history: &History) -> Vec<&str> {
        history.iter().collect()
    }

    #[test]
    fn duplicates_keep_only_the_latest_position() {
        let mut history = History::new(10);
        for line in ["a", "b", "a", "c", "b"] {
            assert!(history.add(line));
        }
        assert_eq!(entries(&history), ["a", "c", "b"]);
    }

    #[test]
    fn empty_whitespace_and_control_lines_are_not_recorded() {
        let mut history = History::new(10);
        assert!(!history.add(""));
        assert!(!history.add("   "));
        assert!(!history.add("a\u{7}"));
        assert!(history.is_empty());
    }

    #[test]
    fn the_oldest_entries_are_dropped_beyond_the_capacity() {
        let mut history = History::new(2);
        for line in ["a", "b", "c"] {
            history.add(line);
        }
        assert_eq!(entries(&history), ["b", "c"]);
        assert_eq!((history.get(0), history.get(2)), (Some("b"), None));
        let mut none = History::new(0);
        assert!(!none.add("a"));
    }
}
