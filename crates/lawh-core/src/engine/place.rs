//! The place rule: where `p`, `P` (and later `o`, `O`) put a group of objects next to the current
//! object, below or above it, clear of the other objects.

use kurbo::{Point, Rect, Size};

use super::cursor::round3;
use crate::board::{MAX_COORDINATE, Shape};

/// Space left between a placed group and the object it goes below or above, and between it and
/// any object it is moved past, in grid units.
///
/// Half a unit reads as "next to" at any zoom where the objects are legible, yet keeps a group
/// from touching its neighbour, so the bounds of the two never overlap and `w`, `J` and `K`
/// still tell them apart.
pub const PLACE_GAP: f64 = 0.5;

/// Which side of the current object a group goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    /// `p`, `o`.
    Below,
    /// `P`, `O`.
    Above,
}

/// The top-left corner for a box of `size` placed on side `on` of `reference`, clear of every box in
/// `obstacles`, or `None` when there is no room within [`MAX_COORDINATE`].
///
/// Below: the box starts [`PLACE_GAP`] under `reference`, its left edge on `reference`'s. While
/// it overlaps obstacles it moves down to [`PLACE_GAP`] under the lowest of them. Above is the
/// mirror image. Each try rounds y away from `reference` to a multiple of 0.1 (so files show
/// `y=4.3`, and the box never moves back into what it just cleared) and x with [`round3`]: the
/// box tested is the box placed. Boxes that only touch do not overlap.
pub(crate) fn place(reference: Rect, size: Size, obstacles: &[Rect], on: Side) -> Option<Point> {
    let x = round3(reference.x0);
    let mut y = match on {
        Side::Below => ceil_tenth(reference.y1 + PLACE_GAP),
        Side::Above => floor_tenth(reference.y0 - PLACE_GAP - size.height),
    };
    // Every move clears at least one obstacle for good (y only moves away from it), so one try
    // per obstacle and a last one are enough.
    for _ in 0..=obstacles.len() {
        let candidate = Rect::from_origin_size((x, y), size);
        if !within_limits(candidate) {
            return None;
        }
        let hits = obstacles.iter().filter(|o| overlaps(candidate, **o));
        let next = match on {
            Side::Below => hits
                .map(|o| o.y1)
                .max_by(f64::total_cmp)
                .map(|bottom| ceil_tenth(bottom + PLACE_GAP)),
            Side::Above => hits
                .map(|o| o.y0)
                .min_by(f64::total_cmp)
                .map(|top| floor_tenth(top - PLACE_GAP - size.height)),
        };
        match next {
            None => return Some(Point::new(x, y)),
            Some(next) => y = next,
        }
    }
    None
}

/// The two boxes share more than an edge. A box of zero width or height overlaps a box it lies
/// strictly inside.
pub(crate) fn overlaps(a: Rect, b: Rect) -> bool {
    a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
}

fn within_limits(r: Rect) -> bool {
    let ok = |v: f64| v.abs() <= MAX_COORDINATE;
    ok(r.x0) && ok(r.x1) && ok(r.y0) && ok(r.y1)
}

/// `v` rounded up to a multiple of 0.1, after [`round3`] so float noise (`2.3000000000000003`)
/// does not round up a whole step.
fn ceil_tenth(v: f64) -> f64 {
    ((v * 1000.0).round() / 100.0).ceil() / 10.0 + 0.0
}

/// `v` rounded down to a multiple of 0.1, after [`round3`].
fn floor_tenth(v: f64) -> f64 {
    ((v * 1000.0).round() / 100.0).floor() / 10.0 + 0.0
}

/// `shape` moved by `(dx, dy)`. Moved coordinates are rounded with [`round3`]; a coordinate that
/// does not move stays exactly as it was, so a put back where the objects came from restores
/// them exactly.
pub(crate) fn translate(shape: &Shape, dx: f64, dy: f64) -> Shape {
    let shift = |v: f64, d: f64| if d == 0.0 { v } else { round3(v + d) };
    let point = |p: Point| Point::new(shift(p.x, dx), shift(p.y, dy));
    match shape {
        Shape::Text { at, text } => Shape::Text {
            at: point(*at),
            text: text.clone(),
        },
        Shape::Math { at, src } => Shape::Math {
            at: point(*at),
            src: src.clone(),
        },
        Shape::Ellipse { center, rx, ry } => Shape::Ellipse {
            center: point(*center),
            rx: *rx,
            ry: *ry,
        },
        Shape::Rect { origin, w, h } => Shape::Rect {
            origin: point(*origin),
            w: *w,
            h: *h,
        },
        Shape::Line { a, b } => Shape::Line {
            a: point(*a),
            b: point(*b),
        },
        Shape::Arrow { a, b } => Shape::Arrow {
            a: point(*a),
            b: point(*b),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::new(x0, y0, x1, y1)
    }

    #[test]
    fn below_and_above_leave_a_gap() {
        let reference = r(1.0, 1.0, 3.0, 2.0);
        let size = Size::new(4.0, 1.0);
        assert_eq!(
            place(reference, size, &[reference], Side::Below),
            Some(Point::new(1.0, 2.5))
        );
        assert_eq!(
            place(reference, size, &[reference], Side::Above),
            Some(Point::new(1.0, -0.5))
        );
    }

    #[test]
    fn obstacles_push_the_box_past_them() {
        let reference = r(0.0, 0.0, 2.0, 1.0);
        let size = Size::new(2.0, 1.0);
        // The first spot (y 1.5) hits the first obstacle, the next (y 3.5) the second, which
        // ends at 4.25: the box goes to 4.75, rounded up to 4.8.
        let obstacles = [r(1.0, 1.2, 5.0, 3.0), r(-3.0, 3.6, 0.5, 4.25)];
        assert_eq!(
            place(reference, size, &obstacles, Side::Below),
            Some(Point::new(0.0, 4.8))
        );
        // A box that only touches does not move it: the first spot is 1.5 to 2.5.
        let touching = [r(2.0, 1.0, 3.0, 9.0), r(0.0, 2.5, 3.0, 3.0)];
        assert_eq!(
            place(reference, size, &touching, Side::Below),
            Some(Point::new(0.0, 1.5))
        );
        // Above: the obstacle starts at -2.05, so the box ends at -2.55 and starts at -3.55,
        // rounded down to -3.6.
        let above = [r(0.0, -2.05, 1.0, -0.2)];
        assert_eq!(
            place(reference, size, &above, Side::Above),
            Some(Point::new(0.0, -3.6))
        );
    }

    #[test]
    fn y_rounds_away_from_the_reference_and_x_to_thousandths() {
        let reference = r(0.123_456, 0.1, 1.0, 0.1 + 0.2);
        let size = Size::new(1.0, 0.6);
        // 0.3 + 0.5 is 0.8 without the noise of 0.1 + 0.2.
        assert_eq!(
            place(reference, size, &[], Side::Below),
            Some(Point::new(0.123, 0.8))
        );
        let reference = r(0.0, 0.0, 1.0, 1.01);
        assert_eq!(
            place(reference, size, &[], Side::Below),
            Some(Point::new(0.0, 1.6))
        );
        assert_eq!(
            place(reference, size, &[], Side::Above),
            Some(Point::new(0.0, -1.1))
        );
    }

    #[test]
    fn no_room_past_max_coordinate() {
        let low = r(0.0, MAX_COORDINATE - 2.0, 1.0, MAX_COORDINATE - 1.0);
        let size = Size::new(1.0, 1.0);
        assert_eq!(place(low, size, &[], Side::Below), None);
        assert!(place(low, size, &[], Side::Above).is_some());
        let high = r(0.0, -MAX_COORDINATE + 1.0, 1.0, -MAX_COORDINATE + 1.5);
        assert_eq!(place(high, size, &[], Side::Above), None);
        // An obstacle pushes it past the edge.
        let near = r(0.0, MAX_COORDINATE - 9.0, 1.0, MAX_COORDINATE - 8.0);
        let wall = [r(-5.0, MAX_COORDINATE - 7.0, 5.0, MAX_COORDINATE - 0.5)];
        assert_eq!(place(near, size, &wall, Side::Below), None);
    }

    #[test]
    fn translate_rounds_only_what_moves() {
        let line = Shape::Line {
            a: Point::new(0.1, 0.123_456),
            b: Point::new(1.0, 2.0),
        };
        let moved = translate(&line, 0.2, 0.0);
        assert_eq!(
            moved,
            Shape::Line {
                a: Point::new(0.3, 0.123_456),
                b: Point::new(1.2, 2.0),
            }
        );
    }
}
