//! Pan and zoom maths on a [`Viewport`], kept free of the window so it can be tested.
//!
//! Points here are physical pixel positions in the window, y down, as winit reports them.

use lawh_core::Point;
use lawh_render::Viewport;
use winit::event::MouseScrollDelta;

/// Furthest out the wheel zooms.
pub const MIN_ZOOM: f64 = 0.1;
/// Furthest in the wheel zooms.
pub const MAX_ZOOM: f64 = 20.0;

/// Zoom factor for one wheel notch (one line).
const ZOOM_PER_LINE: f64 = 1.1;

/// Logical pixels of touchpad scrolling that count as one wheel line.
const PIXELS_PER_LINE: f64 = 50.0;

/// Moves the view so the board point that was under `from` is now under `to`.
pub fn pan(vp: &mut Viewport, from: Point, to: Point) {
    let moved = (to - from) / vp.px_per_unit();
    if moved.is_finite() {
        vp.center -= moved;
    }
}

/// Multiplies the zoom by `factor`, keeping the board point under `at` where it is.
///
/// The result stays within [[`MIN_ZOOM`], [`MAX_ZOOM`]]. A view that starts outside that range
/// (a fit to a huge board) is never pushed further out, and never jumps in on a zoom-out.
pub fn zoom_at(vp: &mut Viewport, at: Point, factor: f64) {
    if !(factor.is_finite() && factor > 0.0) {
        return;
    }
    let anchor = vp.to_board(at);
    let zoom = (vp.zoom * factor).clamp(MIN_ZOOM.min(vp.zoom), MAX_ZOOM.max(vp.zoom));
    if !anchor.is_finite() {
        return;
    }
    vp.zoom = zoom;
    let screen_center = Point::new(f64::from(vp.width) / 2.0, f64::from(vp.height) / 2.0);
    vp.center = anchor - (at - screen_center) / vp.px_per_unit();
}

/// The zoom factor for one wheel or touchpad scroll event: [`ZOOM_PER_LINE`] per line, scrolling
/// up zooms in. Pixel deltas (touchpads) are physical pixels, so they are divided by the scale
/// factor before being counted in lines.
pub fn wheel_zoom_factor(delta: MouseScrollDelta, scale_factor: f64) -> f64 {
    let lines = match delta {
        MouseScrollDelta::LineDelta(_, y) => f64::from(y),
        MouseScrollDelta::PixelDelta(pos) => {
            let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
                scale_factor
            } else {
                1.0
            };
            pos.y / scale / PIXELS_PER_LINE
        }
    };
    let factor = ZOOM_PER_LINE.powf(lines);
    if factor.is_finite() && factor > 0.0 {
        factor
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalPosition;

    use super::*;

    fn viewport() -> Viewport {
        Viewport::new(Point::new(3.0, -2.0), 1.5, 800, 600, 2.0)
    }

    fn assert_near(a: Point, b: Point) {
        assert!((a - b).hypot() < 1e-9, "{a:?} is not {b:?}");
    }

    #[test]
    fn pan_moves_the_grabbed_point_to_the_cursor() {
        let mut vp = viewport();
        let (from, to) = (Point::new(100.0, 200.0), Point::new(340.0, 150.0));
        let grabbed = vp.to_board(from);
        pan(&mut vp, from, to);
        assert_near(vp.to_screen(grabbed), to);
        assert!((vp.zoom - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        for factor in [1.1, 1.1_f64.powi(5), 1.0 / 1.1, 0.5] {
            let mut vp = viewport();
            let cursor = Point::new(123.0, 456.0);
            let anchor = vp.to_board(cursor);
            zoom_at(&mut vp, cursor, factor);
            assert!((vp.zoom - 1.5 * factor).abs() < 1e-12);
            assert_near(vp.to_screen(anchor), cursor);
        }
    }

    #[test]
    fn zoom_is_clamped_and_still_keeps_the_point() {
        let cursor = Point::new(700.0, 20.0);

        let mut vp = viewport();
        let anchor = vp.to_board(cursor);
        zoom_at(&mut vp, cursor, 1000.0);
        assert!((vp.zoom - MAX_ZOOM).abs() < f64::EPSILON);
        assert_near(vp.to_screen(anchor), cursor);

        let mut vp = viewport();
        let anchor = vp.to_board(cursor);
        zoom_at(&mut vp, cursor, 0.0001);
        assert!((vp.zoom - MIN_ZOOM).abs() < f64::EPSILON);
        assert_near(vp.to_screen(anchor), cursor);
    }

    #[test]
    fn zoom_from_outside_the_range_never_goes_the_wrong_way() {
        let mut vp = viewport();
        vp.zoom = 0.05;
        zoom_at(&mut vp, Point::new(10.0, 10.0), 1.0 / 1.1);
        assert!(
            (vp.zoom - 0.05).abs() < f64::EPSILON,
            "zoom out must not zoom in"
        );
        zoom_at(&mut vp, Point::new(10.0, 10.0), 1.1);
        assert!(vp.zoom > 0.05);
    }

    #[test]
    fn bad_factors_change_nothing() {
        for factor in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut vp = viewport();
            zoom_at(&mut vp, Point::new(1.0, 2.0), factor);
            assert_eq!(vp, viewport());
        }
    }

    #[test]
    fn wheel_up_zooms_in_one_step_per_line() {
        let up = wheel_zoom_factor(MouseScrollDelta::LineDelta(0.0, 1.0), 1.0);
        let down = wheel_zoom_factor(MouseScrollDelta::LineDelta(0.0, -1.0), 1.0);
        assert!((up - 1.1).abs() < 1e-12);
        assert!((up * down - 1.0).abs() < 1e-12);
    }

    #[test]
    fn pixel_deltas_count_logical_pixels() {
        let line_at_1x = MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, PIXELS_PER_LINE));
        let line_at_2x =
            MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 2.0 * PIXELS_PER_LINE));
        assert!((wheel_zoom_factor(line_at_1x, 1.0) - 1.1).abs() < 1e-12);
        assert!((wheel_zoom_factor(line_at_2x, 2.0) - 1.1).abs() < 1e-12);
        assert!((wheel_zoom_factor(line_at_1x, 0.0) - 1.1).abs() < 1e-12);
    }
}
