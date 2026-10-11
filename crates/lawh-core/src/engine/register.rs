//! The unnamed register: what `d`, `x` and `y` keep and `p` and `P` put back.

use crate::board::{Object, Shape};

/// The content of the unnamed register.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Register {
    /// Objects deleted or yanked on the board, in reading order: the first is the one a put
    /// lines up with a grid cursor.
    Objects(Vec<Object>),
    /// Text deleted inside a block. On the board, `p` makes a new text block of it.
    Text(String),
}

/// The prefix of the fresh ids made for a new object of `shape`'s kind: `t` text, `m` math,
/// `c` circle, `e` other ellipses, `r` rectangle, `l` line, `a` arrow.
pub(crate) fn id_prefix(shape: &Shape) -> &'static str {
    match shape {
        Shape::Text { .. } => "t",
        Shape::Math { .. } => "m",
        Shape::Ellipse { rx, ry, .. } => {
            if is_circle(*rx, *ry) {
                "c"
            } else {
                "e"
            }
        }
        Shape::Rect { .. } => "r",
        Shape::Line { .. } => "l",
        Shape::Arrow { .. } => "a",
    }
}

#[expect(
    clippy::float_cmp,
    reason = "the file writes exactly equal radii as a circle; the id follows it"
)]
fn is_circle(rx: f64, ry: f64) -> bool {
    rx == ry
}
