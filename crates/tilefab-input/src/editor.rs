//! The line editor: dispatch of commands to the line, the kill ring and the undo stack.

use std::ops::Range;

use crossterm::event::KeyEvent;
use tilefab_core::{Buffer, Line, Position, Rect};

use crate::buffer::{self, LineBuffer};
use crate::keymap::{Command, Keymap};
use crate::kill_ring::{KillRing, Side};
use crate::render::{self, EditorStyles, LineView};
use crate::undo::{Snapshot, Step, UndoStack};

/// Default maximum length of the line, in bytes.
const DEFAULT_MAX_LEN: usize = 4096;

/// Settings of a [`LineEditor`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorConfig {
    /// Maximum length of the line, in bytes (default 4096): a huge paste cannot exhaust
    /// memory.
    pub max_len: usize,
    /// Columns of text kept visible on each side of the cursor when the line scrolls
    /// (default 4; reduced on narrow widths).
    pub scroll_margin: u16,
    /// Styles of the hint, the indicators, the search match and the menu.
    pub styles: EditorStyles,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            max_len: DEFAULT_MAX_LEN,
            scroll_margin: 4,
            styles: EditorStyles::default(),
        }
    }
}

/// What the application must do after a key or a paste.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum EditorEvent {
    /// Nothing changed (the key is not an editing key, or the command had no effect).
    Ignored,
    /// The line, the cursor or the menu changed: redraw.
    Changed,
    /// Enter: the submitted line. The editor is empty again.
    Submit(String),
    /// Ctrl+C in normal mode.
    Interrupt,
    /// Ctrl+D on an empty line.
    Eof,
}

/// What the previous command was, for the commands whose effect depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Last {
    /// Anything else.
    Other,
    /// A kill: the next kill is merged into the same kill ring entry.
    Kill,
    /// A yank of kill ring entry `index` into `range`: `YankPop` replaces it.
    Yank { range: (usize, usize), index: usize },
}

/// A single-line editor with readline-style editing keys.
///
/// ```
/// use tilefab_input::{Command, EditorConfig, EditorEvent, LineEditor};
///
/// let mut editor = LineEditor::new(EditorConfig::default());
/// for c in "hi".chars() {
///     assert_eq!(editor.execute(Command::InsertChar(c)), EditorEvent::Changed);
/// }
/// assert_eq!((editor.value(), editor.cursor()), ("hi", 2));
/// assert_eq!(editor.execute(Command::Submit), EditorEvent::Submit("hi".to_string()));
/// assert_eq!(editor.value(), "");
/// ```
///
/// The cursor is a **grapheme** index: `"🤦🏼‍♂️"` is 17 bytes, 5 code points, 1 grapheme and
/// 2 display columns.
#[derive(Debug, Clone)]
pub struct LineEditor {
    config: EditorConfig,
    buffer: LineBuffer,
    keymap: Keymap,
    kill_ring: KillRing,
    undo: UndoStack,
    last: Last,
    /// First visible column of the line, kept between renders.
    scroll: usize,
}

impl LineEditor {
    /// An empty editor.
    #[must_use]
    pub fn new(config: EditorConfig) -> Self {
        Self {
            buffer: LineBuffer::new(config.max_len),
            config,
            keymap: Keymap::emacs(),
            kill_ring: KillRing::default(),
            undo: UndoStack::default(),
            last: Last::Other,
            scroll: 0,
        }
    }

    /// The settings the editor was created with.
    #[must_use]
    pub fn config(&self) -> &EditorConfig {
        &self.config
    }

    /// The text of the line.
    #[must_use]
    pub fn value(&self) -> &str {
        self.buffer.value()
    }

    /// The cursor: number of graphemes before it.
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.buffer.cursor_graphemes()
    }

    /// Replace the line with `value` (control characters removed, cut to the maximum
    /// length), cursor at the end. It is one undo step.
    pub fn set_value(&mut self, value: &str) {
        let before = self.snapshot();
        self.undo.close_group();
        self.buffer.set(value, usize::MAX);
        let _ = self.finish_edit(before, Step::Single);
    }

    /// The key bindings, to change them.
    pub fn keymap_mut(&mut self) -> &mut Keymap {
        &mut self.keymap
    }

    /// Handle a key: look it up in the keymap and run its command. Keys that are not
    /// editing keys (key releases, unbound shortcuts) are [`EditorEvent::Ignored`].
    pub fn handle_key(&mut self, key: KeyEvent) -> EditorEvent {
        match self.keymap.command_for(key) {
            Some(command) => self.execute(command),
            None => EditorEvent::Ignored,
        }
    }

    /// Draw the prompt and the line on the first row of `area` in `buf`, and return the
    /// cursor cell (`None` when `area` is empty). Text hidden by the horizontal scroll is
    /// shown by a dim `…`.
    ///
    /// With a `Frame`, place the cursor it returns:
    ///
    /// ```ignore
    /// if let Some(cursor) = editor.render(area, frame.buffer_mut(), &prompt) {
    ///     frame.set_cursor_position(cursor);
    /// }
    /// ```
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, prompt: &Line<'_>) -> Option<Position> {
        let area = area.intersection(buf.area());
        if area.is_empty() {
            return None;
        }
        let row = Rect::new(area.x, area.y, area.width, 1);
        let view = LineView {
            text: self.buffer.value(),
            cursor: self.buffer.cursor(),
            hint: None,
            highlight: None,
        };
        Some(render::draw_line(
            buf,
            row,
            prompt,
            &view,
            &mut self.scroll,
            self.config.scroll_margin,
            &self.config.styles,
        ))
    }

    /// Run `command`.
    pub fn execute(&mut self, command: Command) -> EditorEvent {
        let last = std::mem::replace(&mut self.last, Last::Other);
        if !matches!(command, Command::InsertChar(c) if !c.is_whitespace()) {
            self.undo.close_group();
        }
        self.edit_command(command, last)
    }

    /// Run an editing `command` in normal mode; `last` is the previous command.
    fn edit_command(&mut self, command: Command, last: Last) -> EditorEvent {
        let text = self.buffer.value();
        let cursor = self.buffer.cursor();
        let len = text.len();
        match command {
            Command::InsertChar(c) => {
                if c.is_control() {
                    return EditorEvent::Ignored;
                }
                let step = if c.is_whitespace() {
                    Step::Single
                } else {
                    Step::Word
                };
                self.edit(step, |b| {
                    b.insert(c.encode_utf8(&mut [0; 4]));
                })
            }
            Command::MoveLeft => self.move_to(buffer::prev_grapheme(text, cursor)),
            Command::MoveRight => self.move_to(buffer::next_grapheme(text, cursor)),
            Command::MoveWordLeft => self.move_to(buffer::word_start_before(text, cursor)),
            Command::MoveWordRight => self.move_to(buffer::word_end_after(text, cursor)),
            Command::MoveStart => self.move_to(0),
            Command::MoveEnd => self.move_to(len),
            Command::DeleteLeft => self.delete(buffer::prev_grapheme(text, cursor)..cursor),
            Command::DeleteRight => self.delete(cursor..buffer::next_grapheme(text, cursor)),
            Command::DeleteRightOrEof if text.is_empty() => EditorEvent::Eof,
            Command::DeleteRightOrEof => self.edit_command(Command::DeleteRight, last),
            Command::KillWordLeft => {
                let start = buffer::word_start_before(text, cursor);
                self.kill(start..cursor, Side::Left, last)
            }
            Command::KillBigWordLeft => {
                let start = buffer::big_word_start_before(text, cursor);
                self.kill(start..cursor, Side::Left, last)
            }
            Command::KillWordRight => {
                let end = buffer::word_end_after(text, cursor);
                self.kill(cursor..end, Side::Right, last)
            }
            Command::KillToStart => self.kill(0..cursor, Side::Left, last),
            Command::KillToEnd => self.kill(cursor..len, Side::Right, last),
            Command::KillLine => self.kill(0..len, Side::Right, last),
            Command::Yank => self.yank(None),
            Command::YankPop => match last {
                Last::Yank { range, index } => self.yank(Some((range.0..range.1, index))),
                _ => EditorEvent::Ignored,
            },
            Command::TransposeChars => self.transpose_chars(),
            Command::TransposeWords => self.transpose_words(),
            Command::UpcaseWord => self.change_case(str::to_uppercase),
            Command::DowncaseWord => self.change_case(str::to_lowercase),
            Command::CapitalizeWord => self.change_case(buffer::capitalize),
            Command::Undo => {
                let current = self.snapshot();
                self.undo
                    .undo(current)
                    .map_or(EditorEvent::Ignored, |s| self.restore(&s))
            }
            Command::Redo => {
                let current = self.snapshot();
                self.undo
                    .redo(current)
                    .map_or(EditorEvent::Ignored, |s| self.restore(&s))
            }
            Command::Submit => self.submit(),
            Command::Interrupt => EditorEvent::Interrupt,
            Command::HistoryPrevious
            | Command::HistoryNext
            | Command::HistoryPreviousAll
            | Command::HistoryNextAll
            | Command::HistoryFirst
            | Command::HistoryLast
            | Command::ReverseSearch
            | Command::ForwardSearch
            | Command::Complete
            | Command::CompletePrevious
            | Command::Cancel
            | Command::Abort => EditorEvent::Ignored,
        }
    }

    /// The line as it is now.
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            value: self.buffer.value().to_string(),
            cursor: self.buffer.cursor(),
        }
    }

    /// Restore a snapshot (undo / redo).
    fn restore(&mut self, snapshot: &Snapshot) -> EditorEvent {
        self.buffer.set(&snapshot.value, snapshot.cursor);
        EditorEvent::Changed
    }

    /// Apply `change` to the line as one edit recorded with `step`.
    fn edit(&mut self, step: Step, change: impl FnOnce(&mut LineBuffer)) -> EditorEvent {
        let before = self.snapshot();
        change(&mut self.buffer);
        self.finish_edit(before, step)
    }

    /// After an edit: record it for undo if the text changed, and say what changed.
    fn finish_edit(&mut self, before: Snapshot, step: Step) -> EditorEvent {
        if self.buffer.value() != before.value {
            self.undo.record(before, step);
            EditorEvent::Changed
        } else if self.buffer.cursor() == before.cursor {
            EditorEvent::Ignored
        } else {
            EditorEvent::Changed
        }
    }

    /// Move the cursor (not an edit: not recorded for undo).
    fn move_to(&mut self, position: usize) -> EditorEvent {
        if position == self.buffer.cursor() {
            return EditorEvent::Ignored;
        }
        self.buffer.set_cursor(position);
        EditorEvent::Changed
    }

    /// Delete `range` without saving it.
    fn delete(&mut self, range: Range<usize>) -> EditorEvent {
        if range.is_empty() {
            return EditorEvent::Ignored;
        }
        self.edit(Step::Single, |b| {
            b.remove(range);
        })
    }

    /// Delete `range` and save it in the kill ring, merged with the previous kill if the
    /// previous command was one.
    fn kill(&mut self, range: Range<usize>, side: Side, last: Last) -> EditorEvent {
        self.last = Last::Kill;
        if range.is_empty() {
            return EditorEvent::Ignored;
        }
        let before = self.snapshot();
        let killed = self.buffer.remove(range);
        self.kill_ring.push(&killed, side, last == Last::Kill);
        self.finish_edit(before, Step::Single)
    }

    /// Yank: insert the newest kill ring entry at the cursor, or, for a yank-pop, replace
    /// the text yanked at `range` with the entry before `index`.
    fn yank(&mut self, previous: Option<(Range<usize>, usize)>) -> EditorEvent {
        let (range, index) = match previous {
            Some((range, index)) => (range, index + 1),
            None => (self.buffer.cursor()..self.buffer.cursor(), 0),
        };
        let Some(text) = self.kill_ring.get(index).map(str::to_string) else {
            return EditorEvent::Ignored;
        };
        let start = range.start;
        let before = self.snapshot();
        self.buffer.replace(range, &text);
        let index = index % self.kill_ring.len();
        self.last = Last::Yank {
            range: (start, self.buffer.cursor()),
            index,
        };
        self.finish_edit(before, Step::Single)
    }

    /// Swap the two graphemes around the cursor (at the end: the last two); the cursor
    /// ends after them.
    fn transpose_chars(&mut self) -> EditorEvent {
        let text = self.buffer.value();
        let cursor = self.buffer.cursor();
        let (middle, end) = if cursor == text.len() {
            (buffer::prev_grapheme(text, cursor), cursor)
        } else {
            (cursor, buffer::next_grapheme(text, cursor))
        };
        let start = buffer::prev_grapheme(text, middle);
        if start == middle || middle == end {
            return EditorEvent::Ignored;
        }
        let swapped = format!("{}{}", &text[middle..end], &text[start..middle]);
        self.edit(Step::Single, |b| {
            b.replace(start..end, &swapped);
        })
    }

    /// Swap the word before the cursor with the word after it (at the end of the line:
    /// the last two words); the cursor ends after them.
    fn transpose_words(&mut self) -> EditorEvent {
        let text = self.buffer.value();
        let cursor = self.buffer.cursor();
        let mut second_end = buffer::word_end_after(text, cursor);
        if !buffer::has_word(&text[cursor..second_end]) {
            // No word after the cursor: use the last word of the line.
            let last_start = buffer::word_start_before(text, text.len());
            second_end = buffer::word_end_after(text, last_start);
        }
        let second_start = buffer::word_start_before(text, second_end);
        let first_start = buffer::word_start_before(text, second_start);
        if first_start == second_start || !buffer::has_word(&text[first_start..second_start]) {
            return EditorEvent::Ignored;
        }
        let first_end = buffer::word_end_after(text, first_start);
        let swapped = format!(
            "{}{}{}",
            &text[second_start..second_end],
            &text[first_end..second_start],
            &text[first_start..first_end]
        );
        self.edit(Step::Single, |b| {
            b.replace(first_start..second_end, &swapped);
        })
    }

    /// Apply `convert` from the cursor to the end of the current or next word; the
    /// cursor ends after it.
    fn change_case(&mut self, convert: impl Fn(&str) -> String) -> EditorEvent {
        let text = self.buffer.value();
        let cursor = self.buffer.cursor();
        let end = buffer::word_end_after(text, cursor);
        let converted = convert(&text[cursor..end]);
        self.edit(Step::Single, |b| {
            b.replace(cursor..end, &converted);
        })
    }

    /// Enter: hand the line over and start a new, empty one.
    fn submit(&mut self) -> EditorEvent {
        let line = self.buffer.value().to_string();
        self.buffer.clear();
        self.undo.clear();
        self.scroll = 0;
        EditorEvent::Submit(line)
    }
}

#[cfg(test)]
// The command tables of the tests read better without the `Command::` prefix.
#[allow(clippy::enum_glob_use)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use proptest::prelude::*;

    /// An editor holding `text`, with the cursor at the byte offset of the `|` in it.
    fn editor(text_with_cursor: &str) -> LineEditor {
        let cursor = text_with_cursor
            .find('|')
            .expect("test text needs a | cursor");
        let text = text_with_cursor.replace('|', "");
        let mut editor = LineEditor::new(EditorConfig::default());
        editor.buffer.set(&text, cursor);
        editor
    }

    /// The editor's text with a `|` at the cursor.
    fn shown(editor: &LineEditor) -> String {
        let mut text = editor.value().to_string();
        text.insert(editor.buffer.cursor(), '|');
        text
    }

    /// Run `command` on `before` and return the event and the resulting line.
    fn run(before: &str, command: Command) -> (EditorEvent, String) {
        let mut e = editor(before);
        let event = e.execute(command);
        (event, shown(&e))
    }

    fn type_text(editor: &mut LineEditor, text: &str) {
        for c in text.chars() {
            let _ = editor.execute(Command::InsertChar(c));
        }
    }

    #[test]
    fn movements_at_the_start_middle_and_end() {
        use Command::*;
        let changed = EditorEvent::Changed;
        let ignored = EditorEvent::Ignored;
        let cases = [
            (MoveLeft, "ab|c", changed.clone(), "a|bc"),
            (MoveLeft, "|abc", ignored.clone(), "|abc"),
            (MoveRight, "a|bc", changed.clone(), "ab|c"),
            (MoveRight, "abc|", ignored.clone(), "abc|"),
            (MoveStart, "ab|c", changed.clone(), "|abc"),
            (MoveEnd, "a|bc", changed.clone(), "abc|"),
            (MoveEnd, "abc|", ignored.clone(), "abc|"),
            (MoveWordLeft, "foo bar|", changed.clone(), "foo |bar"),
            (MoveWordRight, "|foo bar", changed.clone(), "foo| bar"),
            (MoveWordRight, "foo| bar", changed.clone(), "foo bar|"),
            (MoveLeft, "|", ignored.clone(), "|"),
        ];
        for (command, before, event, after) in cases {
            assert_eq!(
                run(before, command),
                (event, after.to_string()),
                "{command:?} on {before:?}"
            );
        }
    }

    #[test]
    fn deletions_at_the_start_middle_and_end() {
        use Command::*;
        let cases = [
            (DeleteLeft, "ab|c", "a|c"),
            (DeleteLeft, "|abc", "|abc"),
            (DeleteRight, "a|bc", "a|c"),
            (DeleteRight, "abc|", "abc|"),
            (DeleteLeft, "e\u{301}|x", "|x"),
            (DeleteRight, "|🤦🏼\u{200d}♂\u{fe0f}x", "|x"),
            (DeleteLeft, "漢字|", "漢|"),
            (DeleteRightOrEof, "a|b", "a|"),
        ];
        for (command, before, after) in cases {
            assert_eq!(run(before, command).1, after, "{command:?} on {before:?}");
        }
        assert_eq!(run("|", DeleteRightOrEof).0, EditorEvent::Eof);
        assert_eq!(run("a|", DeleteRightOrEof).0, EditorEvent::Ignored);
    }

    #[test]
    fn kills() {
        use Command::*;
        let cases = [
            (KillWordLeft, "foo-bar|", "foo-|"),
            (KillBigWordLeft, "x foo-bar|", "x |"),
            (KillBigWordLeft, "x foo  |", "x |"),
            (KillWordRight, "|foo-bar", "|-bar"),
            (KillToStart, "ab|cd", "|cd"),
            (KillToEnd, "ab|cd", "ab|"),
            (KillLine, "ab|cd", "|"),
            (KillToEnd, "abcd|", "abcd|"),
        ];
        for (command, before, after) in cases {
            assert_eq!(run(before, command).1, after, "{command:?} on {before:?}");
        }
    }

    #[test]
    fn consecutive_kills_are_yanked_back_together() {
        let mut e = editor("one two three|");
        let _ = e.execute(Command::KillWordLeft);
        let _ = e.execute(Command::KillWordLeft);
        assert_eq!(shown(&e), "one |");
        let _ = e.execute(Command::KillToStart);
        let _ = e.execute(Command::Yank);
        assert_eq!(shown(&e), "one two three|");
        // Right kills are appended.
        let mut e = editor("|ab cd");
        let _ = e.execute(Command::KillWordRight);
        let _ = e.execute(Command::KillWordRight);
        let _ = e.execute(Command::Yank);
        assert_eq!(shown(&e), "ab cd|");
    }

    #[test]
    fn a_command_between_kills_breaks_the_chain() {
        let mut e = editor("a b|");
        let _ = e.execute(Command::KillWordLeft);
        let _ = e.execute(Command::MoveLeft);
        let _ = e.execute(Command::KillWordLeft);
        let _ = e.execute(Command::Yank);
        assert_eq!(shown(&e), "a| ");
        let _ = e.execute(Command::YankPop);
        assert_eq!(shown(&e), "b| ");
    }

    #[test]
    fn yank_pop_only_right_after_a_yank_and_it_cycles() {
        let mut e = editor("|");
        for word in ["one", "two", "three"] {
            type_text(&mut e, word);
            let _ = e.execute(Command::KillLine);
        }
        assert_eq!(e.execute(Command::YankPop), EditorEvent::Ignored);
        let _ = e.execute(Command::Yank);
        assert_eq!(shown(&e), "three|");
        let _ = e.execute(Command::YankPop);
        assert_eq!(shown(&e), "two|");
        let _ = e.execute(Command::YankPop);
        let _ = e.execute(Command::YankPop);
        assert_eq!(shown(&e), "three|");
        let _ = e.execute(Command::MoveLeft);
        assert_eq!(e.execute(Command::YankPop), EditorEvent::Ignored);
        assert_eq!(run("|", Command::Yank).0, EditorEvent::Ignored);
    }

    #[test]
    fn the_kill_ring_survives_submit() {
        let mut e = editor("hello|");
        let _ = e.execute(Command::KillLine);
        let _ = e.execute(Command::Submit);
        let _ = e.execute(Command::Yank);
        assert_eq!(e.value(), "hello");
    }

    #[test]
    fn transpositions() {
        use Command::*;
        let cases = [
            (TransposeChars, "ab|c", "acb|"),
            (TransposeChars, "abc|", "acb|"),
            (TransposeChars, "|abc", "|abc"),
            (TransposeChars, "a|", "a|"),
            (TransposeChars, "e\u{301}|漢", "漢e\u{301}|"),
            (TransposeWords, "foo |bar", "bar foo|"),
            (TransposeWords, "foo bar|", "bar foo|"),
            (TransposeWords, "a foo-|bar", "a bar-foo|"),
            (TransposeWords, "|foo", "|foo"),
        ];
        for (command, before, after) in cases {
            assert_eq!(run(before, command).1, after, "{command:?} on {before:?}");
        }
    }

    #[test]
    fn case_changes() {
        use Command::*;
        let cases = [
            (UpcaseWord, "|foo bar", "FOO| bar"),
            (UpcaseWord, "f|oo bar", "fOO| bar"),
            (DowncaseWord, "FOO |BAR", "FOO bar|"),
            (CapitalizeWord, "|hELLO été", "Hello| été"),
            (CapitalizeWord, "hello |été", "hello Été|"),
            (UpcaseWord, "foo|", "foo|"),
        ];
        for (command, before, after) in cases {
            assert_eq!(run(before, command).1, after, "{command:?} on {before:?}");
        }
        assert_eq!(run("FOO|", UpcaseWord).0, EditorEvent::Ignored);
    }

    #[test]
    fn control_characters_are_never_inserted() {
        assert_eq!(
            run("a|", Command::InsertChar('\u{1b}')),
            (EditorEvent::Ignored, "a|".to_string())
        );
        assert_eq!(run("a|", Command::InsertChar('\n')).0, EditorEvent::Ignored);
    }

    #[test]
    fn a_full_line_ignores_new_characters() {
        let mut e = LineEditor::new(EditorConfig {
            max_len: 3,
            ..EditorConfig::default()
        });
        type_text(&mut e, "abcd");
        assert_eq!(e.value(), "abc");
        assert_eq!(e.execute(Command::InsertChar('x')), EditorEvent::Ignored);
    }

    #[test]
    fn undo_removes_a_typed_word_at_once() {
        let mut e = editor("|");
        type_text(&mut e, "foo bar");
        let _ = e.execute(Command::Undo);
        assert_eq!(shown(&e), "foo |");
        let _ = e.execute(Command::Undo);
        assert_eq!(shown(&e), "foo|");
        let _ = e.execute(Command::Undo);
        assert_eq!(shown(&e), "|");
        assert_eq!(e.execute(Command::Undo), EditorEvent::Ignored);
        let _ = e.execute(Command::Redo);
        let _ = e.execute(Command::Redo);
        assert_eq!(shown(&e), "foo |");
    }

    #[test]
    fn a_movement_splits_the_typed_word_into_two_steps() {
        let mut e = editor("|");
        type_text(&mut e, "ab");
        let _ = e.execute(Command::MoveLeft);
        type_text(&mut e, "x");
        let _ = e.execute(Command::Undo);
        assert_eq!(shown(&e), "a|b");
        let _ = e.execute(Command::Undo);
        assert_eq!(shown(&e), "|");
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let mut e = editor("|");
        type_text(&mut e, "a");
        let _ = e.execute(Command::Undo);
        type_text(&mut e, "b");
        assert_eq!(e.execute(Command::Redo), EditorEvent::Ignored);
        assert_eq!(e.value(), "b");
    }

    #[test]
    fn set_value_is_one_undo_step_with_the_cursor_at_the_end() {
        let mut e = editor("draft|");
        e.set_value("a\nb\u{7}");
        assert_eq!(shown(&e), "a b|");
        let _ = e.execute(Command::Undo);
        assert_eq!(shown(&e), "draft|");
    }

    #[test]
    fn submit_returns_the_line_and_clears_undo() {
        let mut e = editor("|");
        type_text(&mut e, "ls");
        assert_eq!(
            e.execute(Command::Submit),
            EditorEvent::Submit("ls".to_string())
        );
        assert_eq!(shown(&e), "|");
        assert_eq!(e.execute(Command::Undo), EditorEvent::Ignored);
        assert_eq!(run("|", Command::Interrupt).0, EditorEvent::Interrupt);
    }

    #[test]
    fn every_default_editing_binding_runs_its_command() {
        use crossterm::event::KeyCode::*;
        let (none, ctrl, alt) = (KeyModifiers::NONE, KeyModifiers::CONTROL, KeyModifiers::ALT);
        let cases = [
            (Left, none, "ab|", "a|b"),
            (Char('b'), ctrl, "ab|", "a|b"),
            (Right, none, "|ab", "a|b"),
            (Char('f'), ctrl, "|ab", "a|b"),
            (Left, ctrl, "foo bar|", "foo |bar"),
            (Left, alt, "foo bar|", "foo |bar"),
            (Char('b'), alt, "foo bar|", "foo |bar"),
            (Right, ctrl, "|foo bar", "foo| bar"),
            (Right, alt, "|foo bar", "foo| bar"),
            (Char('f'), alt, "|foo bar", "foo| bar"),
            (Home, none, "ab|", "|ab"),
            (Char('a'), ctrl, "ab|", "|ab"),
            (End, none, "|ab", "ab|"),
            (Char('e'), ctrl, "|ab", "ab|"),
            (Backspace, none, "ab|", "a|"),
            (Char('h'), ctrl, "ab|", "a|"),
            (Delete, none, "|ab", "|b"),
            (Char('d'), ctrl, "|ab", "|b"),
            (Backspace, alt, "x foo-bar|", "x foo-|"),
            (Char('w'), ctrl, "x foo-bar|", "x |"),
            (Char('d'), alt, "|foo bar", "| bar"),
            (Delete, ctrl, "|foo bar", "| bar"),
            (Char('u'), ctrl, "ab|cd", "|cd"),
            (Char('k'), ctrl, "ab|cd", "ab|"),
            (Char('t'), ctrl, "ab|", "ba|"),
            (Char('t'), alt, "foo bar|", "bar foo|"),
            (Char('u'), alt, "|foo", "FOO|"),
            (Char('l'), alt, "|FOO", "foo|"),
            (Char('c'), alt, "|foo", "Foo|"),
            (Char('x'), none, "a|", "ax|"),
        ];
        for (code, modifiers, before, after) in cases {
            let mut e = editor(before);
            let _ = e.handle_key(KeyEvent::new(code, modifiers));
            assert_eq!(shown(&e), after, "{modifiers:?}+{code:?} on {before:?}");
        }
    }

    #[test]
    fn default_bindings_for_submit_interrupt_eof_yank_and_undo() {
        use crossterm::event::KeyCode::*;
        let (none, ctrl, alt) = (KeyModifiers::NONE, KeyModifiers::CONTROL, KeyModifiers::ALT);
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        for (code, modifiers) in [(Enter, none), (Char('j'), ctrl), (Char('m'), ctrl)] {
            let mut e = editor("ls|");
            assert_eq!(
                e.handle_key(key(code, modifiers)),
                EditorEvent::Submit("ls".into())
            );
        }
        assert_eq!(
            editor("x|").handle_key(key(Char('c'), ctrl)),
            EditorEvent::Interrupt
        );
        assert_eq!(
            editor("|").handle_key(key(Char('d'), ctrl)),
            EditorEvent::Eof
        );

        let mut e = editor("one two|");
        let _ = e.handle_key(key(Char('w'), ctrl));
        let _ = e.handle_key(key(Char('w'), ctrl));
        let _ = e.handle_key(key(Char('k'), ctrl));
        let _ = e.handle_key(key(Char('y'), ctrl));
        assert_eq!(shown(&e), "one two|");
        let _ = e.handle_key(key(Char('a'), ctrl));
        let _ = e.handle_key(key(Char('k'), ctrl));
        type_text(&mut e, "x");
        let _ = e.handle_key(key(Char('w'), ctrl));
        let _ = e.handle_key(key(Char('y'), ctrl));
        assert_eq!(shown(&e), "x|");
        let _ = e.handle_key(key(Char('y'), alt));
        assert_eq!(shown(&e), "one two|");

        for (code, modifiers) in [
            (Char('_'), ctrl),
            (Char('/'), ctrl),
            (Char('7'), ctrl),
            (Char('z'), ctrl),
        ] {
            let mut e = editor("|");
            type_text(&mut e, "abc");
            let _ = e.handle_key(key(code, modifiers));
            assert_eq!(e.value(), "", "{modifiers:?}+{code:?}");
            let redo = if code == Char('/') {
                Char('/')
            } else {
                Char('_')
            };
            let _ = e.handle_key(key(redo, alt));
            assert_eq!(e.value(), "abc", "redo after {modifiers:?}+{code:?}");
        }
    }

    #[test]
    fn the_public_cursor_counts_graphemes() {
        let e = editor("🤦🏼\u{200d}♂\u{fe0f}e\u{301}|");
        assert_eq!(e.cursor(), 2);
    }

    /// Random text mixing ASCII, combining marks, wide characters, emoji and controls.
    fn text_strategy() -> impl Strategy<Value = String> {
        let pieces = prop::sample::select(vec![
            "a",
            "Z",
            " ",
            "-",
            "_",
            "é",
            "e\u{301}",
            "漢",
            "🤦🏼\u{200d}♂\u{fe0f}",
            "\u{1f1eb}",
            "\u{1f1f7}",
            "\t",
            "\n",
            "\u{7}",
            "…",
        ]);
        prop::collection::vec(pieces, 0..12).prop_map(|p| p.concat())
    }

    /// Random editing commands (no submit: undo must be able to go back to the start).
    fn command_strategy() -> impl Strategy<Value = Command> {
        use Command::*;
        prop_oneof![
            prop::sample::select(vec!['a', ' ', 'é', '漢', '\u{301}', '\u{1f1eb}', '\u{7}'])
                .prop_map(InsertChar),
            prop::sample::select(vec![
                MoveLeft,
                MoveRight,
                MoveWordLeft,
                MoveWordRight,
                MoveStart,
                MoveEnd,
                DeleteLeft,
                DeleteRight,
                KillWordLeft,
                KillBigWordLeft,
                KillWordRight,
                KillToStart,
                KillToEnd,
                KillLine,
                Yank,
                YankPop,
                TransposeChars,
                TransposeWords,
                UpcaseWord,
                DowncaseWord,
                CapitalizeWord,
                Undo,
                Redo,
            ]),
        ]
    }

    /// Check the invariants of the line.
    fn assert_invariants(e: &LineEditor, max_len: usize) {
        let text = e.value();
        let cursor = e.buffer.cursor();
        assert!(text.len() <= max_len, "line longer than max_len: {text:?}");
        assert!(
            !text.chars().any(char::is_control),
            "control character in {text:?}"
        );
        assert!(
            cursor == text.len()
                || unicode_segmentation::UnicodeSegmentation::grapheme_indices(text, true)
                    .any(|(i, _)| i == cursor),
            "cursor {cursor} not on a grapheme boundary of {text:?}"
        );
    }

    proptest! {
        #[test]
        fn invariants_hold_after_every_command(
            text in text_strategy(),
            commands in prop::collection::vec(command_strategy(), 0..40),
            max_len in 1usize..40,
        ) {
            let mut e = LineEditor::new(EditorConfig { max_len, ..EditorConfig::default() });
            e.set_value(&text);
            assert_invariants(&e, max_len);
            for command in commands {
                let _ = e.execute(command);
                assert_invariants(&e, max_len);
            }
        }

        #[test]
        fn undo_returns_to_the_initial_value(
            text in text_strategy(),
            commands in prop::collection::vec(command_strategy(), 0..40),
        ) {
            let mut e = LineEditor::new(EditorConfig::default());
            e.buffer.set(&text, usize::MAX);
            let initial = e.value().to_string();
            for command in commands {
                let _ = e.execute(command);
            }
            while e.execute(Command::Undo) != EditorEvent::Ignored {}
            prop_assert_eq!(e.value(), initial);
        }
    }
}
