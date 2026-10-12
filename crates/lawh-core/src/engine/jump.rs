//! Jump labels: `s` puts a short label on every object in view, and typing a label goes to its
//! object, as flash.nvim and hop do in Neovim.
//!
//! `s` shows the labels in Normal mode, in Visual mode and after an operator (`d`, `y`); counts
//! are ignored. The labels go on every object whose bounds overlap the view (see
//! [`Engine::set_view`]), edges included, except the current object; with no view, on every
//! object. They are ordered by the distance from the cursor point to the object's anchor,
//! nearest first, ties by reading order, which breaks its own ties by z-order.
//!
//! A label is one or two characters of the `jumplabels` option, in its order (see
//! [`LabelChars`]). The nearest objects get one character and the rest two, never more
//! ([`MAX_LABEL_KEYS`]), and no label is the start of another: as many objects as can get one
//! character do, while the characters left over, as first characters, still give every other
//! object a label of two. With `m` characters that is `m * m` labels at most; objects past
//! that get none, and the message says how many.
//!
//! While labels show, each key goes to them and is not mapped, as the key after Vim's `f`, `t`
//! or `r`, so a label `;` works under `noremap ; l`. Typing a whole label acts on its object:
//!
//! - In Normal mode the cursor goes onto it.
//! - In Visual mode the cursor goes onto it, stretching the rectangle; Visual mode stays.
//! - After `d` or `y` the operator acts on that object alone, from an object or a grid cursor,
//!   and the cursor stays where it was (a remote operation, as flash's `r`). Undo of a remote
//!   delete puts the object back and the cursor where it was. `.` repeats `ds` and shows the
//!   labels again, for a new label.
//!
//! The labels end with no jump on `<Esc>`, as `<Esc>` ends the key Vim's `f` waits for: in
//! Normal and Visual mode only the labels end (Visual mode stays, as after `vf<Esc>`), and after
//! an operator the operator ends too (as after `df<Esc>`). A key that starts no label ends them
//! the same way, is dropped, and the message says `no label "x"`. `<BS>` takes back the first
//! character of a two-character label, and with none typed ends the labels, as `<BS>` on an
//! empty command line. A click ends them. Nothing jumps until a whole label is typed, even with
//! one object labelled. A mapping timeout runs no label keys: they never wait for a mapping.
//!
//! Making the labels measures the board, puts it in reading order and sorts the objects in view,
//! all counted as work (see [`MAX_KEY_WORK`](super::MAX_KEY_WORK)), and so is each label key.

use std::fmt::{self, Debug, Display, Formatter, Write as _};

use kurbo::{Point, Rect};

use super::motion::reading_order;
use super::normal::Operator;
use super::register::Register;
use super::undo::Edit;
use super::work::{LABEL_WORK, ORDER_WORK, PICK_WORK, TAKE_WORK};
use super::{Cursor, Effect, Engine, Measure, anchor};
use crate::board::ObjectId;
use crate::key::{Key, KeyCode, format_keys};

/// Most characters in a jump label: 2.
///
/// Two characters of the default 26 make 676 labels, more than a board holds objects
/// ([`MAX_OBJECTS`](crate::board::MAX_OBJECTS), 500), so with the default `jumplabels` every
/// object in view gets a label of at most two keys, as with 23 characters or more (529). A third
/// key would make every label slower to read and to type, for a smaller alphabet only: with
/// fewer characters, objects past the square of their number get no label, and the message says
/// how many (zoom in, or go nearer, and type `s` again).
pub const MAX_LABEL_KEYS: usize = 2;

/// Fewest characters `jumplabels` may hold: 2.
///
/// One character makes one label only: no second label could start with it, and none can start
/// without it. Two make four labels of at most [`MAX_LABEL_KEYS`] keys.
pub const MIN_LABEL_CHARS: usize = 2;

/// Most characters `jumplabels` may hold: 52, the ASCII letters in both cases.
///
/// From 23 characters on, two keys label a full board (23 * 23 = 529), and labels are quick only
/// on keys the hand finds without looking, which 52 already goes past. The characters are kept
/// in a fixed array, so the option's value stays `Copy` and `Eq` (see [`LabelChars`]): 52 of
/// them take 208 bytes, the largest `:set` value.
pub const MAX_LABEL_CHARS: usize = 52;

/// The `jumplabels` option by default: flash.nvim's
/// (<https://github.com/folke/flash.nvim/blob/main/lua/flash/config.lua>), the home row, then
/// the top row, then the bottom row, each from the left.
pub const DEFAULT_JUMP_LABELS: &str = "asdfghjklqwertyuiopzxcvbnm";

/// The keys that show jump labels, in Normal mode, Visual mode and after an operator.
pub(super) const JUMP_KEYS: &[Key] = &[Key::char('s')];

/// The characters jump labels are made of (`:set jumplabels`), in the order labels take them,
/// the first for the nearest object: from [`MIN_LABEL_CHARS`] to [`MAX_LABEL_CHARS`] different
/// characters, none of them a space or a control character. Kept in a fixed array, so
/// [`OptionValue`](super::OptionValue) stays `Copy` and `Eq`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct LabelChars {
    chars: [char; MAX_LABEL_CHARS],
    /// How many of `chars` are used.
    len: usize,
}

impl LabelChars {
    /// The characters of `s` as an alphabet for labels, or `None` when there are fewer than
    /// [`MIN_LABEL_CHARS`] or more than [`MAX_LABEL_CHARS`], one comes twice, or one is a space
    /// or a control character.
    pub fn new(s: &str) -> Option<Self> {
        let mut chars = ['\0'; MAX_LABEL_CHARS];
        let mut len = 0;
        for c in s.chars() {
            if c.is_whitespace() || c.is_control() || chars.get(..len)?.contains(&c) {
                return None;
            }
            *chars.get_mut(len)? = c;
            len += 1;
        }
        (len >= MIN_LABEL_CHARS).then_some(Self { chars, len })
    }

    /// The characters, in order.
    pub fn as_slice(&self) -> &[char] {
        self.chars.get(..self.len).unwrap_or_default()
    }
}

impl Default for LabelChars {
    /// [`DEFAULT_JUMP_LABELS`].
    fn default() -> Self {
        let mut chars = ['\0'; MAX_LABEL_CHARS];
        let mut len = 0;
        for (slot, c) in chars.iter_mut().zip(DEFAULT_JUMP_LABELS.chars()) {
            *slot = c;
            len += 1;
        }
        Self { chars, len }
    }
}

impl Display for LabelChars {
    /// The characters as `:set` takes them: `asdf`.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.as_slice().iter().try_for_each(|c| f.write_char(*c))
    }
}

impl Debug for LabelChars {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("LabelChars")
            .field(&self.to_string())
            .finish()
    }
}

/// A jump label on an object, for drawing.
#[derive(Debug, Clone, PartialEq)]
pub struct JumpLabel {
    /// The object the label goes to.
    pub id: ObjectId,
    /// Where to draw the label: the object's anchor (see [`anchor`]), in grid units.
    pub at: Point,
    /// The label: one or two characters of `jumplabels`.
    pub text: String,
}

/// What typing a label does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Jump {
    /// Normal mode: the cursor goes onto the object.
    Move,
    /// Visual mode: the cursor goes onto the object, stretching the rectangle.
    Stretch,
    /// After `d` or `y`: the operator acts on the object alone.
    Remote(Operator),
}

/// Jump labels showing, until a label is typed or they end.
#[derive(Debug, Clone)]
pub(super) struct Labels {
    /// Every label, nearest object first.
    shown: Vec<JumpLabel>,
    /// The first character of a two-character label, once typed; else empty.
    typed: String,
    jump: Jump,
    /// The keys that showed the labels, in Vim notation: `s`, `ds`.
    keys: String,
}

impl Labels {
    /// The keys that showed the labels and the character typed since, in Vim notation, for
    /// [`Engine::pending`]: `ds`, `dsk`.
    pub(super) fn pending(&self) -> String {
        let typed: String = self
            .typed
            .chars()
            .map(|c| Key::char(c).to_string())
            .collect();
        self.keys.clone() + &typed
    }
}

/// How `n` objects are labelled with `m` characters: how many get one character, and how many
/// get a label in all (one or two characters).
///
/// Up to `m` objects get one character each. Past that, with `s` objects on one character, the
/// `m - s` characters left start `m` labels of two characters each: the most `s` for which
/// `s + (m - s) * m` covers the objects, as far as `m * m` (no `s` at all).
fn split(m: usize, n: usize) -> (usize, usize) {
    if m < MIN_LABEL_CHARS {
        let labelled = n.min(m);
        return (labelled, labelled);
    }
    let square = m.saturating_mul(m);
    let labelled = n.min(square);
    if labelled <= m {
        return (labelled, labelled);
    }
    // `s + (m - s) * m >= labelled` is `s <= (m * m - labelled) / (m - 1)`.
    let singles = (square - labelled) / (m - 1);
    (singles.min(m), labelled)
}

/// The labels of the first `n` objects, nearest first, made of `chars` (see [`split`]): one
/// character for the first, then two, the first characters taken in order after those used
/// alone, and the second running through all of `chars`. Fewer than `n` when the labels run out.
fn label_texts(chars: &[char], n: usize) -> Vec<String> {
    let m = chars.len();
    let (singles, labelled) = split(m, n);
    let single = chars.iter().take(singles).map(ToString::to_string);
    let firsts = chars.iter().skip(singles);
    let doubles = firsts.flat_map(|a| chars.iter().map(move |b| [*a, *b].iter().collect()));
    single.chain(doubles).take(labelled).collect()
}

/// `bounds` overlap `view`, edges included.
fn in_view(view: Rect, bounds: Rect) -> bool {
    let b = bounds.abs();
    b.x0 <= view.x1 && view.x0 <= b.x1 && b.y0 <= view.y1 && view.y0 <= b.y1
}

impl Engine {
    /// The part of the board in view, in grid units: the part of the window above the status
    /// line. `s` labels the objects whose bounds overlap it, edges included; `None`, the
    /// default, labels every object. A view with a coordinate that is not a number counts as
    /// `None`.
    ///
    /// The labels are made when `s` runs, from the view set then: the app sets the view before
    /// each key ([`Engine::feed`]) and timeout ([`Engine::timeout`]) whenever it has changed.
    /// Labels already showing stay on their objects while the view pans or zooms.
    pub fn set_view(&mut self, view: Option<Rect>) {
        self.view = view
            .filter(|r| ![r.x0, r.y0, r.x1, r.y1].iter().any(|v| v.is_nan()))
            .map(|r| r.abs());
    }

    /// The jump labels showing, nearest object first, and the first character typed of a
    /// two-character label (empty before one is). Only the labels that start with it can still
    /// be typed: those are the ones to draw. `None` when no labels show.
    pub fn jump_labels(&self) -> Option<(&[JumpLabel], &str)> {
        self.labels
            .as_ref()
            .map(|l| (l.shown.as_slice(), l.typed.as_str()))
    }

    /// `s`: shows labels for `jump`, typed with `keys`. Measuring the board, putting it in
    /// reading order and sorting the objects in view count as work; when the key runs out, no
    /// labels show. With nothing to label the message says so.
    pub(super) fn show_labels(&mut self, jump: Jump, keys: &[Key], measure: &mut dyn Measure) {
        if !self.charge(self.layout_work()) {
            return;
        }
        let layout = self.layout(measure);
        if !self.charge(ORDER_WORK.saturating_mul(layout.len())) {
            return;
        }
        let mut rank = vec![usize::MAX; layout.len()];
        for (r, i) in reading_order(&layout).into_iter().enumerate() {
            if let Some(slot) = rank.get_mut(i) {
                *slot = r;
            }
        }
        let current = match &self.cursor {
            Cursor::Object(id) => self.index_of(id),
            Cursor::Grid(_) => None,
        };
        let from = self.cursor_point();
        let view = self.view;
        // (distance, rank in reading order, index), for each object to label.
        let mut targets: Vec<(f64, usize, usize)> = self
            .board
            .objects()
            .iter()
            .zip(&layout)
            .enumerate()
            .filter(|(i, (_, b))| Some(*i) != current && view.is_none_or(|v| in_view(v, **b)))
            .map(|(i, (o, _))| {
                let r = rank.get(i).copied().unwrap_or(usize::MAX);
                (from.distance(anchor(&o.shape)), r, i)
            })
            .collect();
        if targets.is_empty() {
            return self.error("no objects in view");
        }
        if !self.charge(LABEL_WORK.saturating_mul(targets.len())) {
            return;
        }
        targets.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let texts = label_texts(self.settings.jumplabels.as_slice(), targets.len());
        let left_out = targets.len().saturating_sub(texts.len());
        let objects = self.board.objects();
        let shown = targets
            .iter()
            .zip(texts)
            .filter_map(|(&(_, _, i), text)| {
                let object = objects.get(i)?;
                Some(JumpLabel {
                    id: object.id.clone(),
                    at: anchor(&object.shape),
                    text,
                })
            })
            .collect();
        self.labels = Some(Labels {
            shown,
            typed: String::new(),
            jump,
            keys: format_keys(keys),
        });
        match left_out {
            0 => {}
            1 => self.info("1 object in view has no label"),
            n => self.info(format!("{n} objects in view have no label")),
        }
    }

    /// A key while labels show (see the module docs). Looking through the labels counts as
    /// work; when the key runs out, the labels end.
    pub(super) fn label_key(&mut self, key: Key, measure: &mut dyn Measure) -> Vec<Effect> {
        let Some(labels) = self.labels.as_mut() else {
            return Vec::new();
        };
        if key == Key::named(KeyCode::Esc) {
            self.labels = None;
            return Vec::new();
        }
        if key == Key::named(KeyCode::Backspace) {
            if labels.typed.is_empty() {
                self.labels = None;
            } else {
                labels.typed.clear();
            }
            return Vec::new();
        }
        let looked = labels.shown.len();
        if !self.charge(PICK_WORK.saturating_mul(looked)) {
            self.labels = None;
            return Vec::new();
        }
        let Some(labels) = self.labels.take() else {
            return Vec::new();
        };
        let mut typed = labels.typed.clone();
        if let Key {
            code: KeyCode::Char(c),
            mods,
        } = key
            && mods.is_empty()
        {
            typed.push(c);
            // One pass: the label typed, else whether any label goes on from it.
            let mut starts = false;
            for label in &labels.shown {
                if label.text == typed {
                    let id = label.id.clone();
                    self.jump_to(&id, labels.jump, measure);
                    return Vec::new();
                }
                starts = starts || label.text.starts_with(&typed);
            }
            if labels.typed.is_empty() && starts {
                self.labels = Some(Labels { typed, ..labels });
                return Vec::new();
            }
        }
        let typed_keys: Vec<Key> = labels.typed.chars().map(Key::char).collect();
        let shown = format_keys(&typed_keys) + &key.to_string();
        self.error(format!("no label \"{shown}\""));
        Vec::new()
    }

    /// Acts on object `id` for a label typed: see [`Jump`].
    fn jump_to(&mut self, id: &ObjectId, jump: Jump, measure: &mut dyn Measure) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        match jump {
            Jump::Move => self.cursor = Cursor::Object(id.clone()),
            Jump::Stretch => {
                // The selection after the move is paid for first, as for a Visual motion.
                if self.charge(self.layout_work()) {
                    self.cursor = Cursor::Object(id.clone());
                    self.select(measure);
                }
            }
            Jump::Remote(operator) => {
                if self.remote(operator, index) {
                    // Counts are ignored, so `.` replays `ds` alone and shows the labels again.
                    self.repeat.finish(None);
                }
            }
        }
    }

    /// `d` or `y` after `s`: puts the object at position `index` (z-order) in the register, and
    /// deletes it as one undo step or yanks it. The cursor stays where it was, a grid point too,
    /// and an undo puts it back there. True when the board changed.
    fn remote(&mut self, operator: Operator, index: usize) -> bool {
        if !self.charge(TAKE_WORK) {
            return false;
        }
        let Some(object) = self.board.objects().get(index).cloned() else {
            return false;
        };
        match operator {
            Operator::Yank => {
                self.register = Some(Register::Objects(vec![object]));
                false
            }
            Operator::Delete => {
                let edit = Edit::Remove {
                    index,
                    object: object.clone(),
                };
                if let Err(error) = edit.apply(&mut self.board) {
                    self.error(error.to_string());
                    return false;
                }
                self.register = Some(Register::Objects(vec![object]));
                let before = self.cursor.clone();
                self.commit(vec![edit], before);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::MAX_OBJECTS;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn the_default_is_a_good_alphabet() {
        assert_eq!(
            LabelChars::new(DEFAULT_JUMP_LABELS),
            Some(LabelChars::default())
        );
        assert_eq!(LabelChars::default().to_string(), DEFAULT_JUMP_LABELS);
        // Two keys label a full board with the default.
        let m = DEFAULT_JUMP_LABELS.chars().count();
        assert!(m * m >= MAX_OBJECTS);
    }

    #[test]
    fn alphabets_hold_from_min_to_max_label_chars_different_characters() {
        let letters: String = ('a'..='z').chain('A'..='Z').collect();
        assert_eq!(letters.chars().count(), MAX_LABEL_CHARS);
        let all = LabelChars::new(&letters).unwrap();
        assert_eq!(all.as_slice(), chars(&letters));
        assert_eq!(LabelChars::new(&format!("{letters};")), None);
        assert_eq!(LabelChars::new("ab").unwrap().as_slice(), ['a', 'b']);
        assert_eq!(LabelChars::new("a"), None);
        assert_eq!(LabelChars::new(""), None);
        assert_eq!(LabelChars::new("aba"), None);
        assert_eq!(LabelChars::new("a b"), None);
        assert_eq!(LabelChars::new("a\u{7}b"), None);
        assert_eq!(LabelChars::new(";ñ€").unwrap().to_string(), ";ñ€");
    }

    #[test]
    fn labels_split_between_one_and_two_characters() {
        let abc = chars("abc");
        assert_eq!(label_texts(&abc, 0), Vec::<String>::new());
        assert_eq!(label_texts(&abc, 3), ["a", "b", "c"]);
        // One more than the alphabet: two alone, `c` starts the rest.
        assert_eq!(label_texts(&abc, 4), ["a", "b", "ca", "cb"]);
        // 2 + 3 = 5 still fits two alone; 6 needs one alone and two first letters.
        assert_eq!(label_texts(&abc, 5), ["a", "b", "ca", "cb", "cc"]);
        assert_eq!(label_texts(&abc, 6), ["a", "ba", "bb", "bc", "ca", "cb"]);
        // The square, then past it: nine labels at most.
        let nine = label_texts(&abc, 9);
        assert_eq!(nine, ["aa", "ab", "ac", "ba", "bb", "bc", "ca", "cb", "cc"]);
        assert_eq!(label_texts(&abc, 12), nine);
        // 26 letters and 500 objects: seven alone, 19 first letters (seven alone and 493 of two).
        assert_eq!(split(26, 500), (7, 500));
        assert_eq!(split(26, 27), (25, 27));
        assert_eq!(split(26, 676), (0, 676));
        assert_eq!(split(26, 677), (0, 676));
        // Alphabets too small to split (only `Settings` could hold one, and it cannot) label
        // what they can.
        assert_eq!(label_texts(&['a'], 5), ["a"]);
        assert_eq!(label_texts(&[], 5), Vec::<String>::new());
    }

    #[test]
    fn views_take_objects_that_touch_them() {
        let view = Rect::new(0.0, 0.0, 10.0, 5.0);
        assert!(in_view(view, Rect::new(2.0, 2.0, 3.0, 3.0)));
        assert!(in_view(view, Rect::new(-5.0, -5.0, 0.0, 0.0)));
        assert!(in_view(view, Rect::new(10.0, 5.0, 10.0, 5.0)));
        assert!(!in_view(view, Rect::new(10.1, 0.0, 12.0, 1.0)));
        // A box given back to front.
        assert!(in_view(view, Rect::new(12.0, 1.0, 9.0, 0.0)));
    }
}
