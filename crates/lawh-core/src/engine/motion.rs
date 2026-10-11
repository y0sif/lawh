//! Object motions: reading order for `w` and `b`, the nearest object in a direction for
//! `H J K L`. Pure functions over bounds, in the board's z-order (file order), so ties go to the
//! object written first.

use std::cmp::Ordering;

use kurbo::{Point, Rect};

/// Most the centre of an object's bounds may lie below the first object of its row, in grid
/// units, and still be in that row for `w` and `b`.
///
/// Things drawn side by side are rarely aligned exactly: a label beside a circle, or math next
/// to text, has its centre a little higher or lower. Half a unit keeps those in one row, while
/// two lines of text one line apart (0.6 units with
/// [`FixedMeasure`](super::FixedMeasure)) still read as two rows.
pub const ROW_TOLERANCE: f64 = 0.5;

/// A direction for `H J K L`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Direction {
    Left,
    Down,
    Up,
    Right,
}

/// A motion that lands on an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ObjectMotion {
    /// `w`: the next object in reading order.
    Next,
    /// `b`: the previous object in reading order.
    Previous,
    /// `H J K L`: the nearest object in a direction.
    Nearest(Direction),
}

/// Where an object motion starts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Origin {
    /// The object at this index of the bounds.
    Object(usize),
    /// A grid point, taken as an object of zero size.
    Point(Point),
}

/// The objects `motion` visits from `origin`, repeated `count` times, as indices into `bounds`
/// (one per object, in file order), nearest first. Fewer when the motion runs out of objects;
/// empty when it cannot move at all.
pub(super) fn path(
    bounds: &[Rect],
    origin: Origin,
    motion: ObjectMotion,
    count: usize,
) -> Vec<usize> {
    match motion {
        ObjectMotion::Next => walk(bounds, origin, count, true),
        ObjectMotion::Previous => walk(bounds, origin, count, false),
        ObjectMotion::Nearest(direction) => {
            let mut visited = Vec::new();
            let mut from = origin;
            // Each step lands strictly further along the direction, so it never comes back
            // and stops within `bounds.len()` steps.
            for _ in 0..count.min(bounds.len()) {
                let rect = match from {
                    Origin::Object(i) => bounds.get(i).copied().unwrap_or_default(),
                    Origin::Point(p) => Rect::from_points(p, p),
                };
                let others = bounds
                    .iter()
                    .copied()
                    .enumerate()
                    .filter(|(i, _)| from != Origin::Object(*i));
                let Some(next) = nearest(rect, others, direction) else {
                    break;
                };
                visited.push(next);
                from = Origin::Object(next);
            }
            visited
        }
    }
}

/// Up to `count` steps through the reading order from `origin`, forward or back.
fn walk(bounds: &[Rect], origin: Origin, count: usize, forward: bool) -> Vec<usize> {
    // A point joins the order as an object of zero size, first among equals.
    let (order, start, shift) = match origin {
        Origin::Object(i) => (reading_order(bounds), i, 0),
        Origin::Point(p) => {
            let mut items = Vec::with_capacity(bounds.len() + 1);
            items.push(Rect::from_points(p, p));
            items.extend_from_slice(bounds);
            (reading_order(&items), 0, 1)
        }
    };
    let Some(at) = order.iter().position(|&i| i == start) else {
        return Vec::new();
    };
    let steps: Vec<usize> = if forward {
        order.iter().skip(at + 1).take(count).copied().collect()
    } else {
        let before = order.get(..at).unwrap_or_default();
        before.iter().rev().take(count).copied().collect()
    };
    // The point itself is never a step, so no index is below `shift`.
    steps
        .into_iter()
        .filter_map(|i| i.checked_sub(shift))
        .collect()
}

/// The order `w` and `b` walk through `items`: rows from top to bottom, and left to right within
/// a row. Returns indices into `items`.
///
/// Items are taken by the y of their centre; an item more than [`ROW_TOLERANCE`] below the
/// first item of the current row starts the next row. Within a row items go by their left edge.
/// Ties go to the item that comes first in `items`.
pub(super) fn reading_order(items: &[Rect]) -> Vec<usize> {
    let mut by_y: Vec<(usize, Rect)> = items.iter().copied().enumerate().collect();
    by_y.sort_by(|a, b| centre(a.1).y.total_cmp(&centre(b.1).y).then(a.0.cmp(&b.0)));
    let mut order = Vec::with_capacity(items.len());
    let mut row: Vec<(usize, Rect)> = Vec::new();
    let mut row_y = 0.0;
    for item in by_y {
        let y = centre(item.1).y;
        if !row.is_empty() && y - row_y > ROW_TOLERANCE {
            end_row(&mut row, &mut order);
        }
        if row.is_empty() {
            row_y = y;
        }
        row.push(item);
    }
    end_row(&mut row, &mut order);
    order
}

fn end_row(row: &mut Vec<(usize, Rect)>, order: &mut Vec<usize>) {
    row.sort_by(|a, b| a.1.x0.total_cmp(&b.1.x0).then(a.0.cmp(&b.0)));
    order.extend(row.drain(..).map(|(i, _)| i));
}

fn centre(r: Rect) -> Point {
    Point::new(f64::midpoint(r.x0, r.x1), f64::midpoint(r.y0, r.y1))
}

/// The nearest of `candidates` (index and bounds) from `origin` in `direction`, by index.
///
/// Only candidates whose centre lies strictly further along `direction` than the origin's
/// count. First come those whose bounds overlap the origin's across the direction (sharing
/// more than an edge), nearest by the gap between facing edges (0 when they overlap along the
/// direction too), then by the distance between centres along the direction. When there are
/// none, a candidate must lie within a cone: its centre at most twice as far across the
/// direction as along it, scored by the distance along plus twice the distance across. Ties go
/// to the first candidate.
pub(super) fn nearest(
    origin: Rect,
    candidates: impl IntoIterator<Item = (usize, Rect)>,
    direction: Direction,
) -> Option<usize> {
    let mut best: Option<((u8, f64, f64), usize)> = None;
    for (i, rect) in candidates {
        let Some(score) = score(origin, rect, direction) else {
            continue;
        };
        let better = best.is_none_or(|(b, _)| compare(score, b) == Ordering::Less);
        if better {
            best = Some((score, i));
        }
    }
    best.map(|(_, i)| i)
}

/// How near `rect` is from `origin` in `direction`, lower is nearer: the tier (0 overlapping,
/// 1 in the cone) and two distances. `None` when it is not a candidate.
fn score(origin: Rect, rect: Rect, direction: Direction) -> Option<(u8, f64, f64)> {
    let (o, c) = (centre(origin), centre(rect));
    // Along the direction, across it, the gap between facing edges, and whether the two
    // overlap across the direction.
    let (along, across, gap, overlap) = match direction {
        Direction::Right => (
            c.x - o.x,
            c.y - o.y,
            rect.x0 - origin.x1,
            overlaps(origin.y0, origin.y1, rect.y0, rect.y1),
        ),
        Direction::Left => (
            o.x - c.x,
            c.y - o.y,
            origin.x0 - rect.x1,
            overlaps(origin.y0, origin.y1, rect.y0, rect.y1),
        ),
        Direction::Down => (
            c.y - o.y,
            c.x - o.x,
            rect.y0 - origin.y1,
            overlaps(origin.x0, origin.x1, rect.x0, rect.x1),
        ),
        Direction::Up => (
            o.y - c.y,
            c.x - o.x,
            origin.y0 - rect.y1,
            overlaps(origin.x0, origin.x1, rect.x0, rect.x1),
        ),
    };
    if along.is_nan() || along <= 0.0 {
        None
    } else if overlap {
        Some((0, gap.max(0.0), along))
    } else if across.abs() <= 2.0 * along {
        Some((1, along + 2.0 * across.abs(), 0.0))
    } else {
        None
    }
}

fn compare(a: (u8, f64, f64), b: (u8, f64, f64)) -> Ordering {
    a.0.cmp(&b.0)
        .then(a.1.total_cmp(&b.1))
        .then(a.2.total_cmp(&b.2))
}

/// The open ranges `a0..a1` and `b0..b1` share a point: touching ends do not count, but a range
/// of zero length strictly inside the other does.
fn overlaps(a0: f64, a1: f64, b0: f64, b1: f64) -> bool {
    a0 < b1 && b0 < a1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::new(x0, y0, x1, y1)
    }

    #[test]
    fn rows_form_within_the_tolerance_of_their_first_item() {
        // Centres at y 0, 0.4 and 0.8: the third is 0.8 below the row's first, so it starts a
        // new row even though it is only 0.4 below the second.
        let items = [
            r(4.0, -1.0, 6.0, 1.0),
            r(2.0, -0.6, 3.0, 1.4),
            r(0.0, 0.3, 1.0, 1.3),
            r(1.0, 0.0, 2.0, 0.0),
        ];
        // Item 3, a flat line at y 0, ties with item 0 on y and goes by its x.
        assert_eq!(reading_order(&items), [3, 1, 0, 2]);
        assert_eq!(reading_order(&[]), Vec::<usize>::new());
    }

    #[test]
    fn ties_go_to_file_order() {
        let same = r(0.0, 0.0, 1.0, 1.0);
        assert_eq!(reading_order(&[same, same, same]), [0, 1, 2]);
    }

    #[test]
    fn a_point_joins_the_order_first_among_equals() {
        let items = [r(0.0, 0.0, 2.0, 2.0), r(3.0, 0.0, 5.0, 2.0)];
        let from = |x, y| Origin::Point(Point::new(x, y));
        assert_eq!(path(&items, from(0.0, 1.0), ObjectMotion::Next, 5), [0, 1]);
        assert_eq!(path(&items, from(1.0, 1.0), ObjectMotion::Next, 5), [1]);
        assert_eq!(path(&items, from(1.0, 1.0), ObjectMotion::Previous, 5), [0]);
        assert_eq!(path(&items, from(9.0, 1.0), ObjectMotion::Next, 1), []);
        assert_eq!(path(&[], from(0.0, 0.0), ObjectMotion::Next, 1), []);
    }

    #[test]
    fn overlapping_candidates_win_by_facing_gap() {
        let origin = r(0.0, 0.0, 2.0, 2.0);
        let candidates = [
            // In the cone, closer by centre, but not overlapping across.
            (0, r(2.5, 2.5, 3.5, 3.5)),
            // Overlaps across, gap 3.
            (1, r(5.0, 1.5, 6.0, 9.0)),
            // Overlaps across, gap 3 too, centre further along: loses the tie on centres.
            (2, r(5.0, -4.0, 9.0, 0.5)),
            // Only touches at y 2: not overlapping, and outside the cone.
            (3, r(2.0, 2.0, 3.0, 9.0)),
        ];
        assert_eq!(nearest(origin, candidates, Direction::Right), Some(1));
        assert_eq!(nearest(origin, candidates, Direction::Left), None);
        // Below, none overlaps in x (3 only touches at x 2), so the cone decides: 0 scores
        // 2 + 2 x 2 = 6, 3 scores 4.5 + 2 x 1.5 = 7.5 and 1 scores 4.25 + 2 x 4.5 = 13.25.
        assert_eq!(nearest(origin, candidates, Direction::Down), Some(0));
    }

    #[test]
    fn the_cone_scores_along_plus_twice_across() {
        let origin = r(0.0, 0.0, 0.0, 0.0);
        let candidates = [
            // along 4, across 1: 6.
            (0, r(4.0, 1.0, 4.0, 1.0)),
            // along 2, across 2: 6, a tie, file order wins.
            (1, r(2.0, -2.0, 2.0, -2.0)),
            // along 1, across 3: outside the cone.
            (2, r(1.0, 3.0, 1.0, 3.0)),
        ];
        assert_eq!(nearest(origin, candidates, Direction::Right), Some(0));
        assert_eq!(
            nearest(origin, candidates[1..].to_vec(), Direction::Right),
            Some(1)
        );
    }

    #[test]
    fn counts_repeat_from_each_target() {
        let items = [
            r(0.0, 0.0, 1.0, 1.0),
            r(2.0, 0.0, 3.0, 1.0),
            r(4.0, 0.0, 5.0, 1.0),
        ];
        let right = ObjectMotion::Nearest(Direction::Right);
        assert_eq!(path(&items, Origin::Object(0), right, 5), [1, 2]);
        let left = ObjectMotion::Nearest(Direction::Left);
        assert_eq!(path(&items, Origin::Object(0), left, 5), []);
        assert_eq!(
            path(&items, Origin::Object(2), ObjectMotion::Previous, 1),
            [1]
        );
    }
}
