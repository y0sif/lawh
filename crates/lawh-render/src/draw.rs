//! Shape geometry in screen pixels: outlines, arrowheads and the dot grid.
//!
//! Every outline is cut down to `clip`, a rectangle a little larger than the image, in `f64`
//! before `vello_cpu` sees it. Shapes can be millions of pixels across at high zoom; cut down,
//! they cost the same as small ones, and `vello_cpu`'s `f32` coordinates stay precise.

use std::f64::consts::{PI, TAU};

use kurbo::{Arc, BezPath, Circle, Ellipse, Point, Rect, Shape as _, Vec2};

use crate::Viewport;

/// Curve flattening tolerance in physical pixels.
pub(crate) const TOLERANCE_PX: f64 = 0.1;

/// Shape outline width in logical pixels, the same at every zoom.
pub(crate) const STROKE_PX: f64 = 2.0;

/// Arrowhead length in grid units, kept between the pixel limits below.
const ARROW_HEAD_UNITS: f64 = 0.3;
const ARROW_HEAD_MIN_PX: f64 = 8.0;
const ARROW_HEAD_MAX_PX: f64 = 24.0;
/// Arrowhead half-width as a fraction of its length.
const ARROW_HEAD_SPREAD: f64 = 0.4;

/// Grid dots are drawn only from this many physical pixels per unit up.
const GRID_MIN_PX_PER_UNIT: f64 = 12.0;
/// Grid dot diameter in logical pixels.
const GRID_DOT_PX: f64 = 1.5;
/// Most grid dots per row or column, a guard against absurd viewports.
const GRID_MAX_DOTS_PER_AXIS: f64 = 4096.0;

/// An ellipse whose bounds stay within this many pixels of the clip is drawn
/// whole: `vello_cpu` culls the parts outside, and its `f32` coordinates are
/// precise to well under a hundredth of a pixel this close. Larger ones are
/// cut to their visible arcs first.
const WHOLE_ELLIPSE_REACH_PX: f64 = 65_536.0;

/// Room around the image for strokes and arrowheads, in physical pixels: an object is drawn
/// when its bounds reach into the image grown by this much, and outlines are cut at that
/// border, where a stroke's caps and joins can't be seen.
pub(crate) fn cull_padding_px(vp: &Viewport) -> f64 {
    (ARROW_HEAD_MAX_PX + STROKE_PX) * vp.scale_factor
}

pub(crate) fn stroke_width_px(vp: &Viewport) -> f64 {
    STROKE_PX * vp.scale_factor
}

/// The part of an ellipse's outline inside `clip`, or `None` when none of it is.
pub(crate) fn ellipse(
    vp: &Viewport,
    center: Point,
    rx: f64,
    ry: f64,
    clip: Rect,
) -> Option<BezPath> {
    let s = vp.px_per_unit();
    let center = vp.to_screen(center);
    let radii = Vec2::new(rx.abs() * s, ry.abs() * s);
    let bounds = Rect::new(
        center.x - radii.x,
        center.y - radii.y,
        center.x + radii.x,
        center.y + radii.y,
    );
    if !(bounds.is_finite() && bounds.overlaps(clip)) {
        return None;
    }
    let near = clip.inflate(WHOLE_ELLIPSE_REACH_PX, WHOLE_ELLIPSE_REACH_PX);
    if near.contains_rect(bounds) {
        return Some(Ellipse::new(center, radii, 0.0).to_path(TOLERANCE_PX));
    }
    visible_arcs(center, radii, clip)
}

/// The arcs of the ellipse `center`, `radii` (axis-aligned) that lie inside `clip`.
///
/// The angles where the outline crosses the clip's edges split it into arcs that are either
/// wholly inside or wholly outside; the middle of each arc tells which.
fn visible_arcs(center: Point, radii: Vec2, clip: Rect) -> Option<BezPath> {
    let mut cuts = Vec::with_capacity(8);
    for x in [clip.x0, clip.x1] {
        let cos = (x - center.x) / radii.x;
        if cos.abs() <= 1.0 {
            let angle = cos.acos();
            cuts.extend([angle, TAU - angle]);
        }
    }
    for y in [clip.y0, clip.y1] {
        let sin = (y - center.y) / radii.y;
        if sin.abs() <= 1.0 {
            let angle = sin.asin();
            cuts.extend([angle.rem_euclid(TAU), PI - angle]);
        }
    }
    let inside = |angle: f64| {
        clip.contains(center + Vec2::new(radii.x * angle.cos(), radii.y * angle.sin()))
    };
    let Some(&first) = cuts.iter().min_by(|a, b| a.total_cmp(b)) else {
        // No edge of the clip crosses the outline: it is all inside or all outside.
        return inside(0.0).then(|| Ellipse::new(center, radii, 0.0).to_path(TOLERANCE_PX));
    };
    cuts.sort_by(f64::total_cmp);
    let ends = cuts.iter().skip(1).copied().chain([first + TAU]);
    let mut path = BezPath::new();
    for (start, end) in cuts.iter().copied().zip(ends) {
        let sweep = end - start;
        if sweep > 0.0 && inside(start + sweep / 2.0) {
            path.extend(Arc::new(center, radii, start, sweep, 0.0).path_elements(TOLERANCE_PX));
        }
    }
    (!path.elements().is_empty()).then_some(path)
}

/// A rectangle's outline, with the edges outside `clip` moved onto it, or `None` when no edge
/// reaches into `clip`.
pub(crate) fn rect(vp: &Viewport, origin: Point, w: f64, h: f64, clip: Rect) -> Option<BezPath> {
    let s = vp.px_per_unit();
    let rect = Rect::from_origin_size(vp.to_screen(origin), (w * s, h * s)).abs();
    if !(rect.is_finite() && rect.overlaps(clip)) {
        return None;
    }
    let around_clip =
        rect.x0 < clip.x0 && rect.y0 < clip.y0 && rect.x1 > clip.x1 && rect.y1 > clip.y1;
    if around_clip {
        return None;
    }
    // A moved edge lies on the clip's border, outside the image by more than half a stroke.
    Some(rect.intersect(clip).to_path(TOLERANCE_PX))
}

/// The part of a line inside `clip`.
pub(crate) fn line(vp: &Viewport, a: Point, b: Point, clip: Rect) -> Option<BezPath> {
    let (a, b) = clip_segment(vp.to_screen(a), vp.to_screen(b), clip)?;
    Some(segment(a, b))
}

/// An arrow as a shaft to stroke and a head to fill, each cut to `clip` (`None` when outside
/// it). The shaft stops inside the head so its stroke never pokes past the tip. A zero-length
/// arrow has no head.
pub(crate) fn arrow(
    vp: &Viewport,
    a: Point,
    b: Point,
    clip: Rect,
) -> (Option<BezPath>, Option<BezPath>) {
    let (a, b) = (vp.to_screen(a), vp.to_screen(b));
    let dir = b - a;
    let len = dir.hypot();
    if !(len.is_finite() && len > f64::EPSILON) {
        return (clip_segment(a, b, clip).map(|(a, b)| segment(a, b)), None);
    }
    let sf = vp.scale_factor;
    let head = (ARROW_HEAD_UNITS * vp.px_per_unit())
        .max(ARROW_HEAD_MIN_PX * sf)
        .min(ARROW_HEAD_MAX_PX * sf)
        .min(len);
    let along = dir / len;
    let across = Vec2::new(-along.y, along.x) * (head * ARROW_HEAD_SPREAD);
    let base = b - along * head;

    let mut tip = BezPath::new();
    tip.move_to(b);
    tip.line_to(base + across);
    tip.line_to(base - across);
    tip.close_path();
    let tip = tip.bounding_box().overlaps(clip).then_some(tip);
    let shaft = clip_segment(a, b - along * (head * 0.5), clip).map(|(a, b)| segment(a, b));
    (shaft, tip)
}

/// One path holding a dot at every grid point on screen, or `None` when zoomed out too far for
/// a grid to help.
pub(crate) fn grid_dots(vp: &Viewport) -> Option<BezPath> {
    if vp.px_per_unit() < GRID_MIN_PX_PER_UNIT {
        return None;
    }
    let visible = vp.visible_board_rect();
    let (x0, y0) = (visible.x0.floor(), visible.y0.floor());
    let cols = (visible.x1.ceil() - x0).min(GRID_MAX_DOTS_PER_AXIS);
    let rows = (visible.y1.ceil() - y0).min(GRID_MAX_DOTS_PER_AXIS);
    if !(cols.is_finite() && rows.is_finite()) {
        return None;
    }
    let radius = GRID_DOT_PX * vp.scale_factor / 2.0;
    let mut path = BezPath::new();
    for row in 0..=count(rows) {
        let y = y0 + f64::from(row);
        for col in 0..=count(cols) {
            let at = vp.to_screen(Point::new(x0 + f64::from(col), y));
            // Centre each dot on a pixel so it stays a crisp single dot.
            let at = Point::new(at.x.floor() + 0.5, at.y.floor() + 0.5);
            path.extend(Circle::new(at, radius).path_elements(TOLERANCE_PX));
        }
    }
    Some(path)
}

fn segment(a: Point, b: Point) -> BezPath {
    let mut path = BezPath::new();
    path.move_to(a);
    path.line_to(b);
    path
}

/// The part of the segment `a`-`b` inside `clip` (Liang-Barsky), or `None` when none of it is
/// or a coordinate isn't finite. Ends inside `clip` are returned exactly.
fn clip_segment(a: Point, b: Point, clip: Rect) -> Option<(Point, Point)> {
    let d = b - a;
    if !(a.is_finite() && b.is_finite() && d.is_finite()) {
        return None;
    }
    let (mut enter, mut leave) = (0.0_f64, 1.0_f64);
    // For each edge: how fast the segment moves out through it, and how far inside `a` is.
    for (out, room) in [
        (-d.x, a.x - clip.x0),
        (d.x, clip.x1 - a.x),
        (-d.y, a.y - clip.y0),
        (d.y, clip.y1 - a.y),
    ] {
        if out == 0.0 {
            if room < 0.0 {
                return None;
            }
        } else {
            let t = room / out;
            if out < 0.0 {
                enter = enter.max(t);
            } else {
                leave = leave.min(t);
            }
        }
    }
    if enter > leave {
        return None;
    }
    let start = if enter > 0.0 { a + d * enter } else { a };
    let end = if leave < 1.0 { a + d * leave } else { b };
    Some((start, end))
}

/// A non-negative float count, already capped, as an integer.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "callers cap the value at GRID_MAX_DOTS_PER_AXIS and it is clamped at 0 here"
)]
fn count(n: f64) -> u32 {
    n.max(0.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIP: Rect = Rect::new(0.0, 0.0, 100.0, 50.0);

    #[test]
    fn segments_inside_are_kept_exactly() {
        let (a, b) = (Point::new(0.1, 0.2), Point::new(99.7, 49.3));
        assert_eq!(clip_segment(a, b, CLIP), Some((a, b)));
    }

    #[test]
    fn segments_are_cut_at_the_clip() {
        let (a, b) = clip_segment(Point::new(-1e12, 25.0), Point::new(1e12, 25.0), CLIP).unwrap();
        assert!((a - Point::new(0.0, 25.0)).hypot() < 1e-3, "{a:?}");
        assert!((b - Point::new(100.0, 25.0)).hypot() < 1e-3, "{b:?}");

        let (a, b) = clip_segment(Point::new(-1e9, -1e9), Point::new(1e9, 1e9), CLIP).unwrap();
        assert!((a - Point::new(0.0, 0.0)).hypot() < 1e-3, "{a:?}");
        assert!((b - Point::new(50.0, 50.0)).hypot() < 1e-3, "{b:?}");
    }

    #[test]
    fn segments_outside_or_broken_are_dropped() {
        for (a, b) in [
            (Point::new(-10.0, -10.0), Point::new(-1.0, 60.0)),
            (Point::new(0.0, 60.0), Point::new(100.0, 60.0)),
            (Point::new(-1e9, 1e9), Point::new(1e9, 1e9 + 1.0)),
            (Point::new(f64::NAN, 0.0), Point::new(1.0, 1.0)),
            (Point::new(0.0, 0.0), Point::new(f64::INFINITY, 1.0)),
            (Point::new(-f64::MAX, 0.0), Point::new(f64::MAX, 0.0)),
        ] {
            assert_eq!(clip_segment(a, b, CLIP), None, "{a:?} {b:?}");
        }
    }

    /// Every point of `path` is within `tolerance` of the outline of the ellipse.
    fn on_ellipse(path: &BezPath, center: Point, radii: Vec2, tolerance: f64) -> bool {
        path.elements().iter().all(|el| {
            el.end_point().is_none_or(|p| {
                let (x, y) = ((p.x - center.x) / radii.x, (p.y - center.y) / radii.y);
                ((x * x + y * y).sqrt() - 1.0).abs() * radii.x.max(radii.y) < tolerance
            })
        })
    }

    #[test]
    fn huge_ellipses_keep_only_the_visible_arc() {
        // A circle 1.6e9 px in radius whose right edge passes through the clip.
        let center = Point::new(50.0 - 1.6e9, 25.0);
        let radii = Vec2::new(1.6e9, 1.6e9);
        let path = visible_arcs(center, radii, CLIP).unwrap();
        assert!(path.elements().len() < 8, "{}", path.elements().len());
        assert!(on_ellipse(&path, center, radii, 1e-3));
        let bounds = path.bounding_box();
        assert!(CLIP.inflate(1e-3, 1e-3).contains_rect(bounds), "{bounds:?}");
        assert!(bounds.height() > 49.0, "{bounds:?}");
    }

    #[test]
    fn ellipses_around_or_away_from_the_clip_are_dropped() {
        let radii = Vec2::new(1e6, 1e6);
        assert!(visible_arcs(Point::new(50.0, 25.0), radii, CLIP).is_none());
        assert!(visible_arcs(Point::new(5e6, 25.0), radii, CLIP).is_none());
    }

    #[test]
    fn ellipses_crossing_corners_give_each_visible_arc() {
        // Centred on the clip's top-left corner: one quarter is inside.
        let center = Point::new(0.0, 0.0);
        let radii = Vec2::new(30.0, 20.0);
        let path = visible_arcs(center, radii, CLIP).unwrap();
        assert!(on_ellipse(&path, center, radii, 0.05));
        let bounds = path.bounding_box();
        assert!(
            (bounds.x1 - 30.0).abs() < 0.05 && (bounds.y1 - 20.0).abs() < 0.05,
            "{bounds:?}"
        );
        assert!(bounds.x0 > -0.05 && bounds.y0 > -0.05, "{bounds:?}");
    }
}
