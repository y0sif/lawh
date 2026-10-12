//! The board model: objects in z-order, each with a stable id.
//!
//! Coordinates are grid units (`f64`) with y growing down, like a screen.
//!
//! The board keeps every number within fixed limits ([`MAX_COORDINATE`], [`MAX_SIZE`], and
//! [`MIN_TEXT_SIZE`] to [`MAX_TEXT_SIZE`] for the size of text and math), every text within
//! [`MAX_TEXT_CHARS`] and [`MAX_TEXT_LINES`], all its text together within
//! [`MAX_BOARD_TEXT_CHARS`], the number of objects within [`MAX_OBJECTS`] and every id within
//! [`MAX_ID_CHARS`], so bounds, zoom-to-fit and rendering never meet values that overflow or
//! allocate without limit. The file reader enforces the same limits.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use kurbo::Point;

/// A stable object id such as `c1` or `t2`. It also appears in the file.
///
/// Valid ids are non-empty, at most [`MAX_ID_CHARS`] characters long, and use only ASCII
/// letters, digits, `_` and `-`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(String);

impl ObjectId {
    /// Checks `id` and wraps it. An id over [`MAX_ID_CHARS`] is too long whatever its
    /// characters.
    pub fn new(id: impl Into<String>) -> Result<Self, BoardError> {
        let id = id.into();
        if id.len() > MAX_ID_CHARS && id.chars().count() > MAX_ID_CHARS {
            Err(BoardError::IdTooLong(id))
        } else if is_valid_id(&id) {
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

/// Most characters in an object id.
///
/// Ids name objects in the file and in messages, where `c1` or `title-2` is typical. The cap
/// keeps a hand-edited file from putting an id of a megabyte into every message and lookup that
/// names its object.
pub const MAX_ID_CHARS: usize = 64;

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

/// The size of a text or math object that has none in its file: the text height lawh has
/// always drawn, an em of 0.5 grid units (20 logical pixels at zoom 1).
///
/// A size multiplies that height: size 2 draws text twice as tall and wide. Sizes are a
/// multiplier, not pixels or points, so a board looks the same at any zoom and on any screen,
/// and the writer leaves a size of exactly 1 out of the file.
pub const DEFAULT_TEXT_SIZE: f64 = 1.0;

/// Smallest size of a text or math object (see [`DEFAULT_TEXT_SIZE`]).
///
/// Size 0.25 is an em of 0.125 grid units: 5 logical pixels at zoom 1, 10 at zoom 2.
/// Smaller text is a smudge at every view one works in, and the size keys' first step is 0.25.
pub const MIN_TEXT_SIZE: f64 = 0.25;

/// Largest size of a text or math object (see [`DEFAULT_TEXT_SIZE`]).
///
/// Size 16 is an em of 8 grid units: 320 logical pixels at zoom 1, a heading taller than most
/// windows. The renderer's cost per glyph grows with its size on screen. At the app's closest
/// view (zoom 20) on a scale factor 4 screen, a size-16 formula, drawn 1.27 times text's em so
/// its x-height matches text's, has an em of about 32,600 pixels (16 x 0.5 units x 40 pixels a
/// unit x 20 x 4 x 1.27, derived). The frame budget's cost model is calibrated up to 65,536
/// pixels per em (`lawh-render/src/renderer/calibrate.rs`), so the largest text stays inside
/// what the budget has measured.
pub const MAX_TEXT_SIZE: f64 = 16.0;

/// Most characters (Unicode scalar values) in the content of one text or math object.
///
/// Real whiteboard text is labels and short paragraphs, and math sources are shorter still: a
/// paragraph is about 1,000 characters. Every text is laid out again whenever it changes, and a
/// text of 5,000 characters that changes script at every character (the costliest kind
/// measured, see [`MAX_BOARD_TEXT_CHARS`]) takes about 9 ms to lay out. Math sources count the
/// same: one that fails to compile is drawn as its source.
pub const MAX_TEXT_CHARS: usize = 5_000;

/// Most lines in the content of one text or math object: one more than its line breaks.
///
/// Line feed, carriage return, CRLF (one break), vertical tab, form feed, next line (U+0085),
/// line separator (U+2028) and paragraph separator (U+2029) each break a line, as they do in
/// KDL and in the renderer's text layout. 500 lines of text are about 300 grid units tall, far
/// past any note on a whiteboard. A line costs about 0.5 us and 190 bytes to lay out even when
/// it is empty, so a text of nothing but line breaks is the cheapest way to make many lines;
/// [`MAX_BOARD_TEXT_CHARS`] bounds them on the whole board.
pub const MAX_TEXT_LINES: usize = 500;

/// Most characters of text and math source on one board, all objects together.
///
/// Opening a board lays out every text and every formula's source (drawn until the formula
/// compiles) to find the board's bounds, and a frame looks at every text on screen. Measured
/// with `text_caps_worst_cases` in lawh-render's `tests/bench.rs` (Linux `x86_64`, release,
/// 2026-10-11) on the worst boards these caps allow, all text stacked on one spot:
///
/// | board | laying out on open | extra memory | slowest frame |
/// |---|---|---|---|
/// | 8 texts of 5,000 characters, the direction changing at every one, 455 lines each | 69 ms | +10 MiB | 13 ms |
/// | 8 texts of 5,000 characters, the script changing at every one | 68 ms | +14 MiB | 9 ms |
/// | 80 texts of 499 line breaks | 22 ms | +9 MiB | 8 ms |
/// | 500 texts of 80 characters of words | 26 ms | +4 MiB | 19 ms |
/// | 80 failing formulas of 499 line breaks, drawn as their source | 25 ms | +9 MiB | 9 ms |
/// | before these caps: 83 texts of 100,000 line breaks (an 8.3 MB file) | 4,203 ms | +1,475 MiB | 167 ms |
///
/// Laying out costs from 0.4 us a character for plain words to 1.7 us for text that changes
/// script or direction at every character, so the cap keeps opening under about 100 ms even
/// then, and frames stay well inside the renderer's budget. Real boards hold a few thousand
/// characters of text.
pub const MAX_BOARD_TEXT_CHARS: usize = 40_000;

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
/// 50 ms frame. Real boards hold tens to hundreds of objects. Text is bounded by
/// [`MAX_TEXT_CHARS`], [`MAX_TEXT_LINES`] and [`MAX_BOARD_TEXT_CHARS`], not by this: a
/// 75-character line costs about 0.4 ms to draw fitted.
pub const MAX_OBJECTS: usize = 500;

/// True when `v` is a coordinate the board accepts. False for NaN.
pub(crate) fn is_valid_coordinate(v: f64) -> bool {
    v.abs() <= MAX_COORDINATE
}

/// True when `v` is a size the board accepts: above zero and at most [`MAX_SIZE`]. False for NaN.
pub(crate) fn is_valid_size(v: f64) -> bool {
    v > 0.0 && v <= MAX_SIZE
}

/// True when `v` is a size of text or math the board accepts: from [`MIN_TEXT_SIZE`] to
/// [`MAX_TEXT_SIZE`], both included. False for NaN.
pub(crate) fn is_valid_text_size(v: f64) -> bool {
    (MIN_TEXT_SIZE..=MAX_TEXT_SIZE).contains(&v)
}

/// Why a text or math content is over a per-object cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextProblem {
    /// More than [`MAX_TEXT_CHARS`] characters.
    TooLong,
    /// More than [`MAX_TEXT_LINES`] lines.
    TooManyLines,
}

/// The number of characters in `s` when it is within [`MAX_TEXT_CHARS`] and [`MAX_TEXT_LINES`],
/// or the first cap passed, going through `s` from its start.
///
/// The board checks a text block's whole text on every key typed into it, and `.` or a count
/// can type thousands of keys at once. Counting one character at a time cost 20 ns a byte in a
/// debug build, so typing 5,000 characters with `10000ia<Esc>` took 250 ms there. Texts are
/// counted with the standard library's scans instead, which stay fast in a debug build; only
/// one past both caps, or too long to be within them, is counted one character at a time to
/// tell which cap it passes first.
pub(crate) fn check_text(s: &str) -> Result<usize, TextProblem> {
    // Past this many bytes a text has more characters than the cap: no need to count them all.
    let longest = MAX_TEXT_CHARS.saturating_mul(4);
    if s.len() <= longest {
        let chars = s.chars().count();
        // Both counts only grow along the text, so a cap that the whole text keeps is never
        // passed on the way; only a text past both needs telling which comes first.
        match (chars <= MAX_TEXT_CHARS, line_breaks(s) < MAX_TEXT_LINES) {
            (true, true) => return Ok(chars),
            (false, true) => return Err(TextProblem::TooLong),
            (true, false) => return Err(TextProblem::TooManyLines),
            (false, false) => {}
        }
    }
    let mut count = TextCount::new();
    for c in s.chars() {
        count.push(c)?;
    }
    Ok(count.chars())
}

/// The line breaks in `s` that [`TextCount`] counts: each break character, CRLF once.
fn line_breaks(s: &str) -> usize {
    let breaks: usize = [
        '\n', '\r', '\u{B}', '\u{C}', '\u{85}', '\u{2028}', '\u{2029}',
    ]
    .into_iter()
    .map(|c| s.matches(c).count())
    .sum();
    let crlf = if s.contains('\r') {
        s.matches("\r\n").count()
    } else {
        0
    };
    breaks.saturating_sub(crlf)
}

/// Counts the characters and lines of a text one character at a time, against
/// [`MAX_TEXT_CHARS`] and [`MAX_TEXT_LINES`], so a reader can stop at the first cap passed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TextCount {
    chars: usize,
    lines: usize,
    after_cr: bool,
}

impl TextCount {
    pub(crate) fn new() -> Self {
        Self {
            chars: 0,
            lines: 1,
            after_cr: false,
        }
    }

    /// Counts `c`, or fails if the text with `c` passes a cap.
    pub(crate) fn push(&mut self, c: char) -> Result<(), TextProblem> {
        self.chars += 1;
        if self.chars > MAX_TEXT_CHARS {
            return Err(TextProblem::TooLong);
        }
        match c {
            '\n' if self.after_cr => {}
            '\n' | '\r' | '\u{B}' | '\u{C}' | '\u{85}' | '\u{2028}' | '\u{2029}' => {
                self.lines += 1;
                if self.lines > MAX_TEXT_LINES {
                    return Err(TextProblem::TooManyLines);
                }
            }
            _ => {}
        }
        self.after_cr = c == '\r';
        Ok(())
    }

    /// Counts `n` characters none of which breaks a line, or fails, counting nothing, if the
    /// text with them passes [`MAX_TEXT_CHARS`].
    pub(crate) fn push_unbroken(&mut self, n: usize) -> Result<(), TextProblem> {
        if n > MAX_TEXT_CHARS - self.chars.min(MAX_TEXT_CHARS) {
            return Err(TextProblem::TooLong);
        }
        self.chars += n;
        if n > 0 {
            self.after_cr = false;
        }
        Ok(())
    }

    /// Characters counted so far.
    pub(crate) fn chars(&self) -> usize {
        self.chars
    }
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
    /// Plain text. `at` is the top-left corner of the text box. `size` multiplies the text's
    /// height (see [`DEFAULT_TEXT_SIZE`]), from [`MIN_TEXT_SIZE`] to [`MAX_TEXT_SIZE`].
    Text { at: Point, text: String, size: f64 },
    /// Typst math. `at` is the top-left corner of the formula box, `src` the Typst math source.
    /// `size` is as for [`Shape::Text`].
    Math { at: Point, src: String, size: f64 },
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
    /// True when every coordinate and size is a finite number, the size of text and math
    /// included.
    pub fn is_finite(&self) -> bool {
        match self {
            Self::Text { at, size, .. } | Self::Math { at, size, .. } => {
                at.is_finite() && size.is_finite()
            }
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

    /// The size of a text or math object: a multiplier on the text height (see
    /// [`DEFAULT_TEXT_SIZE`]). `None` for the other kinds, which have no size in this sense
    /// (their radii, widths and heights are [`MAX_SIZE`]'s sizes).
    pub fn size(&self) -> Option<f64> {
        match self {
            Self::Text { size, .. } | Self::Math { size, .. } => Some(*size),
            Self::Ellipse { .. } | Self::Rect { .. } | Self::Line { .. } | Self::Arrow { .. } => {
                None
            }
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

    /// The size of a text or math object is from [`MIN_TEXT_SIZE`] to [`MAX_TEXT_SIZE`]. True
    /// for kinds without one.
    pub fn text_size_in_range(&self) -> bool {
        self.size().is_none_or(is_valid_text_size)
    }

    /// The text or math content has at most [`MAX_TEXT_CHARS`] characters and at most
    /// [`MAX_TEXT_LINES`] lines. True for kinds without content.
    pub fn content_in_range(&self) -> bool {
        self.content().is_none_or(|s| check_text(s).is_ok())
    }

    /// Characters (Unicode scalar values) of text or math source: what the shape counts
    /// towards [`MAX_BOARD_TEXT_CHARS`]. 0 for kinds without content.
    pub fn content_chars(&self) -> usize {
        self.content().map_or(0, |s| s.chars().count())
    }
}

/// Why the board refused a change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BoardError {
    #[error("invalid object id {0:?}: use only ASCII letters, digits, `_` and `-`")]
    InvalidId(String),
    /// The id, which is not shown: it may be very long.
    #[error("object id is longer than {MAX_ID_CHARS} characters")]
    IdTooLong(String),
    #[error("an object with id `{0}` is already on the board")]
    DuplicateId(ObjectId),
    #[error("object `{0}` has a coordinate or size that is not a finite number")]
    NonFinite(ObjectId),
    #[error("object `{0}` has a coordinate outside -{max}..={max}", max = MAX_COORDINATE)]
    CoordinateOutOfRange(ObjectId),
    #[error("object `{0}` has a size that is not above 0 and at most {max}", max = MAX_SIZE)]
    InvalidSize(ObjectId),
    /// A text or math object's size is outside [`MIN_TEXT_SIZE`] to [`MAX_TEXT_SIZE`].
    #[error(
        "object `{0}` has a text size outside {min}..={max}",
        min = MIN_TEXT_SIZE,
        max = MAX_TEXT_SIZE
    )]
    TextSizeOutOfRange(ObjectId),
    #[error("object `{0}` has more than {max} characters of text", max = MAX_TEXT_CHARS)]
    TextTooLong(ObjectId),
    #[error("object `{0}` has more than {max} lines of text", max = MAX_TEXT_LINES)]
    TooManyLines(ObjectId),
    #[error(
        "object `{0}` would be too much text: a board holds at most {max} characters of text \
         and math in all",
        max = MAX_BOARD_TEXT_CHARS
    )]
    TooMuchText(ObjectId),
    #[error("object `{0}` would be one too many: a board holds at most {max} objects", max = MAX_OBJECTS)]
    TooManyObjects(ObjectId),
    #[error("there is no object `{0}` on the board")]
    NoSuchObject(ObjectId),
    /// [`Board::insert_at`] was given a position past the end of the board.
    #[error("object `{id}` cannot go at position {index}: the board holds {len} objects")]
    BadPosition {
        id: ObjectId,
        index: usize,
        len: usize,
    },
}

/// One change in a batch for [`Board::apply`].
#[derive(Debug, Clone, Copy)]
pub(crate) enum Change<'a> {
    /// Put a copy of `object` at position `index`, as [`Board::insert_at`] does.
    Insert { index: usize, object: &'a Object },
    /// Take object `id` off the board, as [`Board::remove`] does. `at` is the position it should
    /// be at: the object there is looked at first, so finding it costs nothing when the caller
    /// knows the board.
    Remove { id: &'a ObjectId, at: usize },
    /// Give object `id` a copy of `shape`, as [`Board::replace`] does.
    Replace { id: &'a ObjectId, shape: &'a Shape },
}

/// A batch of changes worked out against the board without changing it (see [`Board::apply`]).
struct Plan<'a> {
    /// The z-order after the changes. An entry below the board's length is the board object at
    /// that position; one past it is the added object at `entry - len`.
    order: Vec<usize>,
    added: Vec<&'a Object>,
    /// Board objects the changes take off, by position.
    removed: Vec<bool>,
    /// The last shape a change gave each entry.
    shapes: HashMap<usize, &'a Shape>,
    /// The board's characters of text after the changes.
    text_chars: usize,
    /// The first position a change put an object at or took one from: the objects before it
    /// stay where they are.
    first_moved: usize,
}

/// The whiteboard: objects in z-order, back to front.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Board {
    objects: Vec<Object>,
    /// Position of each id in `objects`.
    index: HashMap<ObjectId, usize>,
    /// Characters of text and math source in `objects`, for [`MAX_BOARD_TEXT_CHARS`].
    text_chars: usize,
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

    /// The position of object `id` in z-order (0 is the back).
    pub fn position(&self, id: &ObjectId) -> Option<usize> {
        self.index.get(id).copied()
    }

    /// Adds `object` on top of the others.
    ///
    /// Fails if its id is already used, any of its numbers is not finite or outside the limits
    /// ([`MAX_COORDINATE`], [`MAX_SIZE`], [`MIN_TEXT_SIZE`] to [`MAX_TEXT_SIZE`] for the size
    /// of text and math), its text is longer than [`MAX_TEXT_CHARS`] or
    /// [`MAX_TEXT_LINES`], it would take the board's text past [`MAX_BOARD_TEXT_CHARS`], or the
    /// board already holds [`MAX_OBJECTS`] objects.
    pub fn insert(&mut self, object: Object) -> Result<(), BoardError> {
        if let Err(error) = self.check_new(&object) {
            return Err(error(object.id));
        }
        self.push_checked(object);
        Ok(())
    }

    /// Puts `object` at position `index` in z-order (0 is the back), moving the objects from
    /// `index` on one step to the front. `index` may be [`Board::len`], the top.
    ///
    /// Checks `object` as [`Board::insert`] does. An `index` past the end fails with
    /// [`BoardError::BadPosition`] rather than being clamped: the caller's idea of the board is
    /// wrong (an undo history out of step with it, say), and clamping would quietly put the
    /// object at another depth. The position [`Board::remove`] returns is always good.
    pub fn insert_at(&mut self, index: usize, object: Object) -> Result<(), BoardError> {
        if let Err(error) = self.check_new(&object) {
            return Err(error(object.id));
        }
        let len = self.objects.len();
        if index > len {
            return Err(BoardError::BadPosition {
                id: object.id,
                index,
                len,
            });
        }
        self.text_chars = self.text_chars.saturating_add(object.shape.content_chars());
        self.index.insert(object.id.clone(), index);
        self.objects.insert(index, object);
        self.reindex_from(index + 1);
        Ok(())
    }

    /// Takes the object `id` off the board: its position in z-order and the object, or `None`
    /// when there is no such object.
    pub fn remove(&mut self, id: &ObjectId) -> Option<(usize, Object)> {
        let index = self.index.remove(id)?;
        let object = self.objects.remove(index);
        self.text_chars = self.text_chars.saturating_sub(object.shape.content_chars());
        self.reindex_from(index);
        Some((index, object))
    }

    /// Gives the object `id` a new shape, of any kind, keeping its id and position. Returns the
    /// old shape.
    ///
    /// Checks `shape` as [`Board::insert`] does, counting the board's text without the old
    /// shape's. Fails with [`BoardError::NoSuchObject`] when there is no object `id`.
    pub fn replace(&mut self, id: &ObjectId, shape: Shape) -> Result<Shape, BoardError> {
        let missing = || BoardError::NoSuchObject(id.clone());
        let &i = self.index.get(id).ok_or_else(missing)?;
        let freed = self
            .objects
            .get(i)
            .ok_or_else(missing)?
            .shape
            .content_chars();
        let chars = self
            .check_shape(&shape, freed)
            .map_err(|error| error(id.clone()))?;
        let object = self.objects.get_mut(i).ok_or_else(missing)?;
        let old = std::mem::replace(&mut object.shape, shape);
        self.text_chars = self.text_chars.saturating_sub(freed).saturating_add(chars);
        Ok(old)
    }

    /// Makes `changes` in order, each with the checks and errors of the method it is named after
    /// ([`Board::insert_at`], [`Board::remove`] as [`BoardError::NoSuchObject`] when missing,
    /// [`Board::replace`]), as one batch: all or nothing. When a change fails, the board stays as
    /// it was and its error is returned.
    ///
    /// One change at a time, each insert or remove shifts the objects after it and points the
    /// index at their new positions, so a batch of `k` changes on a board of `n` objects cost
    /// `k x n` index updates: undoing a delete of 500 objects took 1.7 ms. Here the changes are
    /// worked out on a list of positions first, and the objects and the index are built once:
    /// about `n + k` work, plus moving `k` lists of `n` small numbers.
    pub(crate) fn apply(&mut self, changes: &[Change<'_>]) -> Result<(), BoardError> {
        let plan = self.plan(changes)?;
        self.commit(&plan);
        Ok(())
    }

    /// Works `changes` out against the board, without changing it: what the board becomes, or
    /// the first change's error.
    fn plan<'a>(&self, changes: &[Change<'a>]) -> Result<Plan<'a>, BoardError> {
        let len = self.objects.len();
        let mut plan = Plan {
            order: (0..len).collect(),
            added: Vec::new(),
            removed: vec![false; len],
            shapes: HashMap::new(),
            text_chars: self.text_chars,
            first_moved: len,
        };
        // The entries of the added objects still on the board, by id.
        let mut added_ids: HashMap<&ObjectId, usize> = HashMap::new();
        for change in changes {
            let find = |id: &ObjectId, plan: &Plan<'_>| {
                added_ids.get(id).copied().or_else(|| {
                    self.index
                        .get(id)
                        .copied()
                        .filter(|&i| !plan.removed.get(i).copied().unwrap_or(true))
                })
            };
            match *change {
                Change::Insert { index, object } => {
                    let id = &object.id;
                    let check: Result<usize, fn(ObjectId) -> BoardError> =
                        if plan.order.len() >= MAX_OBJECTS {
                            Err(BoardError::TooManyObjects)
                        } else if find(id, &plan).is_some() {
                            Err(BoardError::DuplicateId)
                        } else {
                            check_shape(&object.shape, 0, plan.text_chars)
                        };
                    let chars = check.map_err(|error| error(id.clone()))?;
                    let count = plan.order.len();
                    if index > count {
                        return Err(BoardError::BadPosition {
                            id: id.clone(),
                            index,
                            len: count,
                        });
                    }
                    let entry = len + plan.added.len();
                    plan.order.insert(index, entry);
                    plan.first_moved = plan.first_moved.min(index);
                    plan.added.push(object);
                    added_ids.insert(id, entry);
                    plan.text_chars = plan.text_chars.saturating_add(chars);
                }
                Change::Remove { id, at } => {
                    let missing = || BoardError::NoSuchObject(id.clone());
                    let entry = find(id, &plan).ok_or_else(missing)?;
                    let position = if plan.order.get(at) == Some(&entry) {
                        at
                    } else {
                        plan.order
                            .iter()
                            .position(|&e| e == entry)
                            .ok_or_else(missing)?
                    };
                    plan.order.remove(position);
                    plan.first_moved = plan.first_moved.min(position);
                    let freed = self
                        .planned_shape(&plan, entry)
                        .map_or(0, Shape::content_chars);
                    plan.text_chars = plan.text_chars.saturating_sub(freed);
                    match plan.removed.get_mut(entry) {
                        Some(gone) => *gone = true,
                        None => {
                            added_ids.remove(id);
                        }
                    }
                }
                Change::Replace { id, shape } => {
                    let entry =
                        find(id, &plan).ok_or_else(|| BoardError::NoSuchObject(id.clone()))?;
                    let freed = self
                        .planned_shape(&plan, entry)
                        .map_or(0, Shape::content_chars);
                    let chars = check_shape(shape, freed, plan.text_chars)
                        .map_err(|error| error(id.clone()))?;
                    plan.shapes.insert(entry, shape);
                    plan.text_chars = plan.text_chars.saturating_sub(freed).saturating_add(chars);
                }
            }
        }
        Ok(plan)
    }

    /// The shape of `entry` (see [`Plan::order`]) as `plan` leaves it so far.
    fn planned_shape<'a>(&'a self, plan: &Plan<'a>, entry: usize) -> Option<&'a Shape> {
        if let Some(shape) = plan.shapes.get(&entry) {
            return Some(shape);
        }
        match entry.checked_sub(self.objects.len()) {
            None => self.objects.get(entry).map(|o| &o.shape),
            Some(added) => plan.added.get(added).map(|o| &o.shape),
        }
    }

    /// Makes the board what `plan` worked out: the objects in their new order, then the index.
    /// The objects before the first one moved stay where they are, so a batch at the top of
    /// the board (a put, a new object) costs only what it adds.
    fn commit(&mut self, plan: &Plan<'_>) {
        let len = self.objects.len();
        let first = plan.first_moved.min(len);
        // No change moved the objects before `first`: their entries are their positions.
        for (&entry, shape) in &plan.shapes {
            if entry < first
                && let Some(object) = self.objects.get_mut(entry)
            {
                object.shape = (*shape).clone();
            }
        }
        let mut moved: Vec<Option<Object>> = self
            .objects
            .split_off(first)
            .into_iter()
            .map(Some)
            .collect();
        for (slot, gone) in moved.iter().zip(plan.removed.iter().skip(first)) {
            if let (Some(object), true) = (slot, gone) {
                self.index.remove(&object.id);
            }
        }
        for &entry in plan.order.iter().skip(first) {
            let object = match entry.checked_sub(len) {
                None => entry
                    .checked_sub(first)
                    .and_then(|i| moved.get_mut(i))
                    .and_then(Option::take),
                Some(added) => plan.added.get(added).map(|o| (*o).clone()),
            };
            // Every entry names one object, once.
            let Some(mut object) = object else {
                continue;
            };
            if let Some(shape) = plan.shapes.get(&entry) {
                object.shape = (*shape).clone();
            }
            let at = self.objects.len();
            if let Some(slot) = self.index.get_mut(&object.id) {
                *slot = at;
            } else {
                self.index.insert(object.id.clone(), at);
            }
            self.objects.push(object);
        }
        self.text_chars = plan.text_chars;
    }

    /// What [`Board::insert`] would refuse about `object`, if anything.
    fn check_new(&self, object: &Object) -> Result<(), fn(ObjectId) -> BoardError> {
        if self.is_full() {
            Err(BoardError::TooManyObjects)
        } else if self.contains(&object.id) {
            Err(BoardError::DuplicateId)
        } else {
            self.check_shape(&object.shape, 0).map(|_| ())
        }
    }

    /// The characters of text in `shape` when the board can hold it once `freed` characters of
    /// text have left it (a replaced shape's), or what is wrong with it.
    fn check_shape(
        &self,
        shape: &Shape,
        freed: usize,
    ) -> Result<usize, fn(ObjectId) -> BoardError> {
        check_shape(shape, freed, self.text_chars)
    }

    /// Points the index at the positions of the objects from `start` on, after they moved.
    fn reindex_from(&mut self, start: usize) {
        for (i, object) in self.objects.iter().enumerate().skip(start) {
            if let Some(slot) = self.index.get_mut(&object.id) {
                *slot = i;
            }
        }
    }

    /// True when the board holds [`MAX_OBJECTS`] objects, so [`Board::insert`] refuses another.
    pub fn is_full(&self) -> bool {
        self.objects.len() >= MAX_OBJECTS
    }

    /// Characters of text and math source on the board, all objects together; at most
    /// [`MAX_BOARD_TEXT_CHARS`].
    pub fn text_chars(&self) -> usize {
        self.text_chars
    }

    /// True when `chars` more characters of text fit within [`MAX_BOARD_TEXT_CHARS`].
    pub fn has_room_for_text(&self, chars: usize) -> bool {
        chars <= MAX_BOARD_TEXT_CHARS.saturating_sub(self.text_chars)
    }

    /// Adds an object the caller has already checked as [`Board::insert`] does, the object
    /// count and the board's text included.
    pub(crate) fn push_checked(&mut self, object: Object) {
        self.text_chars = self.text_chars.saturating_add(object.shape.content_chars());
        self.index.insert(object.id.clone(), self.objects.len());
        self.objects.push(object);
    }

    /// The first unused id of the form `{prefix}{n}`, counting from 1.
    ///
    /// Characters that are not allowed in ids are dropped from `prefix`, and its end is cut so
    /// the id fits in [`MAX_ID_CHARS`].
    pub fn fresh_id(&self, prefix: &str) -> ObjectId {
        self.fresh_id_from(prefix, &mut 1, |_| false)
    }

    /// [`Board::fresh_id`] counting from `*from`, and skipping the ids `taken` says are in use
    /// too; `*from` moves past the id returned. Ids for objects not yet on the board (a put
    /// adds them all at once) come from one pass this way, not one pass each.
    pub(crate) fn fresh_id_from(
        &self,
        prefix: &str,
        from: &mut usize,
        taken: impl Fn(&ObjectId) -> bool,
    ) -> ObjectId {
        let prefix: String = prefix
            .chars()
            .filter(|c| u8::try_from(*c).is_ok_and(is_id_byte))
            .collect();
        loop {
            let n = *from;
            *from = n.saturating_add(1);
            let n_text = n.to_string();
            // ASCII only, so any byte offset is a character boundary.
            let keep = prefix.len().min(MAX_ID_CHARS.saturating_sub(n_text.len()));
            let id = ObjectId(format!(
                "{}{n_text}",
                prefix.get(..keep).unwrap_or_default()
            ));
            if !self.contains(&id) && !taken(&id) {
                return id;
            }
        }
    }
}

/// The characters of text in `shape` when a board holding `text_chars` characters of text can
/// hold it once `freed` of them have left (a replaced shape's), or what is wrong with it.
fn check_shape(
    shape: &Shape,
    freed: usize,
    text_chars: usize,
) -> Result<usize, fn(ObjectId) -> BoardError> {
    if !shape.is_finite() {
        Err(BoardError::NonFinite)
    } else if !shape.coordinates_in_range() {
        Err(BoardError::CoordinateOutOfRange)
    } else if !shape.sizes_in_range() {
        Err(BoardError::InvalidSize)
    } else if !shape.text_size_in_range() {
        Err(BoardError::TextSizeOutOfRange)
    } else {
        match shape.content().map(check_text) {
            Some(Err(TextProblem::TooLong)) => Err(BoardError::TextTooLong),
            Some(Err(TextProblem::TooManyLines)) => Err(BoardError::TooManyLines),
            Some(Ok(chars))
                if chars
                    > MAX_BOARD_TEXT_CHARS.saturating_sub(text_chars.saturating_sub(freed)) =>
            {
                Err(BoardError::TooMuchText)
            }
            Some(Ok(chars)) => Ok(chars),
            None => Ok(0),
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
    fn ids_are_at_most_max_id_chars_long() {
        let longest = "a".repeat(MAX_ID_CHARS);
        assert_eq!(
            ObjectId::new(longest.as_str()).map(|id| id.0),
            Ok(longest.clone())
        );
        for over in [format!("{longest}b"), format!("{longest} "), "é".repeat(65)] {
            assert_eq!(
                ObjectId::new(over.as_str()),
                Err(BoardError::IdTooLong(over.clone())),
                "{over:?}"
            );
        }
        // Too long is counted in characters: 64 of them in 128 bytes is only invalid.
        let wide = "é".repeat(MAX_ID_CHARS);
        assert_eq!(
            ObjectId::new(wide.as_str()),
            Err(BoardError::InvalidId(wide.clone()))
        );
        assert_eq!(
            BoardError::IdTooLong("x".repeat(1000)).to_string(),
            "object id is longer than 64 characters"
        );
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
                    size: DEFAULT_TEXT_SIZE,
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
                    size: DEFAULT_TEXT_SIZE,
                },
                Shape::Math {
                    at: Point::new(0.0, bad),
                    src: String::new(),
                    size: DEFAULT_TEXT_SIZE,
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

    fn sized(name: &str, math: bool, size: f64) -> Object {
        let (at, content) = (Point::ZERO, "x".to_owned());
        let shape = if math {
            Shape::Math {
                at,
                src: content,
                size,
            }
        } else {
            Shape::Text {
                at,
                text: content,
                size,
            }
        };
        Object::new(id(name), shape)
    }

    #[test]
    fn text_and_math_sizes_are_kept_from_min_to_max() {
        let mut board = Board::new();
        let sizes = [
            MIN_TEXT_SIZE,
            DEFAULT_TEXT_SIZE,
            1.0 + f64::EPSILON,
            2.5,
            MAX_TEXT_SIZE,
        ];
        for (i, size) in sizes.into_iter().enumerate() {
            for math in [false, true] {
                let object = sized(&format!("o{i}{math}"), math, size);
                board.insert(object.clone()).unwrap();
                assert_eq!(
                    board.get(&object.id).and_then(|o| o.shape.size()),
                    Some(size)
                );
            }
        }
        assert_eq!(board.len(), 2 * sizes.len());
        assert_eq!(circle("c1", 1.0).shape.size(), None);
        assert_eq!(rect("r1", Point::ZERO, 1.0, 1.0).shape.size(), None);
    }

    #[test]
    fn text_and_math_sizes_out_of_range_are_refused() {
        let mut board = Board::new();
        let below = f64::from_bits(MIN_TEXT_SIZE.to_bits() - 1);
        let above = f64::from_bits(MAX_TEXT_SIZE.to_bits() + 1);
        for bad in [below, above, 0.0, -0.0, -1.0, 5e-324, 17.0, 1e300] {
            for math in [false, true] {
                assert_eq!(
                    board.insert(sized("t1", math, bad)),
                    Err(BoardError::TextSizeOutOfRange(id("t1"))),
                    "{bad:?}"
                );
            }
        }
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for math in [false, true] {
                assert_eq!(
                    board.insert(sized("t1", math, bad)),
                    Err(BoardError::NonFinite(id("t1"))),
                    "{bad:?}"
                );
            }
        }
        assert!(board.is_empty());
        // `replace` checks the size too, and leaves the old shape.
        board.insert(sized("t1", false, 2.0)).unwrap();
        let big = sized("t1", true, above).shape;
        assert_eq!(
            board.replace(&id("t1"), big.clone()),
            Err(BoardError::TextSizeOutOfRange(id("t1")))
        );
        let change = Change::Replace {
            id: &id("t1"),
            shape: &big,
        };
        assert_eq!(
            board.apply(&[change]),
            Err(BoardError::TextSizeOutOfRange(id("t1")))
        );
        assert_eq!(board.get(&id("t1")), Some(&sized("t1", false, 2.0)));
        assert_eq!(
            BoardError::TextSizeOutOfRange(id("t1")).to_string(),
            "object `t1` has a text size outside 0.25..=16"
        );
    }

    #[test]
    fn insert_rejects_text_over_the_limit() {
        let mut board = Board::new();
        // Multi-byte characters: the limit counts characters, not bytes.
        let longest = "é".repeat(MAX_TEXT_CHARS);
        board.insert(text("t1", longest.clone())).unwrap();
        assert_eq!(
            board.insert(text("t2", format!("{longest}x"))),
            Err(BoardError::TextTooLong(id("t2")))
        );
        let math = Shape::Math {
            at: Point::ZERO,
            src: "x".repeat(MAX_TEXT_CHARS + 1),
            size: DEFAULT_TEXT_SIZE,
        };
        assert_eq!(
            board.insert(Object::new(id("m1"), math)),
            Err(BoardError::TextTooLong(id("m1")))
        );
        assert_eq!(board.len(), 1);
    }

    fn text(name: &str, text: String) -> Object {
        Object::new(
            id(name),
            Shape::Text {
                at: Point::ZERO,
                text,
                size: DEFAULT_TEXT_SIZE,
            },
        )
    }

    #[test]
    fn insert_rejects_text_over_the_line_limit() {
        let mut board = Board::new();
        // Every kind of line break counts, CRLF once.
        let breaks = [
            "\n", "\r", "\r\n", "\u{B}", "\u{C}", "\u{85}", "\u{2028}", "\u{2029}",
        ];
        let most: String = breaks
            .iter()
            .cycle()
            .take(MAX_TEXT_LINES - 1)
            .copied()
            .collect();
        board.insert(text("t1", most.clone())).unwrap();
        assert_eq!(
            board.insert(text("t2", format!("{most}\n"))),
            Err(BoardError::TooManyLines(id("t2")))
        );
        // A CR then an LF far apart are two breaks.
        assert_eq!(
            board.insert(text(
                "t3",
                format!("{}\rx\n", "\n".repeat(MAX_TEXT_LINES - 2))
            )),
            Err(BoardError::TooManyLines(id("t3")))
        );
        board
            .insert(text("t4", "\r\n".repeat(MAX_TEXT_LINES - 1)))
            .unwrap();
        let math = Shape::Math {
            at: Point::ZERO,
            src: "x\n".repeat(MAX_TEXT_LINES),
            size: DEFAULT_TEXT_SIZE,
        };
        assert_eq!(
            board.insert(Object::new(id("m1"), math)),
            Err(BoardError::TooManyLines(id("m1")))
        );
        assert_eq!(board.len(), 2);
    }

    #[test]
    fn insert_stops_at_the_board_text_limit() {
        let mut board = Board::new();
        let full = MAX_BOARD_TEXT_CHARS / MAX_TEXT_CHARS;
        for i in 0..full {
            board
                .insert(text(&format!("t{i}"), "é".repeat(MAX_TEXT_CHARS)))
                .unwrap();
        }
        let left = MAX_BOARD_TEXT_CHARS - full * MAX_TEXT_CHARS;
        let math = |name: &str, n: usize| {
            Object::new(
                id(name),
                Shape::Math {
                    at: Point::ZERO,
                    src: "x".repeat(n),
                    size: DEFAULT_TEXT_SIZE,
                },
            )
        };
        assert_eq!(
            board.insert(math("m1", left + 1)),
            Err(BoardError::TooMuchText(id("m1")))
        );
        board.insert(math("m2", left)).unwrap();
        assert_eq!(board.text_chars(), MAX_BOARD_TEXT_CHARS);
        assert!(!board.has_room_for_text(1));
        // Objects without text still fit, and so does empty text.
        board.insert(circle("c1", 1.0)).unwrap();
        board.insert(text("empty", String::new())).unwrap();
        assert_eq!(
            board.insert(text("t-last", "x".to_owned())),
            Err(BoardError::TooMuchText(id("t-last")))
        );
        assert_eq!(board.len(), full + 3);
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
            "object `t1` has more than 5000 characters of text"
        );
        assert_eq!(
            BoardError::TooManyLines(id("t1")).to_string(),
            "object `t1` has more than 500 lines of text"
        );
        assert_eq!(
            BoardError::TooMuchText(id("t1")).to_string(),
            "object `t1` would be too much text: a board holds at most 40000 characters of text \
             and math in all"
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

    /// The index points at every object's position and nothing else, and `text_chars` is the
    /// sum of the objects' text.
    fn assert_consistent(board: &Board) {
        assert_eq!(board.index.len(), board.objects.len());
        for (i, object) in board.objects.iter().enumerate() {
            assert_eq!(board.index.get(&object.id), Some(&i), "{}", object.id);
        }
        let sum: usize = board.objects.iter().map(|o| o.shape.content_chars()).sum();
        assert_eq!(board.text_chars, sum);
    }

    fn ids(board: &Board) -> Vec<&str> {
        board.objects().iter().map(|o| o.id.as_str()).collect()
    }

    #[test]
    fn insert_at_puts_objects_at_any_depth() {
        let mut board = Board::new();
        board.insert_at(0, circle("c1", 1.0)).unwrap();
        board.insert_at(0, circle("c0", 1.0)).unwrap();
        board.insert_at(2, circle("c3", 1.0)).unwrap();
        board.insert_at(2, text("t2", "ab".to_owned())).unwrap();
        assert_eq!(ids(&board), ["c0", "c1", "t2", "c3"]);
        assert_eq!(board.text_chars(), 2);
        assert_consistent(&board);
    }

    #[test]
    fn insert_at_refuses_a_position_past_the_end() {
        let mut board = Board::new();
        board.insert(circle("c1", 1.0)).unwrap();
        assert_eq!(
            board.insert_at(2, circle("c2", 1.0)),
            Err(BoardError::BadPosition {
                id: id("c2"),
                index: 2,
                len: 1
            })
        );
        assert_eq!(
            BoardError::BadPosition {
                id: id("c2"),
                index: 2,
                len: 1
            }
            .to_string(),
            "object `c2` cannot go at position 2: the board holds 1 objects"
        );
        assert_eq!(ids(&board), ["c1"]);
        assert_consistent(&board);
    }

    #[test]
    fn insert_at_checks_the_object_as_insert_does() {
        let mut board = Board::new();
        board.insert(circle("c1", 1.0)).unwrap();
        assert_eq!(
            board.insert_at(0, circle("c1", 2.0)),
            Err(BoardError::DuplicateId(id("c1")))
        );
        assert_eq!(
            board.insert_at(0, circle("c2", -1.0)),
            Err(BoardError::InvalidSize(id("c2")))
        );
        assert_eq!(
            board.insert_at(0, text("t1", "x".repeat(MAX_TEXT_CHARS + 1))),
            Err(BoardError::TextTooLong(id("t1")))
        );
        for i in 1..MAX_OBJECTS {
            board.insert(circle(&format!("f{i}"), 1.0)).unwrap();
        }
        // The object checks come before the position: a full board refuses even a bad one.
        assert_eq!(
            board.insert_at(MAX_OBJECTS + 7, circle("c2", 1.0)),
            Err(BoardError::TooManyObjects(id("c2")))
        );
        assert_eq!(board.len(), MAX_OBJECTS);
        assert_consistent(&board);
    }

    #[test]
    fn remove_returns_the_position_and_frees_the_text() {
        let mut board = Board::new();
        board.insert(circle("c1", 1.0)).unwrap();
        board.insert(text("t1", "héllo".to_owned())).unwrap();
        board.insert(circle("c2", 2.0)).unwrap();
        assert_eq!(board.text_chars(), 5);
        assert_eq!(
            board.remove(&id("t1")),
            Some((1, text("t1", "héllo".to_owned())))
        );
        assert_eq!(board.text_chars(), 0);
        assert_eq!(ids(&board), ["c1", "c2"]);
        assert_eq!(board.get(&id("c2")), Some(&circle("c2", 2.0)));
        assert_eq!(board.remove(&id("t1")), None);
        assert_consistent(&board);
        // The id is free again, and the removed object goes back where it was.
        board.insert_at(1, text("t1", "héllo".to_owned())).unwrap();
        assert_eq!(ids(&board), ["c1", "t1", "c2"]);
        assert_consistent(&board);
    }

    #[test]
    fn replace_keeps_the_id_and_depth_and_returns_the_old_shape() {
        let mut board = Board::new();
        board.insert(circle("c1", 1.0)).unwrap();
        board.insert(circle("o1", 2.0)).unwrap();
        board.insert(circle("c2", 3.0)).unwrap();
        let rect = Shape::Rect {
            origin: Point::new(1.0, 2.0),
            w: 3.0,
            h: 4.0,
        };
        assert_eq!(
            board.replace(&id("o1"), rect.clone()),
            Ok(circle("o1", 2.0).shape)
        );
        assert_eq!(ids(&board), ["c1", "o1", "c2"]);
        assert_eq!(board.get(&id("o1")).map(|o| &o.shape), Some(&rect));
        assert_eq!(
            board.replace(&id("nope"), rect),
            Err(BoardError::NoSuchObject(id("nope")))
        );
        assert_eq!(
            BoardError::NoSuchObject(id("nope")).to_string(),
            "there is no object `nope` on the board"
        );
        assert_consistent(&board);
    }

    #[test]
    fn replace_checks_the_new_shape_and_leaves_a_refused_one_out() {
        let mut board = Board::new();
        board.insert(circle("c1", 1.0)).unwrap();
        let bad = [
            (
                circle("c1", f64::NAN).shape,
                BoardError::NonFinite(id("c1")),
            ),
            (circle("c1", 0.0).shape, BoardError::InvalidSize(id("c1"))),
            (
                rect("c1", Point::new(0.0, -2e6), 1.0, 1.0).shape,
                BoardError::CoordinateOutOfRange(id("c1")),
            ),
            (
                text("c1", "x".repeat(MAX_TEXT_CHARS + 1)).shape,
                BoardError::TextTooLong(id("c1")),
            ),
            (
                text("c1", "\n".repeat(MAX_TEXT_LINES)).shape,
                BoardError::TooManyLines(id("c1")),
            ),
        ];
        for (shape, error) in bad {
            assert_eq!(board.replace(&id("c1"), shape), Err(error));
        }
        assert_eq!(board.get(&id("c1")), Some(&circle("c1", 1.0)));
        assert_consistent(&board);
    }

    #[test]
    fn replace_counts_the_board_text_without_the_old_shape() {
        let mut board = Board::new();
        let full = MAX_BOARD_TEXT_CHARS / MAX_TEXT_CHARS;
        for i in 0..full {
            board
                .insert(text(&format!("t{i}"), "x".repeat(MAX_TEXT_CHARS)))
                .unwrap();
        }
        assert_eq!(board.text_chars(), MAX_BOARD_TEXT_CHARS);
        // A full text can change to another full text, or shrink.
        let other = text("t0", "y".repeat(MAX_TEXT_CHARS)).shape;
        board.replace(&id("t0"), other).unwrap();
        board
            .replace(&id("t1"), text("t1", "z".to_owned()).shape)
            .unwrap();
        assert_eq!(
            board.text_chars(),
            MAX_BOARD_TEXT_CHARS - MAX_TEXT_CHARS + 1
        );
        // A circle cannot become text past the board's limit.
        board.insert(circle("c1", 1.0)).unwrap();
        assert_eq!(
            board.replace(&id("c1"), text("c1", "x".repeat(MAX_TEXT_CHARS)).shape),
            Err(BoardError::TooMuchText(id("c1")))
        );
        board
            .replace(&id("c1"), text("c1", "x".repeat(MAX_TEXT_CHARS - 1)).shape)
            .unwrap();
        assert_eq!(board.text_chars(), MAX_BOARD_TEXT_CHARS);
        assert_consistent(&board);
    }

    /// Cases for the property tests: `LAWH_PROPTEST_CASES` if set, otherwise `default`.
    fn cases(default: u32) -> u32 {
        std::env::var("LAWH_PROPTEST_CASES")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(default)
    }

    /// One edit, on a small pool of ids so edits meet the same objects. A text length of 0
    /// means a circle.
    #[derive(Debug, Clone)]
    enum Edit {
        Insert(u8, usize),
        InsertAt(usize, u8, usize),
        Remove(u8),
        Replace(u8, usize),
    }

    fn pool_object(name: u8, chars: usize) -> Object {
        let name = format!("o{name}");
        if chars == 0 {
            circle(&name, 1.0)
        } else {
            text(&name, "é".repeat(chars))
        }
    }

    fn edit() -> impl proptest::strategy::Strategy<Value = Edit> {
        use proptest::prelude::*;
        // Up to 12 texts of up to MAX_TEXT_CHARS pass MAX_BOARD_TEXT_CHARS now and then.
        let chars = prop_oneof![Just(0), 1..=MAX_TEXT_CHARS, Just(MAX_TEXT_CHARS)];
        prop_oneof![
            (0..12_u8, chars.clone()).prop_map(|(n, c)| Edit::Insert(n, c)),
            (0..16_usize, 0..12_u8, chars.clone()).prop_map(|(i, n, c)| Edit::InsertAt(i, n, c)),
            (0..12_u8).prop_map(Edit::Remove),
            (0..12_u8, chars).prop_map(|(n, c)| Edit::Replace(n, c)),
        ]
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(cases(256)))]

        /// After any edits the index and the text count stay right, an edit that succeeds does
        /// what a plain list of objects would do, and one that fails changes nothing.
        #[test]
        fn edits_keep_the_index_and_text_count(edits in proptest::collection::vec(edit(), 0..40)) {
            let mut board = Board::new();
            let mut model: Vec<Object> = Vec::new();
            for edit in edits {
                let before = board.clone();
                match edit {
                    Edit::Insert(n, chars) => {
                        let object = pool_object(n, chars);
                        if board.insert(object.clone()).is_ok() {
                            model.push(object);
                        }
                    }
                    Edit::InsertAt(i, n, chars) => {
                        let object = pool_object(n, chars);
                        if board.insert_at(i, object.clone()).is_ok() {
                            model.insert(i, object);
                        }
                    }
                    Edit::Remove(n) => {
                        let target = id(&format!("o{n}"));
                        let at = model.iter().position(|o| o.id == target);
                        let removed = board.remove(&target);
                        proptest::prop_assert_eq!(removed.as_ref().map(|r| r.0), at);
                        if let Some((i, object)) = removed {
                            proptest::prop_assert_eq!(&model.remove(i), &object);
                        }
                    }
                    Edit::Replace(n, chars) => {
                        let shape = pool_object(n, chars).shape;
                        let target = id(&format!("o{n}"));
                        if let Ok(old) = board.replace(&target, shape.clone()) {
                            let slot = model.iter_mut().find(|o| o.id == target).unwrap();
                            let model_old = std::mem::replace(&mut slot.shape, shape);
                            proptest::prop_assert_eq!(&model_old, &old);
                        } else {
                            proptest::prop_assert_eq!(&board, &before);
                        }
                    }
                }
                proptest::prop_assert_eq!(board.objects(), model.as_slice());
                assert_consistent(&board);
            }
        }
    }

    /// A batch for [`Board::apply`]: a change on the pool of ids, a position hint for removes.
    fn change() -> impl proptest::strategy::Strategy<Value = (Edit, usize)> {
        (edit(), 0..16_usize)
    }

    /// Makes `edit` with the one-object methods, as [`Board::apply`] must.
    fn one_by_one(board: &mut Board, edit: &Edit) -> Result<(), BoardError> {
        match *edit {
            Edit::Insert(n, chars) => board.insert_at(board.len(), pool_object(n, chars)),
            Edit::InsertAt(i, n, chars) => board.insert_at(i, pool_object(n, chars)),
            Edit::Remove(n) => {
                let target = id(&format!("o{n}"));
                board
                    .remove(&target)
                    .map(|_| ())
                    .ok_or(BoardError::NoSuchObject(target))
            }
            Edit::Replace(n, chars) => board
                .replace(&id(&format!("o{n}")), pool_object(n, chars).shape)
                .map(|_| ()),
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(cases(256)))]

        /// A batch does what its changes do one by one, or, when one fails, nothing but return
        /// that change's error.
        #[test]
        fn a_batch_is_its_changes_one_by_one_or_nothing(
            start in proptest::collection::vec(edit(), 0..20),
            batch in proptest::collection::vec(change(), 0..20),
        ) {
            let mut board = Board::new();
            for edit in &start {
                let _ = one_by_one(&mut board, edit);
            }
            let objects: Vec<Object> = batch
                .iter()
                .map(|(edit, _)| match *edit {
                    Edit::Insert(n, c) | Edit::InsertAt(_, n, c) | Edit::Replace(n, c) => {
                        pool_object(n, c)
                    }
                    Edit::Remove(n) => pool_object(n, 0),
                })
                .collect();
            let changes: Vec<Change<'_>> = batch
                .iter()
                .zip(&objects)
                .map(|((edit, hint), object)| match *edit {
                    Edit::Insert(..) => Change::Insert { index: *hint, object },
                    Edit::InsertAt(i, ..) => Change::Insert { index: i, object },
                    Edit::Remove(_) => Change::Remove { id: &object.id, at: *hint },
                    Edit::Replace(..) => Change::Replace { id: &object.id, shape: &object.shape },
                })
                .collect();
            let before = board.clone();
            let mut expected = board.clone();
            let outcome = changes.iter().try_for_each(|change| match *change {
                Change::Insert { index, object } => expected.insert_at(index, object.clone()),
                Change::Remove { id, .. } => expected
                    .remove(id)
                    .map(|_| ())
                    .ok_or_else(|| BoardError::NoSuchObject(id.clone())),
                Change::Replace { id, shape } => expected.replace(id, shape.clone()).map(|_| ()),
            });
            if outcome.is_err() {
                expected = before;
            }
            let result = board.apply(&changes);
            proptest::prop_assert_eq!(result, outcome);
            proptest::prop_assert_eq!(&board, &expected);
            assert_consistent(&board);
        }
    }

    /// [`check_text`] one character at a time, as it always counted.
    fn check_text_slowly(s: &str) -> Result<usize, TextProblem> {
        let mut count = TextCount::new();
        for c in s.chars() {
            count.push(c)?;
        }
        Ok(count.chars())
    }

    /// Runs of one character, with every line break and characters of every length.
    fn runs() -> impl proptest::strategy::Strategy<Value = String> {
        use proptest::prelude::*;
        let c = prop::sample::select(vec![
            'a', 'é', '€', '😀', '\n', '\r', '\u{B}', '\u{C}', '\u{85}', '\u{2028}', '\u{2029}',
            '\u{C5}', '\u{20AC}',
        ]);
        prop::collection::vec((c, 0..1200_usize), 0..12).prop_map(|runs| {
            runs.into_iter()
                .map(|(c, n)| c.to_string().repeat(n))
                .collect()
        })
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(cases(256)))]

        /// The fast count agrees with one character at a time, near both caps and past them.
        #[test]
        fn check_text_counts_as_one_character_at_a_time_does(s in runs()) {
            proptest::prop_assert_eq!(check_text(&s), check_text_slowly(&s));
        }
    }

    #[test]
    fn check_text_tells_which_cap_comes_first() {
        let breaks = "\n".repeat(MAX_TEXT_LINES);
        let chars = "a".repeat(MAX_TEXT_CHARS + 1);
        assert_eq!(
            check_text(&(breaks.clone() + &chars)),
            Err(TextProblem::TooManyLines)
        );
        assert_eq!(check_text(&(chars + &breaks)), Err(TextProblem::TooLong));
        // CRLF is one break.
        let crlf = "\r\n".repeat(MAX_TEXT_LINES - 1);
        assert_eq!(check_text(&crlf), Ok(2 * (MAX_TEXT_LINES - 1)));
        assert_eq!(check_text(&(crlf + "\r")), Err(TextProblem::TooManyLines));
        // Four-byte characters up to the cap.
        assert_eq!(check_text(&"😀".repeat(MAX_TEXT_CHARS)), Ok(MAX_TEXT_CHARS));
        assert_eq!(
            check_text(&"😀".repeat(MAX_TEXT_CHARS + 1)),
            Err(TextProblem::TooLong)
        );
    }

    #[test]
    fn a_batch_of_a_whole_board_is_linear() {
        // Taking 500 objects off and putting them back where they were, the way undo does a
        // delete: the old one-by-one way updated the index 125,000 times each way.
        let mut board = Board::new();
        for i in 0..MAX_OBJECTS {
            board.insert(circle(&format!("c{i}"), 1.0)).unwrap();
        }
        let start = board.clone();
        let objects = start.objects();
        let removes: Vec<Change<'_>> = objects
            .iter()
            .enumerate()
            .rev()
            .map(|(at, o)| Change::Remove { id: &o.id, at })
            .collect();
        let inserts: Vec<Change<'_>> = objects
            .iter()
            .enumerate()
            .map(|(index, object)| Change::Insert { index, object })
            .collect();
        let started = std::time::Instant::now();
        for _ in 0..20 {
            board.apply(&removes).unwrap();
            assert!(board.is_empty());
            board.apply(&inserts).unwrap();
            assert_eq!(board, start);
        }
        assert_consistent(&board);
        // 40 ms in a debug build (1 ms a batch of 500); the old way took 1.7 ms a batch in
        // release.
        let elapsed = started.elapsed();
        assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");
    }

    #[test]
    fn fresh_id_cuts_a_long_prefix_to_fit() {
        let mut board = Board::new();
        let prefix = "p".repeat(MAX_ID_CHARS + 10);
        for n in 1..=10 {
            let fresh = board.fresh_id(&prefix);
            let digits = n.to_string();
            let cut = "p".repeat(MAX_ID_CHARS - digits.len());
            assert_eq!(fresh.as_str(), format!("{cut}{digits}"));
            // `circle` checks the id again.
            board.insert(circle(fresh.as_str(), 1.0)).unwrap();
        }
    }
}
