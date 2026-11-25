//! Editor commands, and the table that binds keys to them.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// An action of the editor: one per binding of the default keymap.
///
/// [`crate::LineEditor::execute`] runs a command directly, without a key: applications
/// and tests can drive the editor with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
// New variants may come in minor versions: `match` on it needs a `_` arm.
#[non_exhaustive]
pub enum Command {
    /// Insert a character at the cursor (ignored for a control character, or when the
    /// line is full).
    InsertChar(char),
    /// One grapheme left.
    MoveLeft,
    /// One grapheme right; at the end of the line, accept the whole hint.
    MoveRight,
    /// To the start of the previous word.
    MoveWordLeft,
    /// To the end of the next word; at the end of the line, accept one word of the hint.
    MoveWordRight,
    /// To the start of the line.
    MoveStart,
    /// To the end of the line; at the end of the line, accept the whole hint.
    MoveEnd,
    /// Delete the grapheme before the cursor.
    DeleteLeft,
    /// Delete the grapheme under the cursor.
    DeleteRight,
    /// Kill to the start of the previous word.
    KillWordLeft,
    /// Kill to the start of the previous whitespace-delimited word (readline
    /// `unix-word-rubout`).
    KillBigWordLeft,
    /// Kill to the end of the next word.
    KillWordRight,
    /// Kill from the start of the line to the cursor (readline `unix-line-discard`).
    KillToStart,
    /// Kill from the cursor to the end of the line.
    KillToEnd,
    /// Kill the whole line.
    KillLine,
    /// Insert the newest kill ring entry.
    Yank,
    /// Right after [`Command::Yank`] or `YankPop` only: replace the yanked text with the
    /// previous kill ring entry.
    YankPop,
    /// Swap the graphemes around the cursor (at the end of the line: the last two).
    TransposeChars,
    /// Swap the word before the cursor with the word after it.
    TransposeWords,
    /// Upper-case from the cursor to the end of the current or next word.
    UpcaseWord,
    /// Lower-case from the cursor to the end of the current or next word.
    DowncaseWord,
    /// Capitalize from the cursor to the end of the current or next word.
    CapitalizeWord,
    /// Undo the last edit.
    Undo,
    /// Redo the last undone edit.
    Redo,
    /// Submit the line ([`crate::EditorEvent::Submit`]).
    Submit,
    /// Interrupt ([`crate::EditorEvent::Interrupt`]).
    Interrupt,
    /// End of input on an empty line ([`crate::EditorEvent::Eof`]), otherwise
    /// [`Command::DeleteRight`].
    DeleteRightOrEof,
    /// Previous history entry that starts with the draft.
    HistoryPrevious,
    /// Next history entry that starts with the draft (after the newest: the draft).
    HistoryNext,
    /// Previous history entry, unfiltered.
    HistoryPreviousAll,
    /// Next history entry, unfiltered.
    HistoryNextAll,
    /// Oldest history entry.
    HistoryFirst,
    /// Newest history entry.
    HistoryLast,
    /// Start a reverse incremental search, or go to the next older match.
    ReverseSearch,
    /// During a search: go to the next newer match.
    ForwardSearch,
    /// Complete the word before the cursor; in the menu, select the next candidate.
    Complete,
    /// Open the completion menu on its last candidate; in the menu, select the previous
    /// candidate.
    CompletePrevious,
    /// `Esc`: dismiss the hint, leave the history or the search, close the menu.
    Cancel,
    /// Ctrl+G: cancel a search or the menu and restore the line as it was before.
    Abort,
}

/// A key with its modifiers, as the [`Keymap`] stores it.
///
/// Built with [`KeyBinding::new`], which normalizes it the same way as incoming keys:
/// `META` counts as `ALT`; for a character, `SHIFT` is dropped (the character already
/// carries the case) and with `CONTROL` an ASCII letter is lowercased; `BackTab` has no
/// modifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyBinding {
    /// The key.
    pub code: KeyCode,
    /// Its modifiers (a combination of `SHIFT`, `CONTROL` and `ALT`).
    pub modifiers: KeyModifiers,
}

impl KeyBinding {
    /// A normalized binding. Return `None` for modifiers the keymap does not handle
    /// (`SUPER`, `HYPER`).
    #[must_use]
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Option<Self> {
        let mut modifiers = modifiers;
        if modifiers.contains(KeyModifiers::META) {
            modifiers.remove(KeyModifiers::META);
            modifiers.insert(KeyModifiers::ALT);
        }
        let handled = KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT;
        if !modifiers.difference(handled).is_empty() {
            return None;
        }
        let code = match code {
            KeyCode::Char(c) => {
                modifiers.remove(KeyModifiers::SHIFT);
                if modifiers.contains(KeyModifiers::CONTROL) {
                    KeyCode::Char(c.to_ascii_lowercase())
                } else {
                    KeyCode::Char(c)
                }
            }
            KeyCode::BackTab => {
                modifiers = KeyModifiers::NONE;
                KeyCode::BackTab
            }
            other => other,
        };
        Some(Self { code, modifiers })
    }

    /// The binding of an incoming key, `None` for a key release or an unhandled modifier.
    #[must_use]
    pub fn from_event(key: KeyEvent) -> Option<Self> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        Self::new(key.code, key.modifiers)
    }
}

/// The table that maps keys to [`Command`]s. The application can change any binding.
///
/// ```
/// use crossterm::event::{KeyCode, KeyModifiers};
/// use tilefab_input::{Command, KeyBinding, Keymap};
///
/// let mut keymap = Keymap::emacs();
/// let ctrl_u = KeyBinding::new(KeyCode::Char('u'), KeyModifiers::CONTROL).unwrap();
/// assert_eq!(keymap.get(ctrl_u), Some(Command::KillToStart));
/// keymap.bind(ctrl_u, Command::KillLine);
/// assert_eq!(keymap.get(ctrl_u), Some(Command::KillLine));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    bindings: HashMap<KeyBinding, Command>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::emacs()
    }
}

impl Keymap {
    /// A keymap without any binding: only plain characters are inserted.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    /// The default bindings, those of readline in Emacs mode.
    #[must_use]
    pub fn emacs() -> Self {
        use Command::{
            Abort, Cancel, CapitalizeWord, Complete, CompletePrevious, DeleteLeft, DeleteRight,
            DeleteRightOrEof, DowncaseWord, ForwardSearch, HistoryFirst, HistoryLast, HistoryNext,
            HistoryNextAll, HistoryPrevious, HistoryPreviousAll, Interrupt, KillBigWordLeft,
            KillToEnd, KillToStart, KillWordLeft, KillWordRight, MoveEnd, MoveLeft, MoveRight,
            MoveStart, MoveWordLeft, MoveWordRight, Redo, ReverseSearch, Submit, TransposeChars,
            TransposeWords, Undo, UpcaseWord, Yank, YankPop,
        };
        use KeyCode::{
            BackTab, Backspace, Char, Delete, Down, End, Enter, Esc, Home, Left, Right, Tab, Up,
        };
        const NONE: KeyModifiers = KeyModifiers::NONE;
        const CTRL: KeyModifiers = KeyModifiers::CONTROL;
        const ALT: KeyModifiers = KeyModifiers::ALT;
        let table = [
            (Enter, NONE, Submit),
            (Char('j'), CTRL, Submit),
            (Char('m'), CTRL, Submit),
            (Char('c'), CTRL, Interrupt),
            (Char('d'), CTRL, DeleteRightOrEof),
            (Left, NONE, MoveLeft),
            (Char('b'), CTRL, MoveLeft),
            (Right, NONE, MoveRight),
            (Char('f'), CTRL, MoveRight),
            (Left, CTRL, MoveWordLeft),
            (Left, ALT, MoveWordLeft),
            (Char('b'), ALT, MoveWordLeft),
            (Right, CTRL, MoveWordRight),
            (Right, ALT, MoveWordRight),
            (Char('f'), ALT, MoveWordRight),
            (Home, NONE, MoveStart),
            (Char('a'), CTRL, MoveStart),
            (End, NONE, MoveEnd),
            (Char('e'), CTRL, MoveEnd),
            (Backspace, NONE, DeleteLeft),
            (Char('h'), CTRL, DeleteLeft),
            (Delete, NONE, DeleteRight),
            (Backspace, ALT, KillWordLeft),
            (Char('w'), CTRL, KillBigWordLeft),
            (Char('d'), ALT, KillWordRight),
            (Delete, CTRL, KillWordRight),
            (Char('u'), CTRL, KillToStart),
            (Char('k'), CTRL, KillToEnd),
            (Char('y'), CTRL, Yank),
            (Char('y'), ALT, YankPop),
            (Char('t'), CTRL, TransposeChars),
            (Char('t'), ALT, TransposeWords),
            (Char('u'), ALT, UpcaseWord),
            (Char('l'), ALT, DowncaseWord),
            (Char('c'), ALT, CapitalizeWord),
            // Most terminals send Ctrl+_ and Ctrl+/ as the byte 0x1F, which crossterm
            // reports as Ctrl+7.
            (Char('_'), CTRL, Undo),
            (Char('/'), CTRL, Undo),
            (Char('7'), CTRL, Undo),
            (Char('z'), CTRL, Undo),
            (Char('_'), ALT, Redo),
            (Char('/'), ALT, Redo),
            (Up, NONE, HistoryPrevious),
            (Down, NONE, HistoryNext),
            (Char('p'), CTRL, HistoryPreviousAll),
            (Char('n'), CTRL, HistoryNextAll),
            (Char('<'), ALT, HistoryFirst),
            (Char('>'), ALT, HistoryLast),
            (Char('r'), CTRL, ReverseSearch),
            (Char('s'), CTRL, ForwardSearch),
            (Tab, NONE, Complete),
            (BackTab, NONE, CompletePrevious),
            (Esc, NONE, Cancel),
            (Char('g'), CTRL, Abort),
        ];
        let mut keymap = Self::empty();
        for (code, modifiers, command) in table {
            if let Some(key) = KeyBinding::new(code, modifiers) {
                keymap.bind(key, command);
            }
        }
        keymap
    }

    /// Bind `key` to `command`; return the command it was bound to before.
    pub fn bind(&mut self, key: KeyBinding, command: Command) -> Option<Command> {
        let key = KeyBinding::new(key.code, key.modifiers)?;
        self.bindings.insert(key, command)
    }

    /// Remove the binding of `key`; return the command it was bound to.
    pub fn unbind(&mut self, key: KeyBinding) -> Option<Command> {
        let key = KeyBinding::new(key.code, key.modifiers)?;
        self.bindings.remove(&key)
    }

    /// The command bound to `key`.
    #[must_use]
    pub fn get(&self, key: KeyBinding) -> Option<Command> {
        let key = KeyBinding::new(key.code, key.modifiers)?;
        self.bindings.get(&key).copied()
    }

    /// The command for an incoming key: its binding, else [`Command::InsertChar`] for an
    /// unbound character typed without modifier or with exactly `CONTROL | ALT` (`AltGr` on
    /// Windows). `None` for anything else: key releases, unbound Alt or Ctrl shortcuts,
    /// unhandled modifiers.
    #[must_use]
    pub fn command_for(&self, key: KeyEvent) -> Option<Command> {
        let key = KeyBinding::from_event(key)?;
        if let Some(command) = self.bindings.get(&key) {
            return Some(*command);
        }
        match key.code {
            KeyCode::Char(c)
                if key.modifiers.is_empty()
                    || key.modifiers == KeyModifiers::CONTROL.union(KeyModifiers::ALT) =>
            {
                Some(Command::InsertChar(c))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;
    const ALT: KeyModifiers = KeyModifiers::ALT;
    const SHIFT: KeyModifiers = KeyModifiers::SHIFT;

    #[test]
    fn meta_counts_as_alt() {
        let keymap = Keymap::emacs();
        let meta_b = press(KeyCode::Char('b'), KeyModifiers::META);
        assert_eq!(keymap.command_for(meta_b), Some(Command::MoveWordLeft));
        let alt_b = press(KeyCode::Char('b'), ALT);
        assert_eq!(keymap.command_for(alt_b), Some(Command::MoveWordLeft));
    }

    #[test]
    fn shift_is_dropped_on_characters_and_ctrl_letters_are_lowercased() {
        let keymap = Keymap::emacs();
        let upper = press(KeyCode::Char('A'), SHIFT);
        assert_eq!(keymap.command_for(upper), Some(Command::InsertChar('A')));
        let ctrl_shift_a = press(KeyCode::Char('A'), CTRL | SHIFT);
        assert_eq!(keymap.command_for(ctrl_shift_a), Some(Command::MoveStart));
        let alt_gt = press(KeyCode::Char('>'), ALT | SHIFT);
        assert_eq!(keymap.command_for(alt_gt), Some(Command::HistoryLast));
    }

    #[test]
    fn ctrl_7_and_backtab_with_any_modifier() {
        let keymap = Keymap::emacs();
        assert_eq!(
            keymap.command_for(press(KeyCode::Char('7'), CTRL)),
            Some(Command::Undo)
        );
        assert_eq!(
            keymap.command_for(press(KeyCode::BackTab, SHIFT)),
            Some(Command::CompletePrevious)
        );
    }

    #[test]
    fn releases_unbound_shortcuts_and_super_are_ignored() {
        let keymap = Keymap::emacs();
        let release = KeyEvent::new_with_kind(KeyCode::Char('x'), NONE, KeyEventKind::Release);
        assert_eq!(keymap.command_for(release), None);
        let repeat = KeyEvent::new_with_kind(KeyCode::Char('x'), NONE, KeyEventKind::Repeat);
        assert_eq!(keymap.command_for(repeat), Some(Command::InsertChar('x')));
        assert_eq!(keymap.command_for(press(KeyCode::Char('q'), ALT)), None);
        assert_eq!(keymap.command_for(press(KeyCode::Char('q'), CTRL)), None);
        let super_a = press(KeyCode::Char('a'), KeyModifiers::SUPER);
        assert_eq!(keymap.command_for(super_a), None);
        assert_eq!(keymap.command_for(press(KeyCode::F(1), NONE)), None);
    }

    #[test]
    fn altgr_characters_are_inserted() {
        let keymap = Keymap::emacs();
        let altgr = press(KeyCode::Char('@'), CTRL | ALT);
        assert_eq!(keymap.command_for(altgr), Some(Command::InsertChar('@')));
    }

    #[test]
    fn bind_and_unbind() {
        let mut keymap = Keymap::emacs();
        let ctrl_w = KeyBinding::new(KeyCode::Char('W'), CTRL | SHIFT).unwrap();
        assert_eq!(ctrl_w.code, KeyCode::Char('w'));
        assert_eq!(
            keymap.bind(ctrl_w, Command::KillWordLeft),
            Some(Command::KillBigWordLeft)
        );
        assert_eq!(
            keymap.command_for(press(KeyCode::Char('w'), CTRL)),
            Some(Command::KillWordLeft)
        );
        assert_eq!(keymap.unbind(ctrl_w), Some(Command::KillWordLeft));
        assert_eq!(keymap.command_for(press(KeyCode::Char('w'), CTRL)), None);
        assert_eq!(
            Keymap::empty().command_for(press(KeyCode::Enter, NONE)),
            None
        );
    }
}
