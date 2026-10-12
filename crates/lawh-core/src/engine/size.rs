//! Sizes of text and math blocks: the size keys `<C-a>` and `<C-x>`, and `:si[ze]`. The size of
//! new blocks is the `size` option (`:set size`, in `settings.rs`).
//!
//! A size multiplies the text's height (see
//! [`DEFAULT_TEXT_SIZE`](crate::board::DEFAULT_TEXT_SIZE)). `<C-a>` and `<C-x>` step the current
//! text or math block along [`SIZE_STEPS`], larger or smaller; a count steps that many times,
//! and from a size between two steps the first step goes to the next step that way. At an end
//! the size stays and the message says so. `.` repeats them on the board, with a new count if
//! one is typed. In block-Normal mode they are not bound.
//!
//! In Visual mode they resize every text and math block selected, each from its own size, as one
//! undo step, and Visual mode stays. Shapes in the selection are skipped. The selection stays
//! the same objects, those that grew out of the rectangle too, and takes in none that the
//! rectangle holds only because the cursor's object grew: the keys can be pressed again, and
//! `d`, `x`, `y` and `:si` act on what stays highlighted. The next motion selects from the
//! rectangle again. As other Visual changes, `.` cannot repeat them, so they forget the last
//! change.
//!
//! `:si[ze] N` sets the size, `:si +N` and `:si -N` add to it or take from it, on the current
//! object, or on the text and math of the selection when `:` was typed in Visual mode: the
//! command line keeps the selection (and [`Engine::selection`] shows it) until it closes. A size
//! out of range is refused whole, so a selection changes all or nothing. A size `+N` or `-N`
//! works out is rounded with [`round3`], as coordinates are, so files never show float noise.
//!
//! On a shape, or a grid point, the keys and `:si` change nothing and the message says why. Each
//! change is one undo step, with the cursor where it was.

use super::command::SizeChange;
use super::text_motion::content;
use super::undo::Edit;
use super::work::{COPY_WORK, SIZE_WORK};
use super::{Cursor, Engine, exec, round3};
use crate::board::{Change, MAX_TEXT_SIZE, MIN_TEXT_SIZE, ObjectId, Shape, is_valid_text_size};

/// The sizes `<C-a>` and `<C-x>` step through, smallest first.
///
/// Quarter steps up to 1.5, half steps to 3, whole ones to 6, then 8, 10, 12 and 16. From 0.75
/// up each step is 1.2 to 1.33 times the one before, so every press looks about as large a
/// change, and 15 presses go from one end to the other. The ends are [`MIN_TEXT_SIZE`] and
/// [`MAX_TEXT_SIZE`], and [`DEFAULT_TEXT_SIZE`](crate::board::DEFAULT_TEXT_SIZE) is a step (a
/// test checks all three).
pub const SIZE_STEPS: [f64; 16] = [
    0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 16.0,
];

/// Which way the size keys step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Grow {
    /// `<C-a>`.
    Larger,
    /// `<C-x>`.
    Smaller,
}

/// The message for the size keys and `:si` on an object without a size.
const NOT_SIZED: &str = "only text and math have a size";

/// The message for the size keys and `:si` on a grid point.
const NO_OBJECT: &str = "no object under the cursor";

/// The range of sizes, for messages: `sizes go from 0.25 to 16`.
pub(super) fn range_message() -> String {
    format!("sizes go from {MIN_TEXT_SIZE} to {MAX_TEXT_SIZE}")
}

/// `size` stepped `count` times along [`SIZE_STEPS`] toward `grow`, and whether it stopped at
/// an end before taking them all (it may not have moved at all).
fn step(size: f64, grow: Grow, count: u32) -> (f64, bool) {
    let steps: Vec<f64> = match grow {
        Grow::Larger => SIZE_STEPS.iter().copied().filter(|&s| s > size).collect(),
        Grow::Smaller => SIZE_STEPS
            .iter()
            .rev()
            .copied()
            .filter(|&s| s < size)
            .collect(),
    };
    let index = usize::try_from(count.max(1) - 1).unwrap_or(usize::MAX);
    match steps.get(index) {
        Some(&stepped) => (stepped, false),
        None => (steps.last().copied().unwrap_or(size), true),
    }
}

/// `shape` at size `size`; `None` for kinds without a size.
fn with_size(shape: &Shape, size: f64) -> Option<Shape> {
    match shape {
        Shape::Text { at, text, .. } => Some(Shape::Text {
            at: *at,
            text: text.clone(),
            size,
        }),
        Shape::Math { at, src, .. } => Some(Shape::Math {
            at: *at,
            src: src.clone(),
            size,
        }),
        Shape::Ellipse { .. } | Shape::Rect { .. } | Shape::Line { .. } | Shape::Arrow { .. } => {
            None
        }
    }
}

/// A text or math object to resize: its id, its shape and its size.
struct Sized {
    id: ObjectId,
    shape: Shape,
    size: f64,
}

impl Engine {
    /// `<C-a>` (larger) and `<C-x>` (smaller) on the board: steps the current object's size
    /// `count` times. True when the board changed.
    pub(super) fn size_key(&mut self, grow: Grow, count: u32) -> bool {
        let Some(id) = self.current_for_size() else {
            return false;
        };
        self.step_sizes(&[id], grow, count)
    }

    /// `<C-a>` and `<C-x>` in Visual mode: steps the size of every text and math block in the
    /// selection `count` times, as one change. Visual mode stays.
    pub(super) fn visual_size_key(&mut self, grow: Grow, count: u32) {
        if self.selection.is_empty() {
            return self.info("nothing selected");
        }
        let ids = self.selection.clone();
        if self.step_sizes(&ids, grow, count) {
            self.repeat.forget();
        }
    }

    /// `:si[ze]`: on the current object, or on the selection when the command line was opened
    /// from Visual mode.
    pub(super) fn size_command(&mut self, change: SizeChange) {
        let ids = if self.visual_command {
            if self.selection.is_empty() {
                return self.info("nothing selected");
            }
            self.selection.clone()
        } else {
            match self.current_for_size() {
                Some(id) => vec![id],
                None => return,
            }
        };
        let Some(targets) = self.sized(&ids) else {
            return;
        };
        let mut resized = Vec::with_capacity(targets.len());
        for one in targets {
            let size = match change {
                SizeChange::To(size) => size,
                SizeChange::By(by) => round3(one.size + by),
            };
            if !is_valid_text_size(size) {
                return self.error(format!("size {size} is out of range: {}", range_message()));
            }
            resized.push((one, size));
        }
        self.resize(resized);
    }

    /// The current object, for the size keys and `:si`; `None` on a grid point, with the
    /// message.
    fn current_for_size(&mut self) -> Option<ObjectId> {
        match &self.cursor {
            Cursor::Object(id) => Some(id.clone()),
            Cursor::Grid(_) => {
                self.error(NO_OBJECT);
                None
            }
        }
    }

    /// Steps the size of each text and math object of `ids` `count` times toward `grow`, as one
    /// undo step. Says so when one stops at an end: as information when others changed, as an
    /// error when nothing did. True when the board changed.
    fn step_sizes(&mut self, ids: &[ObjectId], grow: Grow, count: u32) -> bool {
        let Some(sized) = self.sized(ids) else {
            return false;
        };
        let mut stopped = false;
        let resized: Vec<(Sized, f64)> = sized
            .into_iter()
            .map(|one| {
                let (size, at_end) = step(one.size, grow, count);
                stopped |= at_end;
                (one, size)
            })
            .collect();
        let changed = self.resize(resized);
        if stopped && !self.out_of_work() {
            let end = match grow {
                Grow::Larger => format!("{MAX_TEXT_SIZE} is the largest size"),
                Grow::Smaller => format!("{MIN_TEXT_SIZE} is the smallest size"),
            };
            if changed {
                self.info(end);
            } else {
                self.error(end);
            }
        }
        changed
    }

    /// The text and math objects of `ids`, in the order given, with their shapes and sizes.
    /// Looking at each counts as work (see [`MAX_KEY_WORK`](super::MAX_KEY_WORK)), and once
    /// more for the change. `None` when the key runs out of work, or when none has a size, with
    /// the message.
    fn sized(&mut self, ids: &[ObjectId]) -> Option<Vec<Sized>> {
        if !self.charge(SIZE_WORK.saturating_mul(ids.len().saturating_add(1))) {
            return None;
        }
        let sized: Vec<Sized> = ids
            .iter()
            .filter_map(|id| {
                let shape = &self.board.get(id)?.shape;
                let size = shape.size()?;
                Some(Sized {
                    id: id.clone(),
                    shape: shape.clone(),
                    size,
                })
            })
            .collect();
        if sized.is_empty() {
            self.error(NOT_SIZED);
            return None;
        }
        Some(sized)
    }

    /// Gives each object its new size, as one batch and one undo step, leaving out those whose
    /// size stays. Copying their text counts as work. True when the board changed; a refusal
    /// (which the callers' checks leave no room for) becomes the message.
    fn resize(&mut self, resized: Vec<(Sized, f64)>) -> bool {
        #[expect(
            clippy::float_cmp,
            reason = "only a size that stays exactly the same makes no edit"
        )]
        let edits: Vec<Edit> = resized
            .into_iter()
            .filter(|(one, size)| one.size != *size)
            .filter_map(|(one, size)| {
                let after = with_size(&one.shape, size)?;
                Some(Edit::Replace {
                    id: one.id,
                    before: one.shape,
                    after,
                })
            })
            .collect();
        if edits.is_empty() {
            return false;
        }
        // Copying each text and the board counting it again, as an edit in a block does.
        let copied: usize = edits
            .iter()
            .map(|edit| match edit {
                Edit::Replace { after, .. } => content(after)
                    .map_or(0, |text| text.len().saturating_add(after.content_chars())),
                Edit::Insert { .. } | Edit::Remove { .. } => 0,
            })
            .sum();
        if !self.charge(COPY_WORK.saturating_mul(copied)) {
            return false;
        }
        let changes: Vec<Change<'_>> = edits.iter().map(Edit::forward).collect();
        if let Err(error) = self.board.apply(&changes) {
            self.error(exec::refusal(&error));
            return false;
        }
        let before = self.cursor.clone();
        self.commit(edits, before);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::DEFAULT_TEXT_SIZE;

    #[test]
    fn the_steps_run_from_the_smallest_size_to_the_largest() {
        assert_eq!(SIZE_STEPS.first(), Some(&MIN_TEXT_SIZE));
        assert_eq!(SIZE_STEPS.last(), Some(&MAX_TEXT_SIZE));
        assert!(SIZE_STEPS.contains(&DEFAULT_TEXT_SIZE));
        assert!(SIZE_STEPS.windows(2).all(|w| w[0] < w[1]));
        assert!(SIZE_STEPS.iter().all(|&s| is_valid_text_size(s)));
    }

    #[test]
    fn steps_go_to_the_next_step_and_stop_at_the_ends() {
        use Grow::{Larger, Smaller};
        assert_eq!(step(1.0, Larger, 1), (1.25, false));
        assert_eq!(step(1.0, Smaller, 1), (0.75, false));
        assert_eq!(step(1.0, Larger, 3), (2.0, false));
        // Between two steps: the next one that way.
        assert_eq!(step(1.1, Larger, 1), (1.25, false));
        assert_eq!(step(1.1, Smaller, 1), (1.0, false));
        assert_eq!(step(13.0, Smaller, 2), (10.0, false));
        // Onto an end exactly, then past it.
        assert_eq!(step(12.0, Larger, 1), (16.0, false));
        assert_eq!(step(12.0, Larger, 2), (16.0, true));
        assert_eq!(step(16.0, Larger, 1), (16.0, true));
        assert_eq!(step(0.5, Smaller, 10_000), (0.25, true));
        assert_eq!(step(0.25, Smaller, 1), (0.25, true));
        // A count past the steps costs no more than one.
        assert_eq!(step(0.25, Larger, u32::MAX), (16.0, true));
        assert_eq!(step(0.25, Larger, 15), (16.0, false));
    }
}
