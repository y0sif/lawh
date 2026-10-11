//! The board cursor: a grid point or an object.

use kurbo::Point;

use crate::board::{MAX_COORDINATE, ObjectId, Shape};

/// Where the cursor is on the board: a point on the grid, or an object (the current object).
#[derive(Debug, Clone, PartialEq)]
pub enum Cursor {
    Grid(Point),
    Object(ObjectId),
}

/// The point that places an object: where the cursor stands on it, where motions start from it
/// and where new objects go next to it. The top-left corner of text, math and rectangles, the
/// centre of an ellipse, the start of a line or arrow.
pub fn anchor(shape: &Shape) -> Point {
    match shape {
        Shape::Text { at, .. } | Shape::Math { at, .. } => *at,
        Shape::Ellipse { center, .. } => *center,
        Shape::Rect { origin, .. } => *origin,
        Shape::Line { a, .. } | Shape::Arrow { a, .. } => *a,
    }
}

/// `v` rounded to 0.001 unit, the precision of every coordinate the engine makes, so files
/// show `0.3`, not `0.30000000000000004`.
pub fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// `p` moved by `(dx, dy)`, kept within [`MAX_COORDINATE`] and rounded with [`round3`].
pub(crate) fn offset(p: Point, dx: f64, dy: f64) -> Point {
    let step = |v: f64, d: f64| round3((v + d).clamp(-MAX_COORDINATE, MAX_COORDINATE));
    Point::new(step(p.x, dx), step(p.y, dy))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_are_the_placing_points() {
        let p = Point::new(1.0, 2.0);
        let q = Point::new(5.0, 6.0);
        let shapes = [
            Shape::Text {
                at: p,
                text: String::new(),
            },
            Shape::Math {
                at: p,
                src: String::new(),
            },
            Shape::Ellipse {
                center: p,
                rx: 1.0,
                ry: 2.0,
            },
            Shape::Rect {
                origin: p,
                w: 3.0,
                h: 4.0,
            },
            Shape::Line { a: p, b: q },
            Shape::Arrow { a: p, b: q },
        ];
        for shape in shapes {
            assert_eq!(anchor(&shape), p, "{shape:?}");
        }
    }

    #[test]
    fn round3_drops_float_noise() {
        assert_eq!(round3(0.1 + 0.2), 0.3);
        assert_eq!(round3(-1.234_56), -1.235);
        assert_eq!(round3(1.0004), 1.0);
        assert_eq!(round3(1.0006), 1.001);
        assert_eq!(round3(MAX_COORDINATE), MAX_COORDINATE);
        assert_eq!(round3(-MAX_COORDINATE), -MAX_COORDINATE);
    }

    #[test]
    fn offset_clamps_to_the_coordinate_limit() {
        let far = offset(Point::new(MAX_COORDINATE - 1.0, 0.5), 10.0, -0.25);
        assert_eq!(far, Point::new(MAX_COORDINATE, 0.25));
        let near = offset(Point::new(-MAX_COORDINATE + 0.5, 0.0), -3.0, 1e9);
        assert_eq!(near, Point::new(-MAX_COORDINATE, MAX_COORDINATE));
    }
}
