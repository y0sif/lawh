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
//! Reading goes through the `kdl` crate's parser, so hand-written files may use any KDL v2
//! syntax: comments, quoted or raw strings, properties in any order. Writing is done here, in one
//! fixed shape per object, so saved files diff cleanly.

mod error;
mod read;
mod write;

pub use error::FormatError;
pub use read::from_lawh;
pub use write::to_lawh;
