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
    /// The hint drawn by `render`, computed again only when the line or the history changed.
    hint_cache: HintCache,
    mode: Mode,
}

/// The last hint computed for `render`: a frame that does not change the line asks nothing
/// of the hinter, and allocates nothing.
#[derive(Debug, Default)]
struct HintCache {
    /// The line the hint was computed for.
    text: String,
    /// The hint, if any.
    hint: Option<String>,
    /// False when the history or the hinter changed since.
    valid: bool,
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
            hint_cache: HintCache::default(),
            mode: Mode::Normal,
        }
    }

    /// Use `history` (for instance one loaded from a file) instead of an empty one.
    #[must_use]
    pub fn with_history(mut self, history: History) -> Self {
        self.history = history;
        self.hint_cache.valid = false;
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
        self.hint_cache.valid = false;
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
    ///
    /// It also ends what the previous keys had started: a search or the completion menu is
    /// closed (the new value is kept, not the match nor the candidate), and a following
    /// `YankPop` has no yank to replace.
    pub fn set_value(&mut self, value: &str) {
        self.last = Last::Other;
        self.mode = Mode::Normal;
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
        let hint = if self.refresh_hint() {
            self.hint_cache.hint.as_deref()
        } else {
            None
        };
        let (shown_prompt, highlight) = match &self.mode {
            Mode::Search(search) => (
                render::Prompt::Parts(search.prompt_parts()),
                search.found.as_ref().map(|(_, range)| range.clone()),
            ),
            Mode::Normal | Mode::Menu(_) => (render::Prompt::Line(prompt), None),
        };
        let view = LineView {
            text: self.buffer.value(),
            cursor: self.buffer.cursor(),
            hint,
            highlight,
        };
        let cursor = render::draw_line(
            buf,
            row,
            shown_prompt,
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

    /// Whether a hint may be shown now: hints enabled, cursor at the end of a non-empty line,
    /// not dismissed with `Esc`, no search nor menu.
    fn hint_allowed(&self) -> bool {
        let text = self.buffer.value();
        self.config.hints
            && matches!(self.mode, Mode::Normal)
            && !self.hint_dismissed
            && !text.is_empty()
            && self.buffer.cursor() == text.len()
    }

    /// Bring the hint cache up to date for `render`; false when no hint may be shown now.
    fn refresh_hint(&mut self) -> bool {
        if !self.hint_allowed() {
            return false;
        }
        if !self.hint_cache.valid || self.hint_cache.text != self.buffer.value() {
            self.hint_cache.hint = self.current_hint();
            self.hint_cache.text.clear();
            self.hint_cache.text.push_str(self.buffer.value());
            self.hint_cache.valid = true;
        }
        true
    }

    /// The hint to show after the line, if any: hints enabled, cursor at the end of a
    /// non-empty line, not dismissed with `Esc`. Sanitized and cut so that accepting it
    /// keeps the line within its maximum length.
    fn current_hint(&self) -> Option<String> {
        if !self.hint_allowed() {
            return None;
        }
        let text = self.buffer.value();
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
        // No modulo here: `KillRing::get` wraps the index around the ring.
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
            self.hint_cache.valid = false;
        }
        self.buffer.clear();
        self.undo.clear();
        self.line_changed();
        self.scroll = 0;
        EditorEvent::Submit(line)
    }
}

#[cfg(test)]
mod tests;
