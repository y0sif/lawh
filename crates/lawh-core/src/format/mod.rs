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
//! otherwise overflow the stack. Files are limited to [`MAX_FILE_BYTES`], block comments to
//! [`MAX_COMMENT_DEPTH`] levels, and numbers, texts and the object count to the board's limits
//! ([`MAX_COORDINATE`](crate::board::MAX_COORDINATE), [`MAX_SIZE`](crate::board::MAX_SIZE),
//! [`MAX_TEXT_CHARS`](crate::board::MAX_TEXT_CHARS),
//! [`MAX_TEXT_LINES`](crate::board::MAX_TEXT_LINES),
//! [`MAX_BOARD_TEXT_CHARS`](crate::board::MAX_BOARD_TEXT_CHARS),
//! [`MAX_OBJECTS`](crate::board::MAX_OBJECTS)).

mod error;
mod read;
mod scan;
mod write;

/// Largest `.lawh` file [`from_lawh`] reads, in bytes (8 MiB).
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

/// Deepest nesting of block comments `/* /* */ */` in a `.lawh` file.
pub const MAX_COMMENT_DEPTH: usize = 16;

pub use error::FormatError;
pub use read::from_lawh;
pub use write::to_lawh;
