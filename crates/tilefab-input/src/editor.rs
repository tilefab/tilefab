//! The line editor: dispatch of commands to the line, the kill ring and the undo stack.

use std::ops::Range;

use crossterm::event::KeyEvent;
use tilefab_core::{Buffer, Line, Position, Rect};

use crate::buffer::{self, LineBuffer};
use crate::completion::{self, Completer, Menu};
use crate::hint::{Hinter, HistoryHinter};
use crate::history::{self, History};
use crate::keymap::{Command, Keymap};
use crate::kill_ring::{KillRing, Side};
use crate::render::{self, EditorStyles, LineView};
use crate::search::Search;
use crate::undo::{Snapshot, Step, UndoStack};

/// Default maximum length of the line, in bytes.
const DEFAULT_MAX_LEN: usize = 4096;

/// Settings of a [`LineEditor`].
///
/// New fields may come in minor versions, so it cannot be written as a struct literal outside
/// this crate: start from `EditorConfig::default()` and chain the setters below (or assign the
/// public fields of a mutable value).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EditorConfig {
    /// Maximum length of the line, in bytes (default 4096): a huge paste cannot exhaust
    /// memory.
    pub max_len: usize,
    /// Columns of text kept visible on each side of the cursor when the line scrolls
    /// (default 4; reduced on narrow widths).
    pub scroll_margin: u16,
    /// Maximum number of history entries of the default history (default 1000).
    pub history_capacity: usize,
    /// A line that starts with a space is not recorded in the history (default true).
    pub history_ignore_space: bool,
    /// Show hints (autosuggestions) after the line (default true).
    pub hints: bool,
    /// Tab accepts the whole hint when completion has nothing to do (no completer, no
    /// candidate, or the word is already complete) (default false).
    pub tab_accepts_hint: bool,
    /// Maximum number of rows of the completion menu (default 5).
    pub menu_max_rows: u16,
    /// Styles of the hint, the indicators, the search match and the menu.
    pub styles: EditorStyles,
}

/// Setters, to chain from [`EditorConfig::default`].
impl EditorConfig {
    /// Set [`max_len`](Self::max_len).
    #[must_use]
    pub const fn max_len(mut self, max_len: usize) -> Self {
        self.max_len = max_len;
        self
    }

    /// Set [`scroll_margin`](Self::scroll_margin).
    #[must_use]
    pub const fn scroll_margin(mut self, scroll_margin: u16) -> Self {
        self.scroll_margin = scroll_margin;
        self
    }

    /// Set [`history_capacity`](Self::history_capacity).
    #[must_use]
    pub const fn history_capacity(mut self, history_capacity: usize) -> Self {
        self.history_capacity = history_capacity;
        self
    }

    /// Set [`history_ignore_space`](Self::history_ignore_space).
    #[must_use]
    pub const fn history_ignore_space(mut self, history_ignore_space: bool) -> Self {
        self.history_ignore_space = history_ignore_space;
        self
    }

    /// Set [`hints`](Self::hints).
    #[must_use]
    pub const fn hints(mut self, hints: bool) -> Self {
        self.hints = hints;
        self
    }

    /// Set [`tab_accepts_hint`](Self::tab_accepts_hint).
    #[must_use]
    pub const fn tab_accepts_hint(mut self, tab_accepts_hint: bool) -> Self {
        self.tab_accepts_hint = tab_accepts_hint;
        self
    }

    /// Set [`menu_max_rows`](Self::menu_max_rows).
    #[must_use]
    pub const fn menu_max_rows(mut self, menu_max_rows: u16) -> Self {
        self.menu_max_rows = menu_max_rows;
        self
    }

    /// Set [`styles`](Self::styles).
    #[must_use]
    pub const fn styles(mut self, styles: EditorStyles) -> Self {
        self.styles = styles;
        self
    }
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            max_len: DEFAULT_MAX_LEN,
            scroll_margin: 4,
            history_capacity: history::DEFAULT_CAPACITY,
            history_ignore_space: true,
            hints: true,
            tab_accepts_hint: false,
            menu_max_rows: 5,
            styles: EditorStyles::default(),
        }
    }
}

/// What the application must do after a key or a paste.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
// New variants may come in minor versions: `match` on it needs a `_` arm.
#[non_exhaustive]
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

/// What the keys act on.
#[derive(Debug)]
enum Mode {
    /// Editing the line.
    Normal,
    /// Reverse incremental search: keys edit the query.
    Search(Search),
    /// The completion menu is open: keys move the selection.
    Menu(Menu),
}

/// Which history entry to show next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recall {
    Older,
    Newer,
    Oldest,
    Newest,
}

/// Browsing the history: the line shows entry `index`.
#[derive(Debug)]
struct Navigation {
    /// The line as it was when the navigation started, restored after the newest entry.
    draft: Snapshot,
    /// The entry shown.
    index: usize,
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
pub struct LineEditor {
    config: EditorConfig,
    buffer: LineBuffer,
    keymap: Keymap,
    kill_ring: KillRing,
    undo: UndoStack,
    last: Last,
    /// First visible column of the line, kept between renders.
    scroll: usize,
    history: History,
    navigation: Option<Navigation>,
    hinter: Option<Box<dyn Hinter>>,
    completer: Option<Box<dyn Completer>>,
    /// `Esc` hid the hint; shown again after the next change of the line.
    hint_dismissed: bool,
    mode: Mode,
}

impl std::fmt::Debug for LineEditor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LineEditor")
            .field("value", &self.buffer.value())
            .field("cursor", &self.buffer.cursor())
            .field("history_len", &self.history.len())
            .field("navigation", &self.navigation)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl LineEditor {
    /// An empty editor.
    #[must_use]
    pub fn new(config: EditorConfig) -> Self {
        let history = History::new(config.history_capacity);
        Self {
            buffer: LineBuffer::new(config.max_len),
            config,
            keymap: Keymap::emacs(),
            kill_ring: KillRing::default(),
            undo: UndoStack::default(),
            last: Last::Other,
            scroll: 0,
            history,
            navigation: None,
            hinter: Some(Box::new(HistoryHinter)),
            completer: None,
            hint_dismissed: false,
            mode: Mode::Normal,
        }
    }

    /// Use `history` (for instance one loaded from a file) instead of an empty one.
    #[must_use]
    pub fn with_history(mut self, history: History) -> Self {
        self.history = history;
        self
    }

    /// Use `hinter` instead of the default [`HistoryHinter`].
    #[must_use]
    pub fn with_hinter(mut self, hinter: impl Hinter + 'static) -> Self {
        self.hinter = Some(Box::new(hinter));
        self
    }

    /// Complete with `completer` on Tab (without one, Tab does nothing).
    #[must_use]
    pub fn with_completer(mut self, completer: impl Completer + 'static) -> Self {
        self.completer = Some(Box::new(completer));
        self
    }

    /// The history of submitted lines.
    #[must_use]
    pub fn history(&self) -> &History {
        &self.history
    }

    /// The history, to add entries or replace it (for instance with one loaded from a
    /// file, see [`History::with_file`]).
    pub fn history_mut(&mut self) -> &mut History {
        &mut self.history
    }

    /// The last error writing the history file, once (see [`History::take_error`]).
    pub fn take_history_error(&mut self) -> Option<std::io::Error> {
        self.history.take_error()
    }

    /// Insert pasted text at the cursor, as **one** undo step: line breaks and tabs
    /// become spaces, other control characters are removed, and the text is cut to the
    /// maximum length. It never submits. During a search, the text is added to the query;
    /// with the menu open, the menu is closed first.
    pub fn paste(&mut self, text: &str) -> EditorEvent {
        let text = buffer::sanitize(text);
        self.last = Last::Other;
        self.undo.close_group();
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Search(mut search) => {
                let room = self.config.max_len.saturating_sub(search.query.len());
                let text = buffer::truncate(&text, room);
                if text.is_empty() {
                    self.mode = Mode::Search(search);
                    return EditorEvent::Ignored;
                }
                search.query.push_str(text);
                let from = search
                    .found
                    .as_ref()
                    .map_or(self.history.len(), |(index, _)| *index);
                search.find(&self.history, from, false);
                self.show_search(search)
            }
            Mode::Menu(menu) => {
                self.close_menu(menu);
                let _ = self.edit(Step::Single, |b| {
                    b.insert(&text);
                });
                EditorEvent::Changed
            }
            Mode::Normal => self.edit(Step::Single, |b| {
                b.insert(&text);
            }),
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

    /// Draw the prompt and the line on the first row of `area` in `buf`, the completion
    /// menu on the following rows, and return the cursor cell (`None` when `area` is
    /// empty). Text hidden by the horizontal scroll is
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
        let hint = self.current_hint();
        let (search_prompt, highlight) = match &self.mode {
            Mode::Search(search) => (
                Some(Line::from(search.prompt())),
                search.found.as_ref().map(|(_, range)| range.clone()),
            ),
            Mode::Normal | Mode::Menu(_) => (None, None),
        };
        let view = LineView {
            text: self.buffer.value(),
            cursor: self.buffer.cursor(),
            hint: hint.as_deref(),
            highlight,
        };
        let cursor = render::draw_line(
            buf,
            row,
            search_prompt.as_ref().unwrap_or(prompt),
            &view,
            &mut self.scroll,
            self.config.scroll_margin,
            &self.config.styles,
        );
        if let Mode::Menu(menu) = &mut self.mode {
            let rows = area.height.saturating_sub(1).min(self.config.menu_max_rows);
            let menu_area = Rect::new(area.x, area.y + 1, area.width, rows);
            completion::draw_menu(menu, menu_area, buf, &self.config.styles);
        }
        Some(cursor)
    }

    /// Run `command`.
    pub fn execute(&mut self, command: Command) -> EditorEvent {
        let last = std::mem::replace(&mut self.last, Last::Other);
        if !matches!(command, Command::InsertChar(c) if !c.is_whitespace()) {
            self.undo.close_group();
        }
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => self.edit_command(command, last),
            Mode::Search(search) => self.search_command(search, command, last),
            Mode::Menu(menu) => self.menu_command(menu, command, last),
        }
    }

    /// Run `command` with the completion menu open: Tab / ↓ and `BackTab` / ↑ select the
    /// next and previous candidate, ← / → move by column; Enter keeps the selection and
    /// closes the menu without submitting; `Esc`, Ctrl+G and Ctrl+C restore the line as it
    /// was before Tab. Any other command closes the menu, keeping the selection, then
    /// runs as usual (a typed character is inserted after the selected candidate).
    fn menu_command(&mut self, mut menu: Menu, command: Command, last: Last) -> EditorEvent {
        match command {
            Command::Complete | Command::HistoryNext => menu.step(true),
            Command::CompletePrevious | Command::HistoryPrevious => menu.step(false),
            Command::MoveRight => menu.step_column(true),
            Command::MoveLeft => menu.step_column(false),
            Command::Cancel | Command::Abort | Command::Interrupt => {
                self.buffer.set(&menu.original.value, menu.original.cursor);
                return EditorEvent::Changed;
            }
            Command::Submit => {
                self.close_menu(menu);
                return EditorEvent::Changed;
            }
            other => {
                self.close_menu(menu);
                let _ = self.edit_command(other, last);
                return EditorEvent::Changed;
            }
        }
        self.show_menu(menu)
    }

    /// Preview the selected candidate in the line and keep the menu open.
    fn show_menu(&mut self, menu: Menu) -> EditorEvent {
        let (value, cursor) = menu.preview();
        self.buffer.set(&value, cursor);
        self.mode = Mode::Menu(menu);
        EditorEvent::Changed
    }

    /// Close the menu, keeping the selected candidate (one undo step back to the line
    /// before Tab).
    fn close_menu(&mut self, menu: Menu) {
        if self.buffer.value() != menu.original.value {
            self.undo.record(menu.original, Step::Single);
            self.line_changed();
        }
    }

    /// Tab (or `BackTab` with `backward`) in normal mode. One candidate: it replaces the
    /// completed text. Several: their longest common prefix is inserted; when that adds
    /// nothing, the menu opens (`BackTab` opens it at once, on the last candidate).
    fn complete(&mut self, backward: bool) -> EditorEvent {
        let Some(completer) = &self.completer else {
            return EditorEvent::Ignored;
        };
        let line = self.buffer.value();
        let cursor = self.buffer.cursor();
        let completions = completer.complete(line, cursor);
        let Some((start, candidates)) = completion::sanitize(completions, line, cursor) else {
            return EditorEvent::Ignored;
        };
        if let [candidate] = candidates.as_slice()
            && !backward
        {
            let mut text = candidate.replacement.clone();
            if candidate.append_space && !line[cursor..].starts_with(' ') {
                text.push(' ');
            }
            return self.edit(Step::Single, |b| {
                b.replace(start..cursor, &text);
            });
        }
        let typed = &line[start..cursor];
        let prefix = completion::common_prefix(&candidates);
        if !backward && prefix.len() > typed.len() && prefix.starts_with(typed) {
            let prefix = prefix.to_string();
            return self.edit(Step::Single, |b| {
                b.replace(start..cursor, &prefix);
            });
        }
        let selected = if backward { candidates.len() - 1 } else { 0 };
        let rows = candidates.len();
        let menu = Menu {
            start,
            candidates,
            original: self.snapshot(),
            selected,
            top: 0,
            rows,
        };
        self.show_menu(menu)
    }

    /// Rows needed below the prompt line (the completion menu) in an area `width` columns
    /// wide: the application adds them to the height of the viewport.
    #[must_use]
    pub fn extra_rows(&self, width: u16) -> u16 {
        match &self.mode {
            Mode::Menu(menu) => completion::rows_needed(menu, width, self.config.menu_max_rows),
            Mode::Normal | Mode::Search(_) => 0,
        }
    }

    /// Run `command` during a search: typing edits the query, Ctrl+R / Ctrl+S go to the
    /// next older / newer match, Enter submits the match, Ctrl+G and Ctrl+C restore the
    /// line as it was before the search; any other command leaves the search with the
    /// match in the line, then runs as usual.
    fn search_command(&mut self, mut search: Search, command: Command, last: Last) -> EditorEvent {
        let newest = self.history.len();
        match command {
            Command::InsertChar(c) => {
                if c.is_control() || search.query.len() + c.len_utf8() > self.config.max_len {
                    self.mode = Mode::Search(search);
                    return EditorEvent::Ignored;
                }
                search.query.push(c);
                let from = search.found.as_ref().map_or(newest, |(index, _)| *index);
                search.find(&self.history, from, false);
                self.show_search(search)
            }
            Command::DeleteLeft => {
                if search.query.is_empty() {
                    self.mode = Mode::Search(search);
                    return EditorEvent::Ignored;
                }
                let end = buffer::prev_grapheme(&search.query, search.query.len());
                search.query.truncate(end);
                search.found = None;
                search.failing = false;
                if !search.query.is_empty() {
                    search.find(&self.history, newest, false);
                }
                self.show_search(search)
            }
            Command::ReverseSearch | Command::ForwardSearch => {
                let newer = command == Command::ForwardSearch;
                match (&search.found, newer) {
                    _ if search.query.is_empty() => {
                        self.mode = Mode::Search(search);
                        return EditorEvent::Ignored;
                    }
                    // Nothing older than the oldest entry, nothing newer without a match.
                    (Some((0, _)), false) | (None, true) => search.failing = true,
                    (Some((index, _)), false) => {
                        let from = index - 1;
                        search.find(&self.history, from, false);
                    }
                    (Some((index, _)), true) => {
                        let from = index + 1;
                        search.find(&self.history, from, true);
                    }
                    (None, false) => {
                        search.find(&self.history, newest, false);
                    }
                }
                self.show_search(search)
            }
            Command::Abort | Command::Interrupt => {
                self.buffer.set(&search.draft.value, search.draft.cursor);
                EditorEvent::Changed
            }
            Command::Cancel => {
                self.leave_search(search);
                EditorEvent::Changed
            }
            Command::Submit => {
                self.leave_search(search);
                self.submit()
            }
            other => {
                self.leave_search(search);
                let _ = self.edit_command(other, last);
                EditorEvent::Changed
            }
        }
    }

    /// Show the current match of `search` in the line (cursor on the match), or the
    /// draft when there is none, and stay in search mode.
    fn show_search(&mut self, search: Search) -> EditorEvent {
        match &search.found {
            Some((index, range)) => {
                let entry = self.history.get(*index).unwrap_or_default();
                self.buffer.set(entry, range.start);
            }
            None => self.buffer.set(&search.draft.value, search.draft.cursor),
        }
        self.mode = Mode::Search(search);
        EditorEvent::Changed
    }

    /// Leave the search, keeping the match in the line (one undo step back to the draft).
    fn leave_search(&mut self, search: Search) {
        if self.buffer.value() != search.draft.value {
            self.undo.record(search.draft, Step::Single);
            self.line_changed();
        }
    }

    /// Ctrl+R in normal mode: start a search from the current line.
    fn start_search(&mut self) -> EditorEvent {
        self.navigation = None;
        self.mode = Mode::Search(Search::new(self.snapshot()));
        EditorEvent::Changed
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
            Command::MoveRight | Command::MoveEnd if cursor == len => self.accept_hint(false),
            Command::MoveWordRight if cursor == len => self.accept_hint(true),
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
            Command::HistoryPrevious => self.recall(Recall::Older, true),
            Command::HistoryNext => self.recall(Recall::Newer, true),
            Command::HistoryPreviousAll => self.recall(Recall::Older, false),
            Command::HistoryNextAll => self.recall(Recall::Newer, false),
            Command::HistoryFirst => self.recall(Recall::Oldest, false),
            Command::HistoryLast => self.recall(Recall::Newest, false),
            Command::Cancel => self.cancel(),
            Command::ReverseSearch => self.start_search(),
            Command::Complete => match self.complete(false) {
                EditorEvent::Ignored if self.config.tab_accepts_hint => self.accept_hint(false),
                event => event,
            },
            Command::CompletePrevious => self.complete(true),
            Command::ForwardSearch | Command::Abort => EditorEvent::Ignored,
        }
    }

    /// The hint to show after the line, if any: hints enabled, cursor at the end of a
    /// non-empty line, not dismissed with `Esc`. Sanitized and cut so that accepting it
    /// keeps the line within its maximum length.
    fn current_hint(&self) -> Option<String> {
        let text = self.buffer.value();
        if !self.config.hints
            || !matches!(self.mode, Mode::Normal)
            || self.hint_dismissed
            || text.is_empty()
            || self.buffer.cursor() != text.len()
        {
            return None;
        }
        let hint = self.hinter.as_ref()?.hint(text, &self.history)?;
        let hint = buffer::sanitize(&hint);
        let room = self.config.max_len.saturating_sub(text.len());
        let hint = buffer::truncate(&hint, room);
        (!hint.is_empty()).then(|| hint.to_string())
    }

    /// Accept the hint (one undo step): all of it, or only up to the end of its first word.
    fn accept_hint(&mut self, one_word: bool) -> EditorEvent {
        let Some(hint) = self.current_hint() else {
            return EditorEvent::Ignored;
        };
        let end = if one_word {
            buffer::word_end_after(&hint, 0)
        } else {
            hint.len()
        };
        self.edit(Step::Single, |b| {
            b.insert(&hint[..end]);
        })
    }

    /// `Esc`: leave the history (back to the draft), else hide the hint.
    fn cancel(&mut self) -> EditorEvent {
        if let Some(navigation) = self.navigation.take() {
            self.buffer
                .set(&navigation.draft.value, navigation.draft.cursor);
            self.undo.clear();
            return EditorEvent::Changed;
        }
        if self.current_hint().is_some() {
            self.hint_dismissed = true;
            return EditorEvent::Changed;
        }
        EditorEvent::Ignored
    }

    /// Show another history entry. With `filtered`, only the entries that start with the
    /// draft (the line when the navigation started) are visited, and an entry identical
    /// to the line is skipped. Going newer than the newest entry restores the draft.
    fn recall(&mut self, direction: Recall, filtered: bool) -> EditorEvent {
        let len = self.history.len();
        let (draft, current) = match &self.navigation {
            Some(navigation) => (navigation.draft.value.as_str(), navigation.index),
            None => (self.buffer.value(), len),
        };
        let shown = self.buffer.value();
        let matches = |i: &usize| {
            let entry = self.history.get(*i).unwrap_or_default();
            !filtered || (entry.starts_with(draft) && entry != shown)
        };
        let target = match direction {
            Recall::Older => (0..current).rev().find(matches),
            Recall::Newer => (current + 1..len).find(matches),
            Recall::Oldest => (0..len).find(matches),
            Recall::Newest => (0..len).rev().find(matches),
        };
        let target = target.filter(|&i| i != current);
        match (target, direction) {
            (Some(index), _) => {
                let draft = match self.navigation.take() {
                    Some(navigation) => navigation.draft,
                    None => self.snapshot(),
                };
                let entry = self.history.get(index).unwrap_or_default().to_string();
                self.buffer.set(&entry, usize::MAX);
                // Undo goes back to the draft.
                self.undo.clear();
                self.undo.record(draft.clone(), Step::Single);
                self.navigation = Some(Navigation { draft, index });
                self.hint_dismissed = false;
                EditorEvent::Changed
            }
            (None, Recall::Newer) if self.navigation.is_some() => self.cancel(),
            (None, _) => EditorEvent::Ignored,
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
        self.line_changed();
        EditorEvent::Changed
    }

    /// The text was edited: it becomes the new draft for the history (the next ↑ searches
    /// with it), and a dismissed hint can show again.
    fn line_changed(&mut self) {
        self.navigation = None;
        self.hint_dismissed = false;
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
            self.line_changed();
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
        if !(self.config.history_ignore_space && line.starts_with(' ')) {
            self.history.add(&line);
        }
        self.buffer.clear();
        self.undo.clear();
        self.line_changed();
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

    /// An editor whose history holds `entries`, oldest first.
    fn with_history(entries: &[&str]) -> LineEditor {
        let mut history = History::new(100);
        for entry in entries {
            history.add(entry);
        }
        LineEditor::new(EditorConfig::default()).with_history(history)
    }

    fn submit(editor: &mut LineEditor, line: &str) {
        type_text(editor, line);
        let _ = editor.execute(Command::Submit);
    }

    #[test]
    fn submit_records_the_line_except_empty_or_leading_space() {
        let mut e = LineEditor::new(EditorConfig::default());
        submit(&mut e, "ls");
        submit(&mut e, "  ");
        submit(&mut e, " secret");
        submit(&mut e, "pwd");
        submit(&mut e, "ls");
        assert_eq!(e.history().iter().collect::<Vec<_>>(), ["pwd", "ls"]);
        let mut keep_space = LineEditor::new(EditorConfig {
            history_ignore_space: false,
            ..EditorConfig::default()
        });
        submit(&mut keep_space, " secret");
        assert_eq!(keep_space.history().get(0), Some(" secret"));
    }

    #[test]
    fn up_and_down_visit_the_entries_that_start_with_the_draft() {
        let mut e = with_history(&["start a", "list", "start b", "help"]);
        type_text(&mut e, "st");
        assert_eq!(e.execute(Command::HistoryPrevious), EditorEvent::Changed);
        assert_eq!(shown(&e), "start b|");
        let _ = e.execute(Command::HistoryPrevious);
        assert_eq!(shown(&e), "start a|");
        // The oldest match: no wrap-around.
        assert_eq!(e.execute(Command::HistoryPrevious), EditorEvent::Ignored);
        let _ = e.execute(Command::HistoryNext);
        assert_eq!(shown(&e), "start b|");
        // Past the newest match: the draft again, then nothing.
        let _ = e.execute(Command::HistoryNext);
        assert_eq!(shown(&e), "st|");
        assert_eq!(e.execute(Command::HistoryNext), EditorEvent::Ignored);
    }

    #[test]
    fn an_empty_draft_visits_every_entry_and_ctrl_p_ignores_the_draft() {
        let mut e = with_history(&["a", "b"]);
        let _ = e.execute(Command::HistoryPrevious);
        assert_eq!(e.value(), "b");
        let _ = e.execute(Command::HistoryPrevious);
        assert_eq!(e.value(), "a");
        let mut e = with_history(&["a", "xb"]);
        type_text(&mut e, "x");
        let _ = e.execute(Command::HistoryPreviousAll);
        let _ = e.execute(Command::HistoryPreviousAll);
        assert_eq!(e.value(), "a");
        let _ = e.execute(Command::HistoryNextAll);
        let _ = e.execute(Command::HistoryNextAll);
        assert_eq!(e.value(), "x");
    }

    #[test]
    fn first_and_last_entries() {
        let mut e = with_history(&["a", "b", "c"]);
        let _ = e.execute(Command::HistoryFirst);
        assert_eq!(e.value(), "a");
        assert_eq!(e.execute(Command::HistoryFirst), EditorEvent::Ignored);
        let _ = e.execute(Command::HistoryLast);
        assert_eq!(e.value(), "c");
        assert_eq!(
            with_history(&[]).execute(Command::HistoryLast),
            EditorEvent::Ignored
        );
    }

    #[test]
    fn esc_restores_the_draft_and_undo_goes_back_to_it() {
        let mut e = with_history(&["start a"]);
        type_text(&mut e, "st");
        let _ = e.execute(Command::MoveLeft);
        let _ = e.execute(Command::HistoryPrevious);
        assert_eq!(e.execute(Command::Cancel), EditorEvent::Changed);
        assert_eq!(shown(&e), "s|t");
        let _ = e.execute(Command::HistoryPrevious);
        let _ = e.execute(Command::Undo);
        assert_eq!(shown(&e), "s|t");
    }

    #[test]
    fn an_edited_entry_becomes_the_new_draft() {
        let mut e = with_history(&["start alpha", "start beta", "list"]);
        let _ = e.execute(Command::HistoryPrevious);
        let _ = e.execute(Command::HistoryPrevious);
        assert_eq!(e.value(), "start beta");
        for _ in 0.."beta".len() {
            let _ = e.execute(Command::DeleteLeft);
        }
        type_text(&mut e, "a");
        let _ = e.execute(Command::HistoryPrevious);
        assert_eq!(e.value(), "start alpha");
        // The history itself is unchanged.
        assert_eq!(e.history().len(), 3);
        assert_eq!(e.history().get(1), Some("start beta"));
    }

    #[test]
    fn the_hint_is_the_rest_of_the_newest_matching_entry() {
        let mut e = with_history(&["start alpha", "start beta"]);
        type_text(&mut e, "start");
        assert_eq!(e.current_hint().as_deref(), Some(" beta"));
        // Hidden when the cursor is not at the end, or the line is empty.
        let _ = e.execute(Command::MoveLeft);
        assert_eq!(e.current_hint(), None);
        assert_eq!(with_history(&["a"]).current_hint(), None);
        let mut off = LineEditor::new(EditorConfig {
            hints: false,
            ..EditorConfig::default()
        })
        .with_history(with_history(&["start"]).history);
        type_text(&mut off, "s");
        assert_eq!(off.current_hint(), None);
    }

    #[test]
    fn right_end_and_ctrl_f_accept_the_whole_hint_alt_f_one_word() {
        for command in [Command::MoveRight, Command::MoveEnd] {
            let mut e = with_history(&["start alpha now"]);
            type_text(&mut e, "st");
            assert_eq!(e.execute(command), EditorEvent::Changed);
            assert_eq!(shown(&e), "start alpha now|", "{command:?}");
            // Accepting is one undo step.
            let _ = e.execute(Command::Undo);
            assert_eq!(shown(&e), "st|");
        }
        let mut e = with_history(&["start alpha now"]);
        type_text(&mut e, "st");
        let _ = e.execute(Command::MoveWordRight);
        assert_eq!(shown(&e), "start|");
        let _ = e.execute(Command::MoveWordRight);
        assert_eq!(shown(&e), "start alpha|");
    }

    #[test]
    fn esc_dismisses_the_hint_until_the_line_changes() {
        let mut e = with_history(&["start alpha"]);
        type_text(&mut e, "st");
        assert_eq!(e.execute(Command::Cancel), EditorEvent::Changed);
        assert_eq!(e.current_hint(), None);
        assert_eq!(e.execute(Command::MoveRight), EditorEvent::Ignored);
        assert_eq!(e.execute(Command::Cancel), EditorEvent::Ignored);
        type_text(&mut e, "a");
        assert_eq!(e.current_hint().as_deref(), Some("rt alpha"));
    }

    #[test]
    fn a_custom_hinter_is_sanitized_and_kept_within_max_len() {
        struct Noisy;
        impl Hinter for Noisy {
            fn hint(&self, _: &str, _: &History) -> Option<String> {
                Some("a\tb\u{7}cdef".to_string())
            }
        }
        let mut e = LineEditor::new(EditorConfig {
            max_len: 5,
            ..EditorConfig::default()
        })
        .with_hinter(Noisy);
        type_text(&mut e, "x");
        assert_eq!(e.current_hint().as_deref(), Some("a bc"));
    }

    #[test]
    fn the_hint_is_drawn_dim_after_the_line() {
        let mut e = with_history(&["start alpha"]);
        type_text(&mut e, "st");
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        let cursor = e.render(buf.area(), &mut buf, &Line::from("> "));
        assert_eq!(buf.lines()[0].trim_end(), "> start alpha");
        assert_eq!(cursor, Some(Position::new(4, 0)));
    }

    /// An editor in search mode over `entries`, after typing `query`.
    fn searching(entries: &[&str], draft: &str, query: &str) -> LineEditor {
        let mut e = with_history(entries);
        type_text(&mut e, draft);
        assert_eq!(e.execute(Command::ReverseSearch), EditorEvent::Changed);
        type_text(&mut e, query);
        e
    }

    fn search_prompt(e: &LineEditor) -> Option<String> {
        match &e.mode {
            Mode::Search(search) => Some(search.prompt()),
            Mode::Normal | Mode::Menu(_) => None,
        }
    }

    #[test]
    fn typing_shows_the_newest_entry_that_contains_the_query() {
        let mut e = searching(&["start alpha", "list", "start beta"], "draft", "sta");
        assert_eq!(shown(&e), "|start beta");
        assert_eq!(search_prompt(&e).as_deref(), Some("(search) sta: "));
        type_text(&mut e, "rt a");
        assert_eq!(shown(&e), "|start alpha");
        // Smart case: an upper-case letter makes the search case-sensitive.
        let e = searching(&["Make", "make"], "", "M");
        assert_eq!(e.value(), "Make");
        let e = searching(&["Make", "other"], "", "ma");
        assert_eq!(e.value(), "Make");
    }

    #[test]
    fn ctrl_r_and_ctrl_s_go_to_older_and_newer_matches() {
        let mut e = searching(&["a1", "b", "a2", "a3"], "", "a");
        assert_eq!(e.value(), "a3");
        let _ = e.execute(Command::ReverseSearch);
        assert_eq!(e.value(), "a2");
        let _ = e.execute(Command::ReverseSearch);
        assert_eq!(e.value(), "a1");
        // Nothing older: failing, the line keeps the last match.
        let _ = e.execute(Command::ReverseSearch);
        assert_eq!(e.value(), "a1");
        assert!(search_prompt(&e).unwrap().starts_with("(failing search) "));
        let _ = e.execute(Command::ForwardSearch);
        assert_eq!(e.value(), "a2");
        assert_eq!(search_prompt(&e).as_deref(), Some("(search) a: "));
    }

    #[test]
    fn a_failing_query_keeps_the_last_match_and_backspace_searches_again() {
        let mut e = searching(&["make test", "make"], "", "make");
        assert_eq!(e.value(), "make");
        type_text(&mut e, " t");
        assert_eq!(e.value(), "make test");
        type_text(&mut e, "z");
        assert_eq!(e.value(), "make test");
        assert!(search_prompt(&e).unwrap().starts_with("(failing search)"));
        let _ = e.execute(Command::DeleteLeft);
        let _ = e.execute(Command::DeleteLeft);
        let _ = e.execute(Command::DeleteLeft);
        assert_eq!(e.value(), "make");
        // An empty query shows the draft.
        for _ in 0..4 {
            let _ = e.execute(Command::DeleteLeft);
        }
        assert_eq!(e.value(), "");
        assert_eq!(e.execute(Command::DeleteLeft), EditorEvent::Ignored);
    }

    #[test]
    fn enter_submits_the_match() {
        let mut e = searching(&["start alpha", "list"], "", "alp");
        assert_eq!(
            e.execute(Command::Submit),
            EditorEvent::Submit("start alpha".into())
        );
        assert_eq!(search_prompt(&e), None);
    }

    #[test]
    fn exit_keys_keep_the_match_ready_to_edit() {
        let cases = [
            (Command::Cancel, "start |alpha"),
            (Command::MoveLeft, "start| alpha"),
            (Command::MoveRight, "start a|lpha"),
            (Command::MoveStart, "|start alpha"),
            (Command::MoveEnd, "start alpha|"),
        ];
        for (command, after) in cases {
            let mut e = searching(&["start alpha", "list"], "draft", "alp");
            assert_eq!(e.execute(command), EditorEvent::Changed);
            assert_eq!(shown(&e), after, "{command:?}");
            assert_eq!(search_prompt(&e), None, "{command:?}");
            // One undo step back to the draft.
            let _ = e.execute(Command::Undo);
            assert_eq!(e.value(), "draft", "{command:?}");
        }
    }

    #[test]
    fn ctrl_g_and_ctrl_c_cancel_and_restore_the_draft() {
        for command in [Command::Abort, Command::Interrupt] {
            let mut e = searching(&["start alpha"], "dr", "alp");
            // Ctrl+C does not reach the application as an interrupt here.
            assert_eq!(e.execute(command), EditorEvent::Changed, "{command:?}");
            assert_eq!(shown(&e), "dr|", "{command:?}");
            assert_eq!(search_prompt(&e), None, "{command:?}");
        }
    }

    #[test]
    fn another_command_leaves_the_search_then_runs() {
        let mut e = searching(&["start alpha"], "", "alp");
        let _ = e.execute(Command::KillToEnd);
        assert_eq!(shown(&e), "start |");
        assert_eq!(search_prompt(&e), None);
    }

    #[test]
    fn the_search_prompt_and_the_match_are_drawn() {
        let e = &mut searching(&["start alpha"], "", "alp");
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        let cursor = e.render(buf.area(), &mut buf, &Line::from("> "));
        assert_eq!(buf.lines()[0].trim_end(), "(search) alp: start alpha");
        // The cursor is on the match, which is underlined.
        assert_eq!(cursor, Some(Position::new(20, 0)));
        let underlined = |x| {
            buf.get(x, 0)
                .style()
                .add_modifier
                .contains(tilefab_core::Modifier::UNDERLINED)
        };
        assert!(underlined(20) && underlined(22) && !underlined(23) && !underlined(19));
    }

    /// Completes the first word with fixed names; `start` takes an argument.
    struct Names(&'static [&'static str]);

    impl Completer for Names {
        fn complete(&self, line: &str, cursor: usize) -> completion::Completions {
            let word = &line[..cursor];
            let candidates = self
                .0
                .iter()
                .filter(|name| name.starts_with(word))
                .map(|name| {
                    let candidate = completion::Candidate::new(*name);
                    if *name == "start" {
                        candidate.append_space()
                    } else {
                        candidate
                    }
                })
                .collect();
            completion::Completions {
                start: 0,
                candidates,
            }
        }
    }

    fn completing(names: &'static [&'static str], typed: &str) -> LineEditor {
        let mut e = LineEditor::new(EditorConfig::default()).with_completer(Names(names));
        type_text(&mut e, typed);
        e
    }

    fn menu_open(e: &LineEditor) -> bool {
        matches!(e.mode, Mode::Menu(_))
    }

    #[test]
    fn tab_with_no_candidate_or_no_completer_is_ignored() {
        let mut e = completing(&["list"], "x");
        assert_eq!(e.execute(Command::Complete), EditorEvent::Ignored);
        let mut plain = editor("li|");
        assert_eq!(plain.execute(Command::Complete), EditorEvent::Ignored);
    }

    #[test]
    fn tab_accepts_the_hint_only_when_enabled_and_completion_has_nothing_to_do() {
        let history = || with_history(&["start alpha now"]).history;
        let config = EditorConfig {
            tab_accepts_hint: true,
            ..EditorConfig::default()
        };
        // Completion first: the command name, then the hint for the rest.
        let mut e = LineEditor::new(config.clone())
            .with_completer(Names(&["start", "list"]))
            .with_history(history());
        type_text(&mut e, "st");
        assert_eq!(e.execute(Command::Complete), EditorEvent::Changed);
        assert_eq!(shown(&e), "start |");
        assert_eq!(e.execute(Command::Complete), EditorEvent::Changed);
        assert_eq!(shown(&e), "start alpha now|");
        let _ = e.execute(Command::Undo);
        assert_eq!(shown(&e), "start |");
        // Without a completer too.
        let mut plain = LineEditor::new(config).with_history(history());
        type_text(&mut plain, "sta");
        assert_eq!(plain.execute(Command::Complete), EditorEvent::Changed);
        assert_eq!(shown(&plain), "start alpha now|");
        // Disabled (the default): Tab leaves the hint alone.
        let mut off = LineEditor::new(EditorConfig::default())
            .with_completer(Names(&["start"]))
            .with_history(history());
        type_text(&mut off, "start ");
        assert_eq!(off.execute(Command::Complete), EditorEvent::Ignored);
        assert_eq!(shown(&off), "start |");
    }

    #[test]
    fn a_unique_candidate_replaces_the_word_with_its_space() {
        let mut e = completing(&["start", "list"], "st");
        assert_eq!(e.execute(Command::Complete), EditorEvent::Changed);
        assert_eq!(shown(&e), "start |");
        let mut e = completing(&["start", "list"], "l");
        let _ = e.execute(Command::Complete);
        assert_eq!(shown(&e), "list|");
        // Already complete: nothing changes.
        assert_eq!(e.execute(Command::Complete), EditorEvent::Ignored);
        let _ = e.execute(Command::Undo);
        assert_eq!(shown(&e), "l|");
    }

    #[test]
    fn several_candidates_insert_the_common_prefix_then_open_the_menu() {
        let mut e = completing(&["status", "stop", "start"], "s");
        assert_eq!(e.execute(Command::Complete), EditorEvent::Changed);
        assert_eq!(shown(&e), "st|");
        assert!(!menu_open(&e));
        // The prefix adds nothing more: the menu opens on the first candidate.
        let _ = e.execute(Command::Complete);
        assert!(menu_open(&e));
        assert_eq!(shown(&e), "status|");
        assert_eq!(e.extra_rows(80), 1);
        let _ = e.execute(Command::Complete);
        assert_eq!(shown(&e), "stop|");
        let _ = e.execute(Command::HistoryNext);
        assert_eq!(shown(&e), "start|");
        // Wrapping around.
        let _ = e.execute(Command::Complete);
        assert_eq!(shown(&e), "status|");
        let _ = e.execute(Command::HistoryPrevious);
        assert_eq!(shown(&e), "start|");
    }

    #[test]
    fn backtab_opens_the_menu_on_the_last_candidate() {
        let mut e = completing(&["status", "stop"], "st");
        let _ = e.execute(Command::CompletePrevious);
        assert!(menu_open(&e));
        assert_eq!(shown(&e), "stop|");
        let _ = e.execute(Command::CompletePrevious);
        assert_eq!(shown(&e), "status|");
    }

    #[test]
    fn enter_keeps_the_selection_without_submitting() {
        let mut e = completing(&["status", "stop"], "st");
        let _ = e.execute(Command::CompletePrevious);
        assert_eq!(e.execute(Command::Submit), EditorEvent::Changed);
        assert!(!menu_open(&e));
        assert_eq!(shown(&e), "stop|");
        assert_eq!(e.extra_rows(80), 0);
        // One undo step back to the line before Tab.
        let _ = e.execute(Command::Undo);
        assert_eq!(shown(&e), "st|");
    }

    #[test]
    fn esc_ctrl_g_and_ctrl_c_restore_the_line() {
        for command in [Command::Cancel, Command::Abort, Command::Interrupt] {
            let mut e = completing(&["status", "stop"], "st");
            let _ = e.execute(Command::CompletePrevious);
            assert_eq!(e.execute(command), EditorEvent::Changed, "{command:?}");
            assert!(!menu_open(&e));
            assert_eq!(shown(&e), "st|", "{command:?}");
        }
    }

    #[test]
    fn typing_keeps_the_selection_and_backspace_closes_and_deletes() {
        let mut e = completing(&["status", "stop"], "st");
        let _ = e.execute(Command::CompletePrevious);
        type_text(&mut e, "x");
        assert!(!menu_open(&e));
        assert_eq!(shown(&e), "stopx|");
        let mut e = completing(&["status", "stop"], "st");
        let _ = e.execute(Command::CompletePrevious);
        let _ = e.execute(Command::DeleteLeft);
        assert!(!menu_open(&e));
        assert_eq!(shown(&e), "sto|");
    }

    #[test]
    fn the_menu_is_drawn_below_the_prompt_with_the_selection_reversed() {
        let mut e = completing(&["status", "stop", "start"], "st");
        let _ = e.execute(Command::Complete);
        assert_eq!(e.extra_rows(30), 1);
        assert_eq!(e.extra_rows(10), 3);
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 2));
        let cursor = e.render(buf.area(), &mut buf, &Line::from("> "));
        let rows: Vec<String> = buf
            .lines()
            .iter()
            .map(|l| l.trim_end().to_string())
            .collect();
        assert_eq!(rows, ["> status", "status  stop    start"]);
        assert_eq!(cursor, Some(Position::new(8, 0)));
        let reversed = |x| {
            buf.get(x, 1)
                .style()
                .add_modifier
                .contains(tilefab_core::Modifier::REVERSED)
        };
        assert!(reversed(0) && reversed(5) && !reversed(8));
    }

    #[test]
    fn a_completer_cannot_break_the_invariants() {
        struct Bad;
        impl Completer for Bad {
            fn complete(&self, _: &str, _: usize) -> completion::Completions {
                completion::Completions {
                    start: 99,
                    candidates: vec![completion::Candidate::new("x")],
                }
            }
        }
        let mut e = LineEditor::new(EditorConfig::default()).with_completer(Bad);
        type_text(&mut e, "a");
        assert_eq!(e.execute(Command::Complete), EditorEvent::Ignored);
    }

    #[test]
    fn a_paste_is_one_undo_step_on_one_line_and_never_submits() {
        let mut e = editor("a|z");
        assert_eq!(
            e.paste("one\r\ntwo\nthree\tfour\u{1b}[31m"),
            EditorEvent::Changed
        );
        assert_eq!(shown(&e), "aone two three four[31m|z");
        let _ = e.execute(Command::Undo);
        assert_eq!(shown(&e), "a|z");
        assert_eq!(e.paste("\u{7}"), EditorEvent::Ignored);
        let mut small = LineEditor::new(EditorConfig {
            max_len: 4,
            ..EditorConfig::default()
        });
        let _ = small.paste("abcdef");
        assert_eq!(small.value(), "abcd");
    }

    #[test]
    fn a_paste_extends_the_search_query_or_closes_the_menu() {
        let mut e = searching(&["start alpha", "list"], "", "st");
        let _ = e.paste("art a");
        assert_eq!(search_prompt(&e).as_deref(), Some("(search) start a: "));
        assert_eq!(e.value(), "start alpha");
        let mut e = completing(&["status", "stop"], "st");
        let _ = e.execute(Command::CompletePrevious);
        let _ = e.paste(" x");
        assert!(!menu_open(&e));
        assert_eq!(shown(&e), "stop x|");
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
