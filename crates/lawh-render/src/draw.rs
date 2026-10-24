//! Shape geometry in screen pixels: outlines, arrowheads, the dot grid and math paths.
//!
//! Every outline is cut down to `clip`, a rectangle a little larger than the image, in `f64`
//! before `vello_cpu` sees it. Shapes can be millions of pixels across at high zoom; cut down,
//! they cost the same as small ones, and `vello_cpu`'s `f32` coordinates stay precise.
//! `vello_cpu` flattens every curve that reaches into the image whole, so an uncut curve
//! billions of pixels long would take gigabytes.

use std::f64::consts::{PI, TAU};

use kurbo::{
    Arc, BezPath, Circle, CubicBez, Ellipse, Line, ParamCurve as _, PathEl, PathSeg, Point,
    QuadBez, Rect, Shape as _, Vec2,
};

use crate::Viewport;

/// Curve flattening tolerance in physical pixels.
pub(crate) const TOLERANCE_PX: f64 = 0.1;

/// Shape outline width in logical pixels, the same at every zoom.
pub(crate) const STROKE_PX: f64 = 2.0;

/// Arrowhead length in grid units, kept between the pixel limits below.
pub(crate) const ARROW_HEAD_UNITS: f64 = 0.3;
pub(crate) const ARROW_HEAD_MIN_PX: f64 = 8.0;
pub(crate) const ARROW_HEAD_MAX_PX: f64 = 24.0;
/// Arrowhead half-width as a fraction of its length.
const ARROW_HEAD_SPREAD: f64 = 0.4;

/// Grid dots are drawn only from this many physical pixels per unit up.
const GRID_MIN_PX_PER_UNIT: f64 = 12.0;
/// Grid dot diameter in logical pixels.
const GRID_DOT_PX: f64 = 1.5;
/// Most grid dots per row or column, a guard against absurd viewports.
const GRID_MAX_DOTS_PER_AXIS: f64 = 4096.0;
/// Most grid dots a frame draws before thinning them (see [`grid_dots`]): about 4 ms at
/// 2880x1800 and scale factor 2, measured in release.
const MAX_GRID_DOTS: f64 = 4000.0;
/// Thinning never skips more than this many grid points in a row.
const GRID_MAX_STEP: f64 = 4096.0;

/// An ellipse whose bounds stay within this many pixels of the clip is drawn
/// whole: `vello_cpu` culls the parts outside, and its `f32` coordinates are
/// precise to well under a hundredth of a pixel this close. Larger ones are
/// cut to their visible arcs first.
const WHOLE_ELLIPSE_REACH_PX: f64 = 65_536.0;

/// How far past the clip a piece of a cut path may reach and still be drawn as a curve: a
/// piece that big flattens into at most a few hundred lines.
const CUT_REACH_PX: f64 = 1024.0;

/// Most halvings while cutting one path. Each halving brings a piece crossing the clip's edge
/// to half the size, so about 60 per crossing take a curve 1e21 pixels across down to
/// [`CUT_REACH_PX`]. Past the budget, pieces are drawn as straight lines, so the cost of a
/// path stays bounded whatever it holds.
const MAX_HALVINGS: u32 = 1 << 16;

/// Room around the image for strokes and arrowheads, in physical pixels: an object is drawn
/// when its bounds reach into the image grown by this much, and outlines are cut at that
/// border, where a stroke's caps and joins can't be seen.
pub(crate) fn cull_padding_px(vp: &Viewport) -> f64 {
    (ARROW_HEAD_MAX_PX + STROKE_PX) * vp.scale_factor
}

/// The screen rectangle objects are cut to: the image grown by [`cull_padding_px`].
pub(crate) fn clip_rect(vp: &Viewport) -> Rect {
    let pad = cull_padding_px(vp);
    Rect::new(0.0, 0.0, f64::from(vp.width), f64::from(vp.height)).inflate(pad, pad)
}

pub(crate) fn stroke_width_px(vp: &Viewport) -> f64 {
    STROKE_PX * vp.scale_factor
}

/// How long an arrowhead is through `vp`, in physical pixels, before an arrow shorter than that
/// cuts it down: [`ARROW_HEAD_UNITS`] kept between the pixel limits.
fn arrow_head_px(vp: &Viewport) -> f64 {
    let sf = vp.scale_factor;
    (ARROW_HEAD_UNITS * vp.px_per_unit())
        .max(ARROW_HEAD_MIN_PX * sf)
        .min(ARROW_HEAD_MAX_PX * sf)
}

/// An arrow from `a` to `b` with a head `head` long, cut down to the arrow's length, in any
/// units: where its shaft ends, inside the head so the shaft's stroke never pokes past the tip,
/// and the head's corners, tip first. `None` for an arrow of no length (or not finite), which
/// has no head. The window ([`arrow`], in screen pixels) and exports (`crate::scene`, in grid
/// units) both build arrows with this.
pub(crate) fn arrow_parts(a: Point, b: Point, head: f64) -> Option<(Point, [Point; 3])> {
    let dir = b - a;
    let len = dir.hypot();
    if !(len.is_finite() && len > f64::EPSILON) {
        return None;
    }
    let head = head.min(len);
    let along = dir / len;
    let across = Vec2::new(-along.y, along.x) * (head * ARROW_HEAD_SPREAD);
    let base = b - along * head;
    Some((b - along * (head * 0.5), [b, base + across, base - across]))
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
    let Some((end, corners)) = arrow_parts(a, b, arrow_head_px(vp)) else {
        return (clip_segment(a, b, clip).map(|(a, b)| segment(a, b)), None);
    };
    let tip = triangle(corners);
    let tip = tip.bounding_box().overlaps(clip).then_some(tip);
    let shaft = clip_segment(a, end, clip).map(|(a, b)| segment(a, b));
    (shaft, tip)
}

/// The closed path through `corners`.
pub(crate) fn triangle(corners: [Point; 3]) -> BezPath {
    let [a, b, c] = corners;
    let mut path = BezPath::new();
    path.move_to(a);
    path.line_to(b);
    path.line_to(c);
    path.close_path();
    path
}

/// `path`, in screen pixels, with everything far from `clip` straightened, so `vello_cpu`
/// never flattens more of a curve than reaches near `clip`. Fill it, or stroke it when `clip`
/// has been grown by the stroke width; inside `clip` the result is the same as for `path`.
///
/// A curve or line that reaches into `clip` and more than [`CUT_REACH_PX`] past it is halved
/// until each piece either stays within that reach or lies outside `clip`. A piece outside
/// becomes a straight line between its ends, both moved to within the reach. Two rectangles
/// that don't overlap are apart along one axis, so the piece and its line lie beyond the same
/// edge of `clip`, and they wind the same way around every point inside it: the fill there is
/// unchanged, and neither one's stroke reaches in. `vello_cpu` then sees curves no larger than
/// `clip` plus the reach, and lines, which cost the same at any length.
///
/// With `close`, an open subpath gets its closing line here, as a fill would close it, so that
/// line is cut down too. A path that doesn't start with a move is malformed and comes back
/// empty.
pub(crate) fn cut_path(path: &BezPath, clip: Rect, close: bool) -> BezPath {
    let Some(PathEl::MoveTo(first)) = path.elements().first() else {
        return BezPath::new();
    };
    let near = clip.inflate(CUT_REACH_PX, CUT_REACH_PX);
    if near.contains_rect(path.control_box()) {
        return path.clone();
    }
    let mut cut = Cut {
        clip,
        near,
        out: BezPath::new(),
        pieces: Vec::new(),
        halvings_left: MAX_HALVINGS,
    };
    let (mut start, mut last) = (*first, *first);
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                if close && last != start {
                    cut.segment(PathSeg::Line(Line::new(last, start)));
                }
                cut.out.move_to(cut.moved_near(p));
                (start, last) = (p, p);
            }
            PathEl::LineTo(p) => {
                cut.segment(PathSeg::Line(Line::new(last, p)));
                last = p;
            }
            PathEl::QuadTo(a, p) => {
                cut.segment(PathSeg::Quad(QuadBez::new(last, a, p)));
                last = p;
            }
            PathEl::CurveTo(a, b, p) => {
                cut.segment(PathSeg::Cubic(CubicBez::new(last, a, b, p)));
                last = p;
            }
            PathEl::ClosePath => {
                if last != start {
                    cut.segment(PathSeg::Line(Line::new(last, start)));
                }
                cut.out.close_path();
                last = start;
            }
        }
    }
    if close && last != start {
        cut.segment(PathSeg::Line(Line::new(last, start)));
    }
    cut.out
}

/// The state of one [`cut_path`].
struct Cut {
    clip: Rect,
    /// `clip` grown by [`CUT_REACH_PX`]: pieces inside it are kept as they are.
    near: Rect,
    out: BezPath,
    /// Pieces of the current segment still to look at, the next one last.
    pieces: Vec<PathSeg>,
    halvings_left: u32,
}

impl Cut {
    /// Appends `segment`, whose start is the end of what `out` holds, cut down.
    fn segment(&mut self, segment: PathSeg) {
        self.pieces.push(segment);
        while let Some(piece) = self.pieces.pop() {
            let bounds = control_box(piece);
            if self.near.contains_rect(bounds) {
                // Its ends are inside `near`, so the line before it, if any, ended exactly
                // where it starts.
                self.out.push(piece.as_path_el());
            } else if bounds.overlaps(self.clip) && self.halvings_left > 0 {
                self.halvings_left -= 1;
                let (a, b) = halves(piece);
                self.pieces.extend([b, a]);
            } else {
                // Outside `clip`, or out of budget.
                let end = self.moved_near(piece.end());
                self.out.line_to(end);
            }
        }
    }

    /// `p` moved to the nearest point of `near`. Never panics, unlike `f64::clamp`.
    fn moved_near(&self, p: Point) -> Point {
        Point::new(
            p.x.max(self.near.x0).min(self.near.x1),
            p.y.max(self.near.y0).min(self.near.y1),
        )
    }
}

/// The smallest rectangle holding a segment's points, control points included; the segment
/// lies inside it.
fn control_box(segment: PathSeg) -> Rect {
    match segment {
        PathSeg::Line(l) => Rect::from_points(l.p0, l.p1),
        PathSeg::Quad(q) => Rect::from_points(q.p0, q.p1).union_pt(q.p2),
        PathSeg::Cubic(c) => Rect::from_points(c.p0, c.p1).union_pt(c.p2).union_pt(c.p3),
    }
}

/// A segment split in two at its parameter midpoint; the ends stay exactly where they were.
fn halves(segment: PathSeg) -> (PathSeg, PathSeg) {
    match segment {
        PathSeg::Line(l) => {
            let mid = l.p0.midpoint(l.p1);
            (
                PathSeg::Line(Line::new(l.p0, mid)),
                PathSeg::Line(Line::new(mid, l.p1)),
            )
        }
        PathSeg::Quad(q) => {
            let (a, b) = q.subdivide();
            (PathSeg::Quad(a), PathSeg::Quad(b))
        }
        PathSeg::Cubic(c) => {
            let (a, b) = c.subdivide();
            (PathSeg::Cubic(a), PathSeg::Cubic(b))
        }
    }
}

/// One path holding a dot at every grid point on screen, and how many dots it holds, or `None`
/// when zoomed out too far for a grid to help.
///
/// Past [`MAX_GRID_DOTS`] dots, only every second grid point is drawn (along each axis), or
/// every fourth, and so on: dots cost about a microsecond each, and a dot every 12 pixels would
/// take tens of milliseconds a frame on a large screen. The thinned dots stay on the same board
/// points as the view moves.
pub(crate) fn grid_dots(vp: &Viewport) -> Option<(BezPath, usize)> {
    if vp.px_per_unit() < GRID_MIN_PX_PER_UNIT {
        return None;
    }
    let visible = vp.visible_board_rect();
    if !visible.is_finite() {
        return None;
    }
    let mut step = 1.0;
    let (cols, rows) = loop {
        let (x0, y0) = ((visible.x0 / step).floor(), (visible.y0 / step).floor());
        let cols = ((visible.x1 / step).ceil() - x0).min(GRID_MAX_DOTS_PER_AXIS);
        let rows = ((visible.y1 / step).ceil() - y0).min(GRID_MAX_DOTS_PER_AXIS);
        if (cols + 1.0) * (rows + 1.0) <= MAX_GRID_DOTS || step >= GRID_MAX_STEP {
            break (cols, rows);
        }
        step *= 2.0;
    };
    let (x0, y0) = (
        (visible.x0 / step).floor() * step,
        (visible.y0 / step).floor() * step,
    );
    let radius = GRID_DOT_PX * vp.scale_factor / 2.0;
    let mut path = BezPath::new();
    let mut dots = 0;
    for row in 0..=count(rows) {
        let y = y0 + f64::from(row) * step;
        for col in 0..=count(cols) {
            let at = vp.to_screen(Point::new(x0 + f64::from(col) * step, y));
            // Centre each dot on a pixel so it stays a crisp single dot.
            let at = Point::new(at.x.floor() + 0.5, at.y.floor() + 0.5);
            path.extend(Circle::new(at, radius).path_elements(TOLERANCE_PX));
            dots += 1;
        }
    }
    Some((path, dots))
}

pub(crate) fn segment(a: Point, b: Point) -> BezPath {
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

    /// `path` and `cut` wind the same way around points spread over `CLIP`, and `cut` stays
    /// near `CLIP`.
    fn assert_cut_matches(path: &BezPath, cut: &BezPath) {
        let near = CLIP.inflate(CUT_REACH_PX, CUT_REACH_PX);
        assert!(
            near.contains_rect(cut.control_box()),
            "{:?}",
            cut.control_box()
        );
        let mut inside = 0;
        for i in 0..100 {
            for j in 0..50 {
                let p = Point::new(f64::from(i) + 0.37, f64::from(j) + 0.61);
                let winding = path.winding(p);
                assert_eq!(cut.winding(p), winding, "at {p:?}");
                inside += usize::from(winding != 0);
            }
        }
        // Each test path crosses the clip, so some points are inside and some aren't, apart
        // from the one around the whole clip.
        assert!(inside > 0, "no point inside");
    }

    #[test]
    fn small_paths_are_not_cut() {
        let path = Circle::new((50.0, 25.0), 500.0).to_path(0.1);
        assert_eq!(cut_path(&path, CLIP, true), path);
        assert_eq!(cut_path(&BezPath::new(), CLIP, true), BezPath::new());
    }

    #[test]
    fn cut_paths_fill_the_same_inside_the_clip() {
        // A circle 1.6e9 px in radius whose edge passes through the clip.
        let circle = Circle::new((50.0 - 1.6e9, 25.0), 1.6e9).to_path(0.1);
        let cut = cut_path(&circle, CLIP, true);
        assert!(cut.elements().len() < circle.elements().len() + 200);
        assert_cut_matches(&circle, &cut);

        // The same circle twice, so the inside winds twice, and a square around the whole clip.
        let mut twice = circle.clone();
        twice.extend(circle.iter());
        assert_cut_matches(&twice, &cut_path(&twice, CLIP, true));
        let around = Rect::new(-1e12, -1e12, 1e12, 1e12).to_path(0.0);
        assert_cut_matches(&around, &cut_path(&around, CLIP, true));

        // A loop crossing itself inside the clip.
        let mut knot = BezPath::new();
        knot.move_to((-1e10, -1e10));
        knot.curve_to((1e11, 3e10), (-1e11, 3e10), (1e10, -1e10));
        knot.close_path();
        assert_cut_matches(&knot, &cut_path(&knot, CLIP, true));
    }

    #[test]
    fn open_paths_are_closed_before_the_cut_when_filled() {
        // A triangle open on its long side, x + y = 60, which crosses the clip diagonally.
        // Closing it after the cut, between its corners moved near the clip, would close it
        // along another diagonal.
        let mut open = BezPath::new();
        open.move_to((-1e9, 1e9 + 60.0));
        open.line_to((-1e9, -1e9));
        open.line_to((1e9 + 60.0, -1e9));
        let mut closed = open.clone();
        closed.close_path();
        assert_cut_matches(&closed, &cut_path(&open, CLIP, true));

        // A stroke keeps it open: nothing but the lines outside the clip.
        let stroked = cut_path(&open, CLIP, false);
        let near = CLIP.inflate(CUT_REACH_PX, CUT_REACH_PX);
        assert!(stroked.segments().all(|seg| {
            let bounds = seg.bounding_box();
            near.contains_rect(bounds) && !bounds.overlaps(CLIP)
        }));
    }

    #[test]
    fn curves_far_too_big_for_vello_are_cut_to_a_few_pieces() {
        // The verifier's repro as a 2e21 px arc (35 elements cut to 153), and one near the
        // largest float, which takes about a thousand halvings per crossing.
        for (radius, most_added) in [(2e21, 200), (1e300, 2 * MAX_HALVINGS as usize)] {
            let circle = Circle::new((50.0 - radius, 25.0), radius).to_path(radius * 1e-9);
            let cut = cut_path(&circle, CLIP, true);
            let near = CLIP.inflate(CUT_REACH_PX, CUT_REACH_PX);
            assert!(near.contains_rect(cut.control_box()), "{radius}");
            let added = cut.elements().len() - circle.elements().len();
            assert!(added < most_added, "{radius}: {added} elements added");
            // The visible arc survives as curves crossing the clip.
            assert!(
                cut.segments().any(
                    |seg| matches!(seg, PathSeg::Cubic(_)) && seg.bounding_box().overlaps(CLIP)
                ),
                "{radius}"
            );
        }
    }

    #[test]
    fn dense_grids_are_thinned_to_whole_steps() {
        // 40 px per unit: every grid point, 49 x 28 of them.
        let vp = Viewport::new(Point::new(0.3, 0.2), 1.0, 1920, 1080, 1.0);
        let (_, dots) = grid_dots(&vp).unwrap();
        assert!((1300..=1500).contains(&dots), "{dots}");
        // 12 px per unit at 2880x1800: 36,000 grid points, thinned to every fourth.
        for center in [Point::ZERO, Point::new(1.7, -3.2)] {
            let vp = Viewport::new(center, 0.15, 2880, 1800, 2.0);
            let (path, dots) = grid_dots(&vp).unwrap();
            assert!(dots >= 1000 && dots as f64 <= MAX_GRID_DOTS, "{dots}");
            // Every dot sits on a board point whose coordinates are multiples of 4, so the
            // thinned grid doesn't move as the view pans. A dot's outline starts at its right
            // edge, and dots are snapped to pixel centres: within 2 px, a sixth of a unit.
            for el in path.elements() {
                if let PathEl::MoveTo(p) = el {
                    let board = vp.to_board(*p);
                    for v in [board.x, board.y] {
                        let off = (v / 4.0 - (v / 4.0).round()).abs() * 4.0;
                        assert!(off < 0.25, "{board:?}");
                    }
                }
            }
        }
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
