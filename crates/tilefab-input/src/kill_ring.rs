//! The kill ring: text removed by the kill commands, pasted back by Ctrl+Y / Alt+Y.

use std::collections::VecDeque;

/// Maximum number of entries kept.
const MAX_ENTRIES: usize = 16;

/// Which side of the cursor a kill removed text from: it decides how consecutive kills
/// are merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    /// Text before the cursor: prepended to the previous kill.
    Left,
    /// Text after the cursor: appended to the previous kill.
    Right,
}

/// The killed texts, newest first.
#[derive(Debug, Clone, Default)]
pub(crate) struct KillRing {
    entries: VecDeque<String>,
}

impl KillRing {
    /// Record killed `text`. With `chain` (the previous command was also a kill), it is
    /// merged into the newest entry instead of starting a new one.
    pub(crate) fn push(&mut self, text: &str, side: Side, chain: bool) {
        if text.is_empty() {
            return;
        }
        if chain && let Some(newest) = self.entries.front_mut() {
            match side {
                Side::Left => newest.insert_str(0, text),
                Side::Right => newest.push_str(text),
            }
            return;
        }
        self.entries.push_front(text.to_string());
        self.entries.truncate(MAX_ENTRIES);
    }

    /// The entry `index` steps back from the newest one, wrapping around.
    pub(crate) fn get(&self, index: usize) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }
        self.entries
            .get(index % self.entries.len())
            .map(String::as_str)
    }

    /// Number of entries.
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chained_kills_are_merged_by_side() {
        let mut ring = KillRing::default();
        ring.push("world", Side::Left, false);
        ring.push("hello ", Side::Left, true);
        assert_eq!(ring.get(0), Some("hello world"));
        ring.push("!", Side::Right, true);
        assert_eq!(ring.get(0), Some("hello world!"));
        assert_eq!(ring.len(), 1);
        ring.push("next", Side::Right, false);
        assert_eq!(
            (ring.get(0), ring.get(1)),
            (Some("next"), Some("hello world!"))
        );
    }

    #[test]
    fn the_ring_keeps_the_newest_entries_and_wraps_around() {
        let mut ring = KillRing::default();
        for i in 0..20 {
            ring.push(&i.to_string(), Side::Right, false);
        }
        assert_eq!(ring.len(), MAX_ENTRIES);
        assert_eq!(ring.get(0), Some("19"));
        assert_eq!(ring.get(MAX_ENTRIES - 1), Some("4"));
        assert_eq!(ring.get(MAX_ENTRIES), Some("19"));
    }

    #[test]
    fn empty_kills_are_not_recorded() {
        let mut ring = KillRing::default();
        ring.push("", Side::Left, false);
        assert_eq!(ring.get(0), None);
    }
}
