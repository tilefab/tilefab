//! The text of the line and its cursor: grapheme and word boundaries, primitive edits.
//!
//! Positions are **byte** offsets into the UTF-8 text, always on a grapheme boundary.
//! They never leave this crate: the public cursor of [`crate::LineEditor`] is a grapheme
//! index.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

/// The edited text and its cursor.
///
/// Invariants, kept by every method:
/// - `cursor` is on a grapheme boundary of `text` (0 and `text.len()` included);
/// - `text` contains no control character ([`char::is_control`]);
/// - `text.len()` is at most `max_len` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineBuffer {
    text: String,
    cursor: usize,
    max_len: usize,
}

impl LineBuffer {
    /// An empty line that holds at most `max_len` bytes.
    pub(crate) fn new(max_len: usize) -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            max_len,
        }
    }

    /// The text.
    pub(crate) fn value(&self) -> &str {
        &self.text
    }

    /// The cursor, in bytes.
    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    /// Number of graphemes before the cursor.
    pub(crate) fn cursor_graphemes(&self) -> usize {
        self.text[..self.cursor].graphemes(true).count()
    }

    /// Replace the whole text (sanitized and truncated like a paste) and put the cursor at
    /// `cursor` (clamped, then moved to the next grapheme boundary if needed).
    pub(crate) fn set(&mut self, value: &str, cursor: usize) {
        let value = sanitize(value);
        self.text = truncate(&value, self.max_len).to_string();
        self.cursor = cursor;
        self.snap();
    }

    /// Clear the text.
    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Move the cursor to `position` (clamped, then moved to the next grapheme boundary if
    /// it falls inside a grapheme).
    pub(crate) fn set_cursor(&mut self, position: usize) {
        self.cursor = position;
        self.snap();
    }

    /// Insert `text` at the cursor and move the cursor after it. Control characters are
    /// removed and the text is cut (on a grapheme boundary) so that the line stays within
    /// `max_len`. Return the number of bytes inserted.
    pub(crate) fn insert(&mut self, text: &str) -> usize {
        let at = self.cursor;
        self.replace(at..at, text)
    }

    /// Remove `range` (on grapheme boundaries) and return the removed text. The cursor
    /// keeps its place relative to the remaining text.
    pub(crate) fn remove(&mut self, range: Range<usize>) -> String {
        let removed = self.text[range.clone()].to_string();
        self.text.replace_range(range.clone(), "");
        if self.cursor >= range.end {
            self.cursor -= range.len();
        } else {
            // In the removed range (or before it): at its start (or unchanged).
            self.cursor = self.cursor.min(range.start);
        }
        self.snap();
        removed
    }

    /// Replace `range` with `text` (sanitized and cut to fit `max_len`), and put the cursor
    /// after the inserted text. Return the number of bytes inserted.
    pub(crate) fn replace(&mut self, range: Range<usize>, text: &str) -> usize {
        let text = sanitize(text);
        let room = self.max_len.saturating_sub(self.text.len() - range.len());
        let text = truncate(&text, room);
        self.text.replace_range(range.clone(), text);
        self.cursor = range.start + text.len();
        self.snap();
        text.len()
    }

    /// Restore the cursor invariant: clamp it to the text, and move it to the end of the
    /// grapheme it falls in (an edit can merge graphemes, e.g. two regional indicators
    /// becoming a flag).
    fn snap(&mut self) {
        self.cursor = self.cursor.min(self.text.len());
        for (start, grapheme) in self.text.grapheme_indices(true) {
            let end = start + grapheme.len();
            // Strictly inside the grapheme.
            if (start + 1..end).contains(&self.cursor) {
                self.cursor = end;
                return;
            }
            if end >= self.cursor {
                return;
            }
        }
    }
}

/// `text` without control characters: `\r\n`, `\r`, `\n` and `\t` become one space each,
/// the other control characters are removed.
pub(crate) fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                // `\r\n` is one line break: one space.
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(' ');
            }
            '\n' | '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// The longest prefix of `text` that holds in `max_bytes` and ends on a grapheme boundary.
pub(crate) fn truncate(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = 0;
    for (start, grapheme) in text.grapheme_indices(true) {
        if start + grapheme.len() > max_bytes {
            break;
        }
        end = start + grapheme.len();
    }
    &text[..end]
}

/// True if `position` is a grapheme boundary of `text` (0 and `text.len()` included).
pub(crate) fn is_boundary(text: &str, position: usize) -> bool {
    position == text.len()
        || text
            .grapheme_indices(true)
            .any(|(start, _)| start == position)
}

/// Start of the grapheme before `position` (`position` itself at the start of the text).
pub(crate) fn prev_grapheme(text: &str, position: usize) -> usize {
    text[..position]
        .graphemes(true)
        .next_back()
        .map_or(position, |g| position - g.len())
}

/// End of the grapheme after `position` (`position` itself at the end of the text).
pub(crate) fn next_grapheme(text: &str, position: usize) -> usize {
    text[position..]
        .graphemes(true)
        .next()
        .map_or(position, |g| position + g.len())
}

/// True for a grapheme that belongs to a *word*: it starts with an alphanumeric character
/// or `_` (so a letter followed by combining marks is one word character).
fn is_word(grapheme: &str) -> bool {
    grapheme
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// True for a whitespace grapheme (the separator of *big words*).
fn is_space(grapheme: &str) -> bool {
    grapheme.chars().all(char::is_whitespace)
}

/// Move left from `position`: skip the graphemes for which `inside` is false, then those
/// for which it is true, and return where that stops.
fn skip_left(text: &str, position: usize, inside: impl Fn(&str) -> bool) -> usize {
    let mut graphemes = text[..position].grapheme_indices(true).rev().peekable();
    let mut stop = position;
    while let Some(&(start, g)) = graphemes.peek() {
        if inside(g) {
            break;
        }
        stop = start;
        graphemes.next();
    }
    for (start, g) in graphemes {
        if !inside(g) {
            break;
        }
        stop = start;
    }
    stop
}

/// Start of the word before `position` (readline `backward-word`).
pub(crate) fn word_start_before(text: &str, position: usize) -> usize {
    skip_left(text, position, is_word)
}

/// Start of the big word (whitespace-delimited) before `position` (readline
/// `unix-word-rubout`).
pub(crate) fn big_word_start_before(text: &str, position: usize) -> usize {
    skip_left(text, position, |g| !is_space(g))
}

/// End of the word after `position` (readline `forward-word`: the **end** of the next word,
/// or of the current one when `position` is inside a word).
pub(crate) fn word_end_after(text: &str, position: usize) -> usize {
    let mut graphemes = text[position..].grapheme_indices(true).peekable();
    while graphemes.next_if(|&(_, g)| !is_word(g)).is_some() {}
    while graphemes.next_if(|&(_, g)| is_word(g)).is_some() {}
    graphemes
        .peek()
        .map_or(text.len(), |&(start, _)| position + start)
}

/// True if `text` holds at least one word character.
pub(crate) fn has_word(text: &str) -> bool {
    text.graphemes(true).any(is_word)
}

/// `text` with its first word character in upper case and the rest in lower case.
pub(crate) fn capitalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut seen_word = false;
    for c in text.chars() {
        if seen_word {
            out.extend(c.to_lowercase());
        } else if c.is_alphanumeric() || c == '_' {
            out.extend(c.to_uppercase());
            seen_word = true;
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A buffer holding `text`, with the cursor at byte `cursor`.
    fn buffer(text: &str, cursor: usize) -> LineBuffer {
        let mut b = LineBuffer::new(4096);
        b.set(text, cursor);
        b
    }

    #[test]
    fn replace_at_the_maximum_length_counts_the_replaced_bytes_as_room() {
        let mut b = LineBuffer::new(5);
        b.set("abcde", 5);
        assert_eq!(b.replace(0..2, "xy"), 2);
        assert_eq!(b.value(), "xycde");
        // One byte more than the room: cut.
        assert_eq!(b.replace(0..1, "123"), 1);
        assert_eq!(b.value(), "1ycde");
    }

    #[test]
    fn remove_puts_a_cursor_inside_the_range_at_its_start() {
        let mut b = buffer("abcdef", 3);
        b.remove(1..5);
        assert_eq!((b.value(), b.cursor()), ("af", 1));
        let mut b = buffer("abcdef", 1);
        b.remove(2..4);
        assert_eq!(b.cursor(), 1);
    }

    /// The byte offsets where each word movement stops, applied repeatedly from one end.
    fn stops(text: &str, step: impl Fn(&str, usize) -> usize, from: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut position = from;
        loop {
            let next = step(text, position);
            if next == position {
                return out;
            }
            out.push(next);
            position = next;
        }
    }

    #[test]
    fn insert_at_the_start_middle_and_end() {
        let mut b = buffer("ac", 1);
        assert_eq!(b.insert("b"), 1);
        assert_eq!((b.value(), b.cursor()), ("abc", 2));
        b.set_cursor(0);
        b.insert(">");
        assert_eq!((b.value(), b.cursor()), (">abc", 1));
        b.set_cursor(b.value().len());
        b.insert("<");
        assert_eq!((b.value(), b.cursor()), (">abc<", 5));
    }

    #[test]
    fn insert_into_an_empty_line() {
        let mut b = LineBuffer::new(16);
        b.insert("é");
        assert_eq!((b.value(), b.cursor(), b.cursor_graphemes()), ("é", 2, 1));
    }

    #[test]
    fn control_characters_never_enter_the_text() {
        let mut b = LineBuffer::new(64);
        b.insert("a\u{7}b\u{1b}[31mc\r\nd\te\nf\rg");
        assert_eq!(b.value(), "ab[31mc d e f g");
        assert!(!b.value().chars().any(char::is_control));
    }

    #[test]
    fn the_line_never_exceeds_max_len_and_is_cut_on_a_grapheme_boundary() {
        let mut b = LineBuffer::new(5);
        // "é" is 2 bytes: "aéé" is 5 bytes, a third "é" does not fit.
        assert_eq!(b.insert("aééé"), 5);
        assert_eq!(b.value(), "aéé");
        assert_eq!(b.insert("x"), 0);
        // A ZWJ emoji (17 bytes) is never cut in half.
        let mut b = LineBuffer::new(10);
        assert_eq!(b.insert("ab🤦🏼\u{200d}♂\u{fe0f}"), 2);
        assert_eq!(b.value(), "ab");
    }

    #[test]
    fn remove_keeps_the_cursor_relative_to_the_text() {
        let mut b = buffer("abcdef", 4);
        assert_eq!(b.remove(0..2), "ab");
        assert_eq!((b.value(), b.cursor()), ("cdef", 2));
        assert_eq!(b.remove(1..3), "de");
        assert_eq!((b.value(), b.cursor()), ("cf", 1));
        assert_eq!(b.remove(1..2), "f");
        assert_eq!((b.value(), b.cursor()), ("c", 1));
    }

    #[test]
    fn replace_puts_the_cursor_after_the_new_text() {
        let mut b = buffer("hello world", 0);
        b.replace(6..11, "there");
        assert_eq!((b.value(), b.cursor()), ("hello there", 11));
    }

    #[test]
    fn grapheme_steps_over_unicode_clusters() {
        // Combining sequence, ZWJ emoji, CJK, a flag (two regional indicators), Hangul jamo.
        let text = "e\u{301}🤦🏼\u{200d}♂\u{fe0f}漢🇫🇷\u{1100}\u{1161}";
        let forward = stops(text, next_grapheme, 0);
        assert_eq!(forward, vec![3, 20, 23, 31, 37]);
        let backward = stops(text, prev_grapheme, text.len());
        assert_eq!(backward, vec![31, 23, 20, 3, 0]);
    }

    #[test]
    fn the_cursor_is_snapped_to_a_grapheme_boundary() {
        // Byte 1 is inside "e\u{301}" (the combining accent starts at byte 1).
        let b = buffer("e\u{301}x", 1);
        assert_eq!(b.cursor(), 3);
        // Inserting a regional indicator next to another one merges them into a flag:
        // the cursor moves to the end of the flag.
        let mut b = buffer("\u{1f1eb}", 0);
        b.insert("\u{1f1f7}");
        assert_eq!(b.value().graphemes(true).count(), 1);
        assert_eq!(b.cursor(), b.value().len());
    }

    #[test]
    fn words_are_alphanumeric_runs() {
        let text = "foo-bar baz";
        assert_eq!(stops(text, word_end_after, 0), vec![3, 7, 11]);
        assert_eq!(stops(text, word_start_before, text.len()), vec![8, 4, 0]);
        assert_eq!(stops("a  b", word_end_after, 0), vec![1, 4]);
        assert_eq!(stops("snake_case x", word_end_after, 0), vec![10, 12]);
        // Non-ASCII punctuation is a separator: there is no word at all.
        assert_eq!(stops("…", word_end_after, 0), vec![3]);
        assert_eq!(stops("…", word_start_before, 3), vec![0]);
        // The apostrophe separates "l" from "été"; accents belong to the word.
        assert_eq!(stops("l'été", word_end_after, 0), vec![1, 7]);
        assert_eq!(stops("l'e\u{301}t", word_end_after, 2), vec![6]);
        // CJK characters are alphanumeric.
        assert_eq!(stops("漢字 x", word_end_after, 0), vec![6, 8]);
    }

    #[test]
    fn big_words_are_whitespace_delimited() {
        let text = "foo-bar baz";
        assert_eq!(stops(text, big_word_start_before, text.len()), vec![8, 0]);
        assert_eq!(stops("a  b  ", big_word_start_before, 6), vec![3, 0]);
        assert_eq!(stops("l'été …", big_word_start_before, 11), vec![8, 0]);
    }

    #[test]
    fn sanitize_turns_line_breaks_and_tabs_into_single_spaces() {
        assert_eq!(sanitize("a\r\nb\rc\nd\te"), "a b c d e");
        assert_eq!(sanitize("\r\n\r\n"), "  ");
        assert_eq!(sanitize("x\u{0}\u{7f}\u{9b}y"), "xy");
    }

    #[test]
    fn capitalize_upcases_the_first_word_character_only() {
        assert_eq!(capitalize("  hELLO"), "  Hello");
        assert_eq!(capitalize("-éTÉ"), "-Été");
        assert_eq!(capitalize("..."), "...");
    }

    #[test]
    fn has_word_detects_word_characters() {
        assert!(has_word(" a "));
        assert!(!has_word(" - … "));
    }
}
