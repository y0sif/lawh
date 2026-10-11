//! Visual mode: `v` spans a rectangle from where it was pressed to the cursor and selects the
//! objects inside it, for `d`, `x` and `y`.

use kurbo::Rect;

use super::normal::{Lookup, MOTIONS, Motion, Operator, Pending, lookup};
use super::{Cursor, Effect, Engine, Measure, Mode, bounds};
use crate::board::ObjectId;
use crate::key::{Key, KeyCode};

/// A Visual-mode command other than a motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visual {
    /// `d`, `x` delete the selection, `y` yanks it.
    Operator(Operator),
    /// `v` or `<Esc>`.
    Leave,
    /// `:` leaves Visual mode and opens the command line.
    CommandLine,
}

/// Visual-mode commands by their keys, after any count. Motions come from [`MOTIONS`].
const VISUAL: &[(&[Key], Visual)] = &[
    (&[Key::char('d')], Visual::Operator(Operator::Delete)),
    (&[Key::char('x')], Visual::Operator(Operator::Delete)),
    (&[Key::char('y')], Visual::Operator(Operator::Yank)),
    (&[Key::char('v')], Visual::Leave),
    (&[Key::named(KeyCode::Esc)], Visual::Leave),
    (&[Key::char(':')], Visual::CommandLine),
];

/// What some keys make in Visual mode.
#[derive(Debug, Clone, Copy)]
enum Found {
    Motion(Motion),
    Command(Visual),
}

impl Engine {
    /// The Visual selection: the objects whose bounds lie inside [`Engine::visual_rect`], in
    /// z-order, as measured after the last key. Empty outside Visual mode.
    ///
    /// Every key that moves the cursor in Visual mode pays for the new selection before it
    /// moves (see [`MAX_KEY_WORK`](super::MAX_KEY_WORK)), so a motion that runs out of work part
    /// way still leaves the selection matching the rectangle.
    pub fn selection(&self) -> &[ObjectId] {
        &self.selection
    }

    /// The Visual rectangle: the bounding box of where `v` was pressed (a point, or an
    /// object's bounds) and the cursor (likewise). `None` outside Visual mode.
    pub fn visual_rect(&self, measure: &mut dyn Measure) -> Option<Rect> {
        let anchor = self.visual?;
        Some(anchor.union(self.cursor_region(measure)))
    }

    /// The cursor as a box: its grid point, or the current object's bounds.
    fn cursor_region(&self, measure: &mut dyn Measure) -> Rect {
        let at = |p| Rect::from_points(p, p);
        match &self.cursor {
            Cursor::Grid(p) => at(*p),
            Cursor::Object(id) => self
                .board
                .get(id)
                .map_or_else(|| at(self.cursor_point()), |o| bounds(o, measure)),
        }
    }

    /// `v`: Visual mode from the cursor. Measuring the board for the selection counts as work:
    /// when the key runs out, it stays in Normal mode.
    pub(super) fn enter_visual(&mut self, measure: &mut dyn Measure) {
        if !self.charge(self.layout_work()) {
            return;
        }
        self.visual = Some(self.cursor_region(measure));
        self.mode = Mode::Visual;
        self.select(measure);
    }

    /// Leaves Visual mode, if in it, for Normal mode.
    pub(super) fn leave_visual(&mut self) {
        self.visual = None;
        self.selection.clear();
        if self.mode == Mode::Visual {
            self.mode = Mode::Normal;
        }
    }

    /// Selects the objects whose bounds lie inside the Visual rectangle, edges included. It
    /// measures the board: the caller has counted that work already
    /// ([`Engine::layout_work`]), so the selection is made even once the key has run out.
    fn select(&mut self, measure: &mut dyn Measure) {
        let layout = self.layout(measure);
        self.select_in(&layout, measure);
    }

    /// [`Engine::select`] with the bounds of every object, in z-order, measured already.
    fn select_in(&mut self, layout: &[Rect], measure: &mut dyn Measure) {
        self.selection.clear();
        let Some(rect) = self.visual_rect(measure) else {
            return;
        };
        let inside =
            |b: &Rect| rect.x0 <= b.x0 && b.x1 <= rect.x1 && rect.y0 <= b.y0 && b.y1 <= rect.y1;
        let selected = self
            .board
            .objects()
            .iter()
            .zip(layout)
            .filter(|(_, b)| inside(b))
            .map(|(o, _)| o.id.clone())
            .collect();
        self.selection = selected;
    }

    pub(super) fn visual_key(&mut self, key: Key, measure: &mut dyn Measure) -> Vec<Effect> {
        if self.pending.count_digit(key) {
            return Vec::new();
        }
        self.pending.keys.push(key);
        let keys = &self.pending.keys;
        let found = Lookup::first([
            lookup(MOTIONS, keys).map(Found::Motion),
            lookup(VISUAL, keys).map(Found::Command),
        ]);
        match found {
            Lookup::Prefix => return Vec::new(),
            Lookup::Nothing => {}
            Lookup::Found(Found::Motion(motion)) => {
                // The selection after the motion is paid for first: a motion cut short by the
                // work budget still moves the cursor, and the selection must follow it.
                if self.charge(self.layout_work()) {
                    let count = motion.count(self.pending.count);
                    self.motion(motion, count, measure);
                    self.select(measure);
                }
            }
            Lookup::Found(Found::Command(Visual::Operator(operator))) => {
                self.visual_operate(operator, measure);
            }
            Lookup::Found(Found::Command(Visual::Leave)) => self.leave_visual(),
            Lookup::Found(Found::Command(Visual::CommandLine)) => {
                self.leave_visual();
                self.open_command_line();
            }
        }
        self.pending = Pending::default();
        Vec::new()
    }

    /// Deletes or yanks the selection as one change and returns to Normal mode. `.` cannot
    /// repeat a Visual change in M2, so a delete forgets the last change.
    ///
    /// The selection is made again from the bounds measured for the change, so it is what the
    /// rectangle holds now, at no extra work. When the key runs out of work before measuring,
    /// it only leaves Visual mode.
    fn visual_operate(&mut self, operator: Operator, measure: &mut dyn Measure) {
        if !self.charge(self.layout_work()) {
            self.leave_visual();
            return;
        }
        let bounds = self.layout(measure);
        self.select_in(&bounds, measure);
        let selected: Vec<usize> = self
            .selection
            .iter()
            .filter_map(|id| self.index_of(id))
            .collect();
        self.leave_visual();
        if selected.is_empty() {
            self.info("nothing selected");
            return;
        }
        if self.take_objects(operator, selected, &bounds) {
            self.repeat.forget();
        }
    }
}
