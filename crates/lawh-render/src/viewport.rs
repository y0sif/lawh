//! Which part of the board is on screen, and how big.

use kurbo::{Affine, Point, Rect, Vec2};

use crate::BASE_PX_PER_UNIT;

/// Space [`Viewport::fit`] leaves around the bounds, in logical pixels.
const FIT_MARGIN_PX: f64 = 32.0;

/// [`Viewport::fit`] never zooms in past this, so one small object isn't
/// blown up to fill the window.
const FIT_MAX_ZOOM: f64 = 4.0;

/// Most physical pixels per grid unit a viewport may show; past it only the
/// background is drawn. The app's closest zoom (20) at scale factor 4 is
/// 3,200, and a text em is half a unit, so this is a 5,000-pixel em.
const MAX_PX_PER_UNIT: f64 = 10_000.0;

/// Largest window scale factor a viewport may have; past it only the
/// background is drawn. Real displays stay under 5. Stroke widths, arrowheads
/// and the round joins `vello_cpu` builds for them grow with it, so an absurd
/// one would make every outline absurdly costly.
pub(crate) const MAX_SCALE_FACTOR: f64 = 32.0;

/// A view of the board: the board point at the centre of the image, the
/// zoom, the image size in physical pixels and the window scale factor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// Board point shown at the centre of the image, in grid units.
    pub center: Point,
    /// 1 shows [`BASE_PX_PER_UNIT`] logical pixels per grid unit.
    pub zoom: f64,
    /// Image width in physical pixels.
    pub width: u16,
    /// Image height in physical pixels.
    pub height: u16,
    /// Physical pixels per logical pixel (the window's scale factor).
    pub scale_factor: f64,
}

impl Viewport {
    pub fn new(center: Point, zoom: f64, width: u16, height: u16, scale_factor: f64) -> Self {
        Self {
            center,
            zoom,
            width,
            height,
            scale_factor,
        }
    }

    /// Physical pixels per grid unit.
    pub fn px_per_unit(&self) -> f64 {
        BASE_PX_PER_UNIT * self.zoom * self.scale_factor
    }

    /// Board point → physical pixel position, y down.
    pub fn to_screen(&self, p: Point) -> Point {
        ((p - self.center) * self.px_per_unit() + self.screen_center()).to_point()
    }

    /// Physical pixel position → board point.
    pub fn to_board(&self, p: Point) -> Point {
        self.center + (p.to_vec2() - self.screen_center()) / self.px_per_unit()
    }

    /// The board → screen mapping as an affine transform.
    pub fn transform(&self) -> Affine {
        Affine::translate(self.screen_center())
            * Affine::scale(self.px_per_unit())
            * Affine::translate(-self.center.to_vec2())
    }

    /// The part of the board the image shows, in grid units.
    pub fn visible_board_rect(&self) -> Rect {
        Rect::from_points(
            self.to_board(Point::ZERO),
            self.to_board(Point::new(f64::from(self.width), f64::from(self.height))),
        )
    }

    /// A viewport of `width` x `height` physical pixels that shows all of
    /// `bounds` (grid units) centred, with a margin.
    ///
    /// Empty or degenerate bounds give zoom 1; small bounds stop at zoom 4.
    /// The centre and zoom are always finite, and the zoom above zero.
    pub fn fit(bounds: Rect, width: u16, height: u16, scale_factor: f64) -> Viewport {
        Self::fit_with_margin(bounds, width, height, scale_factor, FIT_MARGIN_PX)
    }

    /// [`Self::fit`] with a margin of `margin_px` logical pixels (times the
    /// scale factor) in place of its own, shrunk to a quarter of the image's
    /// width and height when they are smaller. A margin that isn't a number at
    /// or above zero counts as 0.
    pub fn fit_with_margin(
        bounds: Rect,
        width: u16,
        height: u16,
        scale_factor: f64,
        margin_px: f64,
    ) -> Viewport {
        let margin_px = if margin_px > 0.0 { margin_px } else { 0.0 };
        let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        if !bounds.is_finite() {
            return Viewport::new(Point::ZERO, 1.0, width, height, scale_factor);
        }
        let bounds = bounds.abs();
        let center = bounds.center();
        // Halving first can't overflow, for bounds near the largest float.
        let center = if center.is_finite() {
            center
        } else {
            Point::new(
                bounds.x0 / 2.0 + bounds.x1 / 2.0,
                bounds.y0 / 2.0 + bounds.y1 / 2.0,
            )
        };
        let (w, h) = (f64::from(width), f64::from(height));
        let margin = (margin_px * scale_factor).min(w / 4.0).min(h / 4.0);
        let unit = BASE_PX_PER_UNIT * scale_factor;
        let mut zoom = FIT_MAX_ZOOM;
        if bounds.width() > 0.0 {
            zoom = zoom.min((w - 2.0 * margin).max(1.0) / (bounds.width() * unit));
        }
        if bounds.height() > 0.0 {
            zoom = zoom.min((h - 2.0 * margin).max(1.0) / (bounds.height() * unit));
        }
        // Bounds too wide for their width to be a float give a zoom of 0.
        let zoom = if zoom.is_finite() {
            zoom.max(f64::MIN_POSITIVE)
        } else {
            1.0
        };
        Viewport::new(center, zoom, width, height, scale_factor)
    }

    /// True when the numbers give a usable, non-empty image: finite, above
    /// zero, at most [`MAX_PX_PER_UNIT`] and [`MAX_SCALE_FACTOR`], with a
    /// finite board-to-screen transform.
    pub(crate) fn is_drawable(&self) -> bool {
        let ppu = self.px_per_unit();
        self.width > 0
            && self.height > 0
            && ppu.is_finite()
            && ppu > 0.0
            && ppu <= MAX_PX_PER_UNIT
            && self.scale_factor.is_finite()
            && self.scale_factor > 0.0
            && self.scale_factor <= MAX_SCALE_FACTOR
            && self.center.is_finite()
            && self.transform().is_finite()
    }

    fn screen_center(&self) -> Vec2 {
        Vec2::new(f64::from(self.width) / 2.0, f64::from(self.height) / 2.0)
    }
}
