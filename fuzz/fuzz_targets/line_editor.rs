//! `LineEditor` with arbitrary commands, pastes, `set_value` calls and renders at any width:
//! nothing panics, and the invariants of the editor proptest hold after each call (length at
//! most `max_len`, no control character, cursor within the line); the rendered buffer keeps
//! its invariants and the cursor it returns lies in the area.

#![no_main]

#[path = "common.rs"]
mod common;

use arbitrary::Arbitrary;
use common::{Piece, check_buffer, text};
use libfuzzer_sys::fuzz_target;
use tilefab_core::{Buffer, Line, Rect};
use tilefab_input::{
    Candidate, Command, Completer, Completions, EditorConfig, History, HistoryHinter, LineEditor,
};

/// Completes a few command names, from the start of the line.
struct Names;

impl Completer for Names {
    fn complete(&self, line: &str, cursor: usize) -> Completions {
        let typed = line.get(..cursor).unwrap_or("");
        let candidates = ["start", "status", "stop", "list", "日本語"]
            .into_iter()
            .filter(|name| name.starts_with(typed))
            .map(|name| Candidate::new(name).description("a command"))
            .collect();
        Completions {
            start: 0,
            candidates,
        }
    }
}

/// The editing commands, by index.
const COMMANDS: &[Command] = &[
    Command::MoveLeft,
    Command::MoveRight,
    Command::MoveWordLeft,
    Command::MoveWordRight,
    Command::MoveStart,
    Command::MoveEnd,
    Command::DeleteLeft,
    Command::DeleteRight,
    Command::KillWordLeft,
    Command::KillBigWordLeft,
    Command::KillWordRight,
    Command::KillToStart,
    Command::KillToEnd,
    Command::KillLine,
    Command::Yank,
    Command::YankPop,
    Command::TransposeChars,
    Command::TransposeWords,
    Command::UpcaseWord,
    Command::DowncaseWord,
    Command::CapitalizeWord,
    Command::Undo,
    Command::Redo,
    Command::Submit,
    Command::Interrupt,
    Command::DeleteRightOrEof,
    Command::HistoryPrevious,
    Command::HistoryNext,
    Command::HistoryPreviousAll,
    Command::HistoryNextAll,
    Command::HistoryFirst,
    Command::HistoryLast,
    Command::ReverseSearch,
    Command::ForwardSearch,
    Command::Complete,
    Command::CompletePrevious,
];

#[derive(Debug, Arbitrary)]
enum Action {
    Command(u8),
    Insert(char),
    Paste(Vec<Piece>),
    SetValue(Vec<Piece>),
    Render(u8),
}

#[derive(Debug, Arbitrary)]
struct Input {
    max_len: u8,
    history: Vec<Vec<Piece>>,
    actions: Vec<Action>,
}

fn check(editor: &LineEditor, max_len: usize) {
    let value = editor.value();
    assert!(
        value.len() <= max_len,
        "line longer than max_len: {value:?}"
    );
    assert!(
        !value.chars().any(char::is_control),
        "control character in {value:?}"
    );
    let graphemes = unicode_segmentation::UnicodeSegmentation::graphemes(value, true).count();
    assert!(
        editor.cursor() <= graphemes,
        "cursor past the end of {value:?}"
    );
}

fuzz_target!(|input: Input| {
    let max_len = usize::from(input.max_len).max(1);
    let mut history = History::new(20);
    for entry in input.history.iter().take(20) {
        history.add(&text(entry));
    }
    let mut editor = LineEditor::new(EditorConfig::default().max_len(max_len))
        .with_history(history)
        .with_hinter(HistoryHinter)
        .with_completer(Names);
    let prompt = Line::from("> ");
    for action in input.actions.iter().take(64) {
        match action {
            Action::Command(index) => {
                let _ = editor.execute(COMMANDS[usize::from(*index) % COMMANDS.len()]);
            }
            Action::Insert(c) => {
                let _ = editor.execute(Command::InsertChar(*c));
            }
            Action::Paste(pieces) => {
                let _ = editor.paste(&text(pieces));
            }
            Action::SetValue(pieces) => editor.set_value(&text(pieces)),
            Action::Render(width) => {
                let width = u16::from(*width % 80);
                let rows = 1 + editor.extra_rows(width);
                let area = Rect::new(0, 0, width, rows);
                let mut buf = Buffer::empty(area);
                if let Some(cursor) = editor.render(area, &mut buf, &prompt) {
                    assert!(
                        cursor.x < area.right().max(1) && cursor.y < area.bottom().max(1),
                        "cursor {cursor:?} outside {area:?}"
                    );
                }
                check_buffer(&buf);
            }
        }
        check(&editor, max_len);
    }
});
