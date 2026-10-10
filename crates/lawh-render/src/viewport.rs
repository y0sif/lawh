//! Which part of the board is on screen, and how big.

use kurbo::{Affine, Point, Rect, Vec2};

use crate::BASE_PX_PER_UNIT;

/// Space [`Viewport::fit`] leaves around the bounds, in logical pixels.
const FIT_MARGIN_PX: f64 = 32.0;

/// [`Viewport::fit`] never zooms in past this, so one small object isn't
/// blown up to fill the window.
const FIT_MAX_ZOOM: f64 = 4.0;

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
    pub fn fit(bounds: Rect, width: u16, height: u16, scale_factor: f64) -> Viewport {
        let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        if !bounds.is_finite() {
            return Viewport::new(Point::ZERO, 1.0, width, height, scale_factor);
        }
        let bounds = bounds.abs();
        let (w, h) = (f64::from(width), f64::from(height));
        let margin = (FIT_MARGIN_PX * scale_factor).min(w / 4.0).min(h / 4.0);
        let unit = BASE_PX_PER_UNIT * scale_factor;
        let mut zoom = FIT_MAX_ZOOM;
        if bounds.width() > 0.0 {
            zoom = zoom.min((w - 2.0 * margin).max(1.0) / (bounds.width() * unit));
        }
        if bounds.height() > 0.0 {
            zoom = zoom.min((h - 2.0 * margin).max(1.0) / (bounds.height() * unit));
        }
        if !(zoom.is_finite() && zoom > 0.0) {
            zoom = 1.0;
        }
        Viewport::new(bounds.center(), zoom, width, height, scale_factor)
    }

    /// True when the numbers give a usable, non-empty image.
    pub(crate) fn is_drawable(&self) -> bool {
        let ppu = self.px_per_unit();
        self.width > 0
            && self.height > 0
            && ppu.is_finite()
            && ppu > 0.0
            && self.scale_factor.is_finite()
            && self.scale_factor > 0.0
            && self.center.is_finite()
    }

    fn screen_center(&self) -> Vec2 {
        Vec2::new(f64::from(self.width) / 2.0, f64::from(self.height) / 2.0)
    }
}
