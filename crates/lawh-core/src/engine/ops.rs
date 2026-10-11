//! Board operators and puts: `d` and `y` with their motions, `x`, `p` and `P`.

use std::iter;

use kurbo::{Point, Rect};

use super::motion::{ObjectMotion, Origin, path, reading_order};
use super::normal::Operator;
use super::place::{Side, place, translate};
use super::register::{Register, id_prefix};
use super::undo::Edit;
use super::{Cursor, Engine, Measure, anchor, bounds, exec};
use crate::board::{MAX_OBJECTS, Object, ObjectId, Shape};

/// What an operator acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    /// `dd`, `yy`, `x`: the current object and the next `count - 1` in reading order.
    Objects,
    /// `d` or `y` with an object motion.
    Motion(ObjectMotion),
}

impl Engine {
    /// Runs `operator` on `target` from the current object, `count` times. True when the board
    /// changed.
    ///
    /// `w` is exclusive, as in Vim: `dw` is this object, `d3w` this one and the next two. `b`
    /// takes the objects before this one, not this one. `H J K L` take this object and every
    /// object the motion lands on. A motion that runs out of objects takes what it reached; one
    /// that cannot move at all does nothing.
    pub(super) fn operate(
        &mut self,
        operator: Operator,
        target: Target,
        count: u32,
        measure: &mut dyn Measure,
    ) -> bool {
        let current = match &self.cursor {
            Cursor::Object(id) => self.index_of(id),
            Cursor::Grid(_) => None,
        };
        let Some(current) = current else {
            self.error("no object under the cursor");
            return false;
        };
        let bounds = self.layout(measure);
        let n = usize::try_from(count).unwrap_or(usize::MAX);
        let origin = Origin::Object(current);
        let next = |n| path(&bounds, origin, ObjectMotion::Next, n);
        let objects: Vec<usize> = match target {
            Target::Objects | Target::Motion(ObjectMotion::Next) => iter::once(current)
                .chain(next(n.saturating_sub(1)))
                .collect(),
            Target::Motion(ObjectMotion::Previous) => {
                path(&bounds, origin, ObjectMotion::Previous, n)
            }
            Target::Motion(motion @ ObjectMotion::Nearest(_)) => {
                let landed = path(&bounds, origin, motion, n);
                if landed.is_empty() {
                    Vec::new()
                } else {
                    iter::once(current).chain(landed).collect()
                }
            }
        };
        self.take_objects(operator, objects, &bounds)
    }

    /// Puts the objects at positions `indices` (z-order) in the register, in reading order, and
    /// deletes them as one undo step or yanks them. True when the board changed.
    ///
    /// After a delete the cursor is the grid point at the anchor of the first object in
    /// reading order, so a `p` there puts them back where they were. After a yank it is that
    /// object.
    pub(super) fn take_objects(
        &mut self,
        operator: Operator,
        mut indices: Vec<usize>,
        bounds: &[Rect],
    ) -> bool {
        let mut rank = vec![usize::MAX; bounds.len()];
        for (r, i) in reading_order(bounds).into_iter().enumerate() {
            if let Some(slot) = rank.get_mut(i) {
                *slot = r;
            }
        }
        indices.sort_by_key(|&i| rank.get(i).copied().unwrap_or(usize::MAX));
        indices.dedup();
        let objects: Vec<Object> = indices
            .iter()
            .filter_map(|&i| self.board.objects().get(i).cloned())
            .collect();
        let Some(first) = objects.first() else {
            return false;
        };
        let (first_id, first_anchor) = (first.id.clone(), anchor(&first.shape));
        let ids: Vec<ObjectId> = objects.iter().map(|o| o.id.clone()).collect();
        self.register = Some(Register::Objects(objects));
        match operator {
            Operator::Yank => {
                self.cursor = Cursor::Object(first_id);
                false
            }
            Operator::Delete => {
                let before = self.cursor.clone();
                let edits = ids
                    .iter()
                    .filter_map(|id| self.board.remove(id))
                    .map(|(index, object)| Edit::Remove { index, object })
                    .collect();
                self.cursor = Cursor::Grid(first_anchor);
                self.commit(edits, before);
                true
            }
        }
    }

    /// `p` (below) and `P` (above): puts `count` copies of the register on top of the board as
    /// one undo step. True when the board changed.
    ///
    /// With an object cursor the first copy goes on `side` of it by the place rule; with a grid
    /// cursor it goes where its first object's anchor is at the cursor. Each further copy goes
    /// on `side` of the copy before it. A put object keeps its id when that id is free, else
    /// gets a fresh one for its kind. The put is all or nothing: it is refused, the board
    /// untouched, when the copies would not fit the board's caps or its coordinates. The cursor
    /// ends on the first object of the first copy.
    pub(super) fn put(&mut self, side: Side, count: u32, measure: &mut dyn Measure) -> bool {
        let group = match &self.register {
            None => Vec::new(),
            Some(Register::Objects(objects)) => objects.clone(),
            Some(Register::Text(text)) => vec![Object::new(
                self.board.fresh_id("t"),
                Shape::Text {
                    at: Point::ZERO,
                    text: text.clone(),
                },
            )],
        };
        let Some(first) = group.first() else {
            self.error("nothing to put: the register is empty");
            return false;
        };
        let first_anchor = anchor(&first.shape);
        let copies = usize::try_from(count).unwrap_or(usize::MAX);
        let objects = copies.saturating_mul(group.len());
        if objects > MAX_OBJECTS.saturating_sub(self.board.len()) {
            self.error(format!(
                "no room for {objects} more objects: a board holds at most {MAX_OBJECTS}"
            ));
            return false;
        }
        let chars: usize = group.iter().map(|o| o.shape.content_chars()).sum();
        if !self.board.has_room_for_text(chars.saturating_mul(copies)) {
            self.error(exec::too_much_text());
            return false;
        }

        let member_bounds: Vec<Rect> = group.iter().map(|o| bounds(o, measure)).collect();
        let group_box = member_bounds
            .iter()
            .copied()
            .reduce(|a, b| a.union(b))
            .unwrap_or_default();
        let mut obstacles = self.layout(measure);
        let mut reference = match &self.cursor {
            Cursor::Grid(_) => None,
            Cursor::Object(id) => self.index_of(id).and_then(|i| obstacles.get(i).copied()),
        };
        let before = self.cursor.clone();
        let mut edits: Vec<Edit> = Vec::new();
        let mut first_put = None;
        for _ in 0..copies {
            let offset = match reference {
                Some(reference) => place(reference, group_box.size(), &obstacles, side)
                    .map(|at| at - group_box.origin()),
                None => Some(self.cursor_point() - first_anchor),
            };
            let Some(offset) = offset else {
                return self.undo_put(&edits, "no room for the put");
            };
            for (object, member) in group.iter().zip(&member_bounds) {
                let shape = translate(&object.shape, offset.x, offset.y);
                if !shape.coordinates_in_range() {
                    return self.undo_put(&edits, "no room for the put");
                }
                let id = if self.board.contains(&object.id) {
                    self.board.fresh_id(id_prefix(&shape))
                } else {
                    object.id.clone()
                };
                first_put.get_or_insert_with(|| id.clone());
                let edit = Edit::Insert {
                    index: self.board.len(),
                    object: Object::new(id, shape),
                };
                if let Err(error) = edit.apply(&mut self.board) {
                    return self.undo_put(&edits, &exec::refusal(&error));
                }
                edits.push(edit);
                obstacles.push(*member + offset);
            }
            reference = Some(group_box + offset);
        }
        let Some(first_put) = first_put else {
            return false;
        };
        self.cursor = Cursor::Object(first_put);
        self.commit(edits, before);
        true
    }

    /// Takes back the objects a refused put has made so far and says why. False: the board is
    /// as it was.
    fn undo_put(&mut self, edits: &[Edit], why: &str) -> bool {
        for edit in edits.iter().rev() {
            // Taking back an insert just made cannot fail.
            let _ = edit.revert(&mut self.board);
        }
        self.error(why);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;
    use crate::engine::FixedMeasure;
    use crate::key::parse_keys;
    use crate::to_lawh;

    fn feed(engine: &mut Engine, script: &str) {
        for key in parse_keys(script).unwrap() {
            engine.feed(key, &mut FixedMeasure);
        }
    }

    #[test]
    fn a_text_register_puts_a_new_text_block() {
        let mut engine = Engine::new(Board::new());
        feed(&mut engine, "2l:te ab<CR>");
        engine.register = Some(Register::Text("xyz".to_owned()));
        // Below `t1` (0.6 high at y 0): y 1.1. Then at a grid point, twice: the second copy
        // goes below the first.
        feed(&mut engine, "p3j2p");
        assert_eq!(
            engine.cursor(),
            &Cursor::Object(ObjectId::new("t3").unwrap())
        );
        assert_eq!(
            to_lawh(engine.board()),
            "lawh version=1\n\
             text id=t1 x=2 y=0 \"ab\"\n\
             text id=t2 x=2 y=1.1 \"xyz\"\n\
             text id=t3 x=2 y=4.1 \"xyz\"\n\
             text id=t4 x=2 y=5.2 \"xyz\"\n"
        );
    }
}
