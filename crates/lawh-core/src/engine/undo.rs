//! The undo history: each change to the board is one step of edits, which can be taken back
//! and made again.

use std::collections::VecDeque;
use std::mem::size_of;

use super::cursor::Cursor;
use crate::board::{Board, BoardError, Object, ObjectId, Shape};

/// Most steps the undo history keeps; the oldest are dropped past it. Vim's default
/// `undolevels`.
pub const MAX_UNDO_STEPS: usize = 1000;

/// Most memory the undo history holds, in bytes as estimated from its objects (16 MiB); the
/// oldest steps are dropped past it, though never the newest.
///
/// A step holds every object it changed, whole. The largest step the board's caps allow
/// changes a whole board: 500 objects of up to 184 bytes each (a 120-byte edit on Linux
/// `x86_64` and a 64-character id) and 40,000 characters of text at up to 4 bytes each, or twice
/// that text when it is replaced: about 0.4 MB in all (derived from the caps and the type
/// sizes, not measured). The cap keeps at least 40 such steps. Making a circle takes about 200
/// bytes, so [`MAX_UNDO_STEPS`] of those fit far below it. Editing text keeps the text before
/// and after, so 1,000 edits of a 5,000-character text would hold up to 40 MB without this cap.
pub const MAX_UNDO_BYTES: usize = 16 * 1024 * 1024;

/// One change to the board's objects.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Edit {
    /// `object` was put at position `index` in z-order.
    Insert { index: usize, object: Object },
    /// `object` was taken from position `index`.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "made by the board operators (`d`, `x`), which come next"
        )
    )]
    Remove { index: usize, object: Object },
    /// Object `id` changed shape from `before` to `after`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "made by text editing, which comes next")
    )]
    Replace {
        id: ObjectId,
        before: Shape,
        after: Shape,
    },
}

impl Edit {
    /// Makes the edit on `board`.
    pub(crate) fn apply(&self, board: &mut Board) -> Result<(), BoardError> {
        match self {
            Self::Insert { index, object } => board.insert_at(*index, object.clone()),
            Self::Remove { object, .. } => remove(board, &object.id),
            Self::Replace { id, after, .. } => board.replace(id, after.clone()).map(|_| ()),
        }
    }

    /// Takes the edit back on `board`.
    fn revert(&self, board: &mut Board) -> Result<(), BoardError> {
        match self {
            Self::Insert { object, .. } => remove(board, &object.id),
            Self::Remove { index, object } => board.insert_at(*index, object.clone()),
            Self::Replace { id, before, .. } => board.replace(id, before.clone()).map(|_| ()),
        }
    }

    /// Bytes the edit holds, roughly: itself plus its ids and text.
    fn bytes(&self) -> usize {
        size_of::<Self>()
            + match self {
                Self::Insert { object, .. } | Self::Remove { object, .. } => {
                    object.id.as_str().len() + content_bytes(&object.shape)
                }
                Self::Replace { id, before, after } => {
                    id.as_str().len() + content_bytes(before) + content_bytes(after)
                }
            }
    }
}

fn remove(board: &mut Board, id: &ObjectId) -> Result<(), BoardError> {
    match board.remove(id) {
        Some(_) => Ok(()),
        None => Err(BoardError::NoSuchObject(id.clone())),
    }
}

fn content_bytes(shape: &Shape) -> usize {
    match shape {
        Shape::Text { text, .. } => text.len(),
        Shape::Math { src, .. } => src.len(),
        Shape::Ellipse { .. } | Shape::Rect { .. } | Shape::Line { .. } | Shape::Arrow { .. } => 0,
    }
}

/// What one command did: its edits in order, and the cursor before and after it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Step {
    pub(crate) edits: Vec<Edit>,
    pub(crate) cursor_before: Cursor,
    pub(crate) cursor_after: Cursor,
}

impl Step {
    /// Bytes the step holds, roughly, for [`MAX_UNDO_BYTES`].
    fn bytes(&self) -> usize {
        size_of::<Self>() + self.edits.iter().map(Edit::bytes).sum::<usize>()
    }

    /// Makes the edits in order. When one fails, those made are taken back, so the board is
    /// as it was, and the error is returned.
    fn redo(&self, board: &mut Board) -> Result<(), BoardError> {
        for (made, edit) in self.edits.iter().enumerate() {
            if let Err(error) = edit.apply(board) {
                for edit in self.edits.iter().take(made).rev() {
                    // Taking back an edit just made cannot fail.
                    let _ = edit.revert(board);
                }
                return Err(error);
            }
        }
        Ok(())
    }

    /// Takes the edits back, newest first. When one fails, those taken back are made again,
    /// so the board is as it was, and the error is returned.
    fn undo(&self, board: &mut Board) -> Result<(), BoardError> {
        for (taken, edit) in self.edits.iter().rev().enumerate() {
            if let Err(error) = edit.revert(board) {
                for edit in self.edits.iter().rev().take(taken).rev() {
                    // Making again an edit just taken back cannot fail.
                    let _ = edit.apply(board);
                }
                return Err(error);
            }
        }
        Ok(())
    }
}

/// The steps made so far, oldest first, within [`MAX_UNDO_STEPS`] and [`MAX_UNDO_BYTES`].
///
/// The first `done` steps are on the board; the rest were undone and can be redone until a
/// new step replaces them (a straight line of changes, not Vim's undo tree).
#[derive(Debug, Clone)]
pub(crate) struct History {
    steps: VecDeque<Step>,
    done: usize,
    /// The value of `done` when the board was last saved (or opened): the board is modified
    /// whenever `done` is anything else. `None` once that state has left the history.
    saved: Option<usize>,
    /// Sum of the steps' [`Step::bytes`].
    bytes: usize,
}

impl Default for History {
    fn default() -> Self {
        Self {
            steps: VecDeque::new(),
            done: 0,
            saved: Some(0),
            bytes: 0,
        }
    }
}

impl History {
    /// Adds a step already made on the board. Steps that were undone are dropped, then the
    /// oldest steps past the caps.
    pub(crate) fn push(&mut self, step: Step) {
        while self.steps.len() > self.done {
            if let Some(old) = self.steps.pop_back() {
                self.bytes = self.bytes.saturating_sub(old.bytes());
            }
        }
        if self.saved.is_some_and(|saved| saved > self.done) {
            self.saved = None;
        }
        self.bytes = self.bytes.saturating_add(step.bytes());
        self.steps.push_back(step);
        self.done += 1;
        while self.steps.len() > 1
            && (self.steps.len() > MAX_UNDO_STEPS || self.bytes > MAX_UNDO_BYTES)
        {
            if let Some(old) = self.steps.pop_front() {
                self.bytes = self.bytes.saturating_sub(old.bytes());
            }
            self.done = self.done.saturating_sub(1);
            self.saved = self.saved.and_then(|saved| saved.checked_sub(1));
        }
    }

    /// Takes the newest step back on `board`: the cursor from before it, or `None` when there
    /// is nothing to undo. On an error the board and the history stay as they were.
    pub(crate) fn undo(&mut self, board: &mut Board) -> Option<Result<Cursor, BoardError>> {
        let index = self.done.checked_sub(1)?;
        let step = self.steps.get(index)?;
        if let Err(error) = step.undo(board) {
            return Some(Err(error));
        }
        self.done = index;
        Some(Ok(step.cursor_before.clone()))
    }

    /// Makes the oldest undone step again on `board`: the cursor from after it, or `None` when
    /// there is nothing to redo. On an error the board and the history stay as they were.
    pub(crate) fn redo(&mut self, board: &mut Board) -> Option<Result<Cursor, BoardError>> {
        let step = self.steps.get(self.done)?;
        if let Err(error) = step.redo(board) {
            return Some(Err(error));
        }
        self.done += 1;
        Some(Ok(step.cursor_after.clone()))
    }

    /// The board differs from when it was last saved.
    pub(crate) fn is_modified(&self) -> bool {
        self.saved != Some(self.done)
    }

    /// The board is saved as it is now.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "called when `:w` has written the file, which comes next"
        )
    )]
    pub(crate) fn mark_saved(&mut self) {
        self.saved = Some(self.done);
    }
}

#[cfg(test)]
mod tests {
    use kurbo::Point;

    use super::*;

    fn id(s: &str) -> ObjectId {
        ObjectId::new(s).unwrap()
    }

    fn circle(name: &str, r: f64) -> Object {
        Object::new(
            id(name),
            Shape::Ellipse {
                center: Point::ZERO,
                rx: r,
                ry: r,
            },
        )
    }

    fn grid(x: f64) -> Cursor {
        Cursor::Grid(Point::new(x, 0.0))
    }

    fn step(edits: Vec<Edit>, before: f64, after: f64) -> Step {
        Step {
            edits,
            cursor_before: grid(before),
            cursor_after: grid(after),
        }
    }

    /// Makes `step` on `board` and records it, as the engine does.
    fn make(history: &mut History, board: &mut Board, step: Step) {
        step.redo(board).unwrap();
        history.push(step);
    }

    #[test]
    fn every_kind_of_edit_undoes_and_redoes() {
        let mut board = Board::new();
        let mut history = History::default();
        make(
            &mut history,
            &mut board,
            step(
                vec![
                    Edit::Insert {
                        index: 0,
                        object: circle("c1", 1.0),
                    },
                    Edit::Insert {
                        index: 0,
                        object: circle("c0", 1.0),
                    },
                ],
                0.0,
                1.0,
            ),
        );
        let start = board.clone();
        make(
            &mut history,
            &mut board,
            step(
                vec![
                    Edit::Remove {
                        index: 0,
                        object: circle("c0", 1.0),
                    },
                    Edit::Replace {
                        id: id("c1"),
                        before: circle("c1", 1.0).shape,
                        after: circle("c1", 2.0).shape,
                    },
                ],
                1.0,
                2.0,
            ),
        );
        let end = board.clone();
        assert_eq!(board.objects(), [circle("c1", 2.0)]);

        assert_eq!(history.undo(&mut board), Some(Ok(grid(1.0))));
        assert_eq!(board, start);
        assert_eq!(history.redo(&mut board), Some(Ok(grid(2.0))));
        assert_eq!(board, end);
        assert_eq!(history.redo(&mut board), None);
        assert_eq!(history.undo(&mut board), Some(Ok(grid(1.0))));
        assert_eq!(history.undo(&mut board), Some(Ok(grid(0.0))));
        assert!(board.is_empty());
        assert_eq!(history.undo(&mut board), None);
    }

    #[test]
    fn a_failing_step_leaves_the_board_and_history_as_they_were() {
        let mut board = Board::new();
        board.insert(circle("c9", 1.0)).unwrap();
        let start = board.clone();
        // The second edit fails: `c9` is already on the board.
        let bad = step(
            vec![
                Edit::Insert {
                    index: 0,
                    object: circle("c1", 1.0),
                },
                Edit::Insert {
                    index: 0,
                    object: circle("c9", 1.0),
                },
            ],
            0.0,
            1.0,
        );
        assert_eq!(bad.redo(&mut board), Err(BoardError::DuplicateId(id("c9"))));
        assert_eq!(board, start);

        // Taking back a removal fails half way when the id was taken again since.
        let mut history = History::default();
        let removal = step(
            vec![
                Edit::Remove {
                    index: 0,
                    object: circle("c9", 1.0),
                },
                Edit::Insert {
                    index: 0,
                    object: circle("c2", 1.0),
                },
            ],
            0.0,
            1.0,
        );
        make(&mut history, &mut board, removal);
        board.insert(circle("c9", 5.0)).unwrap();
        let tampered = board.clone();
        assert_eq!(
            history.undo(&mut board),
            Some(Err(BoardError::DuplicateId(id("c9"))))
        );
        assert_eq!(board, tampered);
        assert!(history.is_modified());
        assert_eq!(history.redo(&mut board), None);
    }

    #[test]
    fn modified_follows_the_saved_position() {
        let mut board = Board::new();
        let mut history = History::default();
        assert!(!history.is_modified());
        let insert = |name: &str| {
            step(
                vec![Edit::Insert {
                    index: 0,
                    object: circle(name, 1.0),
                }],
                0.0,
                0.0,
            )
        };
        make(&mut history, &mut board, insert("c1"));
        assert!(history.is_modified());
        history.undo(&mut board).unwrap().unwrap();
        assert!(!history.is_modified());
        history.redo(&mut board).unwrap().unwrap();
        history.mark_saved();
        assert!(!history.is_modified());
        make(&mut history, &mut board, insert("c2"));
        history.undo(&mut board).unwrap().unwrap();
        assert!(!history.is_modified());
        // Undo past the save, then a new change: the saved state is gone for good.
        history.undo(&mut board).unwrap().unwrap();
        make(&mut history, &mut board, insert("c3"));
        history.undo(&mut board).unwrap().unwrap();
        assert!(history.is_modified());
    }

    #[test]
    fn history_keeps_at_most_max_undo_steps() {
        let mut history = History::default();
        let steps = MAX_UNDO_STEPS + 5;
        for i in 0..steps {
            history.push(step(Vec::new(), i as f64, i as f64 + 1.0));
        }
        assert_eq!(history.steps.len(), MAX_UNDO_STEPS);
        // The first five steps left with the saved state.
        assert!(history.is_modified());
        let mut board = Board::new();
        let mut last = None;
        while let Some(cursor) = history.undo(&mut board) {
            last = Some(cursor.unwrap());
        }
        assert_eq!(last, Some(grid(5.0)));
    }

    #[test]
    fn history_keeps_at_most_max_undo_bytes() {
        let mut history = History::default();
        // 5,000 four-byte characters before and after: 40 KB a step.
        let text = |c: char| Shape::Text {
            at: Point::ZERO,
            text: c.to_string().repeat(5_000),
        };
        let replace = step(
            vec![Edit::Replace {
                id: id("t1"),
                before: text('😀'),
                after: text('🙂'),
            }],
            0.0,
            0.0,
        );
        let each = replace.bytes();
        assert!(each > 40_000);
        for _ in 0..MAX_UNDO_STEPS {
            history.push(replace.clone());
        }
        assert_eq!(history.steps.len(), MAX_UNDO_BYTES / each);
        assert_eq!(history.bytes, history.steps.len() * each);
        assert_eq!(history.done, history.steps.len());
        // A step bigger than the cap stays: the newest step is never dropped.
        let mut history = History::default();
        let huge = step(
            vec![replace.edits[0].clone(); MAX_UNDO_BYTES / 40_000 + 1],
            0.0,
            0.0,
        );
        history.push(huge);
        assert!(history.bytes > MAX_UNDO_BYTES);
        assert_eq!(history.steps.len(), 1);
    }

    #[test]
    fn an_edit_is_as_small_as_the_cap_assumes() {
        // MAX_UNDO_BYTES's doc counts 120 bytes an edit (measured on Linux x86_64).
        assert!(size_of::<Edit>() <= 128, "{}", size_of::<Edit>());
    }
}
