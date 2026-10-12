//! Board operators and puts: `d` and `y` with their motions, `x`, `p` and `P`.

use std::collections::{HashMap, HashSet};
use std::iter;

use kurbo::{Point, Rect};

use super::motion::{ObjectMotion, reading_order};
use super::normal::Operator;
use super::place::{Side, place, translate};
use super::register::{Register, id_prefix};
use super::undo::Edit;
use super::work::{ORDER_WORK, PLACE_WORK, PUT_WORK, TAKE_WORK};
use super::{Cursor, Engine, Measure, anchor, bounds, exec};
use crate::board::{Change, DEFAULT_TEXT_SIZE, MAX_OBJECTS, Object, ObjectId, Shape};

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
    /// object the motion lands on. `gg` and `G` take this object, the one they land on and every
    /// object between them in reading order (`dG`: this one to the last). A motion that runs out
    /// of objects takes what it reached; one that cannot move at all does nothing, and so does
    /// one cut short when the key runs out of work (see
    /// [`MAX_KEY_WORK`](super::MAX_KEY_WORK)).
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
        let (motion, steps) = match target {
            Target::Objects | Target::Motion(ObjectMotion::Next) => {
                (ObjectMotion::Next, count.saturating_sub(1))
            }
            Target::Motion(motion) => (motion, count),
        };
        let Some((bounds, landed)) = self.object_path(motion, steps, measure) else {
            return false;
        };
        if self.out_of_work() {
            return false;
        }
        let objects: Vec<usize> = match motion {
            ObjectMotion::Nearest(_) if landed.is_empty() => Vec::new(),
            ObjectMotion::Next | ObjectMotion::Nearest(_) => {
                iter::once(current).chain(landed).collect()
            }
            ObjectMotion::Previous => landed,
            ObjectMotion::First | ObjectMotion::Last => {
                if !self.charge(ORDER_WORK.saturating_mul(bounds.len())) {
                    return false;
                }
                let order = reading_order(&bounds);
                let rank = |i: usize| order.iter().position(|&o| o == i);
                match (rank(current), landed.first().and_then(|&i| rank(i))) {
                    (Some(a), Some(b)) => {
                        order.get(a.min(b)..=a.max(b)).unwrap_or_default().to_vec()
                    }
                    _ => Vec::new(),
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
    /// object. The reading order and the objects taken count as work (see
    /// [`MAX_KEY_WORK`](super::MAX_KEY_WORK)); when the key runs out, nothing is taken.
    pub(super) fn take_objects(
        &mut self,
        operator: Operator,
        mut indices: Vec<usize>,
        bounds: &[Rect],
    ) -> bool {
        let work = ORDER_WORK
            .saturating_mul(bounds.len())
            .saturating_add(TAKE_WORK.saturating_mul(indices.len()));
        if !self.charge(work) {
            return false;
        }
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
        match operator {
            Operator::Yank => {
                self.register = Some(Register::Objects(objects));
                self.cursor = Cursor::Object(first_id);
                false
            }
            Operator::Delete => {
                // Back to front, so each is still where it was when the earlier ones are gone,
                // and undo puts them back front to back.
                let mut positions: Vec<usize> = indices
                    .iter()
                    .copied()
                    .filter(|&i| i < self.board.len())
                    .collect();
                positions.sort_unstable_by(|a, b| b.cmp(a));
                positions.dedup();
                let edits: Vec<Edit> = positions
                    .iter()
                    .filter_map(|&index| {
                        let object = self.board.objects().get(index)?.clone();
                        Some(Edit::Remove { index, object })
                    })
                    .collect();
                let changes: Vec<Change<'_>> = edits.iter().map(Edit::forward).collect();
                if let Err(error) = self.board.apply(&changes) {
                    self.error(error.to_string());
                    return false;
                }
                self.register = Some(Register::Objects(objects));
                let before = self.cursor.clone();
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
    /// untouched, when the copies would not fit the board's caps or its coordinates, or when
    /// the key runs out of work (see [`MAX_KEY_WORK`](super::MAX_KEY_WORK)): the objects to
    /// put count before the register is copied, then the board measured and the obstacles the
    /// place rule looks at. The cursor ends on the first object of the first copy.
    pub(super) fn put(&mut self, side: Side, count: u32, measure: &mut dyn Measure) -> bool {
        let group_len = match &self.register {
            None => 0,
            Some(Register::Objects(objects)) => objects.len(),
            Some(Register::Text(_)) => 1,
        };
        if group_len == 0 {
            self.error("nothing to put: the register is empty");
            return false;
        }
        let copies = usize::try_from(count).unwrap_or(usize::MAX);
        let objects = copies.saturating_mul(group_len);
        if objects > MAX_OBJECTS.saturating_sub(self.board.len()) {
            self.error(format!(
                "no room for {objects} more objects: a board holds at most {MAX_OBJECTS}"
            ));
            return false;
        }
        let work = PUT_WORK
            .saturating_mul(objects)
            .saturating_add(self.layout_work())
            .saturating_add(self.id_work());
        if !self.charge(work) {
            return false;
        }
        let group = match &self.register {
            None => Vec::new(),
            Some(Register::Objects(objects)) => objects.clone(),
            Some(Register::Text(text)) => vec![Object::new(
                self.board.fresh_id("t"),
                Shape::Text {
                    at: Point::ZERO,
                    text: text.clone(),
                    size: DEFAULT_TEXT_SIZE,
                },
            )],
        };
        let Some(first) = group.first() else {
            return false;
        };
        let first_anchor = anchor(&first.shape);
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
        let mut made: Vec<Object> = Vec::with_capacity(objects);
        let mut taken: HashSet<ObjectId> = HashSet::new();
        // Where the search for a fresh id of each kind goes on from.
        let mut fresh_from: HashMap<&str, usize> = HashMap::new();
        for _ in 0..copies {
            let offset = match reference {
                Some(reference) => {
                    let mut looked = 0;
                    let at = place(reference, group_box.size(), &obstacles, side, &mut looked);
                    if !self.charge(PLACE_WORK.saturating_mul(looked)) {
                        return false;
                    }
                    at.map(|at| at - group_box.origin())
                }
                None => Some(self.cursor_point() - first_anchor),
            };
            let Some(offset) = offset else {
                self.error("no room for the put");
                return false;
            };
            for (object, member) in group.iter().zip(&member_bounds) {
                let shape = translate(&object.shape, offset.x, offset.y);
                if !shape.coordinates_in_range() {
                    self.error("no room for the put");
                    return false;
                }
                let id = if self.board.contains(&object.id) || taken.contains(&object.id) {
                    let prefix = id_prefix(&shape);
                    let from = fresh_from.entry(prefix).or_insert(1);
                    self.board
                        .fresh_id_from(prefix, from, |id| taken.contains(id))
                } else {
                    object.id.clone()
                };
                taken.insert(id.clone());
                made.push(Object::new(id, shape));
                obstacles.push(*member + offset);
            }
            reference = Some(group_box + offset);
        }
        self.add_put(made)
    }

    /// Puts `made` on top of the board as one batch and one undo step, with the cursor on the
    /// first of them. False, with the board's refusal as the message, when it does not fit.
    fn add_put(&mut self, made: Vec<Object>) -> bool {
        let Some(first_put) = made.first().map(|o| o.id.clone()) else {
            return false;
        };
        let start = self.board.len();
        let edits: Vec<Edit> = made
            .into_iter()
            .enumerate()
            .map(|(i, object)| Edit::Insert {
                index: start + i,
                object,
            })
            .collect();
        let changes: Vec<Change<'_>> = edits.iter().map(Edit::forward).collect();
        if let Err(error) = self.board.apply(&changes) {
            self.error(exec::refusal(&error));
            return false;
        }
        let before = std::mem::replace(&mut self.cursor, Cursor::Object(first_put));
        self.commit(edits, before);
        true
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
