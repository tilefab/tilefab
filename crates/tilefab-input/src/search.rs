//! Reverse incremental search in the history (Ctrl+R).

use std::ops::Range;

use crate::History;
use crate::undo::Snapshot;

/// State of a search in progress.
#[derive(Debug)]
pub(crate) struct Search {
    /// What the user typed.
    pub(crate) query: String,
    /// The line before the search, restored by Ctrl+G and Ctrl+C.
    pub(crate) draft: Snapshot,
    /// The matched entry and the byte range of the match in it.
    pub(crate) found: Option<(usize, Range<usize>)>,
    /// The last search found nothing (the line keeps the last match).
    pub(crate) failing: bool,
}

impl Search {
    /// A new search from the line `draft`.
    pub(crate) fn new(draft: Snapshot) -> Self {
        Self {
            query: String::new(),
            draft,
            found: None,
            failing: false,
        }
    }

    /// The prompt shown in place of the application's.
    pub(crate) fn prompt(&self) -> String {
        let state = if self.failing {
            "failing search"
        } else {
            "search"
        };
        format!("({state}) {}: ", self.query)
    }

    /// Look for the query from entry `from` (included) toward older entries, or toward
    /// newer ones with `newer`. On success, the match replaces the current one; otherwise
    /// the search is failing and keeps its last match. Return true on success.
    pub(crate) fn find(&mut self, history: &History, from: usize, newer: bool) -> bool {
        let found = if newer {
            (from..history.len()).find_map(|i| self.match_at(history, i))
        } else {
            (0..=from.min(history.len().saturating_sub(1)))
                .rev()
                .find_map(|i| self.match_at(history, i))
        };
        self.failing = found.is_none();
        if found.is_some() {
            self.found = found;
        }
        !self.failing
    }

    fn match_at(&self, history: &History, index: usize) -> Option<(usize, Range<usize>)> {
        let entry = history.get(index)?;
        find_match(entry, &self.query).map(|range| (index, range))
    }
}

/// Byte range of the first occurrence of `query` in `entry`: case-insensitive, unless
/// `query` contains an upper-case letter (smart case). An empty query matches nothing.
///
/// Case-insensitive: `entry` is lowered once, each byte of the result remembering the
/// character of `entry` it comes from, then searched with `str::find` (linear in the lengths
/// of both). A match starts at the start of a character's lowercase form, and covers every
/// character of `entry` it touches (`ß` matched by `ss`, an `İ` partly matched…).
pub(crate) fn find_match(entry: &str, query: &str) -> Option<Range<usize>> {
    if query.is_empty() {
        return None;
    }
    if query.chars().any(char::is_uppercase) {
        return entry.find(query).map(|start| start..start + query.len());
    }
    let needle: String = query.chars().flat_map(char::to_lowercase).collect();
    let mut lowered = String::with_capacity(entry.len());
    // For each byte of `lowered`: the byte range of the character of `entry` it comes from.
    let mut origin: Vec<Range<usize>> = Vec::with_capacity(entry.len());
    for (start, c) in entry.char_indices() {
        let source = start..start + c.len_utf8();
        for lower in c.to_lowercase() {
            lowered.push(lower);
            origin.resize(lowered.len(), source.clone());
        }
    }
    let mut from = 0;
    while let Some(found) = lowered[from..].find(&needle) {
        let start = from + found;
        // Only at the start of a character's lowercase form (not inside the two characters
        // `İ` lowers to, for instance).
        if start == 0 || origin[start - 1].start != origin[start].start {
            return Some(origin[start].start..origin[start + needle.len() - 1].end);
        }
        // The next character boundary: the needle may match again from there.
        from = start + lowered[start..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(entries: &[&str]) -> History {
        let mut history = History::new(100);
        for entry in entries {
            history.add(entry);
        }
        history
    }

    #[test]
    fn smart_case_matching() {
        assert_eq!(find_match("Start Alpha", "alpha"), Some(6..11));
        assert_eq!(find_match("start alpha", "Alpha"), None);
        assert_eq!(find_match("start Alpha", "Alpha"), Some(6..11));
        assert_eq!(find_match("ÉTÉ été", "été"), Some(0..5));
        assert_eq!(find_match("abc", ""), None);
        assert_eq!(find_match("abc", "abcd"), None);
    }

    /// The former matching, character by character from every start (quadratic): the
    /// reference the linear one must agree with.
    fn find_match_slowly(entry: &str, query: &str) -> Option<Range<usize>> {
        if query.is_empty() {
            return None;
        }
        if query.chars().any(char::is_uppercase) {
            return entry.find(query).map(|start| start..start + query.len());
        }
        let needle: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
        'starts: for (start, _) in entry.char_indices() {
            let mut matched = 0;
            for (offset, c) in entry[start..].char_indices() {
                for lower in c.to_lowercase() {
                    if matched == needle.len() {
                        break;
                    }
                    if lower != needle[matched] {
                        continue 'starts;
                    }
                    matched += 1;
                }
                if matched == needle.len() {
                    return Some(start..start + offset + c.len_utf8());
                }
            }
        }
        None
    }

    #[test]
    fn special_lowercase_forms() {
        // `İ` lowers to `i` + a combining dot (U+0307): a match covers the whole `İ`, and
        // cannot start at its dot.
        assert_eq!(find_match("xİy", "i"), Some(1..3));
        assert_eq!(find_match("xİy", "i\u{307}y"), Some(1..4));
        assert_eq!(find_match("xİy", "\u{307}y"), None);
        assert_eq!(
            find_match("xİy", "\u{307}y"),
            find_match_slowly("xİy", "\u{307}y")
        );
        assert_eq!(find_match("aİİb", "i\u{307}b"), Some(3..6));
    }

    #[test]
    fn a_long_query_on_a_long_entry_is_fast() {
        // Quadratic matching compares about 10^10 characters here: a long freeze.
        let entry = "a".repeat(200_000);
        let query = format!("{}b", "a".repeat(50_000));
        assert_eq!(find_match(&entry, &query), None);
        assert_eq!(
            find_match(&format!("{entry}b"), &query),
            Some(150_000..200_001)
        );
    }

    proptest::proptest! {
        #[test]
        fn the_linear_matching_agrees_with_the_former_one(
            entry in "[aAbBßSsİi\u{307}ΣσςéÉ ]{0,12}",
            query in "[abßsİi\u{307}σςé ]{0,4}",
        ) {
            proptest::prop_assert_eq!(find_match(&entry, &query), find_match_slowly(&entry, &query));
        }
    }

    #[test]
    fn older_and_newer_matches_and_the_failing_state() {
        let history = history(&["start a", "list", "start b", "help"]);
        let mut search = Search::new(Snapshot {
            value: String::new(),
            cursor: 0,
        });
        search.query = "start".to_string();
        assert!(search.find(&history, history.len(), false));
        assert_eq!(search.found, Some((2, 0..5)));
        assert!(search.find(&history, 1, false));
        assert_eq!(search.found, Some((0, 0..5)));
        search.query = "nothing".to_string();
        assert!(!search.find(&history, 3, false));
        assert!(search.failing);
        assert_eq!(search.found, Some((0, 0..5)));
        assert!(search.prompt().starts_with("(failing search) "));
        search.query = "start".to_string();
        assert!(search.find(&history, 1, true));
        assert_eq!(search.found, Some((2, 0..5)));
        assert_eq!(search.prompt(), "(search) start: ");
    }
}
