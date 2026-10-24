//! PNG export: the frame drawn with `vello_cpu` at the export's scale, then encoded with the
//! `png` crate (default compression, sRGB chunk).

use std::io;

use kurbo::{Affine, BezPath, Cap, Join, Rect, Stroke};
use lawh_core::export::{MAX_PNG_SCALE_HUNDREDTHS, MIN_PNG_SCALE_HUNDREDTHS};
use lawh_core::{Board, ExportBackground, ExportOptions, ObjectId};
use vello_cpu::peniko::ImageAlphaType;
use vello_cpu::{Glyph, Pixels, Pixmap, RenderContext, Resources};

use super::{ExportError, ExportLimits, ExportPlan, ExportSink};
use crate::budget::{self, Budget, Estimate, MIN_VISIBLE_PX};
use crate::draw::{self, TOLERANCE_PX};
use crate::renderer::{MAX_GLYPH_EM_PX, stroke_px};
use crate::scene::{Glyphs, Item, Run, Sink, Tools, uniform_scale};
use crate::{BASE_PX_PER_UNIT, Renderer};

/// Draws `plan` into a PNG at `options`' scale, shrunk to fit the pixel caps, and encodes it;
/// with the factor it shrank by, if it did.
pub(super) fn export(
    renderer: &mut Renderer,
    board: &Board,
    plan: &mut ExportPlan,
    options: ExportOptions,
    limits: &ExportLimits,
    mut budget: Budget,
) -> Result<(Vec<u8>, Option<f64>), ExportError> {
    let hundredths = options
        .png_scale_hundredths
        .clamp(MIN_PNG_SCALE_HUNDREDTHS, MAX_PNG_SCALE_HUNDREDTHS);
    let full = BASE_PX_PER_UNIT * f64::from(hundredths) / 100.0;
    let frame = plan.frame;
    let size = fit(frame, full, limits).ok_or(ExportError::Empty)?;
    let shrunk = (size.px_per_unit < full).then(|| size.px_per_unit / full);
    if !budget.spend(budget::image(
        f64::from(size.width) * f64::from(size.height),
    )) {
        return Err(ExportError::OverBudget);
    }
    // The frame centred in the image, which is a whole number of pixels.
    let room = (
        (f64::from(size.width) - frame.width() * size.px_per_unit) / 2.0,
        (f64::from(size.height) - frame.height() * size.px_per_unit) / 2.0,
    );
    let to_px = Affine::translate(room)
        * Affine::scale(size.px_per_unit)
        * Affine::translate((-frame.x0, -frame.y0));
    let mut target = PngTarget::new(size, to_px, budget);
    if options.background == ExportBackground::Paper {
        target.ctx.set_paint(renderer.palette.paper);
        target.ctx.fill_rect(&target.clip);
    }
    renderer.draw_export(board, plan, &mut target)?;
    let pixmap = target.finish();
    let bytes = encode(pixmap, limits.max_bytes)?;
    Ok((bytes, shrunk))
}

/// An image's size in pixels and its scale.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Size {
    width: u16,
    height: u16,
    px_per_unit: f64,
}

/// The size of an image of `frame` at `px_per_unit`, or at the largest scale below it that fits
/// the caps of `limits`; `None` for a frame with no size, or one no scale fits.
fn fit(frame: Rect, px_per_unit: f64, limits: &ExportLimits) -> Option<Size> {
    let (w, h) = (frame.width(), frame.height());
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 && px_per_unit > 0.0) {
        return None;
    }
    let side = f64::from(limits.max_side_px);
    let pixels = limits.max_pixels as f64;
    let sized = |scale: f64| {
        let (width, height) = ((w * scale).ceil().max(1.0), (h * scale).ceil().max(1.0));
        (width <= side && height <= side && width * height <= pixels).then(|| Size {
            width: whole_px(width),
            height: whole_px(height),
            px_per_unit: scale,
        })
    };
    if let Some(size) = sized(px_per_unit) {
        return Some(size);
    }
    // Each side within the side cap, and (w s + 1)(h s + 1) within the pixel cap, which the
    // rounded-up sides are below.
    let wh = w * h;
    let pixel_scale =
        (-(w + h) + ((w + h).powi(2) + 4.0 * wh * (pixels - 1.0)).sqrt()) / (2.0 * wh);
    let mut scale = (side / w).min(side / h).min(pixel_scale).min(px_per_unit);
    // Rounding can leave the sides a pixel over: a little less until they fit.
    for _ in 0..64 {
        if !(scale.is_finite() && scale > 0.0) {
            return None;
        }
        if let Some(size) = sized(scale) {
            return Some(size);
        }
        scale *= 0.999;
    }
    None
}

/// A number of pixels already within the side cap (at most 16,384) as a `u16`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the callers keep it between 1 and the side cap, inside u16"
)]
fn whole_px(px: f64) -> u16 {
    px.clamp(1.0, f64::from(u16::MAX)) as u16
}

/// What a PNG is drawn with: a `vello_cpu` context and image of its own (the window's are left
/// alone), the board-to-pixel transform, and the export's budget, which every item is estimated
/// against before it is drawn.
struct PngTarget {
    ctx: RenderContext,
    resources: Resources,
    pixmap: Pixmap,
    to_px: Affine,
    px_per_unit: f64,
    /// The whole image, in pixels.
    clip: Rect,
    budget: Budget,
    /// The estimate of the object being drawn, and how much of it is charged already.
    cost: Estimate,
    charged: f64,
}

impl PngTarget {
    fn new(size: Size, to_px: Affine, budget: Budget) -> Self {
        let clip = Rect::new(0.0, 0.0, f64::from(size.width), f64::from(size.height));
        Self {
            ctx: RenderContext::new(size.width, size.height),
            resources: Resources::new(),
            pixmap: Pixmap::new(size.width, size.height),
            to_px,
            px_per_unit: size.px_per_unit,
            clip,
            budget,
            cost: Estimate::new(clip, size.width, size.height, 0.0),
            charged: 0.0,
        }
    }

    /// Takes what the object's estimate has grown by since it was last charged from the budget;
    /// [`ExportError::OverBudget`] when that doesn't fit.
    fn settle(&mut self) -> Result<(), ExportError> {
        let more = self.cost.ns - self.charged;
        if self.cost.over() || !self.budget.spend(more) {
            return Err(ExportError::OverBudget);
        }
        self.charged = self.cost.ns;
        Ok(())
    }

    /// Renders everything drawn into the image and hands the image over.
    fn finish(mut self) -> Pixmap {
        self.ctx.flush();
        self.ctx.render(&mut self.pixmap, &mut self.resources);
        self.pixmap
    }
}

impl ExportSink for PngTarget {
    fn budget(&mut self) -> &mut Budget {
        &mut self.budget
    }
}

impl Sink for PngTarget {
    type Error = ExportError;

    fn begin(&mut self, _: &mut Tools<'_>, _: &ObjectId, _: Rect) -> Result<(), ExportError> {
        let (width, height) = (self.pixmap.width(), self.pixmap.height());
        self.cost = Estimate::new(self.clip, width, height, self.budget.left());
        self.charged = 0.0;
        self.settle()
    }

    fn item(&mut self, tools: &mut Tools<'_>, item: &Item<'_>) -> Result<(), ExportError> {
        match item {
            Item::Stroke {
                outline,
                width,
                paint,
            } => {
                let path = outline.to_path(self.to_px, TOLERANCE_PX);
                let width = width.abs() * self.px_per_unit;
                self.cost.stroke(&path, width);
                self.settle()?;
                self.ctx.set_transform(Affine::IDENTITY);
                self.ctx.set_paint(tools.palette.color(*paint));
                self.ctx.set_stroke(
                    Stroke::new(width)
                        .with_caps(Cap::Round)
                        .with_join(Join::Round),
                );
                self.ctx.stroke_path(&path);
            }
            Item::Fill { corners, paint } => {
                let path = draw::triangle(corners.map(|corner| self.to_px * corner));
                self.cost.fill(&path);
                self.settle()?;
                self.ctx.set_transform(Affine::IDENTITY);
                self.ctx.set_paint(tools.palette.color(*paint));
                self.ctx.fill_path(&path);
            }
            Item::Glyphs(run) => self.glyphs(tools, run)?,
            Item::Path {
                path,
                to_board,
                fill,
                stroke,
            } => {
                let transform = self.to_px * *to_board;
                let stroke = stroke_px(*stroke, uniform_scale(transform));
                self.cost.bound(path);
                if fill.is_some() {
                    self.cost.path(path, transform, None);
                }
                if let Some((width, _)) = stroke {
                    self.cost.path(path, transform, Some(width));
                }
                self.settle()?;
                // In pixels before `vello_cpu` sees it, as the window draws math paths.
                let mut px = BezPath::clone(path);
                px.apply_affine(transform);
                self.ctx.set_transform(Affine::IDENTITY);
                if let Some(fill) = fill {
                    self.ctx.set_paint(tools.palette.color(*fill));
                    self.ctx.fill_path(&px);
                }
                if let Some((width, paint)) = stroke {
                    self.ctx.set_paint(tools.palette.color(paint));
                    self.ctx.set_stroke(Stroke::new(width));
                    self.ctx.stroke_path(&px);
                }
            }
        }
        Ok(())
    }
}

impl PngTarget {
    /// Draws a glyph run, unless its glyphs are too small to see or too large to draw (as on
    /// screen).
    fn glyphs(&mut self, tools: &mut Tools<'_>, run: &Glyphs<'_>) -> Result<(), ExportError> {
        let transform = self.to_px * run.to_board;
        let em_px = (run.size * uniform_scale(transform)).abs();
        if !(MIN_VISIBLE_PX..=MAX_GLYPH_EM_PX).contains(&em_px) {
            return Ok(());
        }
        self.cost.run();
        for glyph in run.glyphs() {
            if self.cost.over() {
                break;
            }
            let stats = tools.glyphs.stats(run.font, glyph.id);
            self.cost.glyph(Some((stats, em_px)));
        }
        self.settle()?;
        self.ctx.set_transform(transform);
        self.ctx.set_paint(tools.palette.color(run.paint));
        let drawn = match &run.run {
            Run::Text { glyphs, .. } => {
                let font = glyphs.run();
                let coords = bytemuck::cast_slice::<_, i16>(font.normalized_coords());
                self.ctx
                    .glyph_run(&mut self.resources, font.font())
                    .font_size(font.font_size())
                    .hint(false)
                    .normalized_coords(coords)
                    .fill_glyphs(glyphs.positioned_glyphs().map(|glyph| Glyph {
                        id: glyph.id,
                        x: glyph.x,
                        y: glyph.y,
                    }))
            }
            Run::Math(_) => {
                let Some(font) = tools.fonts.get(run.font) else {
                    tracing::warn!(font = run.font, "math glyphs reference an unknown font");
                    return Ok(());
                };
                self.ctx
                    .glyph_run(&mut self.resources, font)
                    .font_size(to_f32(run.size))
                    .hint(false)
                    .fill_glyphs(run.glyphs().map(|glyph| Glyph {
                        id: glyph.id,
                        x: to_f32(glyph.x),
                        y: to_f32(glyph.y),
                    }))
            }
        };
        if let Err(err) = drawn {
            tracing::debug!(?err, "some glyphs failed to draw for export");
        }
        Ok(())
    }
}

/// Encodes `pixmap` as PNG: RGB when every pixel is opaque, else RGBA (not premultiplied), 8
/// bits a channel, the default compression and an sRGB chunk. Moves the image's pixels out of
/// `pixmap` instead of copying them. [`ExportError::TooLarge`] once the file passes
/// `max_bytes`.
fn encode(pixmap: Pixmap, max_bytes: usize) -> Result<Vec<u8>, ExportError> {
    let (width, height) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
    let (color, pixels) = match pixmap.try_take_rgb8(ImageAlphaType::Alpha) {
        Pixels::Rgb8(pixels) => (::png::ColorType::Rgb, pixels),
        Pixels::Rgba8(pixels) => (::png::ColorType::Rgba, pixels),
    };
    let mut out = Capped::new(max_bytes);
    let written = write_png(&mut out, width, height, color, &pixels);
    match (written, out.reached) {
        (_, Some(bytes)) => Err(ExportError::TooLarge { bytes }),
        (Err(err), None) => Err(ExportError::Png(err.to_string())),
        (Ok(()), None) => Ok(out.bytes),
    }
}

fn write_png(
    out: &mut Capped,
    width: u32,
    height: u32,
    color: ::png::ColorType,
    pixels: &[u8],
) -> Result<(), ::png::EncodingError> {
    let mut encoder = ::png::Encoder::new(out, width, height);
    encoder.set_color(color);
    encoder.set_depth(::png::BitDepth::Eight);
    encoder.set_compression(::png::Compression::Balanced);
    encoder.set_source_srgb(::png::SrgbRenderingIntent::Perceptual);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(pixels)?;
    writer.finish()
}

/// Bytes written, refusing to grow past a limit.
struct Capped {
    bytes: Vec<u8>,
    limit: usize,
    /// How many bytes the file had reached when it passed the limit.
    reached: Option<usize>,
}

impl Capped {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            reached: None,
        }
    }
}

impl io::Write for Capped {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let total = self.bytes.len().saturating_add(buf.len());
        if total > self.limit {
            self.reached = Some(total);
            return Err(io::Error::other("the export is over its size cap"));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "glyph sizes and positions in pt are far inside f32 range and precision"
)]
fn to_f32(v: f64) -> f32 {
    v as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(max_side_px: u32, max_pixels: u64) -> ExportLimits {
        ExportLimits {
            max_pixels,
            max_side_px,
            ..ExportLimits::DEFAULT
        }
    }

    #[test]
    fn frames_that_fit_keep_their_scale() {
        let size = fit(
            Rect::new(-1.0, -1.0, 9.0, 4.0),
            80.0,
            &ExportLimits::DEFAULT,
        )
        .unwrap();
        assert_eq!((size.width, size.height), (800, 400));
        assert!((size.px_per_unit - 80.0).abs() < f64::EPSILON);
        // A fraction of a pixel rounds up.
        let size = fit(Rect::new(0.0, 0.0, 2.01, 1.0), 10.0, &ExportLimits::DEFAULT).unwrap();
        assert_eq!((size.width, size.height), (21, 10));
    }

    #[test]
    fn frames_too_large_shrink_to_both_caps() {
        for (frame, limits) in [
            // Wide: the side cap.
            (Rect::new(0.0, 0.0, 1_000.0, 10.0), ExportLimits::DEFAULT),
            // Square: the pixel cap.
            (Rect::new(0.0, 0.0, 500.0, 500.0), ExportLimits::DEFAULT),
            // At the board's extents.
            (Rect::new(-2e6, -2e6, 2e6, 2e6), ExportLimits::DEFAULT),
            (Rect::new(0.0, 0.0, 3.3, 7.7), limits(13, 50)),
            (Rect::new(0.0, 0.0, 1.0, 1e6), limits(16_384, 1_000)),
        ] {
            let size = fit(frame, 320.0, &limits).unwrap();
            assert!(size.px_per_unit < 320.0, "{frame:?}");
            let (w, h) = (u32::from(size.width), u32::from(size.height));
            assert!(
                w <= limits.max_side_px && h <= limits.max_side_px,
                "{frame:?} {size:?}"
            );
            assert!(
                u64::from(w) * u64::from(h) <= limits.max_pixels,
                "{frame:?} {size:?}"
            );
            // As large as the caps let it be, within a few pixels.
            let pixels = u64::from(w) * u64::from(h);
            let tight = w + 3 >= limits.max_side_px.min(16_384)
                || h + 3 >= limits.max_side_px
                || pixels as f64 >= limits.max_pixels as f64 * 0.98;
            assert!(tight, "{frame:?} {size:?}");
        }
    }

    #[test]
    fn frames_with_no_size_do_not_fit() {
        for frame in [
            Rect::ZERO,
            Rect::new(0.0, 0.0, f64::INFINITY, 1.0),
            Rect::new(0.0, 0.0, f64::NAN, 1.0),
        ] {
            assert_eq!(fit(frame, 80.0, &ExportLimits::DEFAULT), None, "{frame:?}");
        }
    }

    #[test]
    fn the_writer_stops_at_its_limit() {
        use std::io::Write as _;
        let mut out = Capped::new(10);
        assert!(out.write_all(b"12345").is_ok());
        assert!(out.write_all(b"678901").is_err());
        assert_eq!(out.reached, Some(11));
        assert_eq!(out.bytes, b"12345");
    }
}
