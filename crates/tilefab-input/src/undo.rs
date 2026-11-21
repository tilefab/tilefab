//! Undo / redo: a stack of snapshots of the line, with grouping of typed words.

/// Maximum number of undo steps kept.
const MAX_STEPS: usize = 100;

/// The line at one point in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Snapshot {
    /// The text.
    pub(crate) value: String,
    /// The cursor, in bytes.
    pub(crate) cursor: usize,
}

/// How an edit is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// A typed non-whitespace character: consecutive ones form one step (one word).
    Word,
    /// Any other edit: one step of its own.
    Single,
}

/// Undo and redo stacks.
#[derive(Debug, Clone, Default)]
pub(crate) struct UndoStack {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// A `Step::Word` group is open: the next `Step::Word` joins it.
    open_group: bool,
}

impl UndoStack {
    /// Record an edit, given the line as it was **before** it. Any new edit clears the
    /// redo stack.
    pub(crate) fn record(&mut self, before: Snapshot, step: Step) {
        self.redo.clear();
        if step == Step::Word && self.open_group {
            return;
        }
        self.undo.push(before);
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
        self.open_group = step == Step::Word;
    }

    /// Close the open word group: the next typed character starts a new step.
    pub(crate) fn close_group(&mut self) {
        self.open_group = false;
    }

    /// The snapshot to restore, given the `current` line (kept for redo).
    pub(crate) fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        self.open_group = false;
        let previous = self.undo.pop()?;
        self.redo.push(current);
        Some(previous)
    }

    /// The snapshot to restore again after an undo, given the `current` line.
    pub(crate) fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        self.open_group = false;
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }

    /// Forget everything.
    pub(crate) fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.open_group = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(value: &str) -> Snapshot {
        Snapshot {
            value: value.to_string(),
            cursor: value.len(),
        }
    }

    #[test]
    fn a_typed_word_is_one_step() {
        let mut stack = UndoStack::default();
        stack.record(snap(""), Step::Word);
        stack.record(snap("a"), Step::Word);
        stack.record(snap("ab"), Step::Word);
        assert_eq!(stack.undo(snap("abc")), Some(snap("")));
        assert_eq!(stack.undo(snap("")), None);
    }

    #[test]
    fn closing_the_group_starts_a_new_step() {
        let mut stack = UndoStack::default();
        stack.record(snap(""), Step::Word);
        stack.close_group();
        stack.record(snap("a"), Step::Word);
        assert_eq!(stack.undo(snap("ab")), Some(snap("a")));
        assert_eq!(stack.undo(snap("a")), Some(snap("")));
    }

    #[test]
    fn redo_is_cleared_by_a_new_edit() {
        let mut stack = UndoStack::default();
        stack.record(snap(""), Step::Single);
        assert_eq!(stack.undo(snap("x")), Some(snap("")));
        assert_eq!(stack.redo(snap("")), Some(snap("x")));
        assert_eq!(stack.undo(snap("x")), Some(snap("")));
        stack.record(snap(""), Step::Single);
        assert_eq!(stack.redo(snap("y")), None);
    }

    #[test]
    fn the_stack_keeps_the_newest_steps() {
        let mut stack = UndoStack::default();
        for i in 0..=MAX_STEPS {
            stack.record(snap(&i.to_string()), Step::Single);
        }
        let mut count = 0;
        let mut last = None;
        while let Some(s) = stack.undo(snap("")) {
            count += 1;
            last = Some(s);
        }
        assert_eq!(count, MAX_STEPS);
        assert_eq!(last, Some(snap("1")));
    }
}
