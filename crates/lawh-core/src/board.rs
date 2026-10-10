//! The board model: objects in z-order, each with a stable id.
//!
//! Coordinates are grid units (`f64`) with y growing down, like a screen.
//!
//! The board keeps every number within fixed limits ([`MAX_COORDINATE`], [`MAX_SIZE`]), every
//! text within [`MAX_TEXT_CHARS`] and the number of objects within [`MAX_OBJECTS`], so bounds,
//! zoom-to-fit and rendering never meet values that overflow or allocate without limit. The file
//! reader enforces the same limits.

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

/// Largest magnitude of any coordinate, in grid units: every `x` and `y` lies in
/// `-MAX_COORDINATE..=MAX_COORDINATE`.
pub const MAX_COORDINATE: f64 = 1_000_000.0;

/// Largest radius, width or height, in grid units.
///
/// Sizes must also be above zero: a zero or negative size draws nothing (or the same thing as its
/// absolute value), so the board rejects it instead of storing it.
pub const MAX_SIZE: f64 = 1_000_000.0;

/// Most characters (Unicode scalar values) in the content of a text or math object.
pub const MAX_TEXT_CHARS: usize = 100_000;

/// Most objects a board holds.
///
/// Every object on screen costs drawing time, so a board of many objects that all reach the screen
/// is slow to draw. Measured with `board_objects_frame_time` in lawh-render's `tests/bench.rs`
/// (Linux `x86_64`, release, fitted at 1920x1080, 2026-10-10), median frames of the worst boards
/// that are cheap to write:
///
/// | objects | full-screen rects | screen-filling circles | `r=1` circles, stacked |
/// |---|---|---|---|
/// | 400 | 54 ms | 56 ms | 22 ms |
/// | 500 | 65 ms | 79 ms | 28 ms |
/// | 1,000 (before the cap) | 130 ms | 141 ms | 49 ms |
///
/// The renderer has a frame budget on top of this, so the cap errs on the generous side of a
/// 50 ms frame. Real boards hold tens to hundreds of objects. Text is bounded per object by
/// [`MAX_TEXT_CHARS`], not by this: a 75-character line costs about 0.4 ms to draw fitted.
pub const MAX_OBJECTS: usize = 500;

/// True when `v` is a coordinate the board accepts. False for NaN.
pub(crate) fn is_valid_coordinate(v: f64) -> bool {
    v.abs() <= MAX_COORDINATE
}

/// True when `v` is a size the board accepts: above zero and at most [`MAX_SIZE`]. False for NaN.
pub(crate) fn is_valid_size(v: f64) -> bool {
    v > 0.0 && v <= MAX_SIZE
}

/// True when `s` has at most [`MAX_TEXT_CHARS`] characters.
pub(crate) fn is_valid_text(s: &str) -> bool {
    // Only the bytes could make it too long, so skip counting short strings.
    s.len() <= MAX_TEXT_CHARS || s.chars().nth(MAX_TEXT_CHARS).is_none()
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

    /// The points that place the shape: one for most kinds, both ends for lines and arrows.
    fn points(&self) -> [Option<Point>; 2] {
        match self {
            Self::Text { at, .. } | Self::Math { at, .. } => [Some(*at), None],
            Self::Ellipse { center, .. } => [Some(*center), None],
            Self::Rect { origin, .. } => [Some(*origin), None],
            Self::Line { a, b } | Self::Arrow { a, b } => [Some(*a), Some(*b)],
        }
    }

    /// The radii of an ellipse or the width and height of a rectangle.
    fn sizes(&self) -> Option<[f64; 2]> {
        match self {
            Self::Ellipse { rx, ry, .. } => Some([*rx, *ry]),
            Self::Rect { w, h, .. } => Some([*w, *h]),
            Self::Text { .. } | Self::Math { .. } | Self::Line { .. } | Self::Arrow { .. } => None,
        }
    }

    /// The text of a text object or the source of a math object.
    fn content(&self) -> Option<&str> {
        match self {
            Self::Text { text, .. } => Some(text),
            Self::Math { src, .. } => Some(src),
            Self::Ellipse { .. } | Self::Rect { .. } | Self::Line { .. } | Self::Arrow { .. } => {
                None
            }
        }
    }

    /// Every coordinate is within [`MAX_COORDINATE`].
    pub fn coordinates_in_range(&self) -> bool {
        self.points()
            .into_iter()
            .flatten()
            .all(|p| is_valid_coordinate(p.x) && is_valid_coordinate(p.y))
    }

    /// Every size is above zero and at most [`MAX_SIZE`]. True for kinds without sizes.
    pub fn sizes_in_range(&self) -> bool {
        self.sizes()
            .is_none_or(|s| s.into_iter().all(is_valid_size))
    }

    /// The text or math content has at most [`MAX_TEXT_CHARS`] characters.
    pub fn content_in_range(&self) -> bool {
        self.content().is_none_or(is_valid_text)
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
    #[error("object `{0}` has a coordinate outside -{max}..={max}", max = MAX_COORDINATE)]
    CoordinateOutOfRange(ObjectId),
    #[error("object `{0}` has a size that is not above 0 and at most {max}", max = MAX_SIZE)]
    InvalidSize(ObjectId),
    #[error("object `{0}` has more than {max} characters of text", max = MAX_TEXT_CHARS)]
    TextTooLong(ObjectId),
    #[error("object `{0}` would be one too many: a board holds at most {max} objects", max = MAX_OBJECTS)]
    TooManyObjects(ObjectId),
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
    /// Fails if its id is already used, any of its numbers is not finite or outside the limits
    /// ([`MAX_COORDINATE`], [`MAX_SIZE`]), its text is longer than [`MAX_TEXT_CHARS`], or the
    /// board already holds [`MAX_OBJECTS`] objects.
    pub fn insert(&mut self, object: Object) -> Result<(), BoardError> {
        let shape = &object.shape;
        let problem: Option<fn(ObjectId) -> BoardError> = if self.is_full() {
            Some(BoardError::TooManyObjects)
        } else if self.contains(&object.id) {
            Some(BoardError::DuplicateId)
        } else if !shape.is_finite() {
            Some(BoardError::NonFinite)
        } else if !shape.coordinates_in_range() {
            Some(BoardError::CoordinateOutOfRange)
        } else if !shape.sizes_in_range() {
            Some(BoardError::InvalidSize)
        } else if !shape.content_in_range() {
            Some(BoardError::TextTooLong)
        } else {
            None
        };
        if let Some(error) = problem {
            return Err(error(object.id));
        }
        self.push_checked(object);
        Ok(())
    }

    /// True when the board holds [`MAX_OBJECTS`] objects, so [`Board::insert`] refuses another.
    pub fn is_full(&self) -> bool {
        self.objects.len() >= MAX_OBJECTS
    }

    /// Adds an object the caller has already checked as [`Board::insert`] does, the object
    /// count included.
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

    fn rect(name: &str, origin: Point, w: f64, h: f64) -> Object {
        Object::new(id(name), Shape::Rect { origin, w, h })
    }

    #[test]
    fn insert_accepts_numbers_at_the_limits() {
        let mut board = Board::new();
        let far = Point::new(MAX_COORDINATE, -MAX_COORDINATE);
        board.insert(rect("r1", far, MAX_SIZE, MAX_SIZE)).unwrap();
        board.insert(rect("r2", far, 5e-324, 1e-9)).unwrap();
        let line = Shape::Line {
            a: Point::new(-MAX_COORDINATE, 0.0),
            b: Point::new(MAX_COORDINATE, 0.0),
        };
        board.insert(Object::new(id("l1"), line)).unwrap();
        assert_eq!(board.len(), 3);
    }

    #[test]
    fn insert_rejects_coordinates_out_of_range() {
        let mut board = Board::new();
        for bad in [
            MAX_COORDINATE * (1.0 + f64::EPSILON),
            -1e7,
            1e308,
            -f64::MAX,
        ] {
            let shapes = [
                Shape::Text {
                    at: Point::new(bad, 0.0),
                    text: String::new(),
                },
                Shape::Math {
                    at: Point::new(0.0, bad),
                    src: String::new(),
                },
                Shape::Ellipse {
                    center: Point::new(bad, bad),
                    rx: 1.0,
                    ry: 1.0,
                },
                Shape::Rect {
                    origin: Point::new(0.0, bad),
                    w: 1.0,
                    h: 1.0,
                },
                Shape::Line {
                    a: Point::ZERO,
                    b: Point::new(bad, 0.0),
                },
                Shape::Arrow {
                    a: Point::new(0.0, bad),
                    b: Point::ZERO,
                },
            ];
            for shape in shapes {
                assert_eq!(
                    board.insert(Object::new(id("o1"), shape.clone())),
                    Err(BoardError::CoordinateOutOfRange(id("o1"))),
                    "{shape:?}"
                );
            }
        }
        assert!(board.is_empty());
    }

    #[test]
    fn insert_rejects_sizes_not_above_zero_or_too_big() {
        let mut board = Board::new();
        for bad in [
            0.0,
            -0.0,
            -1.0,
            -5e-324,
            MAX_SIZE * (1.0 + f64::EPSILON),
            1e60,
        ] {
            assert_eq!(
                board.insert(circle("c1", bad)),
                Err(BoardError::InvalidSize(id("c1"))),
                "{bad:?}"
            );
            assert_eq!(
                board.insert(rect("r1", Point::ZERO, 1.0, bad)),
                Err(BoardError::InvalidSize(id("r1"))),
                "{bad:?}"
            );
            let ellipse = Shape::Ellipse {
                center: Point::ZERO,
                rx: bad,
                ry: 1.0,
            };
            assert_eq!(
                board.insert(Object::new(id("e1"), ellipse)),
                Err(BoardError::InvalidSize(id("e1"))),
                "{bad:?}"
            );
        }
        assert!(board.is_empty());
    }

    #[test]
    fn insert_rejects_text_over_the_limit() {
        let mut board = Board::new();
        // Multi-byte characters: the limit counts characters, not bytes.
        let longest = "é".repeat(MAX_TEXT_CHARS);
        let text = |name: &str, text: String| {
            Object::new(
                id(name),
                Shape::Text {
                    at: Point::ZERO,
                    text,
                },
            )
        };
        board.insert(text("t1", longest.clone())).unwrap();
        assert_eq!(
            board.insert(text("t2", format!("{longest}x"))),
            Err(BoardError::TextTooLong(id("t2")))
        );
        let math = Shape::Math {
            at: Point::ZERO,
            src: "x".repeat(MAX_TEXT_CHARS + 1),
        };
        assert_eq!(
            board.insert(Object::new(id("m1"), math)),
            Err(BoardError::TextTooLong(id("m1")))
        );
        assert_eq!(board.len(), 1);
    }

    #[test]
    fn insert_stops_at_the_object_limit() {
        let mut board = Board::new();
        for i in 0..MAX_OBJECTS {
            board.insert(circle(&format!("c{i}"), 1.0)).unwrap();
        }
        assert!(board.is_full());
        assert_eq!(
            board.insert(circle("extra", 1.0)),
            Err(BoardError::TooManyObjects(id("extra")))
        );
        // A full board refuses before looking at the object: even a duplicate id.
        assert_eq!(
            board.insert(circle("c0", 1.0)),
            Err(BoardError::TooManyObjects(id("c0")))
        );
        assert_eq!(board.len(), MAX_OBJECTS);
    }

    #[test]
    fn limit_errors_name_the_limits() {
        assert_eq!(
            BoardError::CoordinateOutOfRange(id("c1")).to_string(),
            "object `c1` has a coordinate outside -1000000..=1000000"
        );
        assert_eq!(
            BoardError::InvalidSize(id("c1")).to_string(),
            "object `c1` has a size that is not above 0 and at most 1000000"
        );
        assert_eq!(
            BoardError::TextTooLong(id("t1")).to_string(),
            "object `t1` has more than 100000 characters of text"
        );
        assert_eq!(
            BoardError::TooManyObjects(id("c1")).to_string(),
            "object `c1` would be one too many: a board holds at most 500 objects"
        );
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
