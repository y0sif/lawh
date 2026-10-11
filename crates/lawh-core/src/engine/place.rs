//! The place rule: where `p` and `P` put a group of objects, and `o` and `O` a new text block,
//! next to the current object, below or above it, clear of the other objects.

use kurbo::{Point, Rect, Size};

use super::cursor::round3;
use crate::board::{MAX_COORDINATE, Shape};

/// Space left between a placed group and the object it goes below or above, and between it and
/// any object it is moved past, in grid units.
///
/// Half a unit reads as "next to" at any zoom where the objects are legible, yet keeps a group
/// from touching its neighbour, so the bounds of the two never overlap and `w`, `J` and `K`
/// still tell them apart. Flat bounds (a level or upright line) count too: a group never ends
/// up lying on one.
pub const PLACE_GAP: f64 = 0.5;

/// Which side of the current object a group goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    /// `p`, `o`.
    Below,
    /// `P`, `O`.
    Above,
}

/// The top-left corner for a box of `size` placed on side `on` of `reference`, clear of every box
/// in `obstacles`, or `None` when there is no room within [`MAX_COORDINATE`].
///
/// Below: the box starts [`PLACE_GAP`] under `reference`, its left edge on `reference`'s. While
/// it overlaps obstacles it moves down to [`PLACE_GAP`] under the lowest of them. Above is the
/// mirror image. Each try rounds y away from `reference` to a multiple of 0.1 (so files show
/// `y=4.3`, and the box never moves back into what it just cleared) and x with [`round3`]: the
/// box tested is the box placed. Boxes that only touch do not overlap, unless one is flat (see
/// [`overlaps`]).
///
/// Adds the obstacles it looked at, one per obstacle a try, to `looked`: at most about the
/// square of their number, when each try clears only one.
pub(crate) fn place(
    reference: Rect,
    size: Size,
    obstacles: &[Rect],
    on: Side,
    looked: &mut usize,
) -> Option<Point> {
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
        *looked = looked.saturating_add(obstacles.len());
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

/// The two boxes overlap on both axes (see [`overlap`]): two boxes of some size share more than
/// an edge; a flat box (of zero width or height) lies within the other's extent, edges
/// included, so a line on a box's edge or on another line overlaps it.
pub(crate) fn overlaps(a: Rect, b: Rect) -> bool {
    overlap(a.x0, a.x1, b.x0, b.x1) && overlap(a.y0, a.y1, b.y0, b.y1)
}

/// The extents `a0..=a1` and `b0..=b1` on one axis overlap. Two of some length must share more
/// than an end; when either has zero length, sharing a point is enough (two of zero length at
/// the same place overlap).
fn overlap(a0: f64, a1: f64, b0: f64, b1: f64) -> bool {
    if a0 < a1 && b0 < b1 {
        a0 < b1 && b0 < a1
    } else {
        a0 <= b1 && b0 <= a1
    }
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

    /// [`super::place`], not counting what it looks at.
    fn place(reference: Rect, size: Size, obstacles: &[Rect], on: Side) -> Option<Point> {
        super::place(reference, size, obstacles, on, &mut 0)
    }

    #[test]
    fn place_counts_the_obstacles_it_looks_at() {
        let reference = r(0.0, 0.0, 1.0, 1.0);
        let size = Size::new(1.0, 1.0);
        // A column of boxes right under the reference, each in the way of the try before:
        // every try looks at all of them.
        let column: Vec<Rect> = (0..10)
            .map(|i| {
                let y = 1.2 + f64::from(i) * 1.6;
                r(0.0, y, 1.0, y + 1.0)
            })
            .collect();
        let mut looked = 0;
        let at = super::place(reference, size, &column, Side::Below, &mut looked);
        assert_eq!(at, Some(Point::new(0.0, 17.1)));
        assert_eq!(looked, 11 * 10);
        let mut looked = 0;
        super::place(reference, size, &[], Side::Below, &mut looked);
        assert_eq!(looked, 0);
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
    fn flat_boxes_overlap_what_they_lie_on() {
        let box_ = r(0.0, 0.0, 2.0, 2.0);
        // Boxes of some size that share only an edge or a corner do not overlap.
        assert!(!overlaps(box_, r(2.0, 0.0, 3.0, 2.0)));
        assert!(!overlaps(box_, r(2.0, 2.0, 3.0, 3.0)));
        assert!(overlaps(box_, r(1.9, 1.9, 3.0, 3.0)));
        // A level line on the box's edge, inside it, or at the same height as another.
        let level = |y: f64| r(0.0, y, 5.0, y);
        assert!(overlaps(level(2.0), box_));
        assert!(overlaps(level(1.0), box_));
        assert!(!overlaps(level(2.1), box_));
        assert!(overlaps(level(0.5), level(0.5)));
        assert!(!overlaps(level(0.5), level(0.6)));
        // Upright lines on the box's left and right edges; one that only meets its corner from
        // below shares just an end with it along y.
        assert!(overlaps(r(0.0, 0.5, 0.0, 4.0), box_));
        assert!(overlaps(r(2.0, 0.5, 2.0, 1.5), box_));
        assert!(!overlaps(r(2.0, 2.0, 2.0, 4.0), box_));
        assert!(!overlaps(r(2.1, 0.5, 2.1, 1.5), box_));
        // A point.
        assert!(overlaps(r(1.0, 1.0, 1.0, 1.0), box_));
        assert!(overlaps(r(2.0, 2.0, 2.0, 2.0), box_));
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
