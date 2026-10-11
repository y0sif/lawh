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
//! Reading goes through the `kdl` crate's parser, so hand-written files may use flat KDL v2
//! syntax: comments, quoted or raw strings, properties in any order. Writing is done here, in one
//! fixed shape per object, so saved files diff cleanly.
//!
//! Before kdl sees a file, a linear scan checks its structure (see `scan.rs`): kdl's parser
//! recurses on nested blocks, nested comments and error recovery, so hostile files could
//! otherwise overflow the stack. The scan also blanks comments, slashdashed text and other
//! filler, which kdl reads up to a hundred times slower than spaces. Files are limited to
//! [`MAX_FILE_BYTES`], block comments to [`MAX_COMMENT_DEPTH`] levels, and numbers, texts, ids
//! and the object count to the board's limits
//! ([`MAX_COORDINATE`](crate::board::MAX_COORDINATE), [`MAX_SIZE`](crate::board::MAX_SIZE),
//! [`MAX_TEXT_CHARS`](crate::board::MAX_TEXT_CHARS),
//! [`MAX_TEXT_LINES`](crate::board::MAX_TEXT_LINES),
//! [`MAX_BOARD_TEXT_CHARS`](crate::board::MAX_BOARD_TEXT_CHARS),
//! [`MAX_ID_CHARS`](crate::board::MAX_ID_CHARS),
//! [`MAX_OBJECTS`](crate::board::MAX_OBJECTS)). The scan counts the objects, so kdl never parses
//! more than a board holds.

mod error;
mod read;
mod scan;
mod write;

/// Largest `.lawh` file [`from_lawh`] reads, in bytes (1 MiB).
///
/// The largest board the caps allow writes 415,551 bytes: 40,000 characters of text the writer
/// escapes as `\u{200B}`, 492 arrows with 25-character coordinates, and ids of 64 digits, which
/// the writer quotes. The limit leaves room for hand-written comments on top of that.
///
/// Reading time grows with the file, not the board. Measured with `read_time_at_the_size_limit`
/// in `tests/format_errors.rs` (Linux `x86_64`, release, 2026-10-11) on 1 MiB files:
///
/// | file | read |
/// |---|---|
/// | the largest board the caps allow (416 KB) | 68 ms |
/// | line comments, block comments or spaces | 8 to 10 ms |
/// | line breaks, slashdashed nodes, or a mix of filler | 22 to 33 ms |
/// | slashdashed properties on one node | 57 ms |
/// | the same filler before the scan blanked it | up to 4.1 s |
/// | 500 circles, then more up to the limit (the scan stops kdl at the 501st) | 15 ms |
/// | the same circles when kdl read them all before the cap was checked | 1.1 s |
pub const MAX_FILE_BYTES: usize = 1024 * 1024;

/// Deepest nesting of block comments `/* /* */ */` in a `.lawh` file.
pub const MAX_COMMENT_DEPTH: usize = 16;

pub use error::FormatError;
pub use read::from_lawh;
pub use write::to_lawh;
