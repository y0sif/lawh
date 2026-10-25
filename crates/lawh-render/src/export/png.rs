//! PNG export: the frame drawn with `vello_cpu` at the export's scale, then encoded with the
//! `png` crate (sRGB chunk) and streamed through the byte cap.
//!
//! Encoding is charged to the export's budget with the drawing. What compressing an image costs
//! depends on what is in it (paper and text compress fast; random pixels of a few values make
//! deflate search longest), so it is charged at its worst: images up to
//! [`PNG_BALANCED_MAX_PIXELS`] are compressed with zlib's default level
//! (`Compression::Balanced`: smaller files, [`PNG_NS_PER_BYTE_BALANCED`] at worst), larger ones
//! with fdeflate (`Compression::Fast`: [`PNG_NS_PER_BYTE_FAST`] at worst), so the worst image at
//! the pixel caps costs about a second to encode, not ten. fdeflate's charge is taken before
//! anything is drawn, and zlib's extra once the drawing is paid, when it fits; when it doesn't,
//! fdeflate encodes the image (`Compression::within`), so zlib never refuses an export.

use std::io::{self, Write as _};

use kurbo::{Affine, BezPath, Rect};
use lawh_core::export::{MAX_PNG_SCALE_HUNDREDTHS, MIN_PNG_SCALE_HUNDREDTHS};
use lawh_core::{Board, ExportBackground, ExportOptions, ObjectId};
use vello_cpu::peniko::ImageAlphaType;
use vello_cpu::{Pixels, Pixmap, RenderContext, Resources};

use super::{ExportError, ExportLimits, ExportPlan, ExportSink};
use crate::budget::{self, Budget, Estimate, MIN_VISIBLE_PX};
use crate::draw::{self, TOLERANCE_PX};
use crate::renderer::{MAX_GLYPH_EM_PX, Vello, stroke_px};
use crate::scene::{Glyphs, Item, Sink, Tools, uniform_scale};
use crate::{BASE_PX_PER_UNIT, Renderer};

/// Most pixels a PNG is compressed with zlib's default level (`Compression::Balanced`); a
/// larger image is compressed with fdeflate (`Compression::Fast`), and so is a smaller one whose
/// drawing leaves too little of the budget for zlib's worst case (`Compression::within`).
/// 2,000,000 pixels, a board of about 17 x 17 units at the default `pngscale` of 2: zlib's worst
/// case there is charged 0.6 s of the export's 2 s (0.8 s with transparency). Below it
/// files are smaller: the area board at `pngscale` 2 (970 x 802) is 32 KB with zlib and 47 KB
/// with fdeflate, at 8 (3,879 x 3,208) 163 KB and 322 KB, and zlib took 6 and 72 to 75 ms there
/// (`calibrate_png_encoding`, see [`PNG_NS_PER_BYTE_BALANCED`]). fdeflate's worst at the pixel
/// cap, 64 million pixels, is charged 1.25 s (1.66 s with transparency).
pub(crate) const PNG_BALANCED_MAX_PIXELS: u64 = 2_000_000;

/// The export budget's weights for encoding a PNG, in estimated nanoseconds of release work per
/// byte of image data (3 bytes a pixel, 4 with transparency, and one a row), for the worst
/// content. Measured with `calibrate_png_encoding` below (Linux `x86_64`, this laptop, release,
/// load average 1.1 to 1.9, 2026-10-25, four runs), images of 0.25 to 64 million pixels, with
/// moving the pixels out and the sRGB chunk; every image takes at most 0.93 times its estimate:
/// - zlib's default level: the worst content is random pixels of a few values a channel, where
///   deflate finds short matches everywhere and walks its longest hash chains: 3 values took 80
///   to 84.5 ns a byte, 2 random bits 58 to 66, 4 colours 43 to 49; uniform noise 28 to 36, six
///   scribbles of 2,000 lines across the image (the cost review's `scrn6`) 23 to 25, and the
///   area board 1.9 to 2.5.
/// - fdeflate: 6.07 ns a byte at worst (pixels of 2 colours), 5.92 for 4 colours at 8,000 x
///   8,000 (1.14 s), 3.8 to 5.8 for noise, 4.7 to 5.3 for the scribbles, 1.1 to 1.4 for the area
///   board. Its fastest setting, which filters every row one way only, was 15 % faster and made
///   larger files.
pub(crate) const PNG_NS_PER_BYTE_BALANCED: f64 = 100.0;
/// See [`PNG_NS_PER_BYTE_BALANCED`].
pub(crate) const PNG_NS_PER_BYTE_FAST: f64 = 6.5;

/// Bytes of compressed data the encoder holds before it writes them out as one `IDAT` chunk:
/// how far past the byte cap it may compress before the cap stops it, and its buffer. 12 bytes
/// of chunk header and checksum on each are 0.005 % of the file.
const PNG_CHUNK_BYTES: usize = 256 * 1024;

/// How a PNG's image data is compressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Compression {
    /// zlib's default level: small files, slow on content that is hard to compress.
    Balanced,
    /// fdeflate: larger files, 5 to 18 times faster on content that is hard to compress.
    Fast,
}

impl Compression {
    /// For an image of `pixels` pixels: [`Compression::Balanced`] up to
    /// [`PNG_BALANCED_MAX_PIXELS`], [`Compression::Fast`] past it.
    pub(crate) fn for_pixels(pixels: u64) -> Self {
        if pixels <= PNG_BALANCED_MAX_PIXELS {
            Self::Balanced
        } else {
            Self::Fast
        }
    }

    /// How an export compresses an image of `pixels` pixels and `data` bytes of image data, once
    /// fdeflate's charge for it is paid and the image drawn: the compression its size calls for
    /// ([`Compression::for_pixels`]) when what that costs above fdeflate fits what `budget` has
    /// left (taken from it), else [`Compression::Fast`].
    ///
    /// So zlib never refuses an export: where its worst case doesn't fit, the image is encoded
    /// with fdeflate (a larger file), as a slightly larger image past
    /// [`PNG_BALANCED_MAX_PIXELS`] would be. Charging zlib's worst case before drawing refused a
    /// 1.86 million pixel image (0.56 s of zlib) under a budget that wrote the same board at 2.05
    /// million pixels with fdeflate; falling back to fdeflate only there would still refuse an
    /// image whose drawing fits beside fdeflate's charge but not beside zlib's.
    pub(crate) fn within(pixels: u64, data: f64, budget: &mut Budget) -> Self {
        let chosen = Self::for_pixels(pixels);
        let more = chosen.cost(data) - Self::Fast.cost(data);
        if chosen == Self::Fast || budget.spend(more) {
            chosen
        } else {
            Self::Fast
        }
    }

    /// The estimated cost of compressing `data` bytes of image data this way, whatever is in it.
    pub(crate) fn cost(self, data: f64) -> f64 {
        data * match self {
            Self::Balanced => PNG_NS_PER_BYTE_BALANCED,
            Self::Fast => PNG_NS_PER_BYTE_FAST,
        }
    }

    fn png(self) -> ::png::Compression {
        match self {
            Self::Balanced => ::png::Compression::Balanced,
            Self::Fast => ::png::Compression::Fast,
        }
    }
}

/// Bytes of image data a PNG of `width` x `height` with `channels` bytes a pixel compresses:
/// each row's pixels and its filter byte.
fn image_data(width: u16, height: u16, channels: u8) -> f64 {
    (f64::from(width) * f64::from(channels) + 1.0) * f64::from(height)
}

/// Draws `plan` into a PNG at `options`' scale, shrunk to fit the pixel caps, and encodes it;
/// with the factor it shrank by, if it did.
pub(super) fn export(
    renderer: &mut Renderer,
    board: &Board,
    plan: &mut ExportPlan,
    options: ExportOptions,
    limits: &ExportLimits,
    budget: Budget,
) -> Result<(Vec<u8>, Option<f64>), ExportError> {
    let drawn = draw(renderer, board, plan, options, limits, budget)?;
    let bytes = encode_with(drawn.pixmap, limits.max_bytes, drawn.compression)?;
    Ok((bytes, drawn.shrunk))
}

/// A PNG's image, drawn, and how it is to be compressed (charged already).
struct Drawn {
    pixmap: Pixmap,
    shrunk: Option<f64>,
    compression: Compression,
}

/// Sizes the image, charges it and its encoding to `budget` (fdeflate's charge first, zlib's
/// extra after the drawing if it fits: [`Compression::within`]), and draws `plan` into it.
fn draw(
    renderer: &mut Renderer,
    board: &Board,
    plan: &mut ExportPlan,
    options: ExportOptions,
    limits: &ExportLimits,
    mut budget: Budget,
) -> Result<Drawn, ExportError> {
    let hundredths = options
        .png_scale_hundredths
        .clamp(MIN_PNG_SCALE_HUNDREDTHS, MAX_PNG_SCALE_HUNDREDTHS);
    let full = BASE_PX_PER_UNIT * f64::from(hundredths) / 100.0;
    let frame = plan.frame;
    let size = fit(frame, full, limits).ok_or(ExportError::Empty)?;
    let shrunk = (size.px_per_unit < full).then(|| size.px_per_unit / full);
    let pixels = u64::from(size.width) * u64::from(size.height);
    // RGB when every pixel is opaque, which opaque paper makes sure of; else RGBA.
    let opaque = options.background == ExportBackground::Paper
        && renderer.palette.paper.components[3] >= 1.0;
    let channels = if opaque { 3 } else { 4 };
    let data = image_data(size.width, size.height, channels);
    // fdeflate's charge first: only an image that even fdeflate can't encode within the budget
    // is refused, so a smaller scale is never refused where a larger one is written (see
    // `Compression::within`).
    if !budget.spend(budget::image(pixels as f64) + Compression::Fast.cost(data)) {
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
    let compression = Compression::within(pixels, data, &mut target.budget);
    Ok(Drawn {
        pixmap: target.finish(),
        shrunk,
        compression,
    })
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

    /// Its context, as the window's sink draws with it.
    fn vello(&mut self) -> Vello<'_> {
        Vello {
            ctx: &mut self.ctx,
            resources: &mut self.resources,
        }
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
                self.vello()
                    .stroke(&path, width, tools.palette.color(*paint));
            }
            Item::Fill { corners, paint } => {
                let path = draw::triangle(corners.map(|corner| self.to_px * corner));
                self.cost.fill(&path);
                self.settle()?;
                self.vello().fill(&path, tools.palette.color(*paint));
            }
            Item::Glyphs(run) => self.glyphs(tools, run)?,
            Item::Path {
                path,
                transform,
                fill,
                stroke,
            } => {
                let transform = self.to_px * *transform;
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
                if let Some(fill) = fill {
                    self.vello().fill(&px, tools.palette.color(*fill));
                }
                if let Some((width, paint)) = stroke {
                    self.vello().stroke(&px, width, tools.palette.color(paint));
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
        let transform = self.to_px * run.transform;
        let em_px = (run.size * uniform_scale(transform)).abs();
        if !(MIN_VISIBLE_PX..=MAX_GLYPH_EM_PX).contains(&em_px) {
            return Ok(());
        }
        self.cost.run();
        let font = run.font();
        for glyph in run.glyphs() {
            if self.cost.over() {
                break;
            }
            let stats = tools.glyphs.stats(font, glyph.id);
            self.cost.glyph(Some((stats, em_px)));
        }
        self.settle()?;
        let mut vello = self.vello();
        vello.glyph_state(transform, tools.palette.color(run.paint));
        // Every glyph: the frame holds them all.
        vello.glyphs(tools.fonts, run, |_| true);
        Ok(())
    }
}

/// Encodes `pixmap` as PNG with the compression its size calls for
/// ([`Compression::for_pixels`]), within `max_bytes`: [`encode_with`].
/// `Renderer::render_png` encodes with this.
pub(crate) fn encode(pixmap: Pixmap, max_bytes: usize) -> Result<Vec<u8>, ExportError> {
    let pixels = u64::from(pixmap.width()) * u64::from(pixmap.height());
    encode_with(pixmap, max_bytes, Compression::for_pixels(pixels))
}

/// Encodes `pixmap` as PNG: RGB when every pixel is opaque, else RGBA (not premultiplied), 8
/// bits a channel, compressed as `compression` says, with an sRGB chunk. Moves the image's
/// pixels out of `pixmap` instead of copying them, and streams the file through the byte cap:
/// [`ExportError::TooLarge`] as soon as the file would pass `max_bytes`, with nothing past it
/// compressed or held.
fn encode_with(
    pixmap: Pixmap,
    max_bytes: usize,
    compression: Compression,
) -> Result<Vec<u8>, ExportError> {
    let (width, height) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
    let (color, pixels) = match pixmap.try_take_rgb8(ImageAlphaType::Alpha) {
        Pixels::Rgb8(pixels) => (::png::ColorType::Rgb, pixels),
        Pixels::Rgba8(pixels) => (::png::ColorType::Rgba, pixels),
    };
    let mut out = Capped::new(max_bytes);
    let written = write_png(&mut out, width, height, color, &pixels, compression);
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
    compression: Compression,
) -> Result<(), ::png::EncodingError> {
    let mut encoder = ::png::Encoder::new(out, width, height);
    encoder.set_color(color);
    encoder.set_depth(::png::BitDepth::Eight);
    encoder.set_compression(compression.png());
    encoder.set_source_srgb(::png::SrgbRenderingIntent::Perceptual);
    let mut writer = encoder.write_header()?;
    // Streamed: each chunk goes to `out` as it fills, so the cap stops the encoder there, and
    // no second copy of the compressed image is held.
    let mut stream = writer.stream_writer_with_size(PNG_CHUNK_BYTES)?;
    stream.write_all(pixels)?;
    stream.finish()?;
    writer.finish()
}

/// Bytes written, refusing to grow past a limit.
struct Capped {
    bytes: Vec<u8>,
    limit: usize,
    /// How many bytes the file had reached when it first passed the limit.
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
        if self.reached.is_some() || total > self.limit {
            self.reached.get_or_insert(total);
            return Err(io::Error::other("the export is over its size cap"));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::time::Instant;

    use vello_cpu::color::PremulRgba8;

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
        let mut out = Capped::new(10);
        assert!(out.write_all(b"12345").is_ok());
        assert!(out.write_all(b"678901").is_err());
        assert_eq!(out.reached, Some(11));
        // Once over, it stays over, and keeps where it first went over.
        assert!(out.write_all(b"1").is_err());
        assert_eq!(out.reached, Some(11));
        assert_eq!(out.bytes, b"12345");
    }

    /// An image of noise: every channel random (premultiplied), opaque or not. The worst
    /// content to compress.
    fn noise(width: u16, height: u16, opaque: bool) -> Pixmap {
        pattern(width, height, |[r, g, b, a, ..]| {
            let a = if opaque { 255 } else { a.max(1) };
            // Uniform in 0..=a, so the straight colour is spread over 0..=255.
            let scale = |c: u8| ((u16::from(c) * (u16::from(a) + 1)) >> 8) as u8;
            PremulRgba8 {
                r: scale(r),
                g: scale(g),
                b: scale(b),
                a,
            }
        })
    }

    /// An image whose pixels `pixel` makes from 8 random bytes each.
    fn pattern(width: u16, height: u16, mut pixel: impl FnMut([u8; 8]) -> PremulRgba8) -> Pixmap {
        let mut pixmap = Pixmap::new(width, height);
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        for out in pixmap.data_mut() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *out = pixel(state.to_le_bytes());
        }
        pixmap
    }

    /// Images that are hard to compress in other ways than noise: random pixels from a few
    /// colours (short repeats everywhere, which make deflate search longest), and random low
    /// bits.
    fn hard_patterns(width: u16, height: u16) -> Vec<(String, Pixmap)> {
        let opaque = |r: u8, g: u8, b: u8| PremulRgba8 { r, g, b, a: 255 };
        let mut images = Vec::new();
        for colours in [2_u8, 4, 16, 64] {
            images.push((
                format!("{colours} colours {width}x{height}"),
                pattern(width, height, |[x, ..]| {
                    let i = x % colours;
                    opaque(i.wrapping_mul(37), i.wrapping_mul(91), i.wrapping_mul(13))
                }),
            ));
        }
        for bits in [1_u8, 2, 4] {
            let mask = (1 << bits) - 1;
            images.push((
                format!("{bits} random bits {width}x{height}"),
                pattern(width, height, |[r, g, b, ..]| {
                    opaque(r & mask, g & mask, b & mask)
                }),
            ));
        }
        for symbols in [3_u8, 5, 6, 8] {
            images.push((
                format!("{symbols} values a channel {width}x{height}"),
                pattern(width, height, |[r, g, b, ..]| {
                    opaque(r % symbols, g % symbols, b % symbols)
                }),
            ));
        }
        images.push((
            format!("2 random bits, grey {width}x{height}"),
            pattern(width, height, |[x, ..]| {
                let v = x & 3;
                opaque(v, v, v)
            }),
        ));
        images.push((
            format!("2 random bits, red only {width}x{height}"),
            pattern(width, height, |[x, ..]| opaque(x & 3, 0, 0)),
        ));
        images.push((
            format!("2 random bits, rgba {width}x{height}"),
            pattern(width, height, |[r, g, b, a, ..]| PremulRgba8 {
                r: r & 3,
                g: g & 3,
                b: b & 3,
                a: 0xFC | (a & 3),
            }),
        ));
        images.push((format!("runs of 1 to 8 {width}x{height}"), {
            let mut left = 0_u8;
            let mut colour = opaque(0, 0, 0);
            pattern(width, height, |[x, r, g, b, ..]| {
                if left == 0 {
                    left = 1 + x % 8;
                    colour = opaque(r, g, b);
                }
                left -= 1;
                colour
            })
        }));
        images
    }

    #[test]
    fn compression_is_chosen_by_size_and_charged_at_its_worst() {
        assert_eq!(Compression::for_pixels(1), Compression::Balanced);
        assert_eq!(
            Compression::for_pixels(PNG_BALANCED_MAX_PIXELS),
            Compression::Balanced
        );
        assert_eq!(
            Compression::for_pixels(PNG_BALANCED_MAX_PIXELS + 1),
            Compression::Fast
        );
        // The worst content at the largest image either way leaves the drawing some of the
        // export's budget, with transparency too.
        let budget = budget::EXPORT_BUDGET_NS;
        let side = 1_414;
        assert!(u64::from(side) * u64::from(side) <= PNG_BALANCED_MAX_PIXELS);
        let balanced = Compression::Balanced.cost(image_data(side, side, 4));
        assert!(balanced < 0.5 * budget, "{balanced}");
        let image = budget::image(64e6);
        let fast = Compression::Fast.cost(image_data(8_000, 8_000, 3));
        assert!(image + fast < 0.7 * budget, "{fast}");
        let fast = Compression::Fast.cost(image_data(8_000, 8_000, 4));
        assert!(image + fast < 0.9 * budget, "{fast}");
        assert!((image_data(10, 2, 3) - 62.0).abs() < 1e-9);
    }

    #[test]
    fn zlib_is_used_only_when_its_extra_charge_fits() {
        // 1,000 x 1,000 RGB: 3,001,000 bytes, fdeflate 19.5 ms and zlib 300.1 ms at worst.
        let data = image_data(1_000, 1_000, 3);
        let more = Compression::Balanced.cost(data) - Compression::Fast.cost(data);
        let small = 1_000_000;
        let mut budget = Budget::export_within(more + 1.0);
        assert_eq!(
            Compression::within(small, data, &mut budget),
            Compression::Balanced
        );
        assert!((budget.left() - 1.0).abs() < 1e-6, "{}", budget.left());
        // Too little left: fdeflate, and nothing more is taken.
        let mut budget = Budget::export_within(more - 1.0);
        assert_eq!(
            Compression::within(small, data, &mut budget),
            Compression::Fast
        );
        assert!((budget.left() - (more - 1.0)).abs() < 1e-6);
        // Past the threshold it is fdeflate whatever is left, with nothing more taken.
        let large = PNG_BALANCED_MAX_PIXELS + 1;
        let mut budget = Budget::export();
        assert_eq!(
            Compression::within(large, data, &mut budget),
            Compression::Fast
        );
        assert!((budget.left() - budget::EXPORT_BUDGET_NS).abs() < 1e-6);
    }

    #[test]
    fn both_compressions_give_the_same_pixels() {
        for opaque in [true, false] {
            let image = noise(67, 45, opaque);
            let want = image.clone().try_take_rgb8(ImageAlphaType::Alpha);
            for compression in [Compression::Balanced, Compression::Fast] {
                let bytes = encode_with(image.clone(), usize::MAX, compression).unwrap();
                let mut decoder = ::png::Decoder::new(io::Cursor::new(bytes))
                    .read_info()
                    .unwrap();
                let mut buf = vec![0; decoder.output_buffer_size().unwrap()];
                let info = decoder.next_frame(&mut buf).unwrap();
                buf.truncate(info.buffer_size());
                assert_eq!((info.width, info.height), (67, 45));
                let got = if opaque {
                    Pixels::Rgb8(buf)
                } else {
                    Pixels::Rgba8(buf)
                };
                assert_eq!(got, want, "{compression:?} opaque {opaque}");
            }
        }
    }

    #[test]
    fn the_encoder_stops_at_the_byte_cap_while_it_compresses() {
        // Noise doesn't compress: 600 x 600 x 3 bytes, about 1 MB, against a cap of 300,000.
        for compression in [Compression::Balanced, Compression::Fast] {
            let image = noise(600, 600, true);
            match encode_with(image, 300_000, compression) {
                Err(ExportError::TooLarge { bytes }) => {
                    // Stopped at the chunk that would have passed the cap, not at the end.
                    assert!(bytes > 300_000, "{bytes}");
                    assert!(bytes <= 300_000 + PNG_CHUNK_BYTES + 12, "{bytes}");
                }
                other => panic!("{compression:?} {other:?}"),
            }
        }
    }

    /// Median nanoseconds of `runs` runs of `f`, after one warm-up.
    fn median_ns(runs: usize, mut f: impl FnMut()) -> f64 {
        f();
        let mut times: Vec<f64> = (0..runs)
            .map(|_| {
                let started = Instant::now();
                f();
                started.elapsed().as_secs_f64() * 1e9
            })
            .collect();
        times.sort_by(f64::total_cmp);
        times[runs / 2]
    }

    /// What encoding takes, both ways, for noise at sizes up to the pixel cap and for boards'
    /// images: the weights above. Asserts in release that nothing takes more than 1.2 times its
    /// estimate. Ignored by default; run in release on a quiet machine:
    /// `nice -n 19 cargo test --release -p lawh-render --lib -- --ignored --nocapture
    /// --test-threads=1 calibrate_png_encoding`
    #[test]
    #[ignore = "a measurement: run in release with --test-threads=1"]
    fn calibrate_png_encoding() {
        let mut over = Vec::new();
        let mut row = |name: &str, image: &Pixmap, runs: usize| {
            let opaque = matches!(
                image.clone().try_take_rgb8(ImageAlphaType::Alpha),
                Pixels::Rgb8(_)
            );
            let data = image_data(image.width(), image.height(), if opaque { 3 } else { 4 });
            for compression in [Compression::Balanced, Compression::Fast] {
                let mut bytes = 0;
                let ns = median_ns(runs, || {
                    let encoded = encode_with(image.clone(), usize::MAX, compression).unwrap();
                    bytes = encoded.len();
                    black_box(encoded);
                });
                // The clone, timed with it, is a copy of the image: not the encoder's.
                let clone = median_ns(runs, || {
                    black_box(image.clone());
                });
                let ns = (ns - clone).max(0.0);
                let estimate = compression.cost(data);
                println!(
                    "{name:<40} {compression:<8?} {:>9.1} ms {:>6.2} ns a byte (est {:>6.2}), \
                     ratio {:>4.2}, {:>10} B",
                    ns / 1e6,
                    ns / data,
                    estimate / data,
                    ns / estimate,
                    bytes
                );
                if ns > 1.2 * estimate + 0.5e6 {
                    over.push(format!("{name} {compression:?}"));
                }
            }
        };
        for (width, height, runs) in [
            (500, 500, 5),
            (1_000, 1_000, 3),
            (2_000, 2_000, 3),
            (4_000, 4_000, 1),
            (8_000, 8_000, 1),
        ] {
            for opaque in [true, false] {
                let image = noise(width, height, opaque);
                let alpha = if opaque { "rgb" } else { "rgba" };
                row(&format!("noise {width}x{height} {alpha}"), &image, runs);
            }
            if width == 2_000 || (width == 8_000 && std::env::var_os("LAWH_PNG_ALL").is_some()) {
                for (name, image) in hard_patterns(width, height) {
                    row(&name, &image, runs);
                }
            }
        }
        // Boards: drawn as an export draws them, then encoded.
        let mut renderer = Renderer::new_trusted();
        let area = lawh_core::from_lawh(include_str!("../../../../examples/area.lawh")).unwrap();
        let scribbles = scribble_board(6);
        for (name, board, hundredths) in [
            ("area at 2", &area, 200),
            ("area at 8", &area, 800),
            ("6 scribbles of 2,000 lines", &scribbles, 800),
        ] {
            renderer.resolve_math(board);
            let options = ExportOptions {
                png_scale_hundredths: hundredths,
                ..ExportOptions::default()
            };
            let mut budget = Budget::unlimited();
            let mut plan = renderer
                .plan_export(board, None, &ExportLimits::DEFAULT, &mut budget)
                .unwrap();
            let drawn = draw(
                &mut renderer,
                board,
                &mut plan,
                options,
                &ExportLimits::DEFAULT,
                Budget::unlimited(),
            )
            .unwrap();
            let image = drawn.pixmap;
            row(
                &format!("{name} ({}x{})", image.width(), image.height()),
                &image,
                if image.width() > 4_000 { 1 } else { 3 },
            );
        }
        if !cfg!(debug_assertions) {
            assert!(over.is_empty(), "over their estimates: {over:?}");
        }
    }

    /// Peak memory and time of encoding noise at the pixel cap, opaque and not, streamed through
    /// the byte cap (which stops it), after resetting the peak. Linux only; run in release:
    /// `nice -n 19 cargo test --release -p lawh-render --lib -- --ignored --nocapture
    /// --test-threads=1 measure_png_peak`
    #[test]
    #[ignore = "a measurement: run in release with --test-threads=1"]
    #[cfg(target_os = "linux")]
    fn measure_png_peak() {
        let peak_mb = || {
            let status = std::fs::read_to_string("/proc/self/status").unwrap();
            let line = status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .unwrap();
            line.split_whitespace()
                .nth(1)
                .unwrap()
                .parse::<u64>()
                .unwrap()
                / 1024
        };
        for opaque in [true, false] {
            let image = noise(8_000, 8_000, opaque);
            std::fs::write("/proc/self/clear_refs", "5").unwrap();
            let before = peak_mb();
            let started = Instant::now();
            let result = encode_with(image, super::super::MAX_EXPORT_BYTES, Compression::Fast);
            println!(
                "noise 8000x8000 opaque {opaque}: {:?}, peak {} MB (image alone {} MB), {:?}",
                started.elapsed(),
                peak_mb(),
                before,
                result.map(|bytes| bytes.len())
            );
        }
    }

    /// `n` formulas of a polygon of 2,000 points jumping about one box, each its own colour, at
    /// size 16: an image at the pixel cap crossed by lines everywhere, the costliest real
    /// content to compress (the M4 cost review's `scrn` boards).
    fn scribble_board(n: usize) -> Board {
        use std::fmt::Write as _;
        let mut src = String::from("lawh version=1\n");
        for i in 0..n {
            let _ = writeln!(
                src,
                "math id=m{i} x=0 y=0 size=16 \"#polygon(fill: none, stroke: 0.2pt + rgb({}, \
                 {}, {}), ..range(2000).map(i => (calc.rem(i * 7919 + {}, 997) * 0.4pt, \
                 calc.rem(i * 104729 + {}, 991) * 0.4pt)))\"",
                (i * 53) % 256,
                (i * 101) % 256,
                (i * 29) % 256,
                i * 17,
                i * 31
            );
        }
        lawh_core::from_lawh(&src).unwrap()
    }
}
