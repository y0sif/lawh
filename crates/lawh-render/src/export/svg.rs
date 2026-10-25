//! SVG export: the frame's objects as paths, written by hand (no dependency) into a file that
//! holds only geometry.
//!
//! - Glyphs are outlines from the bundled fonts: each glyph (font and glyph id) is defined once,
//!   as a `<path>` in font units inside a `<defs>` written just before the first glyph run that
//!   uses it, so every reference points back to something already written. A glyph run is one
//!   `<g>` with its colour and one transform, holding a `<use>` per glyph.
//! - Shape outlines, arrowheads and math paths are `<path>`s, strokes with round caps and joins
//!   written out, colours as sRGB hex from the [`Palette`] (Typst's colours through the same
//!   mapping as the screen), opacity only where a colour has one.
//! - No `<text>`, and no board text anywhere in the file, not even object ids: text is only its
//!   glyphs' outlines. No reference outside the file (only the `#g0`, `#g1`, ... ids it makes),
//!   no script, no style sheet.
//!
//! The nominal size is [`BASE_PX_PER_UNIT`] (40) pixels a unit, in whole pixels with the frame
//! centred as a PNG at scale 1 has it (`png.rs`), so the two match pixel for pixel; coordinates
//! are those pixels from the image's corner, so numbers stay small. Numbers have a fixed
//! precision ([`COORD_DECIMALS`], [`SCALE_DECIMALS`]) and ids follow the draw list's order, so a
//! board gives the same bytes every time.
//!
//! Bounded as every export is: each byte counts against the byte cap as it is written (the
//! export stops as soon as the file passes it), and each element is charged to the export's
//! budget before it is written, with the weights below (`SVG_NS_*`). A PNG's pixel terms don't
//! apply: the cost of an SVG is what it writes.

use kurbo::{Affine, BezPath, PathEl, Point, Rect};
use lawh_core::{Board, ExportBackground, ExportOptions, ObjectId};

use super::{ExportError, ExportLimits, ExportPlan, ExportSink};
use crate::budget::Budget;
use crate::draw;
use crate::renderer::{MAX_GLYPH_EM_PX, stroke_px};
use crate::scene::{Glyphs, Item, Paint, Sink, Tools, uniform_scale};
use crate::{BASE_PX_PER_UNIT, Palette, Renderer};

/// Ellipses are written as cubic curves at most this far from the true ellipse, in nominal
/// pixels: a viewer can zoom in tenfold before the error reaches a tenth of a pixel. kurbo
/// splits an ellipse into more curves as it grows, slowly (with the sixth root of its size over
/// this): a circle of 3 units (120 pixels) takes 5 curves, one of a million units 41.
const SVG_TOLERANCE_PX: f64 = 0.01;

/// Decimals of coordinates and lengths: nominal pixels, and font units inside glyph
/// definitions and glyph runs. A hundredth of a pixel is a 4,000th of a grid unit; a font unit
/// is about a hundredth of a pixel for text of size 1 (Inter has 2,048 to the em, an em is 20
/// pixels), so glyph positions keep a ten-thousandth of a pixel.
const COORD_DECIMALS: u32 = 2;

/// The smallest coordinate that isn't written as 0 at [`COORD_DECIMALS`]: half its last place.
const MIN_COORD: f64 = {
    let mut min = 0.5;
    let mut place = 0;
    while place < COORD_DECIMALS {
        min /= 10.0;
        place += 1;
    }
    min
};

/// Decimals of the scale of a glyph run's transform, pixels per font unit: about 0.01 for text
/// of size 1 and 0.025 for math, 0.0024 for text of size 0.25. Nine decimals keep it within
/// 5e-10, so a glyph a line of 5,000 characters along (6,000,000 font units) lands within
/// 0.003 pixels.
const SCALE_DECIMALS: u32 = 9;

/// Decimals of an opacity, alpha over 255: three tell every alpha byte apart.
const OPACITY_DECIMALS: u32 = 3;

/// The export budget's weights for SVG, in estimated nanoseconds of release work, measured with
/// `calibrate_svg` below (Linux `x86_64`, this laptop, load average 2.5, 2026-10-25) and set
/// high: every board it writes takes 0.1 to 0.8 times its estimate. A PNG's pixel terms don't
/// apply: what an SVG costs is what it writes.
///
/// Each object: walking the draw list to it (looking its text up in the text cache, its
/// glyph runs), past what its items cost. 500 failing formulas, each drawn as text of about 9
/// glyphs, took 2.0 us an object with their glyphs, about 1.1 us without.
const SVG_NS_PER_OBJECT: f64 = 2_000.0;
/// Each element written (a path, a glyph run's group, a glyph's definition, the paper) with its
/// attributes and colours, past its segments: an arrowhead, an element of four segments, took
/// 170 ns in all.
const SVG_NS_PER_ELEMENT: f64 = 300.0;
/// Each path element written (a move, line, curve or close), in shapes, math paths and glyph
/// definitions alike, its numbers formatted. The costliest: the curves of ellipses a million
/// units across, worked out by kurbo and written with coordinates of 12 digits, about 200 ns
/// each (500 such circles, 21,000 segments, took 4.1 ms); a cubic curve of 12-digit
/// coordinates alone took 120 ns, and the segments of glyph outlines (small numbers) less.
const SVG_NS_PER_SEGMENT: f64 = 250.0;
/// Each glyph of a glyph run: looking up its definition, and writing its `<use>`. A text of
/// 5,000 characters took 94 to 121 ns a glyph; 250 texts of 160 characters (34,500 glyphs
/// drawn) 134 ns a glyph, the objects' cost included.
const SVG_NS_PER_GLYPH: f64 = 150.0;
/// Each glyph defined, past its segments: reading its outline from the font. Every glyph of
/// both bundled fonts (7,351 with outlines, 28.6 segments on average) took 27 ms in all, 3.6 us
/// a glyph: about half what their segments alone are estimated at.
const SVG_NS_PER_GLYPH_DEF: f64 = 1_000.0;

/// Draws `plan` into an SVG file and returns its bytes.
pub(super) fn export(
    renderer: &mut Renderer,
    board: &Board,
    plan: &mut ExportPlan,
    options: ExportOptions,
    limits: &ExportLimits,
    budget: Budget,
) -> Result<Vec<u8>, ExportError> {
    write(renderer, board, plan, options, limits, budget).map(|(bytes, _)| bytes)
}

/// [`export`], and the budget as it was left.
fn write(
    renderer: &mut Renderer,
    board: &Board,
    plan: &mut ExportPlan,
    options: ExportOptions,
    limits: &ExportLimits,
    budget: Budget,
) -> Result<(Vec<u8>, Budget), ExportError> {
    let frame = plan.frame;
    if !(frame.is_finite() && frame.width() > 0.0 && frame.height() > 0.0) {
        return Err(ExportError::Empty);
    }
    // Whole pixels, the frame centred in them, as a PNG at scale 1.
    let px = BASE_PX_PER_UNIT;
    let (width, height) = (
        (frame.width() * px).ceil().max(1.0),
        (frame.height() * px).ceil().max(1.0),
    );
    let room = (
        (width - frame.width() * px) / 2.0,
        (height - frame.height() * px) / 2.0,
    );
    let to_px =
        Affine::translate(room) * Affine::scale(px) * Affine::translate((-frame.x0, -frame.y0));
    let mut target = SvgTarget::new(limits.max_bytes, to_px, budget);
    target.header(width, height)?;
    if options.background == ExportBackground::Paper {
        target.paper(
            width,
            height,
            renderer.palette.paper.to_rgba8().to_u8_array(),
        )?;
    }
    renderer.draw_export(board, plan, &mut target)?;
    target.out.str("</svg>\n")?;
    Ok((target.out.bytes, target.budget))
}

/// What an SVG is written with: the file so far, the board-to-pixel transform, the export's
/// budget, and the glyphs defined so far.
struct SvgTarget {
    out: Out,
    to_px: Affine,
    budget: Budget,
    defs: Defs,
}

impl SvgTarget {
    fn new(limit: usize, to_px: Affine, budget: Budget) -> Self {
        Self {
            out: Out::new(limit),
            to_px,
            budget,
            defs: Defs::default(),
        }
    }

    /// Takes `ns` from the budget; [`ExportError::OverBudget`] when it doesn't fit.
    fn spend(&mut self, ns: f64) -> Result<(), ExportError> {
        if self.budget.spend(ns) {
            Ok(())
        } else {
            Err(ExportError::OverBudget)
        }
    }

    /// Charges writing `path` as one element.
    fn spend_path(&mut self, path: &BezPath) -> Result<(), ExportError> {
        self.spend(SVG_NS_PER_ELEMENT + SVG_NS_PER_SEGMENT * path.elements().len() as f64)
    }

    /// The XML declaration and the `<svg>` element, `width` x `height` pixels.
    fn header(&mut self, width: f64, height: f64) -> Result<(), ExportError> {
        self.spend(SVG_NS_PER_ELEMENT)?;
        self.out.str(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <svg xmlns=\"http://www.w3.org/2000/svg\" \
             xmlns:xlink=\"http://www.w3.org/1999/xlink\" version=\"1.1\"",
        )?;
        self.out
            .start()
            .str(" width=\"")
            .num::<0>(width)
            .str("\" height=\"")
            .num::<0>(height)
            .str("\" viewBox=\"0 0 ")
            .num::<0>(width)
            .str(" ")
            .num::<0>(height)
            .str("\">\n");
        self.out.finish()
    }

    /// The paper: a rectangle over the whole image.
    fn paper(&mut self, width: f64, height: f64, rgba: [u8; 4]) -> Result<(), ExportError> {
        self.spend(SVG_NS_PER_ELEMENT)?;
        self.out
            .start()
            .str("<rect width=\"")
            .num::<0>(width)
            .str("\" height=\"")
            .num::<0>(height)
            .str("\"")
            .paint("fill", rgba)
            .str("/>\n");
        self.out.finish()
    }

    /// Writes a `<path>` of `path` mapped by `transform`, filled and stroked as asked: `fill`
    /// and `stroke` (its width in pixels) as sRGB bytes. Strokes have round caps and joins.
    fn path(
        &mut self,
        path: &BezPath,
        transform: Affine,
        fill: Option<[u8; 4]>,
        stroke: Option<(f64, [u8; 4])>,
    ) -> Result<(), ExportError> {
        self.spend_path(path)?;
        self.out.str("<path d=\"")?;
        self.out.path_data(path, transform)?;
        let chunk = self.out.start();
        chunk.str("\"");
        match fill {
            Some(rgba) => chunk.paint("fill", rgba),
            None => chunk.str(" fill=\"none\""),
        };
        if let Some((width, rgba)) = stroke {
            chunk
                .paint("stroke", rgba)
                .str(" stroke-width=\"")
                .num::<COORD_DECIMALS>(width)
                .str("\" stroke-linecap=\"round\" stroke-linejoin=\"round\"");
        }
        chunk.str("/>\n");
        self.out.finish()
    }

    /// The definition of glyph `id` of `font`, written now (opening `<defs>` first, `open`
    /// tracking that) if this is its first use; `None` when it has no outline.
    fn define(
        &mut self,
        tools: &Tools<'_>,
        font: usize,
        id: u32,
        open: &mut bool,
    ) -> Result<Option<u32>, ExportError> {
        match self.defs.get(font, id) {
            Def::Defined(def) => return Ok(Some(def)),
            Def::NoOutline => return Ok(None),
            Def::Unseen => {}
        }
        let Some(outline) = tools.glyphs.outline(font, id) else {
            self.defs.set(font, id, None);
            return Ok(None);
        };
        self.spend(SVG_NS_PER_GLYPH_DEF)?;
        self.spend_path(&outline)?;
        if !*open {
            self.out.str("<defs>\n")?;
            *open = true;
        }
        let def = self.defs.next;
        self.out
            .start()
            .str("<path id=\"g")
            .uint(u64::from(def))
            .str("\" d=\"");
        self.out.finish()?;
        self.out.path_data(&outline, Affine::IDENTITY)?;
        self.out.str("\"/>\n")?;
        self.defs.next = def.saturating_add(1);
        self.defs.set(font, id, Some(def));
        Ok(Some(def))
    }

    /// Writes a glyph run: the definitions of glyphs it is the first to use, then a group with
    /// its colour and transform holding a `<use>` per glyph. Nothing for glyphs with no
    /// outline, or a run too large to draw (as on screen, [`MAX_GLYPH_EM_PX`]). Glyphs too
    /// small to see at the nominal size are kept, unlike on screen: a viewer can zoom in.
    fn glyphs(&mut self, tools: &Tools<'_>, run: &Glyphs<'_>) -> Result<(), ExportError> {
        let transform = self.to_px * run.transform;
        let em_px = run.size * uniform_scale(transform);
        let font = run.font();
        let Some(units_per_em) = tools.glyphs.units_per_em(font) else {
            return Ok(());
        };
        // The run's units per font unit.
        let per_unit = run.size / units_per_em;
        if !(em_px.is_finite() && em_px > 0.0 && em_px <= MAX_GLYPH_EM_PX && per_unit > 0.0) {
            tracing::debug!(em_px, "glyphs too large to draw, or of no size");
            return Ok(());
        }
        let Some(first) = run.glyphs().next() else {
            return Ok(());
        };
        self.spend(SVG_NS_PER_ELEMENT)?;
        let mut open = false;
        let mut drawn = 0_usize;
        for glyph in run.glyphs() {
            self.spend(SVG_NS_PER_GLYPH)?;
            if self.define(tools, font, glyph.id, &mut open)?.is_some() {
                drawn += 1;
            }
        }
        if open {
            self.out.str("</defs>\n")?;
        }
        if drawn == 0 {
            return Ok(());
        }
        // The first glyph's baseline goes into the transform: a text run's glyphs share it, so
        // their `<use>`s need no `y`.
        let baseline = first.y;
        let to_px = transform * Affine::translate((0.0, baseline)) * Affine::scale(per_unit);
        let [xx, yx, xy, yy, dx, dy] = to_px.as_coeffs();
        self.out
            .start()
            .str("<g")
            .paint("fill", rgba(tools.palette, run.paint))
            .str(" transform=\"matrix(")
            .num::<SCALE_DECIMALS>(xx)
            .str(" ")
            .num::<SCALE_DECIMALS>(yx)
            .str(" ")
            .num::<SCALE_DECIMALS>(xy)
            .str(" ")
            .num::<SCALE_DECIMALS>(yy)
            .str(" ")
            .num::<COORD_DECIMALS>(dx)
            .str(" ")
            .num::<COORD_DECIMALS>(dy)
            .str(")\">\n");
        self.out.finish()?;
        for glyph in run.glyphs() {
            let Def::Defined(def) = self.defs.get(font, glyph.id) else {
                continue;
            };
            let chunk = self.out.start();
            chunk
                .str("<use xlink:href=\"#g")
                .uint(u64::from(def))
                .str("\" x=\"")
                .num::<COORD_DECIMALS>(glyph.x / per_unit);
            let y = (glyph.y - baseline) / per_unit;
            if y.abs() >= MIN_COORD {
                chunk.str("\" y=\"").num::<COORD_DECIMALS>(y);
            }
            chunk.str("\"/>\n");
            self.out.finish()?;
        }
        self.out.str("</g>\n")
    }
}

impl ExportSink for SvgTarget {
    fn budget(&mut self) -> &mut Budget {
        &mut self.budget
    }
}

impl Sink for SvgTarget {
    type Error = ExportError;

    fn begin(&mut self, _: &mut Tools<'_>, _: &ObjectId, _: Rect) -> Result<(), ExportError> {
        self.spend(SVG_NS_PER_OBJECT)
    }

    fn item(&mut self, tools: &mut Tools<'_>, item: &Item<'_>) -> Result<(), ExportError> {
        match item {
            Item::Stroke {
                outline,
                width,
                paint,
            } => {
                let path = outline.to_path(self.to_px, SVG_TOLERANCE_PX);
                let width = width.abs() * uniform_scale(self.to_px);
                let color = rgba(tools.palette, *paint);
                self.path(&path, Affine::IDENTITY, None, Some((width, color)))
            }
            Item::Fill { corners, paint } => {
                let path = draw::triangle(corners.map(|corner| self.to_px * corner));
                let color = rgba(tools.palette, *paint);
                self.path(&path, Affine::IDENTITY, Some(color), None)
            }
            Item::Glyphs(run) => self.glyphs(tools, run),
            Item::Path {
                path,
                transform,
                fill,
                stroke,
            } => {
                let transform = self.to_px * *transform;
                // As on screen, a stroke too wide to draw is left out.
                let stroke = stroke_px(*stroke, uniform_scale(transform))
                    .map(|(width, paint)| (width, rgba(tools.palette, paint)));
                let fill = fill.map(|paint| rgba(tools.palette, paint));
                if fill.is_none() && stroke.is_none() {
                    return Ok(());
                }
                self.path(path, transform, fill, stroke)
            }
        }
    }
}

/// The colour `paint` stands for as sRGB bytes (red, green, blue, alpha, not premultiplied),
/// through the palette as the screen paints it: a colour math asked for keeps its bytes.
fn rgba(palette: &Palette, paint: Paint) -> [u8; 4] {
    palette.color(paint).to_rgba8().to_u8_array()
}

/// The glyphs met so far, by font (an index into `lawh_math::fonts`) and glyph id: the number
/// of each one's definition (`g0`, `g1`, ...), or that it has no outline.
#[derive(Default)]
struct Defs {
    /// By font, then by glyph id, grown as ids are met: [`UNSEEN`], [`NO_OUTLINE`] or the
    /// definition's number. Glyph ids are 16 bits in a font, so each holds at most 65,536.
    by_font: Vec<Vec<u32>>,
    /// The next definition's number.
    next: u32,
}

/// A glyph not met yet, in [`Defs`].
const UNSEEN: u32 = u32::MAX;
/// A glyph with no outline, in [`Defs`].
const NO_OUTLINE: u32 = u32::MAX - 1;

/// What is known of a glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Def {
    Unseen,
    NoOutline,
    /// Defined, as `g` and this number.
    Defined(u32),
}

impl Defs {
    /// What is known of glyph `id` of `font`.
    fn get(&self, font: usize, id: u32) -> Def {
        let slot = self
            .by_font
            .get(font)
            .and_then(|slots| slots.get(usize::try_from(id).ok()?))
            .copied();
        match slot {
            None | Some(UNSEEN) => Def::Unseen,
            Some(NO_OUTLINE) => Def::NoOutline,
            Some(def) => Def::Defined(def),
        }
    }

    /// Records what glyph `id` of `font` is; ids past 16 bits, which no font has, are not kept.
    fn set(&mut self, font: usize, id: u32, def: Option<u32>) {
        let Ok(id) = u16::try_from(id) else {
            return;
        };
        let id = usize::from(id);
        if self.by_font.len() <= font {
            self.by_font.resize_with(font + 1, Vec::new);
        }
        let Some(slots) = self.by_font.get_mut(font) else {
            return;
        };
        if slots.len() <= id {
            slots.resize(id + 1, UNSEEN);
        }
        if let Some(slot) = slots.get_mut(id) {
            *slot = def.unwrap_or(NO_OUTLINE);
        }
    }
}

/// The file's bytes, refusing to grow past a limit, and a chunk to put pieces of it together
/// in ([`Out::start`], [`Out::finish`]).
struct Out {
    bytes: Vec<u8>,
    limit: usize,
    chunk: Chunk,
}

impl Out {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            chunk: Chunk::new(),
        }
    }

    /// The chunk, emptied, to put the next piece of the file together in.
    fn start(&mut self) -> &mut Chunk {
        self.chunk.len = 0;
        &mut self.chunk
    }

    /// Appends what was put in the chunk since [`Out::start`].
    fn finish(&mut self) -> Result<(), ExportError> {
        let Self {
            bytes,
            limit,
            chunk,
        } = self;
        let total = bytes.len().saturating_add(chunk.len);
        if total > *limit {
            return Err(ExportError::TooLarge { bytes: total });
        }
        bytes.extend_from_slice(chunk.as_bytes());
        Ok(())
    }

    /// Appends `bytes`; [`ExportError::TooLarge`] with the size the file would have reached
    /// when that passes the limit, leaving the file as it was.
    fn raw(&mut self, bytes: &[u8]) -> Result<(), ExportError> {
        let total = self.bytes.len().saturating_add(bytes.len());
        if total > self.limit {
            return Err(ExportError::TooLarge { bytes: total });
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn str(&mut self, s: &str) -> Result<(), ExportError> {
        self.raw(s.as_bytes())
    }

    /// The path data of `path` mapped by `transform`: `M`, `L`, `Q`, `C` and `Z` commands with
    /// absolute coordinates, [`COORD_DECIMALS`] each, a segment at a time.
    fn path_data(&mut self, path: &BezPath, transform: Affine) -> Result<(), ExportError> {
        for element in path.elements() {
            let chunk = self.start();
            let points: &[Point] = match element {
                PathEl::MoveTo(p) => {
                    chunk.str("M");
                    &[*p]
                }
                PathEl::LineTo(p) => {
                    chunk.str("L");
                    &[*p]
                }
                PathEl::QuadTo(p1, p2) => {
                    chunk.str("Q");
                    &[*p1, *p2]
                }
                PathEl::CurveTo(p1, p2, p3) => {
                    chunk.str("C");
                    &[*p1, *p2, *p3]
                }
                PathEl::ClosePath => {
                    chunk.str("Z");
                    &[]
                }
            };
            for (i, point) in points.iter().enumerate() {
                let point = transform * *point;
                if i > 0 {
                    chunk.str(" ");
                }
                chunk
                    .num::<COORD_DECIMALS>(point.x)
                    .str(" ")
                    .num::<COORD_DECIMALS>(point.y);
            }
            self.finish()?;
        }
        Ok(())
    }
}

/// Room in a [`Chunk`]. The largest written is a glyph run's group: its tag, colour and
/// opacity (about 60 bytes) and six numbers of at most [`NUM_BYTES`] each with their spaces;
/// a path segment takes at most 7 + 6 x [`NUM_BYTES`].
const CHUNK_BYTES: usize = 384;

/// A few hundred bytes put together on the stack before they go into the file at once: an
/// element's start or attributes, a path segment, a glyph's `<use>`. The file's cap is checked
/// for each chunk, so the file passes it by no more than one chunk's bytes before it stops.
struct Chunk {
    buf: [u8; CHUNK_BYTES],
    len: usize,
}

impl Chunk {
    fn new() -> Self {
        Self {
            buf: [0; CHUNK_BYTES],
            len: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        self.buf.get(..self.len).unwrap_or_default()
    }

    /// Appends `bytes`, which always fit: every chunk written is far smaller than
    /// [`CHUNK_BYTES`] (a test holds the largest to it).
    fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        let end = self.len.saturating_add(bytes.len());
        if let Some(room) = self.buf.get_mut(self.len..end) {
            room.copy_from_slice(bytes);
            self.len = end;
        }
        self
    }

    fn str(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }

    /// `v` with at most `DECIMALS` decimals ([`fixed`]).
    fn num<const DECIMALS: u32>(&mut self, v: f64) -> &mut Self {
        let mut digits = [0; NUM_BYTES];
        let len = fixed::<DECIMALS>(v, &mut digits);
        self.bytes(digits.get(NUM_BYTES - len..).unwrap_or_default())
    }

    fn uint(&mut self, n: u64) -> &mut Self {
        let mut buf = [0; NUM_BYTES];
        let len = digits(n, &mut buf, NUM_BYTES);
        self.bytes(buf.get(NUM_BYTES - len..).unwrap_or_default())
    }

    /// ` {name}="#rrggbb"`, lowercase, and ` {name}-opacity="..."` when `rgba` isn't opaque.
    fn paint(&mut self, name: &str, rgba: [u8; 4]) -> &mut Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let [r, g, b, a] = rgba;
        let mut hex = [b'#'; 7];
        for (i, byte) in [r, g, b].into_iter().enumerate() {
            hex[1 + 2 * i] = HEX[usize::from(byte >> 4)];
            hex[2 + 2 * i] = HEX[usize::from(byte & 0xF)];
        }
        self.str(" ").str(name).str("=\"").bytes(&hex).str("\"");
        if a != u8::MAX {
            self.str(" ")
                .str(name)
                .str("-opacity=\"")
                .num::<OPACITY_DECIMALS>(f64::from(a) / f64::from(u8::MAX))
                .str("\"");
        }
        self
    }
}

/// Room for any number [`fixed`] writes: 19 digits, a point and a sign.
const NUM_BYTES: usize = 24;

/// Writes `v` rounded to `DECIMALS` decimals (at most 9) into the end of `buf`, without
/// trailing zeros or a trailing point, and returns how many bytes it took: `1.5`, `-0.25`, `3`,
/// never `-0`. A number that isn't finite is written as `0`; the export's numbers are all
/// finite (shapes are checked, math layouts are checked by lawh-math, the frame is finite), so
/// that is only a guard against writing `NaN` into a file. `DECIMALS` is a constant so the
/// divisions by its power of ten compile to multiplications.
fn fixed<const DECIMALS: u32>(v: f64, buf: &mut [u8; NUM_BYTES]) -> usize {
    let decimals = DECIMALS.min(9);
    let pow = 10_u64.pow(decimals);
    let scaled = (v * pow as f64).round();
    let scaled = if scaled.is_finite() {
        scaled.clamp(-9e18, 9e18)
    } else {
        0.0
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "rounded and within i64's range just above"
    )]
    let n = scaled as i64;
    let magnitude = n.unsigned_abs();
    let (whole, mut fraction) = (magnitude / pow, magnitude % pow);
    let mut places = decimals;
    while places > 0 && fraction % 10 == 0 {
        fraction /= 10;
        places -= 1;
    }
    let mut at = NUM_BYTES;
    for _ in 0..places {
        at -= 1;
        buf[at] = digit(fraction);
        fraction /= 10;
    }
    if places > 0 {
        at -= 1;
        buf[at] = b'.';
    }
    at -= digits(whole, buf, at);
    if n < 0 {
        at -= 1;
        buf[at] = b'-';
    }
    NUM_BYTES - at
}

/// Writes `n` in decimal into `buf` just before `end`, and returns how many digits it took.
/// Two digits at a time, from a table.
fn digits(mut n: u64, buf: &mut [u8; NUM_BYTES], end: usize) -> usize {
    let mut at = end;
    while n >= 100 {
        let pair = pair(n % 100);
        n /= 100;
        at -= 2;
        buf[at] = pair[0];
        buf[at + 1] = pair[1];
    }
    if n >= 10 {
        let pair = pair(n);
        at -= 2;
        buf[at] = pair[0];
        buf[at + 1] = pair[1];
    } else {
        at -= 1;
        buf[at] = digit(n);
    }
    end - at
}

/// `00` to `99`, as ASCII pairs.
const PAIRS: [[u8; 2]; 100] = {
    let mut pairs = [[0; 2]; 100];
    let mut i = 0;
    while i < 100 {
        #[expect(clippy::cast_possible_truncation, reason = "digits, 0 to 9")]
        let (tens, ones) = ((i / 10) as u8, (i % 10) as u8);
        pairs[i] = [b'0' + tens, b'0' + ones];
        i += 1;
    }
    pairs
};

/// The two decimal digits of `n`, below 100, as ASCII.
fn pair(n: u64) -> [u8; 2] {
    usize::try_from(n)
        .ok()
        .and_then(|n| PAIRS.get(n).copied())
        .unwrap_or(*b"00")
}

/// The last decimal digit of `n`, as ASCII.
fn digit(n: u64) -> u8 {
    b'0' + (n % 10) as u8
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::hint::black_box;
    use std::time::Instant;

    use lawh_core::board::{MAX_BOARD_TEXT_CHARS, MAX_OBJECTS, MAX_TEXT_CHARS};

    use super::*;
    use crate::scene;
    use crate::text::TextShaper;

    fn fixed_str(v: f64, decimals: u32) -> String {
        let mut buf = [0; NUM_BYTES];
        let len = match decimals {
            0 => fixed::<0>(v, &mut buf),
            2 => fixed::<2>(v, &mut buf),
            3 => fixed::<3>(v, &mut buf),
            9 => fixed::<9>(v, &mut buf),
            _ => unreachable!(),
        };
        String::from_utf8(buf[NUM_BYTES - len..].to_vec()).unwrap()
    }

    #[test]
    fn numbers_are_written_with_fixed_precision() {
        for (v, decimals, expected) in [
            (0.0, 2, "0"),
            (-0.0, 2, "0"),
            (-0.001, 2, "0"),
            (1.0, 2, "1"),
            (1.5, 2, "1.5"),
            (-1.25, 2, "-1.25"),
            (12.345_67, 2, "12.35"),
            (0.009_765_625, 9, "0.009765625"),
            (1e-10, 9, "0"),
            (160_000_040.123, 2, "160000040.12"),
            (99.999, 2, "100"),
            (-7.0, 0, "-7"),
            (0.502, 3, "0.502"),
            (f64::NAN, 2, "0"),
            (f64::INFINITY, 2, "0"),
            (1e300, 2, "90000000000000000"),
        ] {
            assert_eq!(fixed_str(v, decimals), expected, "{v} to {decimals}");
        }
        let mut buf = [0; NUM_BYTES];
        assert_eq!(digits(0, &mut buf, NUM_BYTES), 1);
        assert_eq!(digits(u64::MAX, &mut buf, NUM_BYTES), 20);
        // The widest numbers: 19 digits, a point and a sign at most.
        for decimals in [0, 2, 3, 9] {
            for v in [-9e300, -1.234_567_890_123_4e10, -0.123_456_789_123] {
                assert!(fixed_str(v, decimals).len() <= 21, "{v} to {decimals}");
            }
        }
    }

    #[test]
    fn the_largest_chunks_fit() {
        // A glyph run's group with the widest numbers and a translucent colour, and a cubic
        // segment of the widest numbers.
        let widest = -1.234_567_890_123e9;
        let mut chunk = Chunk::new();
        chunk
            .str("<g")
            .paint("fill", [1, 2, 3, 4])
            .str(" transform=\"matrix(")
            .num::<SCALE_DECIMALS>(widest)
            .str(" ")
            .num::<SCALE_DECIMALS>(widest)
            .str(" ")
            .num::<SCALE_DECIMALS>(widest)
            .str(" ")
            .num::<SCALE_DECIMALS>(widest)
            .str(" ")
            .num::<COORD_DECIMALS>(-9e16)
            .str(" ")
            .num::<COORD_DECIMALS>(-9e16)
            .str(")\">\n");
        let group = chunk.len;
        assert!(chunk.as_bytes().ends_with(b")\">\n"), "{group}");
        assert!(group < CHUNK_BYTES / 2, "{group}");
        let mut out = Out::new(usize::MAX);
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.curve_to((-9e16, -9e16), (-9e16, -9e16), (-9e16, -9e16));
        out.path_data(&path, Affine::IDENTITY).unwrap();
        assert!(out.bytes.len() < CHUNK_BYTES / 2, "{}", out.bytes.len());
        assert!(out.bytes.ends_with(b"-90000000000000000"));
        // A stroke's attributes, translucent.
        let mut chunk = Chunk::new();
        chunk
            .str("\"")
            .paint("fill", [1, 2, 3, 4])
            .paint("stroke", [1, 2, 3, 4])
            .str(" stroke-width=\"")
            .num::<COORD_DECIMALS>(-9e16)
            .str("\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>\n");
        assert!(chunk.as_bytes().ends_with(b"/>\n"));
        assert!(chunk.len < CHUNK_BYTES / 2, "{}", chunk.len);
    }

    #[test]
    fn definitions_are_kept_by_font_and_glyph() {
        let mut defs = Defs::default();
        assert_eq!(defs.get(1, 7), Def::Unseen);
        defs.set(1, 7, Some(0));
        defs.set(1, 3, None);
        defs.set(0, 65_535, Some(1));
        defs.set(0, 65_536, Some(2));
        assert_eq!(defs.get(1, 7), Def::Defined(0));
        assert_eq!(defs.get(1, 3), Def::NoOutline);
        assert_eq!(defs.get(1, 4), Def::Unseen);
        assert_eq!(defs.get(0, 65_535), Def::Defined(1));
        assert_eq!(defs.get(0, 65_536), Def::Unseen);
        assert_eq!(defs.get(2, 0), Def::Unseen);
    }

    #[test]
    fn colours_keep_their_bytes() {
        let palette = Palette::LIGHT;
        assert_eq!(rgba(&palette, Paint::Ink), [0x1F, 0x1F, 0x1F, 0xFF]);
        assert_eq!(rgba(&palette, Paint::Error), [0xC0, 0x39, 0x2B, 0xFF]);
        for v in 0..=u8::MAX {
            let bytes = [v, v.wrapping_add(85), v.wrapping_mul(3), v];
            assert_eq!(rgba(&palette, Paint::Rgba(bytes)), bytes);
        }
    }

    #[test]
    fn the_file_stops_at_its_limit() {
        let mut out = Out::new(10);
        out.str("12345").unwrap();
        assert_eq!(out.str("678901"), Err(ExportError::TooLarge { bytes: 11 }));
        assert_eq!(out.bytes, b"12345");
        out.start()
            .paint("fill", [0x1F, 0xAB, 0x00, 0xFF])
            .num::<2>(-2.5)
            .uint(42);
        assert_eq!(out.finish(), Err(ExportError::TooLarge { bytes: 26 }));
        let mut out = Out::new(100);
        out.start()
            .paint("fill", [0x1F, 0xAB, 0x00, 0xFF])
            .num::<2>(-2.5)
            .uint(42);
        out.finish().unwrap();
        out.start().paint("stroke", [0, 0, 0xFF, 0x80]);
        out.finish().unwrap();
        assert_eq!(
            String::from_utf8(out.bytes).unwrap(),
            " fill=\"#1fab00\"-2.542 stroke=\"#0000ff\" stroke-opacity=\"0.502\""
        );
    }

    /// An SVG target with no limit, for measuring.
    fn target() -> SvgTarget {
        SvgTarget::new(
            usize::MAX,
            Affine::scale(BASE_PX_PER_UNIT),
            Budget::unlimited(),
        )
    }

    /// Median nanoseconds of `runs` runs of `f`, after two warm-ups.
    fn median_ns(runs: usize, mut f: impl FnMut()) -> f64 {
        f();
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

    /// Writing an SVG, measured piece by piece (for the `SVG_NS_*` weights) and whole (each
    /// export against its estimate). Estimates lean high: in release, no export may take more
    /// than 1.2 times its estimate, give or take 0.2 ms of timer noise, taking the least of
    /// three medians for one that does. Ignored by default; run in release on a quiet machine:
    /// `nice -n 19 cargo test --release -p lawh-render --lib -- --ignored --nocapture
    /// --test-threads=1 calibrate_svg`
    #[test]
    #[ignore = "a measurement: run in release with --test-threads=1"]
    fn calibrate_svg() {
        let mut renderer = Renderer::new_trusted();
        measure_pieces(&mut renderer);
        assert_exports_within_estimates(&mut renderer);
    }

    /// Prints what each piece of an SVG costs to write.
    fn measure_pieces(renderer: &mut Renderer) {
        let ink = [0x1F, 0x1F, 0x1F, 0xFF];
        // Path segments: 100,000 cubic curves, six numbers each, the most a segment writes, of
        // 12 digits, as far out as a board reaches (a million units, 40,000,000 pixels).
        let mut curves = BezPath::new();
        curves.move_to((4e7, 0.0));
        for i in 0..100_000 {
            let x = 4e7 + f64::from(i) * 1.37;
            curves.curve_to((x + 0.31, 4e7), (x + 0.93, -4e7), (x + 1.37, 1.234));
        }
        let segments = curves.elements().len() as f64;
        let ns = median_ns(9, || {
            let mut svg = target();
            svg.path(&curves, Affine::IDENTITY, Some(ink), Some((2.0, ink)))
                .unwrap();
            black_box(svg.out.bytes.len());
        });
        let per_segment = ns / segments;
        println!("{per_segment:>8.1} ns a cubic segment");

        // Elements: 20,000 arrowheads, filled triangles of four segments.
        let corners = [
            Point::new(1.0, 2.0),
            Point::new(1.3, 2.1),
            Point::new(1.25, 1.8),
        ];
        let heads = 20_000;
        let mut tools = renderer.tools();
        let ns = median_ns(9, || {
            let mut svg = target();
            for _ in 0..heads {
                svg.item(
                    &mut tools,
                    &Item::Fill {
                        corners,
                        paint: Paint::Ink,
                    },
                )
                .unwrap();
            }
            black_box(svg.out.bytes.len());
        });
        println!(
            "{:>8.1} ns an arrowhead (an element of 4 segments)",
            ns / f64::from(heads)
        );

        // Glyph uses: a text of 5,000 characters on one line, its glyphs all defined.
        let mut shaper = TextShaper::new();
        let text: String = "lorem ipsum dolor sit amet "
            .chars()
            .cycle()
            .take(MAX_TEXT_CHARS)
            .collect();
        let layout = shaper.build(&text);
        let items: Vec<Item<'_>> =
            scene::text_items(&layout, &text, Affine::scale(0.025), Paint::Ink).collect();
        let glyphs: usize = items
            .iter()
            .map(|item| match item {
                Item::Glyphs(run) => run.glyphs().count(),
                _ => 0,
            })
            .sum();
        let mut svg = target();
        let ns = median_ns(9, || {
            svg.out.bytes.clear();
            for item in &items {
                svg.item(&mut tools, item).unwrap();
            }
            black_box(svg.out.bytes.len());
        });
        let per_glyph = ns / glyphs as f64;
        println!(
            "{per_glyph:>8.1} ns a glyph used ({glyphs} in {} runs)",
            items.len()
        );

        measure_definitions(&tools);
    }

    /// Prints what defining every glyph of both bundled fonts costs.
    fn measure_definitions(tools: &Tools<'_>) {
        // Glyph definitions: every glyph of both fonts, once.
        let ids: Vec<(usize, u32)> = lawh_math::fonts()
            .iter()
            .enumerate()
            .flat_map(|(font, asset)| {
                let count = ttf_parser::Face::parse(asset.data, asset.index)
                    .unwrap()
                    .number_of_glyphs();
                (0..u32::from(count)).map(move |id| (font, id))
            })
            .collect();
        let mut outlined = 0.0;
        let mut defined = 0.0;
        for &(font, id) in &ids {
            if let Some(outline) = tools.glyphs.outline(font, id) {
                outlined += outline.elements().len() as f64;
                defined += 1.0;
            }
        }
        let ns = median_ns(5, || {
            let mut svg = target();
            let mut open = false;
            for &(font, id) in &ids {
                svg.define(tools, font, id, &mut open).unwrap();
            }
            black_box(svg.out.bytes.len());
        });
        println!(
            "{:>8.1} ns a glyph defined, {:.1} segments on average ({defined} glyphs, {:.1} ms)",
            ns / defined,
            outlined / defined,
            ns / 1e6
        );
    }

    /// Times exporting each of [`calibration_boards`], and asserts in release that none took
    /// more than 1.2 times its estimate, give or take 0.2 ms.
    fn assert_exports_within_estimates(renderer: &mut Renderer) {
        let mut over = Vec::new();
        for (name, board) in calibration_boards() {
            let mut budget = Budget::unlimited();
            let mut plan = renderer
                .plan_export(&board, None, &ExportLimits::DEFAULT, &mut budget)
                .unwrap();
            let options = ExportOptions {
                format: lawh_core::ExportFormat::Svg,
                ..ExportOptions::default()
            };
            let mut write_once = |renderer: &mut Renderer| {
                write(
                    renderer,
                    &board,
                    &mut plan,
                    options,
                    &ExportLimits::DEFAULT,
                    Budget::unlimited(),
                )
                .unwrap()
            };
            let (bytes, budget) = write_once(renderer);
            let estimate_ms = budget.spent / 1e6;
            let mut took_ms = f64::INFINITY;
            for _ in 0..3 {
                took_ms = took_ms.min(
                    median_ns(5, || {
                        black_box(write_once(renderer));
                    }) / 1e6,
                );
                if took_ms <= 1.2 * estimate_ms + 0.2 {
                    break;
                }
            }
            println!(
                "{name:<40} est {estimate_ms:>9.3} ms  took {took_ms:>9.3} ms  ratio {:>5.2}  \
                 {} bytes",
                took_ms / estimate_ms,
                bytes.len()
            );
            if took_ms > 1.2 * estimate_ms + 0.2 {
                over.push(name);
            }
        }
        if !cfg!(debug_assertions) {
            assert!(over.is_empty(), "over their estimates: {over:?}");
        }
    }

    /// Boards that each stress one part of writing an SVG.
    fn calibration_boards() -> Vec<(&'static str, Board)> {
        let board = |src: String| lawh_core::from_lawh(&format!("lawh version=1\n{src}")).unwrap();
        let many = |each: &dyn Fn(usize) -> String| (0..MAX_OBJECTS).map(each).collect::<String>();
        let line: String = "lorem ipsum dolor sit amet, consectetur adipiscing elit "
            .chars()
            .cycle()
            .take(MAX_BOARD_TEXT_CHARS / 250)
            .collect();
        // Every letter of Latin, Greek and Cyrillic and every mathematical operator: glyphs
        // defined once each.
        let distinct: String = ('\u{21}'..='\u{24F}')
            .chain('\u{370}'..='\u{4FF}')
            .chain('\u{2200}'..='\u{22FF}')
            .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
            .collect();
        let chunks: Vec<String> = distinct
            .chars()
            .collect::<Vec<_>>()
            .chunks(60)
            .map(|chunk| chunk.iter().collect())
            .collect();
        vec![
            (
                "area",
                lawh_core::from_lawh(include_str!("../../../../examples/area.lawh")).unwrap(),
            ),
            (
                "250 texts of 160 chars",
                board((0..250).fold(String::new(), |mut src, i| {
                    writeln!(src, "text id=t{i} x=0 y={} \"{line}\"", i * 2).unwrap();
                    src
                })),
            ),
            (
                "distinct glyphs",
                board(
                    chunks
                        .iter()
                        .enumerate()
                        .fold(String::new(), |mut src, (i, chunk)| {
                            writeln!(src, "text id=t{i} x=0 y={i} \"{chunk}\"").unwrap();
                            src
                        }),
                ),
            ),
            (
                "500 circles r 1e6",
                board(many(&|i| {
                    format!("circle id=c{i} x=0 y=0 r={}\n", 1e6 - i as f64)
                })),
            ),
            (
                "500 arrows",
                board(many(&|i| {
                    format!("arrow id=a{i} x1=0 y1={i} x2=10 y2={i}\n")
                })),
            ),
            (
                "500 formulas",
                board(many(&|i| {
                    format!(
                        "math id=m{i} x={} y={} \"sum_(i=1)^n i = (n(n+1))/2 + sqrt(x_{i})\"\n",
                        (i % 10) * 12,
                        (i / 10) * 3
                    )
                })),
            ),
            (
                "500 failing formulas",
                board(many(&|i| format!("math id=m{i} x=0 y={i} \"frac({i},\"\n"))),
            ),
        ]
    }

    #[test]
    fn path_data_is_absolute_commands() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((1.0, 2.0));
        path.quad_to((3.0, 4.0), (5.0, 6.0));
        path.curve_to((7.0, 8.0), (9.0, 10.0), (11.5, -12.25));
        path.close_path();
        let mut out = Out::new(1000);
        out.path_data(&path, Affine::translate((1.0, 0.0))).unwrap();
        assert_eq!(
            String::from_utf8(out.bytes).unwrap(),
            "M1 0L2 2Q4 4 6 6C8 8 10 10 12.5 -12.25Z"
        );
    }
}
