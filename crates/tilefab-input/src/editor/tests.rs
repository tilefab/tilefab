//! Tests of the line editor: the commands, the history, the search, the completion menu,
//! the rendering, and a proptest of the invariants after any command.
//!
//! The command tables of the tests read better without the `Command::` prefix.

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
fn set_value_after_a_yank_forgets_the_yanked_range() {
    let mut e = editor("|");
    e.set_value("hello");
    let _ = e.execute(Command::KillLine);
    let _ = e.execute(Command::Yank);
    e.set_value("x");
    // The yank no longer applies to the line: nothing to replace.
    assert_eq!(e.execute(Command::YankPop), EditorEvent::Ignored);
    assert_eq!(shown(&e), "x|");
}

#[test]
fn set_value_closes_the_menu_and_keeps_the_new_value() {
    let mut e = completing(&["status", "stop"], "st");
    let _ = e.execute(Command::CompletePrevious);
    assert!(menu_open(&e));
    e.set_value("list");
    assert!(!menu_open(&e));
    let _ = e.execute(Command::Complete);
    assert_eq!(e.value(), "list");
}

#[test]
fn set_value_leaves_the_search_and_keeps_the_new_value() {
    let mut e = searching(&["start alpha", "list"], "", "st");
    e.set_value("help");
    assert_eq!(search_prompt(&e), None);
    assert_eq!(shown(&e), "help|");
    type_text(&mut e, "!");
    assert_eq!(e.value(), "help!");
}

#[test]
fn the_public_cursor_counts_graphemes() {
    let e = editor("🤦🏼\u{200d}♂\u{fe0f}e\u{301}|");
    assert_eq!(e.cursor(), 2);
}

#[test]
fn the_accessors_reach_the_editor_itself() {
    let config = EditorConfig::default().max_len(12);
    let mut e = LineEditor::new(config.clone());
    assert_eq!(e.config().max_len, 12);
    e.history_mut().add("from outside");
    assert_eq!(e.history().get(0), Some("from outside"));
    // A key bound through `keymap_mut` runs its command.
    let key = crate::KeyBinding::new(crossterm::event::KeyCode::F(2), KeyModifiers::NONE).unwrap();
    e.keymap_mut().bind(key, Command::InsertChar('!'));
    let _ = e.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::F(2),
        KeyModifiers::NONE,
    ));
    assert_eq!(e.value(), "!");
    let debug = format!("{e:?}");
    assert!(
        debug.contains("LineEditor") && debug.contains("\"!\""),
        "{debug}"
    );
}

#[test]
fn a_history_write_error_is_reported_once_by_the_editor() {
    let dir = crate::test_dir::TempDir::new("editor");
    let path = dir.0.join("history");
    let (history, error) = History::with_file(&path, 10);
    assert!(error.is_none());
    // The file becomes a directory: the next append fails.
    std::fs::create_dir_all(&path).unwrap();
    let mut e = LineEditor::new(EditorConfig::default()).with_history(history);
    type_text(&mut e, "ls");
    let _ = e.execute(Command::Submit);
    assert!(e.take_history_error().is_some());
    assert!(e.take_history_error().is_none());
}

#[test]
fn the_search_query_refuses_control_characters_and_stays_within_max_len() {
    let mut history = History::new(10);
    history.add("abc");
    let mut e = LineEditor::new(EditorConfig::default().max_len(3)).with_history(history);
    let _ = e.execute(Command::ReverseSearch);
    assert_eq!(
        e.execute(Command::InsertChar('\u{7}')),
        EditorEvent::Ignored
    );
    for c in ['a', 'b', 'c'] {
        assert_ne!(e.execute(Command::InsertChar(c)), EditorEvent::Ignored);
    }
    // 3 bytes, the maximum: a fourth is refused.
    assert_eq!(e.execute(Command::InsertChar('d')), EditorEvent::Ignored);
    assert_eq!(search_prompt(&e).as_deref(), Some("(search) abc: "));
}

#[test]
fn next_and_previous_match_need_a_query() {
    let mut e = searching(&["start alpha"], "", "");
    assert_eq!(e.execute(Command::ReverseSearch), EditorEvent::Ignored);
    assert_eq!(e.execute(Command::ForwardSearch), EditorEvent::Ignored);
    assert_eq!(search_prompt(&e).as_deref(), Some("(search) : "));
}

#[test]
fn unfiltered_history_goes_back_down_to_newer_entries() {
    let mut e = with_history(&["one", "two", "three"]);
    let _ = e.execute(Command::HistoryPreviousAll);
    let _ = e.execute(Command::HistoryPreviousAll);
    assert_eq!(e.value(), "two");
    let _ = e.execute(Command::HistoryNextAll);
    assert_eq!(e.value(), "three");
}

#[test]
fn transpose_words_uses_the_word_after_the_cursor_when_there_is_one() {
    assert_eq!(
        run("one |two three", Command::TransposeWords).1,
        "two one| three"
    );
}

/// The hint drawn after the line, as `render` shows it.
fn drawn_hint(e: &mut LineEditor) -> String {
    let mut buf = Buffer::empty(Rect::new(0, 0, 30, 1));
    let _ = e.render(buf.area(), &mut buf, &Line::from(""));
    buf.lines()[0].trim_end()[e.value().len()..].to_string()
}

#[test]
fn the_drawn_hint_follows_the_line_and_the_history() {
    let mut e = with_history(&["start alpha"]);
    type_text(&mut e, "st");
    assert_eq!(drawn_hint(&mut e), "art alpha");
    // The same line, a new history entry: computed again.
    e.history_mut().add("stop now");
    assert_eq!(drawn_hint(&mut e), "op now");
    // Another line: computed again.
    type_text(&mut e, "a");
    assert_eq!(drawn_hint(&mut e), "rt alpha");
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

/// A call on the editor: a command, or one of the methods that replace or insert text.
#[derive(Debug, Clone)]
enum Action {
    Run(Command),
    SetValue(String),
    Paste(String),
}

fn action_strategy() -> impl Strategy<Value = Action> {
    prop_oneof![
        8 => command_strategy().prop_map(Action::Run),
        1 => text_strategy().prop_map(Action::SetValue),
        1 => text_strategy().prop_map(Action::Paste),
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
    fn invariants_hold_after_set_value_and_paste_calls(
        text in text_strategy(),
        actions in prop::collection::vec(action_strategy(), 0..40),
        max_len in 1usize..40,
    ) {
        let mut e = LineEditor::new(EditorConfig { max_len, ..EditorConfig::default() });
        e.set_value(&text);
        for action in actions {
            match action {
                Action::Run(command) => {
                    let _ = e.execute(command);
                }
                Action::SetValue(text) => e.set_value(&text),
                Action::Paste(text) => {
                    let _ = e.paste(&text);
                }
            }
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
