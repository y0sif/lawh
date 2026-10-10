//! The board model: objects in z-order, each with a stable id.
//!
//! Coordinates are grid units (`f64`) with y growing down, like a screen.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use kurbo::Point;

/// A stable object id such as `c1` or `t2`. It also appears in the file.
///
/// Valid ids are non-empty and use only ASCII letters, digits, `_` and `-`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(String);

impl ObjectId {
    /// Checks `id` and wraps it.
    pub fn new(id: impl Into<String>) -> Result<Self, BoardError> {
        let id = id.into();
        if is_valid_id(&id) {
            Ok(Self(id))
        } else {
            Err(BoardError::InvalidId(id))
        }
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ObjectId {
    type Err = BoardError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl AsRef<str> for ObjectId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(is_id_byte)
}

fn is_id_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// One object on the board.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub id: ObjectId,
    pub shape: Shape,
}

impl Object {
    pub fn new(id: ObjectId, shape: Shape) -> Self {
        Self { id, shape }
    }
}

/// What an object is and where it sits, in grid units.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// Plain text. `at` is the top-left corner of the text box.
    Text { at: Point, text: String },
    /// Typst math. `at` is the top-left corner of the formula box, `src` the Typst math source.
    Math { at: Point, src: String },
    /// An ellipse; a circle when `rx == ry`.
    Ellipse { center: Point, rx: f64, ry: f64 },
    /// A rectangle. `origin` is the top-left corner.
    Rect { origin: Point, w: f64, h: f64 },
    /// A straight line from `a` to `b`.
    Line { a: Point, b: Point },
    /// An arrow from `a` to `b`, with the arrowhead at `b`.
    Arrow { a: Point, b: Point },
}

impl Shape {
    /// True when every coordinate and size is a finite number.
    pub fn is_finite(&self) -> bool {
        match self {
            Self::Text { at, .. } | Self::Math { at, .. } => at.is_finite(),
            Self::Ellipse { center, rx, ry } => {
                center.is_finite() && rx.is_finite() && ry.is_finite()
            }
            Self::Rect { origin, w, h } => origin.is_finite() && w.is_finite() && h.is_finite(),
            Self::Line { a, b } | Self::Arrow { a, b } => a.is_finite() && b.is_finite(),
        }
    }
}

/// Why the board refused a change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BoardError {
    #[error("invalid object id {0:?}: use only ASCII letters, digits, `_` and `-`")]
    InvalidId(String),
    #[error("an object with id `{0}` is already on the board")]
    DuplicateId(ObjectId),
    #[error("object `{0}` has a coordinate or size that is not a finite number")]
    NonFinite(ObjectId),
}

/// The whiteboard: objects in z-order, back to front.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Board {
    objects: Vec<Object>,
    /// Position of each id in `objects`.
    index: HashMap<ObjectId, usize>,
}

impl Board {
    pub fn new() -> Self {
        Self::default()
    }

    /// All objects, back to front.
    pub fn objects(&self) -> &[Object] {
        &self.objects
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    pub fn get(&self, id: &ObjectId) -> Option<&Object> {
        self.index.get(id).and_then(|&i| self.objects.get(i))
    }

    pub fn contains(&self, id: &ObjectId) -> bool {
        self.index.contains_key(id)
    }

    /// Adds `object` on top of the others.
    ///
    /// Fails if its id is already used or any of its numbers is not finite.
    pub fn insert(&mut self, object: Object) -> Result<(), BoardError> {
        if self.contains(&object.id) {
            return Err(BoardError::DuplicateId(object.id));
        }
        if !object.shape.is_finite() {
            return Err(BoardError::NonFinite(object.id));
        }
        self.push_checked(object);
        Ok(())
    }

    /// Adds an object the caller has already checked: unique id, finite numbers.
    pub(crate) fn push_checked(&mut self, object: Object) {
        self.index.insert(object.id.clone(), self.objects.len());
        self.objects.push(object);
    }

    /// The first unused id of the form `{prefix}{n}`, counting from 1.
    ///
    /// Characters that are not allowed in ids are dropped from `prefix`.
    pub fn fresh_id(&self, prefix: &str) -> ObjectId {
        let prefix: String = prefix
            .chars()
            .filter(|c| u8::try_from(*c).is_ok_and(is_id_byte))
            .collect();
        let mut n: usize = 1;
        loop {
            let id = ObjectId(format!("{prefix}{n}"));
            if !self.contains(&id) {
                return id;
            }
            n += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> ObjectId {
        ObjectId::new(s).unwrap()
    }

    fn circle(name: &str, r: f64) -> Object {
        Object::new(
            id(name),
            Shape::Ellipse {
                center: Point::new(0.0, 0.0),
                rx: r,
                ry: r,
            },
        )
    }

    #[test]
    fn ids_accept_letters_digits_underscore_dash() {
        for ok in ["c1", "t2", "A", "_", "-", "9", "a-b_c", "true"] {
            assert!(ObjectId::new(ok).is_ok(), "{ok}");
        }
        for bad in ["", " ", "c 1", "c.1", "é", "c1\n", "a/b", "\"x\""] {
            assert_eq!(
                ObjectId::new(bad),
                Err(BoardError::InvalidId(bad.to_owned())),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn id_display_and_parse() {
        let parsed: ObjectId = "c1".parse().unwrap();
        assert_eq!(parsed.as_str(), "c1");
        assert_eq!(parsed.to_string(), "c1");
        assert!("c 1".parse::<ObjectId>().is_err());
    }

    #[test]
    fn insert_keeps_z_order_and_indexes_ids() {
        let mut board = Board::new();
        board.insert(circle("c1", 1.0)).unwrap();
        board.insert(circle("c2", 2.0)).unwrap();
        let ids: Vec<&str> = board.objects().iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["c1", "c2"]);
        assert_eq!(board.get(&id("c2")), Some(&circle("c2", 2.0)));
        assert_eq!(board.get(&id("c3")), None);
        assert_eq!(board.len(), 2);
    }

    #[test]
    fn insert_rejects_duplicate_id() {
        let mut board = Board::new();
        board.insert(circle("c1", 1.0)).unwrap();
        assert_eq!(
            board.insert(circle("c1", 2.0)),
            Err(BoardError::DuplicateId(id("c1")))
        );
        assert_eq!(board.len(), 1);
    }

    #[test]
    fn insert_rejects_non_finite_numbers() {
        let mut board = Board::new();
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                board.insert(circle("c1", bad)),
                Err(BoardError::NonFinite(id("c1")))
            );
            let text = Object::new(
                id("t1"),
                Shape::Text {
                    at: Point::new(0.0, bad),
                    text: String::new(),
                },
            );
            assert_eq!(board.insert(text), Err(BoardError::NonFinite(id("t1"))));
        }
        assert!(board.is_empty());
    }

    #[test]
    fn fresh_id_counts_up_past_used_ids() {
        let mut board = Board::new();
        assert_eq!(board.fresh_id("c"), id("c1"));
        board.insert(circle("c1", 1.0)).unwrap();
        board.insert(circle("c3", 1.0)).unwrap();
        assert_eq!(board.fresh_id("c"), id("c2"));
        board.insert(circle("c2", 1.0)).unwrap();
        assert_eq!(board.fresh_id("c"), id("c4"));
        assert_eq!(board.fresh_id("t"), id("t1"));
    }

    #[test]
    fn fresh_id_drops_invalid_prefix_characters() {
        let board = Board::new();
        assert_eq!(board.fresh_id("a b.c"), id("abc1"));
        assert_eq!(board.fresh_id(""), id("1"));
        assert_eq!(board.fresh_id("é"), id("1"));
    }
}
