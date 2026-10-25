//! PDF export: the frame's objects on one page, written with krilla.
//!
//! - Board text is real text: each glyph run is drawn with the bundled font it was shaped in
//!   (embedded as a subset, by krilla), with its glyph ids, positions and the text each glyph
//!   shows, so a viewer can search and copy it (krilla writes a `ToUnicode` map per font, and
//!   `ActualText` where a glyph shows other text in one place than in another).
//! - Math is drawn as paths, since formulas carry no source text a viewer could copy: each glyph
//!   (font, glyph id and colour) is a form `XObject` holding its outline (`GlyphTable::outline`),
//!   written once in the file and drawn wherever the glyph is used, so a glyph's outline is never
//!   repeated. Math's own paths (bars, radicals, glyphs Typst rotated) are paths. Measured in
//!   release on a formula of 22,500 glyphs: 0.89 to 1.1 us and 44 bytes of content a glyph, a
//!   73 KB file, where writing each glyph's outline where it is used took 47 us and 1,644 bytes a
//!   glyph, a 7.2 MB file.
//! - Shape outlines are stroked with round caps and joins, arrowheads filled, colours from the
//!   [`Palette`] (a colour math asked for keeps its bytes) in ICC-based sRGB, as the screen's.
//!   Paper is a rectangle over the page unless the export is transparent.
//! - No date, no metadata, no tags: a board gives the same bytes every time.
//!
//! The page is [`PDF_PT_PER_UNIT`] (30) pt a unit, lawh at zoom 1 seen at 100 % in a viewer (40
//! pixels of 0.75 pt), in whole pixels of 0.75 pt with the frame centred, as a PNG at scale 1
//! has it (`png.rs`): on a 96 dpi screen at 100 % the two are the same pixels. A frame too large
//! for [`MAX_PDF_PAGE_PT`] a side is shrunk to fit (`ExportReport::shrunk`). Page coordinates are
//! worked out in `f64` from the frame's corner and handed to krilla in `f32` only then, so they
//! keep a step under 0.001 pt wherever on the board the frame is.
//!
//! krilla panics on a glyph id of 65,536 or more and fails the whole file on one past its font's
//! glyph count, so every glyph id is checked against the font before it reaches krilla
//! ([`ExportError::Pdf`]). krilla's surface panics when its pushes and pops don't match, so each
//! push is popped in the same block, with nothing that can return in between. Tagging, which
//! panics when nested, is off.
//!
//! Bounded as every export is: each piece is charged to the export's budget before it is drawn
//! (`PDF_NS_*`, which include compressing what it writes and the file's own work at the end),
//! and the content it writes is counted against the byte cap as it goes (`PDF_BYTES_*`, an
//! estimate of krilla's uncompressed content, which leans high): krilla builds the file in
//! memory, so a hostile board stops while what it holds is still small. The finished file is
//! checked against the cap too.

use std::collections::HashMap;
use std::sync::OnceLock;

use krilla::color::rgb;
use krilla::geom::{Path as PdfPath, PathBuilder, Point as PdfPoint, Transform};
use krilla::graphic::Graphic;
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule, LineCap, LineJoin, Stroke};
use krilla::surface::Surface;
use krilla::text::{Font, GlyphId, KrillaGlyph};
use krilla::{Data, Document, SerializeSettings};
use kurbo::{Affine, BezPath, PathEl, Rect};
use lawh_core::{Board, ExportBackground, ExportOptions, ObjectId};

use super::{ExportError, ExportLimits, ExportPlan, ExportSink, PDF_PT_PER_UNIT};
use crate::budget::Budget;
use crate::draw;
use crate::renderer::{MAX_GLYPH_EM_PX, stroke_px};
use crate::scene::{Glyphs, Item, Sink, Tools, uniform_scale};
#[cfg(doc)]
use crate::{Palette, export::MAX_PDF_PAGE_PT};

/// A pixel of a PNG at scale 1 (40 a unit) in pt, at 30 pt a unit: the step a page's sides are
/// rounded up to.
const PT_PER_PX: f64 = 0.75;

/// Shortest side of a page, in pt: PDF 1.x's implementation limit (ISO 32000-1, Annex C) is 3 to
/// 14,400. Only a frame shrunk far gets near it (a line 4,000,000 units long); its page is then
/// this high, the frame centred.
const MIN_PAGE_PT: f64 = 3.0;

/// Ellipses are written as cubic curves at most this far from the true ellipse, in pt: a viewer
/// zoomed to 6,400 % is still within a pixel.
const PDF_TOLERANCE_PT: f64 = 0.01;

/// The export budget's weights for PDF, in estimated nanoseconds of release work, measured with
/// `calibrate_pdf` below (Linux `x86_64`, this laptop, load average 0.5 to 1.6, 2026-10-25) and
/// set high: every board it writes takes 0.30 to 1.14 times its estimate (500 circles a million
/// units across the closest). Each includes what krilla does with the piece later: compressing
/// its content, and the file's work at the end.
///
/// The file: the document, the page, the colour profile, writing the objects at the end. A file
/// with one stroke took 0.05 ms in all.
const PDF_NS_PER_FILE: f64 = 200_000.0;
/// Each font text is drawn in: subsetting it when the file is finished, past its glyphs. One
/// glyph of Inter took 0.21 ms, of Libertinus Math 0.14 ms.
const PDF_NS_PER_FONT: f64 = 300_000.0;
/// Each glyph of text drawn for the first time: subsetting it, and its `ToUnicode` entry. 2,053
/// distinct glyphs of Inter took 4.9 us each.
const PDF_NS_PER_SUBSET_GLYPH: f64 = 6_000.0;
/// Each object: walking the draw list to it (its text in the text cache, its glyph runs).
const PDF_NS_PER_OBJECT: f64 = 2_000.0;
/// Each path drawn (an outline, an arrowhead, a math path, the paper), past its segments: its
/// colour, its stroke's settings, its transform. An arrowhead, four segments and 102 bytes,
/// took 0.98 us.
const PDF_NS_PER_DRAW: f64 = 1_000.0;
/// Each path segment (a move, line, curve or close) drawn, in shapes, math paths and glyph
/// definitions alike, past its bytes: converted to `f32`, its numbers formatted by krilla. A
/// cubic curve took 1.4 to 1.5 us with its 61 bytes, a line 0.51 us with its 22.
const PDF_NS_PER_SEGMENT: f64 = 800.0;
/// Each text glyph run: its font, its colour, its text matrix. A run of 50 characters took
/// 17.6 us.
const PDF_NS_PER_RUN: f64 = 3_000.0;
/// Each glyph of a text run, past its bytes: krilla maps it to its text (`ToUnicode`), groups
/// and encodes it. A text of 5,000 characters took 0.38 us a glyph with its 8.5 bytes.
const PDF_NS_PER_GLYPH: f64 = 300.0;
/// Each math glyph drawn, past its bytes: its transform, and its form `XObject` looked up. The
/// 22,500 glyphs of one formula took 0.89 us each with their 44 bytes.
const PDF_NS_PER_GLYPH_USE: f64 = 500.0;
/// Each math glyph defined as a form `XObject`, past its segments and bytes: its outline read from
/// the font, its stream's bounds, its object written. Every glyph of Libertinus Math defined and
/// drawn once took 40 us each (27.8 segments, 627 bytes), of Inter 45 us (29.8, 858).
const PDF_NS_PER_GLYPH_DEF: f64 = 12_000.0;
/// Each byte of content written (as `PDF_BYTES_*` estimate them): formatting it, compressing it,
/// and copying it into the file at the end. Numbers that repeat each other's digits compress
/// slowest: 500 circles a million units across, 12 curves each, took 29 us a circle.
const PDF_NS_PER_BYTE: f64 = 18.0;

/// Estimated bytes of uncompressed content krilla writes for each piece, set a little high
/// (measured with `calibrate_pdf`: every board's estimate is 1.07 to 1.94 times the content it
/// writes, the circles closest). Counted against the byte cap while drawing; the finished file
/// is compressed, so its content is smaller, and it is checked against the cap once more with
/// its fonts.
///
/// A number in page pt (within 14,400): `f32` written as briefly as it reads back, and the
/// space after it.
const PDF_BYTES_PER_NUMBER: usize = 10;
/// A number in font units, in a math glyph's definition.
const PDF_BYTES_PER_FONT_NUMBER: usize = 8;
/// A path drawn, past its segments: saving and restoring the graphics state, its transform (six
/// numbers), its colour (three) and, for a stroke, its width, caps and joins.
const PDF_BYTES_PER_DRAW: usize = 100;
/// A path segment's operator, past its numbers.
const PDF_BYTES_PER_SEGMENT: usize = 3;
/// A text run, past its glyphs: the graphics state, transform, colour, font, text matrix and
/// the text operators.
const PDF_BYTES_PER_RUN: usize = 160;
/// A glyph of a text run: its two-byte code as hex, and a position adjustment where it isn't
/// where the font's advance would put it (kerning: up to 17 bytes, 8.5 on average in words).
const PDF_BYTES_PER_GLYPH: usize = 16;
/// A math glyph drawn: the graphics state, its transform and the form `XObject`'s name (44 bytes
/// on a small page, 63 where the page's numbers are long).
const PDF_BYTES_PER_GLYPH_USE: usize = 72;

/// Draws `plan` onto one PDF page and returns the file's bytes, with the factor its scale shrank
/// by to fit [`MAX_PDF_PAGE_PT`], if it did.
pub(super) fn export(
    renderer: &mut crate::Renderer,
    board: &Board,
    plan: &mut ExportPlan,
    options: ExportOptions,
    limits: &ExportLimits,
    budget: Budget,
) -> Result<(Vec<u8>, Option<f64>), ExportError> {
    write(renderer, board, plan, options, limits, budget)
        .map(|written| (written.bytes, written.shrunk))
}

/// What [`write()`] made: the file, the factor the scale shrank by, the budget as it was left and
/// the content estimated.
struct Written {
    bytes: Vec<u8>,
    shrunk: Option<f64>,
    #[cfg_attr(not(test), expect(dead_code, reason = "read by the calibration"))]
    budget: Budget,
    #[cfg_attr(not(test), expect(dead_code, reason = "read by the calibration"))]
    content: usize,
}

/// [`export`], and the budget and the content's estimate as they were left.
fn write(
    renderer: &mut crate::Renderer,
    board: &Board,
    plan: &mut ExportPlan,
    options: ExportOptions,
    limits: &ExportLimits,
    budget: Budget,
) -> Result<Written, ExportError> {
    let page = fit(plan.frame, limits.max_page_pt).ok_or(ExportError::Empty)?;
    let shrunk = (page.pt_per_unit < PDF_PT_PER_UNIT).then_some(page.pt_per_unit / PDF_PT_PER_UNIT);
    let frame = plan.frame;
    // The frame centred in the page, which is a whole number of pixels.
    let room = (
        (page.width - frame.width() * page.pt_per_unit) / 2.0,
        (page.height - frame.height() * page.pt_per_unit) / 2.0,
    );
    let to_page = Affine::translate(room)
        * Affine::scale(page.pt_per_unit)
        * Affine::translate((-frame.x0, -frame.y0));
    let settings =
        PageSettings::from_wh(to_f32(page.width), to_f32(page.height)).ok_or(ExportError::Empty)?;
    let paper = match options.background {
        ExportBackground::Paper => Some(renderer.palette.paper.to_rgba8().to_u8_array()),
        ExportBackground::None => None,
    };

    let mut document = Document::new_with(serialize_settings());
    let (budget, content) = {
        let mut pdf_page = document.start_page_with(settings);
        let mut target = PdfTarget::new(pdf_page.surface(), to_page, budget, limits.max_bytes);
        target.spend(PDF_NS_PER_FILE, 0)?;
        if let Some(rgba) = paper {
            target.paper(page.width, page.height, rgba)?;
        }
        renderer.draw_export(board, plan, &mut target)?;
        // The surface, then the page, are finished as the block ends: krilla compresses the
        // page's content then, and does so too when an error returns early.
        (target.budget, target.content)
    };
    let bytes = document
        .finish()
        .map_err(|err| ExportError::Pdf(err.to_string()))?;
    if bytes.len() > limits.max_bytes {
        return Err(ExportError::TooLarge { bytes: bytes.len() });
    }
    Ok(Written {
        bytes,
        shrunk,
        budget,
        content,
    })
}

/// How krilla writes an export: compressed, colours in ICC-based sRGB (krilla's bundled profile)
/// as the screen's are, no XMP metadata and no date, so the bytes are the same every time, and
/// no tags.
fn serialize_settings() -> SerializeSettings {
    SerializeSettings {
        pretty: false,
        compress_content_streams: true,
        no_device_cs: true,
        ascii_compatible: false,
        xmp_metadata: false,
        enable_tagging: false,
        ..SerializeSettings::default()
    }
}

/// A page's size in pt, and its scale.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PageSize {
    width: f64,
    height: f64,
    pt_per_unit: f64,
}

/// The page of `frame` at [`PDF_PT_PER_UNIT`], or at the largest scale below it whose page has
/// no side over `max_page_pt`: each side a whole number of pixels ([`PT_PER_PX`]), at least
/// [`MIN_PAGE_PT`]. `None` for a frame with no size, or one no scale fits.
fn fit(frame: Rect, max_page_pt: f64) -> Option<PageSize> {
    let (w, h) = (frame.width(), frame.height());
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 && max_page_pt >= MIN_PAGE_PT) {
        return None;
    }
    let min_px = (MIN_PAGE_PT / PT_PER_PX).ceil();
    let sized = |pt_per_unit: f64| {
        let px = pt_per_unit / PT_PER_PX;
        let width = (w * px).ceil().max(min_px) * PT_PER_PX;
        let height = (h * px).ceil().max(min_px) * PT_PER_PX;
        (width <= max_page_pt && height <= max_page_pt).then_some(PageSize {
            width,
            height,
            pt_per_unit,
        })
    };
    if let Some(page) = sized(PDF_PT_PER_UNIT) {
        return Some(page);
    }
    // The longest side at the cap; rounding up to whole pixels can leave it one over, so a
    // little less until it fits.
    let mut pt_per_unit = (max_page_pt / w.max(h)).min(PDF_PT_PER_UNIT);
    for _ in 0..64 {
        if !(pt_per_unit.is_finite() && pt_per_unit > 0.0) {
            return None;
        }
        if let Some(page) = sized(pt_per_unit) {
            return Some(page);
        }
        pt_per_unit *= 0.999;
    }
    None
}

/// The bundled fonts (by their index in [`lawh_math::fonts`]) as krilla takes them, parsed once
/// per process: krilla hashes a font's bytes when it is made, which would cost a millisecond an
/// export. `None` for a font krilla can't read.
fn pdf_fonts() -> &'static [Option<Font>] {
    static FONTS: OnceLock<Vec<Option<Font>>> = OnceLock::new();
    FONTS.get_or_init(|| {
        lawh_math::fonts()
            .iter()
            .map(|asset| {
                let font = Font::new(Data::from(asset.data), asset.index);
                if font.is_none() {
                    tracing::warn!(family = asset.family, "bundled font failed to load for PDF");
                }
                font
            })
            .collect()
    })
}

/// A math glyph's form `XObject`, by font (an index into [`lawh_math::fonts`]), glyph id and
/// colour (sRGB bytes).
type GlyphKey = (usize, u32, [u8; 4]);

/// What a PDF is drawn with: krilla's surface for the page, the board-to-page transform, the
/// export's budget, the content written so far (estimated) and its cap, and the math glyphs
/// defined so far.
struct PdfTarget<'p> {
    surface: Surface<'p>,
    to_page: Affine,
    budget: Budget,
    /// Bytes of content written so far, as `PDF_BYTES_*` estimate them.
    content: usize,
    max_bytes: usize,
    /// Each math glyph met so far: its form `XObject`, or `None` when it has no outline.
    defs: HashMap<GlyphKey, Option<Graphic>>,
    /// By font, a bit for each glyph text has used: what the file's font subsets will hold.
    subset: Vec<Vec<u64>>,
    /// Scratch for a text run's glyphs and their x positions.
    glyphs: Vec<KrillaGlyph>,
    xs: Vec<f64>,
}

impl<'p> PdfTarget<'p> {
    fn new(surface: Surface<'p>, to_page: Affine, budget: Budget, max_bytes: usize) -> Self {
        Self {
            surface,
            to_page,
            budget,
            content: 0,
            max_bytes,
            defs: HashMap::new(),
            subset: Vec::new(),
            glyphs: Vec::new(),
            xs: Vec::new(),
        }
    }

    /// Takes `ns`, and what compressing `bytes` of content costs, from the budget, and counts the
    /// bytes against the cap: [`ExportError::OverBudget`] or [`ExportError::TooLarge`] (with the
    /// content's estimate) when either doesn't fit.
    fn spend(&mut self, ns: f64, bytes: usize) -> Result<(), ExportError> {
        if !self.budget.spend(ns + bytes as f64 * PDF_NS_PER_BYTE) {
            return Err(ExportError::OverBudget);
        }
        let content = self.content.saturating_add(bytes);
        if content > self.max_bytes {
            return Err(ExportError::TooLarge { bytes: content });
        }
        self.content = content;
        Ok(())
    }

    /// Charges drawing `path`: once for its fill, once for its stroke.
    fn spend_path(&mut self, path: &BezPath, draws: usize) -> Result<(), ExportError> {
        let (segments, bytes) = path_size(path, PDF_BYTES_PER_NUMBER);
        self.spend(
            (PDF_NS_PER_DRAW + PDF_NS_PER_SEGMENT * segments as f64) * draws as f64,
            (PDF_BYTES_PER_DRAW + bytes).saturating_mul(draws),
        )
    }

    /// Charges subsetting glyph `id` of font `font` the first time text uses it, and the font the
    /// first time text uses any of its glyphs.
    fn spend_subset(&mut self, font: usize, id: u32) -> Result<(), ExportError> {
        if self.subset.len() <= font {
            self.subset.resize_with(font + 1, Vec::new);
        }
        let Some(used) = self.subset.get_mut(font) else {
            return Ok(());
        };
        let mut ns = 0.0;
        if used.is_empty() {
            ns += PDF_NS_PER_FONT;
        }
        let (word, bit) = (usize::try_from(id / 64).unwrap_or(usize::MAX), id % 64);
        if used.len() <= word {
            // Ids are checked against the font's glyph count first, so this stays small.
            used.resize(word.saturating_add(1), 0);
        }
        if let Some(bits) = used.get_mut(word)
            && *bits & (1 << bit) == 0
        {
            *bits |= 1 << bit;
            ns += PDF_NS_PER_SUBSET_GLYPH;
        }
        if ns > 0.0 {
            self.spend(ns, 0)?;
        }
        Ok(())
    }

    /// The paper: a rectangle over the whole page.
    fn paper(&mut self, width: f64, height: f64, rgba: [u8; 4]) -> Result<(), ExportError> {
        let paper = Rect::new(0.0, 0.0, width, height);
        let path = kurbo::Shape::to_path(&paper, PDF_TOLERANCE_PT);
        self.path(&path, Affine::IDENTITY, Some(rgba), None)
    }

    /// Draws `path` mapped by `transform` into page pt, filled and stroked as asked: `fill` and
    /// `stroke` (its width in pt) as sRGB bytes. Strokes have round caps and joins.
    fn path(
        &mut self,
        path: &BezPath,
        transform: Affine,
        fill: Option<[u8; 4]>,
        stroke: Option<(f64, [u8; 4])>,
    ) -> Result<(), ExportError> {
        let draws = usize::from(fill.is_some()) + usize::from(stroke.is_some());
        if draws == 0 {
            return Ok(());
        }
        self.spend_path(path, draws)?;
        let Some(pdf) = pdf_path(path, transform) else {
            return Ok(());
        };
        let stroke = match stroke {
            Some((width, rgba)) => {
                let width = to_f32(width);
                if !width.is_finite() {
                    return Ok(());
                }
                Some(pdf_stroke(width, rgba))
            }
            None => None,
        };
        self.surface.set_fill(fill.map(pdf_fill));
        self.surface.set_stroke(stroke);
        self.surface.draw_path(&pdf);
        Ok(())
    }

    /// Draws a glyph run: text as text, math as its glyphs' form `XObject`s. Nothing for a run too
    /// large to draw (as on screen, [`MAX_GLYPH_EM_PX`], here in pt), or one of no size. Glyphs
    /// too small to see at 100 % are kept: a viewer can zoom in.
    fn glyphs(&mut self, tools: &mut Tools<'_>, run: &Glyphs<'_>) -> Result<(), ExportError> {
        let transform = self.to_page * run.transform;
        let em_pt = run.size * uniform_scale(transform);
        if !(em_pt.is_finite() && em_pt > 0.0 && em_pt <= MAX_GLYPH_EM_PX) {
            tracing::debug!(em_pt, "glyphs too large to draw, or of no size");
            return Ok(());
        }
        match run.text() {
            Some(text) => self.text(tools, run, text, transform),
            None => self.math(tools, run, transform),
        }
    }

    /// Draws a text run as text: its glyphs in its font at their positions, each with the bytes
    /// of `text` it shows, so the text can be copied.
    fn text(
        &mut self,
        tools: &Tools<'_>,
        run: &Glyphs<'_>,
        text: &str,
        transform: Affine,
    ) -> Result<(), ExportError> {
        let index = run.font();
        let Some(font) = pdf_fonts().get(index).and_then(Option::as_ref) else {
            tracing::warn!(font = index, "a text run's font isn't one PDF can embed");
            return Ok(());
        };
        let size = run.size;
        self.glyphs.clear();
        self.xs.clear();
        let mut baseline = None;
        for (glyph, range) in run.glyphs_with_text() {
            let id = checked_glyph(tools, index, glyph.id)?;
            self.spend_subset(index, glyph.id)?;
            let y0 = *baseline.get_or_insert(glyph.y);
            // A range outside the text, or not on a character's boundary, would panic in
            // krilla: none (parley's are always good).
            let range = if text.get(range.clone()).is_some() {
                range
            } else {
                0..0
            };
            self.glyphs.push(KrillaGlyph::new(
                id,
                0.0,
                0.0,
                to_f32(-(glyph.y - y0) / size),
                0.0,
                range,
                None,
            ));
            self.xs.push(glyph.x);
        }
        let (Some(&x0), Some(y0)) = (self.xs.first(), baseline) else {
            return Ok(());
        };
        // Each glyph advances to the next one's origin, so krilla writes no adjustment where the
        // font's advance puts it already (most glyphs), and the positions add up to parley's.
        for (glyph, pair) in self.glyphs.iter_mut().zip(self.xs.windows(2)) {
            glyph.x_advance = to_f32((pair[1] - pair[0]) / size);
        }
        let count = self.glyphs.len();
        self.spend(
            PDF_NS_PER_RUN + PDF_NS_PER_GLYPH * count as f64,
            PDF_BYTES_PER_RUN.saturating_add(PDF_BYTES_PER_GLYPH.saturating_mul(count)),
        )?;
        let (Some(pdf_transform), size) = (pdf_transform(transform), to_f32(size)) else {
            return Ok(());
        };
        let start = PdfPoint::from_xy(to_f32(x0), to_f32(y0));
        if !(size.is_finite() && size > 0.0 && start.x.is_finite() && start.y.is_finite()) {
            return Ok(());
        }
        self.surface
            .set_fill(Some(pdf_fill(tools.palette.rgba8(run.paint))));
        self.surface.set_stroke(None);
        self.surface.push_transform(&pdf_transform);
        self.surface
            .draw_glyphs(start, &self.glyphs, font.clone(), text, size, false);
        self.surface.pop();
        Ok(())
    }

    /// Draws a math run: each glyph's form `XObject` (defined on its first use) where the glyph is.
    fn math(
        &mut self,
        tools: &mut Tools<'_>,
        run: &Glyphs<'_>,
        transform: Affine,
    ) -> Result<(), ExportError> {
        let font = run.font();
        let Some(units_per_em) = tools.glyphs.units_per_em(font) else {
            return Ok(());
        };
        // The run's units per font unit.
        let per_unit = run.size / units_per_em;
        if !(per_unit.is_finite() && per_unit > 0.0) {
            return Ok(());
        }
        let rgba = tools.palette.rgba8(run.paint);
        for glyph in run.glyphs() {
            checked_glyph(tools, font, glyph.id)?;
            let Some(graphic) = self.define(tools, (font, glyph.id, rgba))? else {
                continue;
            };
            self.spend(PDF_NS_PER_GLYPH_USE, PDF_BYTES_PER_GLYPH_USE)?;
            let at = transform * Affine::translate((glyph.x, glyph.y)) * Affine::scale(per_unit);
            let Some(at) = pdf_transform(at) else {
                continue;
            };
            self.surface.push_transform(&at);
            self.surface.draw_graphic(graphic);
            self.surface.pop();
        }
        Ok(())
    }

    /// The form `XObject` of a math glyph, made now if this is its first use: its outline in font
    /// units, y down, filled in its colour. `None` when it has no outline (a space).
    fn define(&mut self, tools: &Tools<'_>, key: GlyphKey) -> Result<Option<Graphic>, ExportError> {
        if let Some(def) = self.defs.get(&key) {
            return Ok(def.clone());
        }
        let (font, id, rgba) = key;
        let outline = tools.glyphs.outline(font, id);
        let pdf = outline
            .as_ref()
            .and_then(|outline| pdf_path(outline, Affine::IDENTITY));
        let (Some(outline), Some(pdf)) = (outline, pdf) else {
            self.defs.insert(key, None);
            return Ok(None);
        };
        let (segments, bytes) = path_size(&outline, PDF_BYTES_PER_FONT_NUMBER);
        self.spend(
            PDF_NS_PER_GLYPH_DEF + PDF_NS_PER_SEGMENT * segments as f64,
            PDF_BYTES_PER_DRAW.saturating_add(bytes),
        )?;
        let mut builder = self.surface.stream_builder();
        {
            let mut surface = builder.surface();
            surface.set_fill(Some(pdf_fill(rgba)));
            surface.set_stroke(None);
            surface.draw_path(&pdf);
        }
        let graphic = Graphic::new(builder.finish(), false);
        self.defs.insert(key, Some(graphic.clone()));
        Ok(Some(graphic))
    }
}

impl ExportSink for PdfTarget<'_> {
    fn budget(&mut self) -> &mut Budget {
        &mut self.budget
    }
}

impl Sink for PdfTarget<'_> {
    type Error = ExportError;

    fn begin(&mut self, _: &mut Tools<'_>, _: &ObjectId, _: Rect) -> Result<(), ExportError> {
        self.spend(PDF_NS_PER_OBJECT, 0)
    }

    fn item(&mut self, tools: &mut Tools<'_>, item: &Item<'_>) -> Result<(), ExportError> {
        match item {
            Item::Stroke {
                outline,
                width,
                paint,
            } => {
                let path = outline.to_path(self.to_page, PDF_TOLERANCE_PT);
                let width = width.abs() * uniform_scale(self.to_page);
                let color = tools.palette.rgba8(*paint);
                self.path(&path, Affine::IDENTITY, None, Some((width, color)))
            }
            Item::Fill { corners, paint } => {
                let path = draw::triangle(corners.map(|corner| self.to_page * corner));
                let color = tools.palette.rgba8(*paint);
                self.path(&path, Affine::IDENTITY, Some(color), None)
            }
            Item::Glyphs(run) => self.glyphs(tools, run),
            Item::Path {
                path,
                transform,
                fill,
                stroke,
            } => {
                let transform = self.to_page * *transform;
                // As on screen, a stroke too wide to draw is left out.
                let stroke = stroke_px(*stroke, uniform_scale(transform))
                    .map(|(width, paint)| (width, tools.palette.rgba8(paint)));
                let fill = fill.map(|paint| tools.palette.rgba8(paint));
                self.path(path, transform, fill, stroke)
            }
        }
    }
}

/// Glyph `id` of font `font` as krilla takes it, if the font has it: krilla panics on an id of
/// 65,536 or more and fails the whole file on one past the font's glyph count, so neither may
/// reach it. Glyph ids come from parley on the bundled fonts and from the math worker's reply,
/// which is checked when it is read, so this is a guard.
fn checked_glyph(tools: &Tools<'_>, font: usize, id: u32) -> Result<GlyphId, ExportError> {
    if id < tools.glyphs.glyph_count(font) {
        Ok(GlyphId::new(id))
    } else {
        Err(ExportError::Pdf(format!(
            "glyph {id} is not in font {font}"
        )))
    }
}

/// How many segments `path` has, and the bytes of content they take, estimated with
/// `number_bytes` a number.
fn path_size(path: &BezPath, number_bytes: usize) -> (usize, usize) {
    let elements = path.elements();
    let numbers: usize = elements
        .iter()
        .map(|element| match element {
            PathEl::MoveTo(_) | PathEl::LineTo(_) => 2,
            // krilla writes quadratic curves as cubic ones.
            PathEl::QuadTo(..) | PathEl::CurveTo(..) => 6,
            PathEl::ClosePath => 0,
        })
        .sum();
    let bytes = numbers
        .saturating_mul(number_bytes)
        .saturating_add(elements.len().saturating_mul(PDF_BYTES_PER_SEGMENT));
    (elements.len(), bytes)
}

/// `path` mapped by `transform`, in `f32` as krilla takes it; `None` when it draws nothing or a
/// point isn't finite in `f32`.
fn pdf_path(path: &BezPath, transform: Affine) -> Option<PdfPath> {
    let mut builder = PathBuilder::new();
    let point = |p: kurbo::Point| {
        let p = transform * p;
        (to_f32(p.x), to_f32(p.y))
    };
    for element in path.elements() {
        match *element {
            PathEl::MoveTo(p) => {
                let (x, y) = point(p);
                builder.move_to(x, y);
            }
            PathEl::LineTo(p) => {
                let (x, y) = point(p);
                builder.line_to(x, y);
            }
            PathEl::QuadTo(p1, p2) => {
                let ((x1, y1), (x, y)) = (point(p1), point(p2));
                builder.quad_to(x1, y1, x, y);
            }
            PathEl::CurveTo(p1, p2, p3) => {
                let ((x1, y1), (x2, y2), (x, y)) = (point(p1), point(p2), point(p3));
                builder.cubic_to(x1, y1, x2, y2, x, y);
            }
            PathEl::ClosePath => builder.close(),
        }
    }
    // Its bounds are checked finite here.
    builder.finish()
}

/// `transform` in `f32` as krilla takes it; `None` when a coefficient isn't finite in `f32`.
fn pdf_transform(transform: Affine) -> Option<Transform> {
    let coeffs = transform.as_coeffs().map(to_f32);
    let [sx, ky, kx, sy, tx, ty] = coeffs;
    coeffs
        .iter()
        .all(|v| v.is_finite())
        .then(|| Transform::from_row(sx, ky, kx, sy, tx, ty))
}

/// A fill of sRGB bytes `rgba` (not premultiplied), non-zero as `vello_cpu` fills.
fn pdf_fill(rgba: [u8; 4]) -> Fill {
    let [r, g, b, a] = rgba;
    Fill {
        paint: rgb::Color::new(r, g, b).into(),
        opacity: opacity(a),
        rule: FillRule::NonZero,
    }
}

/// A stroke `width` pt wide of sRGB bytes `rgba`, with round caps and joins.
fn pdf_stroke(width: f32, rgba: [u8; 4]) -> Stroke {
    let [r, g, b, a] = rgba;
    Stroke {
        paint: rgb::Color::new(r, g, b).into(),
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        opacity: opacity(a),
        ..Stroke::default()
    }
}

/// Alpha byte `a` as an opacity.
fn opacity(a: u8) -> NormalizedF32 {
    NormalizedF32::new(f32::from(a) / f32::from(u8::MAX)).unwrap_or(NormalizedF32::ONE)
}

/// `v` in `f32`: page coordinates (within 14,400 pt), run and font units, scales. A value past
/// `f32`'s range becomes infinite, which the callers refuse.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the values are far inside f32's range; one outside becomes infinite and is refused"
)]
fn to_f32(v: f64) -> f32 {
    v as f32
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::hint::black_box;
    use std::time::Instant;

    use hayro::hayro_syntax::Pdf;
    use hayro::hayro_syntax::object::{Dict, Stream};
    use lawh_core::board::{MAX_BOARD_TEXT_CHARS, MAX_OBJECTS, MAX_TEXT_CHARS};

    use super::*;
    use crate::Renderer;
    use crate::scene::{self, Paint};
    use crate::text::TextShaper;

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

    /// Runs `draw` on a page of its own with no limit, and returns the nanoseconds it took, those
    /// finishing the file took, the content it was estimated at, and the file.
    fn on_a_page(draw: impl FnOnce(&mut PdfTarget<'_>)) -> (f64, f64, usize, Vec<u8>) {
        let mut document = Document::new_with(serialize_settings());
        let started = Instant::now();
        let content = {
            let mut page = document.start_page_with(PageSettings::from_wh(1000.0, 1000.0).unwrap());
            let mut target = PdfTarget::new(
                page.surface(),
                Affine::scale(30.0),
                Budget::unlimited(),
                usize::MAX,
            );
            draw(&mut target);
            target.content
        };
        let drawn = started.elapsed().as_secs_f64() * 1e9;
        let started = Instant::now();
        let bytes = document.finish().unwrap();
        let finished = started.elapsed().as_secs_f64() * 1e9;
        (drawn, finished, content, bytes)
    }

    /// The uncompressed content of `pdf`'s page and of its form `XObject`s, in bytes.
    fn content_bytes(pdf: &[u8]) -> usize {
        let pdf = Pdf::new(pdf.to_vec()).unwrap();
        let page = &pdf.pages()[0];
        let mut bytes = page.page_stream().map_or(0, <[u8]>::len);
        let resources: Dict<'_> = page.raw().get(b"Resources").unwrap();
        if let Some(objects) = resources.get::<Dict<'_>>(b"XObject") {
            for key in objects.keys() {
                let stream: Stream<'_> = objects.get(&*key).unwrap();
                bytes += stream.decoded().unwrap().len();
            }
        }
        bytes
    }

    /// Writing a PDF, measured piece by piece (for the `PDF_NS_*` and `PDF_BYTES_*` weights)
    /// and whole (each export against its estimates). Estimates lean high: in release, no
    /// export may take more than 1.2 times its estimate, give or take 0.5 ms of timer noise,
    /// taking the least of three medians for one that does, and none may write more content
    /// than estimated. Ignored by default; run in release on a quiet machine:
    /// `nice -n 19 cargo test --release -p lawh-render --lib -- --ignored --nocapture
    /// --test-threads=1 calibrate_pdf`
    #[test]
    #[ignore = "a measurement: run in release with --test-threads=1"]
    fn calibrate_pdf() {
        let mut renderer = Renderer::new_trusted();
        measure_pieces(&mut renderer);
        assert_exports_within_estimates(&mut renderer);
    }

    /// Prints what each piece of a PDF costs to draw and finish, and the content it takes
    /// against its estimate.
    #[expect(clippy::too_many_lines, reason = "one measurement after another")]
    fn measure_pieces(renderer: &mut Renderer) {
        let ink = [0x1F, 0x1F, 0x1F, 0xFF];
        let row =
            |name: &str,
             count: f64,
             (drawn, finished, estimate, bytes): (f64, f64, usize, Vec<u8>)| {
                let content = content_bytes(&bytes);
                println!(
                    "{name:<44} {:>8.1} ns drawn + {:>8.1} ns finished each; content {:>7.1} B each \
                 (estimated {:>7.1}); file {} B",
                    drawn / count,
                    finished / count,
                    content as f64 / count,
                    estimate as f64 / count,
                    bytes.len()
                );
            };
        // The file alone: a page with a dot.
        let mut dot = BezPath::new();
        dot.move_to((1.0, 1.0));
        dot.line_to((1.0, 1.1));
        let mut least = (f64::INFINITY, f64::INFINITY, 0, Vec::new());
        for _ in 0..9 {
            let run = on_a_page(|target| {
                target
                    .path(&dot, Affine::IDENTITY, None, Some((1.0, ink)))
                    .unwrap();
            });
            if run.0 + run.1 < least.0 + least.1 {
                least = run;
            }
        }
        row("a file with one stroke", 1.0, least);

        // Path segments: 100,000 cubic curves with page coordinates of all lengths.
        let mut curves = BezPath::new();
        curves.move_to((0.5, 0.25));
        for i in 0..100_000 {
            let x = f64::from(i % 1_000) * 14.3 + 0.123_456;
            let y = f64::from(i / 1_000) * 140.1 + 0.987_654;
            curves.curve_to((x + 0.31, y), (x + 0.93, y + 3.7), (x + 1.37, y + 1.234));
        }
        let segments = curves.elements().len() as f64;
        for (name, fill, stroke) in [
            ("a cubic segment, filled", Some(ink), None),
            ("a cubic segment, stroked", None, Some((1.5, ink))),
        ] {
            let run = on_a_page(|target| {
                target
                    .path(&curves, Affine::IDENTITY, fill, stroke)
                    .unwrap();
            });
            row(name, segments, run);
        }
        let mut lines = BezPath::new();
        lines.move_to((0.5, 0.25));
        for i in 0..100_000 {
            let x = f64::from(i % 1_000) * 14.3 + 0.123_456;
            let y = f64::from(i / 1_000) * 140.1 + 0.987_654;
            lines.line_to((x, y));
        }
        let run = on_a_page(|target| {
            target
                .path(&lines, Affine::IDENTITY, None, Some((1.5, ink)))
                .unwrap();
        });
        row("a line segment, stroked", 100_000.0, run);

        // Draws: 20,000 arrowheads, filled triangles of four segments.
        let corners = [
            kurbo::Point::new(1.0, 2.0),
            kurbo::Point::new(1.3, 2.1),
            kurbo::Point::new(1.25, 1.8),
        ];
        let heads = 20_000;
        let mut tools = renderer.tools();
        let run = on_a_page(|target| {
            for i in 0..heads {
                let at = f64::from(i % 100) * 0.3;
                let item = Item::Fill {
                    corners: corners.map(|c| c + kurbo::Vec2::new(at, at)),
                    paint: Paint::Ink,
                };
                target.item(&mut tools, &item).unwrap();
            }
        });
        row("an arrowhead (4 segments)", f64::from(heads), run);

        // Text glyphs: a text of 5,000 characters on one line, and the same as 100 runs.
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
        let run = on_a_page(|target| {
            for item in &items {
                target.item(&mut tools, item).unwrap();
            }
        });
        row(
            &format!("a text glyph ({glyphs} in {} runs)", items.len()),
            glyphs as f64,
            run,
        );
        let short: String = text.chars().take(50).collect();
        let layout = shaper.build(&short);
        let items: Vec<Item<'_>> =
            scene::text_items(&layout, &short, Affine::scale(0.025), Paint::Ink).collect();
        let runs = 400;
        let run = on_a_page(|target| {
            for i in 0..runs {
                for item in &items {
                    let Item::Glyphs(glyphs) = item else { continue };
                    let mut glyphs = glyphs.clone();
                    glyphs.transform =
                        Affine::translate((0.0, f64::from(i) * 0.6)) * glyphs.transform;
                    target.item(&mut tools, &Item::Glyphs(glyphs)).unwrap();
                }
            }
        });
        row("a text run of 50 characters", f64::from(runs), run);

        // Math glyphs: a formula of 22,500 glyphs (each use of a few glyphs), and every glyph of
        // the math font defined.
        let layout = renderer
            .math
            .layout_blocking("#range(150).map(_ => \"x\" * 150).join(linebreak())")
            .unwrap();
        let mut tools = renderer.tools();
        let items: Vec<Item<'_>> = scene::math_items(&layout, Affine::scale(0.03)).collect();
        let uses: usize = items
            .iter()
            .map(|item| match item {
                Item::Glyphs(run) => run.glyphs().count(),
                _ => 0,
            })
            .sum();
        let run = on_a_page(|target| {
            for item in &items {
                target.item(&mut tools, item).unwrap();
            }
        });
        row(&format!("a math glyph used ({uses})"), uses as f64, run);
        // Every glyph of both fonts defined and drawn once, as math (a glyph per run).
        for font in [1, 0] {
            let count = tools.glyphs.glyph_count(font);
            let glyphs: Vec<lawh_math::MathGlyph> = (0..count)
                .map(|id| lawh_math::MathGlyph {
                    id,
                    x: f64::from(id % 50) * 30.0,
                    y: f64::from(id / 50) * 30.0,
                })
                .collect();
            let mut outlined = 0.0;
            let mut defined = 0.0;
            for id in 0..count {
                if let Some(outline) = tools.glyphs.outline(font, id) {
                    outlined += outline.elements().len() as f64;
                    defined += 1.0;
                }
            }
            let run = Glyphs {
                size: 20.0,
                transform: Affine::scale(0.03),
                paint: Paint::Ink,
                run: scene::Run::Math {
                    font,
                    glyphs: &glyphs,
                },
            };
            let row_name = format!(
                "a glyph of font {font} defined and used ({defined}, {:.1} segments)",
                outlined / defined
            );
            let drawn = on_a_page(|target| {
                target.item(&mut tools, &Item::Glyphs(run.clone())).unwrap();
            });
            row(&row_name, defined, drawn);
        }

        // Fonts subset when the file is finished: one glyph of each font, then every glyph of
        // the text font that a character maps to.
        let mut shaper = TextShaper::new();
        for (name, text) in [("Inter", "x"), ("Libertinus Math", "\u{2227}")] {
            let layout = shaper.build(text);
            let items: Vec<Item<'_>> =
                scene::text_items(&layout, text, Affine::scale(0.025), Paint::Ink).collect();
            let mut least = (f64::INFINITY, f64::INFINITY, 0, Vec::new());
            for _ in 0..9 {
                let run = on_a_page(|target| {
                    for item in &items {
                        target.item(&mut tools, item).unwrap();
                    }
                });
                if run.1 < least.1 {
                    least = run;
                }
            }
            row(&format!("a font subset ({name}, one glyph)"), 1.0, least);
        }
        let every: String = (0x20..0x3000_u32)
            .filter_map(char::from_u32)
            .filter(|c| !c.is_control() && tools.glyphs.char_glyph(0, *c).is_some())
            .collect();
        let distinct: std::collections::HashSet<u32> = every
            .chars()
            .filter_map(|c| tools.glyphs.char_glyph(0, c).map(|(id, _)| id))
            .collect();
        let layout = shaper.build(&every);
        let items: Vec<Item<'_>> =
            scene::text_items(&layout, &every, Affine::scale(0.025), Paint::Ink).collect();
        let run = on_a_page(|target| {
            for item in &items {
                target.item(&mut tools, item).unwrap();
            }
        });
        row(
            &format!("a text glyph subset ({} distinct)", distinct.len()),
            distinct.len() as f64,
            run,
        );
    }

    /// Times exporting each of [`calibration_boards`], and asserts in release that none took
    /// more than 1.2 times its estimate, give or take 0.5 ms, and that none wrote more content
    /// than its estimate.
    fn assert_exports_within_estimates(renderer: &mut Renderer) {
        let mut over = Vec::new();
        for (name, board) in calibration_boards() {
            let mut budget = Budget::unlimited();
            let mut plan = renderer
                .plan_export(&board, None, &ExportLimits::DEFAULT, &mut budget)
                .unwrap();
            let options = ExportOptions {
                format: lawh_core::ExportFormat::Pdf,
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
            let written = write_once(renderer);
            let estimate_ms = written.budget.spent / 1e6;
            let content = content_bytes(&written.bytes);
            let mut took_ms = f64::INFINITY;
            for _ in 0..3 {
                took_ms = took_ms.min(
                    median_ns(5, || {
                        black_box(write_once(renderer));
                    }) / 1e6,
                );
                if took_ms <= 1.2 * estimate_ms + 0.5 {
                    break;
                }
            }
            println!(
                "{name:<32} est {estimate_ms:>9.3} ms  took {took_ms:>9.3} ms  ratio {:>5.2}  \
                 content {content:>9} B est {:>9} B ({:>4.2})  file {:>9} B",
                took_ms / estimate_ms,
                written.content,
                written.content as f64 / content as f64,
                written.bytes.len()
            );
            if took_ms > 1.2 * estimate_ms + 0.5 || written.content < content {
                over.push(name);
            }
        }
        if !cfg!(debug_assertions) {
            assert!(over.is_empty(), "over their estimates: {over:?}");
        }
    }

    /// Boards that each stress one part of writing a PDF.
    fn calibration_boards() -> Vec<(&'static str, Board)> {
        let board = |src: String| lawh_core::from_lawh(&format!("lawh version=1\n{src}")).unwrap();
        let many = |each: &dyn Fn(usize) -> String| (0..MAX_OBJECTS).map(each).collect::<String>();
        let line: String = "lorem ipsum dolor sit amet, consectetur adipiscing elit "
            .chars()
            .cycle()
            .take(MAX_BOARD_TEXT_CHARS / 250)
            .collect();
        // Every letter of Latin, Greek and Cyrillic and every mathematical operator: glyphs
        // subset once each.
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
                "500 formulas in 500 colours",
                board(many(&|i| {
                    format!(
                        "math id=m{i} x={} y={} \"#text(fill: rgb({}, 0, 0))[$sum_(i=1)^n x_i$]\"\n",
                        (i % 10) * 12,
                        (i / 10) * 3,
                        i % 256
                    )
                })),
            ),
            (
                "500 failing formulas",
                board(many(&|i| format!("math id=m{i} x=0 y={i} \"frac({i},\"\n"))),
            ),
            (
                "20 formulas of 22,500 glyphs",
                board((0..20).fold(String::new(), |mut src, i| {
                    writeln!(
                        src,
                        "math id=m{i} x=0 y={} \"#range(150).map(_ => \\\"x\\\" * 150).join(linebreak())\"",
                        i * 200
                    )
                    .unwrap();
                    src
                })),
            ),
        ]
    }

    #[test]
    fn pages_are_whole_pixels_at_30_pt_a_unit_or_shrink_to_fit() {
        // 10 x 4.05 units: 300 x 121.5 pt, sides rounded up to 0.75 pt.
        let page = fit(Rect::new(-1.0, 2.0, 9.0, 6.05), 14_400.0).unwrap();
        assert_eq!(page.pt_per_unit, 30.0);
        assert_eq!((page.width, page.height), (300.0, 121.5));
        let page = fit(Rect::new(0.0, 0.0, 10.01, 1.0), 14_400.0).unwrap();
        assert_eq!(page.width, 300.75);
        // 480 units fit at full size; past that, the longest side is at the cap.
        assert_eq!(
            fit(Rect::new(0.0, 0.0, 480.0, 1.0), 14_400.0)
                .unwrap()
                .pt_per_unit,
            30.0
        );
        for (frame, cap) in [
            (Rect::new(0.0, 0.0, 481.0, 10.0), 14_400.0),
            (Rect::new(-2e6, -1e6, 2e6, 1.5e6), 14_400.0),
            (Rect::new(0.0, 0.0, 33.3, 77.7), 300.0),
        ] {
            let page = fit(frame, cap).unwrap();
            assert!(page.width <= cap && page.height <= cap, "{page:?}");
            assert!(page.width.max(page.height) > cap - 1.0, "{page:?}");
            assert!(page.pt_per_unit < 30.0, "{page:?}");
        }
        // A side shrunk to almost nothing is still 3 pt.
        let line = fit(Rect::new(0.0, 0.0, 4e6, 0.1), 14_400.0).unwrap();
        assert_eq!(line.height, 3.0);
        assert_eq!(fit(Rect::ZERO, 14_400.0), None);
        assert_eq!(fit(Rect::new(0.0, 0.0, f64::INFINITY, 1.0), 14_400.0), None);
        assert_eq!(fit(Rect::new(0.0, 0.0, 1.0, 1.0), 1.0), None);
    }

    #[test]
    fn a_glyph_its_font_lacks_is_an_error_not_a_panic() {
        use lawh_math::MathGlyph;

        use crate::scene::Run;

        let mut renderer = Renderer::new_trusted();
        let count = renderer.glyphs.glyph_count(1);
        assert!(count > 4_000, "{count}");
        // Just past the font's glyphs (krilla would fail the whole file), and past 16 bits
        // (krilla would panic).
        for id in [count, 65_535, 65_536, 70_000, u32::MAX] {
            let mut document = Document::new_with(serialize_settings());
            let mut page = document.start_page_with(PageSettings::from_wh(100.0, 100.0).unwrap());
            let mut target = PdfTarget::new(
                page.surface(),
                Affine::IDENTITY,
                Budget::unlimited(),
                usize::MAX,
            );
            let glyphs = [
                MathGlyph {
                    id: 3,
                    x: 0.0,
                    y: 10.0,
                },
                MathGlyph {
                    id,
                    x: 5.0,
                    y: 10.0,
                },
            ];
            let run = Item::Glyphs(Glyphs {
                size: 10.0,
                transform: Affine::IDENTITY,
                paint: Paint::Ink,
                run: Run::Math {
                    font: 1,
                    glyphs: &glyphs,
                },
            });
            let mut tools = renderer.tools();
            assert_eq!(
                target.item(&mut tools, &run),
                Err(ExportError::Pdf(format!("glyph {id} is not in font 1")))
            );
            drop(target);
            page.finish();
            // Nothing krilla can't take reached it: the file still finishes.
            assert!(document.finish().is_ok(), "{id}");
        }
        // A font the bundle doesn't have has no glyphs.
        assert_eq!(renderer.glyphs.glyph_count(7), 0);
    }

    #[test]
    fn paths_and_transforms_reach_krilla_only_when_finite() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((1.0, 2.0));
        assert!(pdf_path(&path, Affine::scale(30.0)).is_some());
        assert!(pdf_path(&path, Affine::scale(1e300)).is_none());
        path.line_to((f64::NAN, 0.0));
        assert!(pdf_path(&path, Affine::IDENTITY).is_none());
        assert!(pdf_path(&BezPath::new(), Affine::IDENTITY).is_none());
        assert!(pdf_transform(Affine::scale(2.0)).is_some());
        assert!(pdf_transform(Affine::translate((1e40, 0.0))).is_none());
        assert!(pdf_transform(Affine::scale(f64::NAN)).is_none());
    }
}
