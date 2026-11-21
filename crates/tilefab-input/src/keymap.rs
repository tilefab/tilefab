//! Editor commands, and the table that binds keys to them.

/// An action of the editor: one per binding of the default keymap.
///
/// [`crate::LineEditor::execute`] runs a command directly, without a key: applications
/// and tests can drive the editor with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
}
