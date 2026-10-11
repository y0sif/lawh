//! The `.lawh` file format: KDL v2 with a `lawh version=1` header and one object per line.
//!
//! ```text
//! lawh version=1
//! text id=t1 x=0 y=-5 "Area of a circle"
//! math id=m1 x=5 y=0 "A = pi r^2"
//! circle id=c1 x=0 y=0 r=3
//! ellipse id=e1 x=0 y=0 rx=3 ry=2
//! rect id=r1 x=0 y=0 w=4 h=2
//! line id=l1 x1=0 y1=0 x2=3 y2=0
//! arrow id=a1 x1=0 y1=0 x2=3 y2=0
//! ```
//!
//! Hand-written files may use flat KDL v2 syntax: comments, slashdashes, quoted, raw or
//! multi-line strings, properties in any order. Writing is done here too, in one fixed shape per
//! object, so saved files diff cleanly.
//!
//! The reader is lawh's own: a parser that walks the file once, without recursion (`parse.rs`),
//! hands each node to the reader (`read.rs`), which checks it and adds its object to the board.
//! Reading stops at the first problem. Every limit is checked while reading, so the time and
//! memory a file costs follow what a board holds, not the length of the file. Files are limited
//! to [`MAX_FILE_BYTES`], block comments to [`MAX_COMMENT_DEPTH`] levels, nodes to
//! [`MAX_ENTRIES`] entries, strings and numbers to [`MAX_STRING_BYTES`] bytes of the file, and
//! numbers, texts, ids and the object count to the board's limits
//! ([`MAX_COORDINATE`](crate::board::MAX_COORDINATE), [`MAX_SIZE`](crate::board::MAX_SIZE),
//! [`MAX_TEXT_CHARS`](crate::board::MAX_TEXT_CHARS),
//! [`MAX_TEXT_LINES`](crate::board::MAX_TEXT_LINES),
//! [`MAX_BOARD_TEXT_CHARS`](crate::board::MAX_BOARD_TEXT_CHARS),
//! [`MAX_ID_CHARS`](crate::board::MAX_ID_CHARS),
//! [`MAX_OBJECTS`](crate::board::MAX_OBJECTS)).

mod error;
mod parse;
mod read;
mod write;

/// Largest `.lawh` file [`from_lawh`] reads, in bytes (1 MiB).
///
/// The largest board the caps allow writes 415,551 bytes: 40,000 characters of text the writer
/// escapes as `\u{200B}`, 492 arrows with 25-character coordinates, and ids of 64 digits, which
/// the writer quotes. The limit leaves room for hand-written comments on top of that.
///
/// Reading costs what the file holds up to the first cap it passes. Measured with
/// `read_time_at_the_size_limit` in `tests/format_errors.rs` (Linux `x86_64`, release, a laptop
/// with other work running, 2026-10-11) on files of up to 1 MiB, against the reader lawh had
/// before, on top of the `kdl` crate:
///
/// | file | read | through kdl |
/// |---|---|---|
/// | spaces, comments or line breaks | 1.4 to 5.7 ms | 8 to 31 ms |
/// | slashdashed nodes or properties, or a mix of filler | 15 to 22 ms | 25 to 57 ms |
/// | the largest board the caps allow (416 KB) | 2.7 ms | 66 ms |
/// | 500 circles, then more up to the limit | 0.6 ms | 14.5 ms |
/// | `lawh version=1` then 524K ` 1` arguments | 0.001 ms | 2.04 s, 416 MB |
/// | 499 circles with 2 KB of arguments each | 0.002 ms | 1.85 s, 347 MB |
/// | `a=1` repeated on the header | 0.002 ms | 1.34 s, 225 MB |
/// | a multi-line text of 349K lines | 0.5 ms | 1.06 s, 30 MB |
/// | one string of 1 MiB | 0.1 ms | 0.41 s |
/// | a multi-line string indented 2,000 spaces, which opened then | 0.03 ms | 0.39 s |
///
/// The last six peaked at 0.2 MB of extra memory or less (the measurement's floor); the sizes
/// above are kdl's peaks.
pub const MAX_FILE_BYTES: usize = 1024 * 1024;

/// Deepest nesting of block comments `/* /* */ */` in a `.lawh` file.
pub const MAX_COMMENT_DEPTH: usize = 16;

/// Most entries (properties and arguments) a node in a `.lawh` file has; reading stops at the
/// next one. No node takes more: the header takes 1, an ellipse, rect, line or arrow 5.
/// Slashdashed entries are not counted.
pub const MAX_ENTRIES: usize = 6;

/// Most bytes a string, bare word or number takes in a `.lawh` file (64 KiB), quotes included.
///
/// The longest string the writer makes is the most text a `text` holds,
/// [`MAX_TEXT_CHARS`](crate::board::MAX_TEXT_CHARS) characters escaped as `\u{200B}`: 40,002
/// bytes. The cap bounds what reading one string costs when its text is short but its source is
/// long, as with deep indentation in a multi-line string or long whitespace escapes.
pub const MAX_STRING_BYTES: usize = 64 * 1024;

pub use error::FormatError;
pub use read::from_lawh;
pub use write::to_lawh;
