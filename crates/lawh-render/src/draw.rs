//! Shape geometry in screen pixels: outlines, arrowheads and the dot grid.

use kurbo::{BezPath, Circle, Ellipse, Point, Rect, Shape as _, Vec2};

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

/// Room around an object's bounds for its stroke and arrowhead, in physical
/// pixels, used when deciding whether it is on screen.
pub(crate) fn cull_padding_px(vp: &Viewport) -> f64 {
    (ARROW_HEAD_MAX_PX + STROKE_PX) * vp.scale_factor
}

pub(crate) fn stroke_width_px(vp: &Viewport) -> f64 {
    STROKE_PX * vp.scale_factor
}

pub(crate) fn ellipse(vp: &Viewport, center: Point, rx: f64, ry: f64) -> BezPath {
    let s = vp.px_per_unit();
    Ellipse::new(vp.to_screen(center), (rx.abs() * s, ry.abs() * s), 0.0).to_path(TOLERANCE_PX)
}

pub(crate) fn rect(vp: &Viewport, origin: Point, w: f64, h: f64) -> BezPath {
    let s = vp.px_per_unit();
    Rect::from_origin_size(vp.to_screen(origin), (w * s, h * s))
        .abs()
        .to_path(TOLERANCE_PX)
}

pub(crate) fn line(vp: &Viewport, a: Point, b: Point) -> BezPath {
    segment(vp.to_screen(a), vp.to_screen(b))
}

/// An arrow as a shaft to stroke and a head to fill. The shaft stops inside
/// the head so its stroke never pokes past the tip. A zero-length arrow has
/// no head.
pub(crate) fn arrow(vp: &Viewport, a: Point, b: Point) -> (BezPath, Option<BezPath>) {
    let (a, b) = (vp.to_screen(a), vp.to_screen(b));
    let dir = b - a;
    let len = dir.hypot();
    if !(len.is_finite() && len > f64::EPSILON) {
        return (segment(a, b), None);
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
    (segment(a, b - along * (head * 0.5)), Some(tip))
}

/// One path holding a dot at every grid point on screen, or `None` when
/// zoomed out too far for a grid to help.
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

/// A non-negative float count, already capped, as an integer.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "callers cap the value at GRID_MAX_DOTS_PER_AXIS and it is clamped at 0 here"
)]
fn count(n: f64) -> u32 {
    n.max(0.0) as u32
}
