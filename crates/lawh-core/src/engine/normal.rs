//! Normal mode keys: counts, operators, the key tables, and the motions Normal and Visual mode
//! share.

use super::motion::{Direction, ObjectMotion, Origin, path};
use super::ops::Target;
use super::place::Side;
use super::repeat::Context;
use super::text::Entry;
use super::{Cursor, Effect, Engine, MAX_COUNT, Measure, Mode};
use crate::key::{Key, KeyCode, format_keys};

/// A motion: where the cursor goes, or what an operator acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Motion {
    /// `h j k l`: whole grid units, `dx` and `dy` a step.
    Grid { dx: i8, dy: i8 },
    /// `w b H J K L`: onto an object.
    Object(ObjectMotion),
}

/// Motions by their keys, in Normal mode, Visual mode and after an operator.
pub(super) const MOTIONS: &[(&[Key], Motion)] = &[
    (&[Key::char('h')], Motion::Grid { dx: -1, dy: 0 }),
    (&[Key::char('j')], Motion::Grid { dx: 0, dy: 1 }),
    (&[Key::char('k')], Motion::Grid { dx: 0, dy: -1 }),
    (&[Key::char('l')], Motion::Grid { dx: 1, dy: 0 }),
    (&[Key::char('w')], Motion::Object(ObjectMotion::Next)),
    (&[Key::char('b')], Motion::Object(ObjectMotion::Previous)),
    (
        &[Key::char('H')],
        Motion::Object(ObjectMotion::Nearest(Direction::Left)),
    ),
    (
        &[Key::char('J')],
        Motion::Object(ObjectMotion::Nearest(Direction::Down)),
    ),
    (
        &[Key::char('K')],
        Motion::Object(ObjectMotion::Nearest(Direction::Up)),
    ),
    (
        &[Key::char('L')],
        Motion::Object(ObjectMotion::Nearest(Direction::Right)),
    ),
];

/// An operator: what `d` and `y` do to the objects a motion covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Operator {
    Delete,
    Yank,
}

/// Operators by their keys. Typing an operator's keys again (`dd`) acts on the current object.
const OPERATORS: &[(&[Key], Operator)] = &[
    (&[Key::char('d')], Operator::Delete),
    (&[Key::char('y')], Operator::Yank),
];

/// A Normal-mode command other than a motion or an operator, run with a count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Normal {
    /// `x`: as `dd`.
    DeleteObject,
    /// `p` below, `P` above.
    Put(Side),
    Visual,
    /// `.`.
    Repeat,
    /// `@:`.
    RepeatCommandLine,
    Undo,
    Redo,
    CommandLine,
    /// `i a I A o O <CR>`: into a text block.
    Text(Entry),
}

/// Normal-mode commands by the keys that run them, after any count.
const NORMAL: &[(&[Key], Normal)] = &[
    (&[Key::char('x')], Normal::DeleteObject),
    (&[Key::char('p')], Normal::Put(Side::Below)),
    (&[Key::char('P')], Normal::Put(Side::Above)),
    (&[Key::char('v')], Normal::Visual),
    (&[Key::char('.')], Normal::Repeat),
    (&[Key::char('@'), Key::char(':')], Normal::RepeatCommandLine),
    (&[Key::char('u')], Normal::Undo),
    (&[Key::ctrl('r')], Normal::Redo),
    (&[Key::char(':')], Normal::CommandLine),
    (&[Key::char('i')], Normal::Text(Entry::Start)),
    (&[Key::char('I')], Normal::Text(Entry::Start)),
    (&[Key::char('a')], Normal::Text(Entry::End)),
    (&[Key::char('A')], Normal::Text(Entry::End)),
    (&[Key::char('o')], Normal::Text(Entry::Open(Side::Below))),
    (&[Key::char('O')], Normal::Text(Entry::Open(Side::Above))),
    (&[Key::named(KeyCode::Enter)], Normal::Text(Entry::Block)),
];

/// What some keys make in a key table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Lookup<T> {
    Found(T),
    /// The start of a longer command: wait for more keys.
    Prefix,
    /// Nothing: drop the keys.
    Nothing,
}

impl<T> Lookup<T> {
    pub(super) fn map<U>(self, f: impl FnOnce(T) -> U) -> Lookup<U> {
        match self {
            Self::Found(t) => Lookup::Found(f(t)),
            Self::Prefix => Lookup::Prefix,
            Self::Nothing => Lookup::Nothing,
        }
    }

    /// The first found, else `Prefix` when any is a prefix, else `Nothing`.
    pub(super) fn first(lookups: impl IntoIterator<Item = Self>) -> Self {
        let mut prefix = false;
        for lookup in lookups {
            match lookup {
                Self::Found(t) => return Self::Found(t),
                Self::Prefix => prefix = true,
                Self::Nothing => {}
            }
        }
        if prefix { Self::Prefix } else { Self::Nothing }
    }
}

pub(super) fn lookup<T: Copy>(table: &[(&[Key], T)], keys: &[Key]) -> Lookup<T> {
    let mut prefix = false;
    for (command_keys, command) in table {
        if *command_keys == keys {
            return Lookup::Found(*command);
        }
        prefix |= command_keys.len() > keys.len() && command_keys.starts_with(keys);
    }
    if prefix {
        Lookup::Prefix
    } else {
        Lookup::Nothing
    }
}

/// What a whole Normal command is.
#[derive(Debug, Clone, Copy)]
enum Found {
    Motion(Motion),
    Operator(Operator),
    Command(Normal),
}

/// Normal and Visual keys typed so far that do not make a whole command yet.
#[derive(Debug, Clone, Default)]
pub(super) struct Pending {
    /// The count typed before the command (or before the motion, after an operator), at most
    /// [`MAX_COUNT`].
    pub(super) count: Option<u32>,
    /// The command's keys so far, after the count.
    pub(super) keys: Vec<Key>,
    /// An operator waiting for its motion.
    operator: Option<PendingOperator>,
}

/// An operator and the count and keys typed for it.
#[derive(Debug, Clone)]
struct PendingOperator {
    operator: Operator,
    count: Option<u32>,
    keys: Vec<Key>,
}

impl Pending {
    pub(super) fn is_empty(&self) -> bool {
        self.count.is_none() && self.keys.is_empty() && self.operator.is_none()
    }

    /// An operator waits for its motion.
    pub(super) fn has_operator(&self) -> bool {
        self.operator.is_some()
    }

    /// The pending keys in Vim notation, counts included: `2d3`.
    pub(super) fn display(&self) -> String {
        let count = |c: Option<u32>| c.map(|n| n.to_string()).unwrap_or_default();
        let operator = self
            .operator
            .as_ref()
            .map(|op| count(op.count) + &format_keys(&op.keys))
            .unwrap_or_default();
        operator + &count(self.count) + &format_keys(&self.keys)
    }

    /// Adds `key` to the count when it is a digit of one: `1` to `9` start a count, `0` only
    /// goes on one. True when it did.
    pub(super) fn count_digit(&mut self, key: Key) -> bool {
        if !self.keys.is_empty() || !key.mods.is_empty() {
            return false;
        }
        let KeyCode::Char(c) = key.code else {
            return false;
        };
        let Some(digit) = c.to_digit(10) else {
            return false;
        };
        if digit == 0 && self.count.is_none() {
            return false;
        }
        let count = self
            .count
            .unwrap_or(0)
            .saturating_mul(10)
            .saturating_add(digit);
        self.count = Some(count.min(MAX_COUNT));
        true
    }
}

/// The count of an operator and its motion together: the two multiplied, as in Vim (`2d3w` is
/// `d6w`), within [`MAX_COUNT`]; `None` when neither was typed.
fn multiply(a: Option<u32>, b: Option<u32>) -> Option<u32> {
    match (a, b) {
        (None, None) => None,
        _ => Some(a.unwrap_or(1).saturating_mul(b.unwrap_or(1)).min(MAX_COUNT)),
    }
}

impl Engine {
    pub(super) fn normal_key(&mut self, key: Key, measure: &mut dyn Measure) -> Vec<Effect> {
        if self.pending.is_empty() {
            self.repeat.start();
        }
        if self.pending.count_digit(key) {
            return Vec::new();
        }
        self.repeat.push(key);
        self.pending.keys.push(key);
        if self.pending.operator.is_some() {
            return self.operator_key(measure);
        }
        let keys = &self.pending.keys;
        let found = Lookup::first([
            lookup(MOTIONS, keys).map(Found::Motion),
            lookup(OPERATORS, keys).map(Found::Operator),
            lookup(NORMAL, keys).map(Found::Command),
        ]);
        match found {
            Lookup::Prefix => Vec::new(),
            Lookup::Nothing => {
                self.pending = Pending::default();
                Vec::new()
            }
            Lookup::Found(Found::Operator(operator)) => {
                let keys = std::mem::take(&mut self.pending.keys);
                let count = self.pending.count.take();
                self.pending.operator = Some(PendingOperator {
                    operator,
                    count,
                    keys,
                });
                Vec::new()
            }
            Lookup::Found(Found::Motion(motion)) => {
                let count = self.pending.count.unwrap_or(1);
                self.pending = Pending::default();
                self.motion(motion, count, measure);
                Vec::new()
            }
            Lookup::Found(Found::Command(command)) => {
                let count = self.pending.count;
                self.pending = Pending::default();
                self.run_normal(command, count, measure)
            }
        }
    }

    /// A key after an operator: its own keys again, a count or an object motion run it; any
    /// other key cancels it, grid motions included (they have no objects to act on).
    fn operator_key(&mut self, measure: &mut dyn Measure) -> Vec<Effect> {
        let Some(op) = &self.pending.operator else {
            return Vec::new();
        };
        let keys = &self.pending.keys;
        let target = if *keys == op.keys {
            Lookup::Found(Target::Objects)
        } else {
            let own = if op.keys.len() > keys.len() && op.keys.starts_with(keys) {
                Lookup::Prefix
            } else {
                Lookup::Nothing
            };
            let motion = match lookup(MOTIONS, keys) {
                Lookup::Found(Motion::Object(motion)) => Lookup::Found(Target::Motion(motion)),
                Lookup::Found(Motion::Grid { .. }) | Lookup::Nothing => Lookup::Nothing,
                Lookup::Prefix => Lookup::Prefix,
            };
            Lookup::first([motion, own])
        };
        match target {
            Lookup::Prefix => {}
            Lookup::Nothing => self.pending = Pending::default(),
            Lookup::Found(target) => {
                let pending = std::mem::take(&mut self.pending);
                let Some(op) = pending.operator else {
                    return Vec::new();
                };
                let count = multiply(op.count, pending.count);
                let changed = self.operate(op.operator, target, count.unwrap_or(1), measure);
                if changed {
                    self.repeat.finish(count);
                }
            }
        }
        Vec::new()
    }

    fn run_normal(
        &mut self,
        command: Normal,
        count: Option<u32>,
        measure: &mut dyn Measure,
    ) -> Vec<Effect> {
        let n = count.unwrap_or(1);
        match command {
            Normal::DeleteObject => {
                if self.operate(Operator::Delete, Target::Objects, n, measure) {
                    self.repeat.finish(count);
                }
            }
            Normal::Put(side) => {
                if self.put(side, n, measure) {
                    self.repeat.finish(count);
                }
            }
            Normal::Visual => self.enter_visual(measure),
            Normal::Repeat => return self.repeat_change(count, measure),
            Normal::RepeatCommandLine => return self.repeat_command_line(),
            Normal::Undo => self.undo(n),
            Normal::Redo => self.redo(n),
            Normal::CommandLine => self.open_command_line(),
            // The Insert session finishes the change for `.` when it ends.
            Normal::Text(entry) => self.enter_text(entry, count, measure),
        }
        Vec::new()
    }

    pub(super) fn open_command_line(&mut self) {
        self.command_line.clear();
        self.mode = Mode::CommandLine;
    }

    /// Moves the cursor by `motion`, `count` times. An object motion that finds no object
    /// leaves the cursor where it is.
    pub(super) fn motion(&mut self, motion: Motion, count: u32, measure: &mut dyn Measure) {
        match motion {
            Motion::Grid { dx, dy } => {
                let steps = f64::from(count);
                self.move_by(f64::from(dx) * steps, f64::from(dy) * steps);
            }
            Motion::Object(motion) => {
                let bounds = self.layout(measure);
                let count = usize::try_from(count).unwrap_or(usize::MAX);
                let last = path(&bounds, self.origin(), motion, count).pop();
                if let Some(object) = last.and_then(|i| self.board.objects().get(i)) {
                    self.cursor = Cursor::Object(object.id.clone());
                }
            }
        }
    }

    /// Where object motions start: the current object, or the cursor's grid point.
    pub(super) fn origin(&self) -> Origin {
        match &self.cursor {
            Cursor::Grid(p) => Origin::Point(*p),
            Cursor::Object(id) => self
                .index_of(id)
                .map_or(Origin::Point(self.cursor_point()), Origin::Object),
        }
    }

    /// `.`: replays the last change's keys, with `count` in place of its own if given, when
    /// the change can be repeated here (see [`Context`]).
    pub(super) fn repeat_change(
        &mut self,
        count: Option<u32>,
        measure: &mut dyn Measure,
    ) -> Vec<Effect> {
        if self.repeat.replaying {
            return Vec::new();
        }
        let Some(change) = self.repeat.last() else {
            return Vec::new();
        };
        let here = match change.context {
            Context::Board => self.mode == Mode::Normal,
            Context::Block => self.mode == Mode::BlockNormal,
            Context::Anywhere => true,
        };
        if !here {
            return Vec::new();
        }
        let keys = change.replay(count);
        self.repeat.replaying = true;
        let mut effects = Vec::new();
        for key in keys {
            effects.extend(self.dispatch(key, measure));
        }
        self.repeat.replaying = false;
        effects
    }

    /// `@:`: runs the last `:` line again. Counts are ignored.
    fn repeat_command_line(&mut self) -> Vec<Effect> {
        if let Some(line) = self.last_command_line.clone() {
            self.execute_line(&line)
        } else {
            self.error("no command line to repeat yet");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_keys_are_in_two_tables() {
        let mut all: Vec<&[Key]> = MOTIONS.iter().map(|(k, _)| *k).collect();
        all.extend(OPERATORS.iter().map(|(k, _)| *k));
        all.extend(NORMAL.iter().map(|(k, _)| *k));
        for (i, keys) in all.iter().enumerate() {
            for other in all.iter().skip(i + 1) {
                assert_ne!(keys, other, "{}", format_keys(keys));
            }
        }
    }

    #[test]
    fn counts_multiply_within_max_count() {
        assert_eq!(multiply(None, None), None);
        assert_eq!(multiply(Some(2), None), Some(2));
        assert_eq!(multiply(None, Some(3)), Some(3));
        assert_eq!(multiply(Some(2), Some(3)), Some(6));
        assert_eq!(multiply(Some(MAX_COUNT), Some(MAX_COUNT)), Some(MAX_COUNT));
    }
}
