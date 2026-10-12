//! Pan and zoom maths on a [`Viewport`], kept free of the window so it can be tested.
//!
//! Points here are physical pixel positions in the window, y down, as winit reports them.

use lawh_core::{Point, Rect};
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

/// Room [`follow`] keeps between what the cursor is on and the window's edges, in logical
/// pixels: a little more than one grid unit at zoom 1, so the cursor's outline and what is next
/// to it stay in view.
const FOLLOW_MARGIN_PX: f64 = 48.0;

/// The view a board opens with: all of it fitted to the window above the bottom `bottom_px`
/// physical pixels (the status bar), [`FOLLOW_MARGIN_PX`] inside the edges (see
/// [`Viewport::fit_with_margin`]); for an empty board the grid's origin at the centre, at zoom 1.
/// `bounds` are the board's, in grid units.
///
/// It leaves the room [`follow`] keeps, so the first key finds the whole board, and the object
/// the cursor starts on, already in view and doesn't pan.
pub fn opening(
    bounds: Option<Rect>,
    width: u16,
    height: u16,
    scale_factor: f64,
    bottom_px: u32,
) -> Viewport {
    let Some(bounds) = bounds else {
        return Viewport::new(Point::ZERO, 1.0, width, height, scale_factor);
    };
    let room = u32::from(height).saturating_sub(bottom_px);
    let room = u16::try_from(room).unwrap_or(height);
    let fit = Viewport::fit_with_margin(bounds, width, room, scale_factor, FOLLOW_MARGIN_PX);
    // The fit centres the board in the room above the bar, whose middle is half the bar's
    // height above the window's.
    let bar = f64::from(height - room);
    let center = Point::new(fit.center.x, fit.center.y + bar / 2.0 / fit.px_per_unit());
    let center = if center.is_finite() {
        center
    } else {
        fit.center
    };
    Viewport::new(center, fit.zoom, width, height, fit.scale_factor)
}

/// The part of the board `vp` shows above the bottom `bottom_px` physical pixels (the status
/// bar), in grid units: where jump labels go (`Engine::set_view`). A window with no room above
/// the bar shows only the line along its top.
pub fn board_area(vp: &Viewport, bottom_px: f64) -> Rect {
    let room = (f64::from(vp.height) - bottom_px.max(0.0)).max(0.0);
    Rect::from_points(
        vp.to_board(Point::ZERO),
        vp.to_board(Point::new(f64::from(vp.width), room)),
    )
}

/// Pans `vp` as little as possible to bring `focus` (grid units) into view, at least
/// [`FOLLOW_MARGIN_PX`] inside the window's edges, above the bottom `bottom_px` physical pixels
/// (the status bar). The zoom never changes.
///
/// A focus too big for the room has its top-left corner brought into view instead. A tiny
/// window gets a smaller margin; a focus or view that isn't finite changes nothing.
pub fn follow(vp: &mut Viewport, focus: Rect, bottom_px: f64) {
    let ppu = vp.px_per_unit();
    if !(focus.is_finite() && ppu.is_finite() && ppu > 0.0 && vp.center.is_finite()) {
        return;
    }
    let scale = if vp.scale_factor.is_finite() && vp.scale_factor > 0.0 {
        vp.scale_factor
    } else {
        1.0
    };
    let width = f64::from(vp.width);
    let height = f64::from(vp.height) - bottom_px.max(0.0);
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let margin = (FOLLOW_MARGIN_PX * scale)
        .min(width / 4.0)
        .min(height / 4.0);
    let a = vp.to_screen(Point::new(focus.x0, focus.y0));
    let b = vp.to_screen(Point::new(focus.x1, focus.y1));
    // Less than half a pixel is no pan: it would only be rounding from the last one.
    let whole = |d: f64| if d.abs() < 0.5 { 0.0 } else { d };
    let dx = whole(into_view(
        a.x.min(b.x),
        a.x.max(b.x),
        margin,
        width - margin,
    ));
    let dy = whole(into_view(
        a.y.min(b.y),
        a.y.max(b.y),
        margin,
        height - margin,
    ));
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    let center = vp.center - Point::new(dx, dy).to_vec2() / ppu;
    if center.is_finite() {
        vp.center = center;
    }
}

/// How far to move the span `from..to` (screen pixels, one axis) to bring it into `lo..hi`:
/// just enough, or, for a span longer than the room, enough to bring its start into it.
fn into_view(from: f64, to: f64, lo: f64, hi: f64) -> f64 {
    if to - from > hi - lo {
        if (lo..=hi).contains(&from) {
            0.0
        } else {
            lo - from
        }
    } else if from < lo {
        lo - from
    } else if to > hi {
        hi - to
    } else {
        0.0
    }
}

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
    fn an_empty_board_opens_at_the_origin_at_zoom_1() {
        let vp = opening(None, 800, 600, 2.0, 48);
        assert_eq!(vp, Viewport::new(Point::ZERO, 1.0, 800, 600, 2.0));
    }

    #[test]
    fn a_board_opens_whole_above_the_status_bar_and_the_first_follow_keeps_it() {
        let boards = [
            // Wider than the window: the fit is set by the width.
            Rect::new(10.0, 10.0, 90.0, 20.0),
            // Taller than it.
            Rect::new(-5.0, -50.0, 0.0, 50.0),
            // Small: the fit stops at its largest zoom.
            Rect::new(1.0, 1.0, 2.0, 1.5),
        ];
        for bounds in boards {
            for (scale, bar) in [(1.0, 24), (2.0, 48)] {
                let vp = opening(Some(bounds), 800, 600, scale, bar);
                assert!((vp.scale_factor - scale).abs() < f64::EPSILON);
                // All of it is in the room above the bar, the margin inside its edges.
                let margin = FOLLOW_MARGIN_PX * scale - 1e-6;
                let (a, b) = (
                    vp.to_screen(Point::new(bounds.x0, bounds.y0)),
                    vp.to_screen(Point::new(bounds.x1, bounds.y1)),
                );
                assert!(a.x >= margin && a.y >= margin, "{bounds:?} {a:?}");
                assert!(b.x <= 800.0 - margin, "{bounds:?} {b:?}");
                assert!(b.y <= 600.0 - f64::from(bar) - margin, "{bounds:?} {b:?}");
                // Centred in that room.
                let mid = vp.to_screen(bounds.center());
                assert!((mid.x - 400.0).abs() < 1e-6, "{mid:?}");
                assert!(
                    (mid.y - (600.0 - f64::from(bar)) / 2.0).abs() < 1e-6,
                    "{mid:?}"
                );
                // Following the whole board, or any corner of it, doesn't pan.
                for focus in [
                    bounds,
                    point(bounds.x0, bounds.y0),
                    point(bounds.x1, bounds.y0),
                    point(bounds.x0, bounds.y1),
                    point(bounds.x1, bounds.y1),
                ] {
                    let mut followed = vp;
                    follow(&mut followed, focus, f64::from(bar));
                    assert_eq!(followed, vp, "{bounds:?} {focus:?} at {scale}");
                }
            }
        }
    }

    /// A 800x600 view at scale factor 1 and zoom 1 (40 px a unit) on the origin, with a 24 px
    /// status bar: the margin is 48 px, so what may be seen without a pan is x from -8.8 to
    /// 8.8 units and y from -6.3 to 5.7.
    fn origin_view() -> Viewport {
        Viewport::new(Point::ZERO, 1.0, 800, 600, 1.0)
    }

    const BAR: f64 = 24.0;

    fn point(x: f64, y: f64) -> Rect {
        Rect::new(x, y, x, y)
    }

    fn assert_rect_near(a: Rect, b: Rect) {
        assert_near(Point::new(a.x0, a.y0), Point::new(b.x0, b.y0));
        assert_near(Point::new(a.x1, a.y1), Point::new(b.x1, b.y1));
    }

    #[test]
    fn the_board_area_is_the_window_above_the_status_bar() {
        // 40 px a unit: 20 by 15 units, less the bar's 0.6 at the bottom.
        assert_rect_near(
            board_area(&origin_view(), BAR),
            Rect::new(-10.0, -7.5, 10.0, 6.9),
        );
        // Panned, zoomed in and at scale factor 2: 160 px a unit, the bar 48 px.
        let vp = Viewport::new(Point::new(30.0, -4.0), 2.0, 800, 600, 2.0);
        assert_rect_near(board_area(&vp, 48.0), Rect::new(27.5, -5.875, 32.5, -2.425));
        // No room above the bar: the top line of the window.
        assert_rect_near(
            board_area(&origin_view(), 900.0),
            Rect::new(-10.0, -7.5, 10.0, -7.5),
        );
        assert_rect_near(
            board_area(&origin_view(), f64::NAN),
            Rect::new(-10.0, -7.5, 10.0, 7.5),
        );
    }

    #[test]
    fn a_focus_in_view_does_not_move_it() {
        for focus in [
            point(0.0, 0.0),
            point(8.8, 5.7),
            point(-8.8, -6.3),
            Rect::new(-3.0, -2.0, 4.0, 3.0),
        ] {
            let mut vp = origin_view();
            follow(&mut vp, focus, BAR);
            assert_eq!(vp, origin_view(), "{focus:?}");
        }
    }

    #[test]
    fn the_view_pans_just_enough() {
        let mut vp = origin_view();
        follow(&mut vp, point(20.0, 0.0), BAR);
        assert_near(
            vp.to_screen(Point::new(20.0, 0.0)),
            Point::new(752.0, 300.0),
        );

        let mut vp = origin_view();
        follow(&mut vp, point(-20.0, -30.0), BAR);
        assert_near(
            vp.to_screen(Point::new(-20.0, -30.0)),
            Point::new(48.0, 48.0),
        );

        // The bottom edge is the status bar's top, less the margin.
        let mut vp = origin_view();
        follow(&mut vp, Rect::new(1.0, 9.0, 2.0, 10.0), BAR);
        assert_near(
            vp.to_screen(Point::new(1.0, 10.0)),
            Point::new(440.0, 528.0),
        );
        assert!((vp.zoom - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_focus_bigger_than_the_window_shows_its_top_left() {
        let big = Rect::new(30.0, 40.0, 100.0, 140.0);
        let mut vp = origin_view();
        follow(&mut vp, big, BAR);
        assert_near(vp.to_screen(Point::new(30.0, 40.0)), Point::new(48.0, 48.0));
        // Once its corner is in view, another follow leaves it.
        let before = vp;
        follow(&mut vp, big, BAR);
        assert_eq!(vp, before);
    }

    #[test]
    fn the_margin_scales_and_shrinks_in_a_tiny_window() {
        let mut vp = Viewport::new(Point::ZERO, 1.0, 800, 600, 2.0);
        follow(&mut vp, point(20.0, 0.0), 48.0);
        assert_near(
            vp.to_screen(Point::new(20.0, 0.0)),
            Point::new(704.0, 300.0),
        );

        let mut vp = Viewport::new(Point::ZERO, 1.0, 40, 40, 1.0);
        follow(&mut vp, point(5.0, 5.0), 24.0);
        // 16 px of room above the bar: a 4 px margin.
        assert_near(vp.to_screen(Point::new(5.0, 5.0)), Point::new(36.0, 12.0));
    }

    #[test]
    fn nothing_moves_for_what_cannot_be_followed() {
        for focus in [
            point(f64::NAN, 0.0),
            Rect::new(0.0, 0.0, f64::INFINITY, 1.0),
        ] {
            let mut vp = origin_view();
            follow(&mut vp, focus, BAR);
            assert_eq!(vp, origin_view(), "{focus:?}");
        }
        let mut vp = origin_view();
        follow(&mut vp, point(100.0, 100.0), 600.0);
        assert_eq!(vp, origin_view(), "no room above the bar");
        let mut vp = Viewport::new(Point::ZERO, 0.0, 800, 600, 1.0);
        follow(&mut vp, point(100.0, 100.0), BAR);
        assert_eq!(vp.center, Point::ZERO, "zoom 0");
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
