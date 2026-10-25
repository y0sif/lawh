//! Exports: a whole board, or the objects of a selection, drawn into one file's bytes.
//!
//! [`Renderer::export`] frames what it exports (the bounds of every object it draws, strokes and
//! arrowheads included, and [`EXPORT_MARGIN_UNITS`] around them) and draws it from the shared
//! draw list (`crate::scene`) at the window's zoom-1 sizes, without the grid, the overlay or any
//! placeholder: it waits for every formula first, and a formula that fails is drawn as its source
//! in the error colour, as on screen, and counted ([`ExportReport::failed_formulas`]). A formula
//! that laid out then but fails when it is fetched again to be drawn stops the export
//! ([`ExportError::MathFailedWhileDrawing`]): the frame was made for its box, not its source.
//!
//! Formats: PNG (`png.rs`, drawn with `vello_cpu`), SVG (`svg.rs`, written by hand: paths and
//! glyph outlines only, no board text in the file) and PDF (`pdf.rs`, written with krilla: one
//! page, board text as real text in embedded font subsets, math as paths).
//!
//! An export is bounded however hostile the board: the formulas' compiles by
//! [`EXPORT_MATH_WAIT`] together, the drawing (or writing) and a PNG's encoding by the export
//! budget (`EXPORT_BUDGET_NS` of estimated work, as the window's frames are), a PNG's size by
//! [`MAX_EXPORT_PIXELS`] and [`MAX_EXPORT_SIDE_PX`] and a PDF page's by [`MAX_PDF_PAGE_PT`] (both
//! shrink to fit), and the file by [`MAX_EXPORT_BYTES`]. Past a cap that stops it, it returns an
//! [`ExportError`] and no bytes. Nothing is written to disk here.
//!
//! Memory: a first pass compiles each formula and keeps only its box, then objects are drawn one
//! at a time, so an export holds one formula's layout at once, not every formula's (the math
//! engine caches some, within its own limit). A PNG's image is moved into the encoder, never
//! copied, and its file streamed through the byte cap.

mod pdf;
mod png;
mod svg;

use std::collections::HashSet;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kurbo::Rect;
use lawh_core::{Board, ExportFormat, ExportOptions, ExportReport, Object, ObjectId, Shape};
use lawh_math::{MathEngine, MathError, MathLayout};

pub(crate) use self::png::encode as encode_png;
use crate::budget::{self, Budget};
use crate::renderer::{MathBox, layout_box, text_box, text_transform};
use crate::scene::{self, Item, SceneObject, Sink, Space, Tally, TextRole, Tools};
use crate::text::TextKey;
use crate::{BASE_PX_PER_UNIT, Renderer, draw};

/// What all of one export's formulas may take together, waiting for the math worker to compile
/// them. Each formula keeps the math engine's own timeout (`lawh_math::DEFAULT_TIMEOUT`, 2 s),
/// so a board of formulas that each take almost that long stops after about five, while a board
/// of real formulas takes milliseconds (a cold compile is about 7 ms, `m1-measurements.md`).
/// Each formula's wait ends when the export's time runs out (`MathEngine::layout_until`), so an
/// export stops within milliseconds of this however long one formula would take (200.1 ms for
/// a limit of 200 ms and a formula of a second, `the_math_wait_ends_on_time`). A formula cut
/// short so is not cached as failed: it carries on compiling, and the next export finds it.
pub const EXPORT_MATH_WAIT: Duration = Duration::from_secs(10);

/// Most pixels a PNG export may have, 8,000 x 8,000; past it the scale shrinks to fit. Its image
/// takes 4 bytes a pixel while it is drawn, 256 MB at the cap, and what `vello_cpu` keeps of the
/// paths drawn into it comes on top until it renders (the most measured: 394 MB in all for two
/// scribbles of 2,000 lines across it, in release). Drawing into it and encoding it are charged
/// to the export's budget: encoding an image this size with fdeflate takes about a second at
/// worst and is charged 1.25 s (1.66 s with transparency, `png.rs`).
pub const MAX_EXPORT_PIXELS: u64 = 64_000_000;

/// Most pixels a side of a PNG export may have; past it the scale shrinks to fit. A common
/// limit of the textures image viewers draw with on the GPU, and inside the `u16` sizes
/// `vello_cpu` draws.
pub const MAX_EXPORT_SIDE_PX: u32 = 16_384;

/// Largest file an export may make, in bytes (64 MiB), counted while it is written: past it the
/// export stops and returns nothing. A PNG streams its compressed data through the cap, so it
/// stops within a chunk of it: a board's PNG at the pixel caps takes a few megabytes (the area
/// board at `pngscale` 8 is 0.32 MB), and only hostile images get near it (two scribbles of
/// 2,000 lines across the image: 65 MB; noise stops here after 0.35 s, measured in release). An
/// SVG takes about 35 bytes a glyph drawn: a board at lawh-core's 40,000 characters of text is
/// 1.2 MB, and only math near lawh-math's glyph caps reaches this (500 formulas of 22,500 glyphs
/// stopped here after 110 ms, measured in release). krilla builds a PDF in memory, so a PDF
/// counts the content it draws, uncompressed and estimated a little high, as it goes (44 to 63
/// bytes a math glyph, 8.5 a glyph of text), and its finished file once more, fonts included:
/// the 40,000 characters take 0.35 MB of content and a 12 KB file, and the 500 formulas stopped
/// here after 1.1 s with 154 MB of memory at the peak (measured in release).
pub const MAX_EXPORT_BYTES: usize = 64 * 1024 * 1024;

/// Longest side a PDF export's page may have, in pt (200 inches); past it the scale shrinks to
/// fit. PDF 1.x's implementation limit (ISO 32000-1, Annex C, which Acrobat keeps): Reader warns
/// that a larger page is out of range, Ghostscript fails on a page of 1.2e8 pt, and PDF/A-2
/// refuses one (veraPDF rule 6.1.13-11), though krilla writes any size (measured,
/// `docs/handoff-m4-research.md`, R2). A board up to 480 units across (with the margin) keeps
/// [`PDF_PT_PER_UNIT`].
pub const MAX_PDF_PAGE_PT: f64 = 14_400.0;

/// Points per grid unit of a PDF export: lawh at zoom 1 seen at 100 % in a viewer (40 pixels a
/// unit at 0.75 pt a pixel, a 96 dpi screen), so text of size 1 is set at 15 pt.
pub const PDF_PT_PER_UNIT: f64 = 30.0;

/// Room around what an export frames, in grid units, on each side: a grid cell, so nothing
/// touches the edge of the image.
pub const EXPORT_MARGIN_UNITS: f64 = 1.0;

/// Width of shape outlines in an export, in grid units: the window's at zoom 1 (2 logical
/// pixels at 40 a unit), 0.05.
pub const EXPORT_STROKE_UNITS: f64 = draw::STROKE_PX / BASE_PX_PER_UNIT;

/// Length of arrowheads in an export, in grid units: the window's at zoom 1, its length in grid
/// units kept between its pixel limits at 40 pixels a unit, 0.3.
pub const EXPORT_ARROW_HEAD_UNITS: f64 = draw::ARROW_HEAD_UNITS.clamp(
    draw::ARROW_HEAD_MIN_PX / BASE_PX_PER_UNIT,
    draw::ARROW_HEAD_MAX_PX / BASE_PX_PER_UNIT,
);

/// An exported file's bytes, and what the export has to say about them.
#[derive(Debug, Clone, PartialEq)]
pub struct Exported {
    pub bytes: Vec<u8>,
    pub report: ExportReport,
}

/// Why an export made no file. Each reads as a short phrase the status line shows after the
/// file's name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExportError {
    /// Nothing to draw: the board is empty, the selection is, or none of its objects is on the
    /// board or has finite bounds.
    #[error("nothing to export")]
    Empty,
    /// Drawing it is estimated to cost more than an export's budget (`EXPORT_BUDGET_NS`).
    #[error("export stopped: drawing it costs too much")]
    OverBudget,
    /// The formulas took longer than [`EXPORT_MATH_WAIT`] to compile, together.
    #[error("export stopped: math took over {} s to compile", EXPORT_MATH_WAIT.as_secs())]
    MathWait,
    /// A formula that laid out while the export was framed failed when it was fetched again to
    /// be drawn: the math engine had dropped its layout since, and compiling it again failed
    /// (the worker crashed, say). The frame was made for the formula's box, so its source drawn
    /// in its place could be cut off; the export stops instead, and the next one frames the
    /// formula as it is then.
    #[error("export stopped: math failed while drawing; export again")]
    MathFailedWhileDrawing,
    /// The file came to more than [`MAX_EXPORT_BYTES`]; `bytes` is how many it had reached when
    /// it stopped (for a PDF stopped while drawing, its content's estimate).
    #[error("export stopped: the file is over {} MiB", MAX_EXPORT_BYTES / (1024 * 1024))]
    TooLarge { bytes: usize },
    /// The PNG encoder failed, other than for the file's size.
    #[error("png encoding failed: {0}")]
    Png(String),
    /// The PDF writer failed (krilla, or a glyph its font doesn't have), other than for the
    /// file's size.
    #[error("pdf writing failed: {0}")]
    Pdf(String),
}

/// The caps an export stops or shrinks at, for tests that need them small. Each is at most its
/// default ([`ExportLimits::DEFAULT`]): [`Renderer::export_with_limits`] lowers any above it, so
/// no caller can make an export cost more.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExportLimits {
    /// Estimated nanoseconds of drawing (`EXPORT_BUDGET_NS`).
    pub budget_ns: f64,
    /// [`EXPORT_MATH_WAIT`].
    pub math_wait: Duration,
    /// [`MAX_EXPORT_PIXELS`].
    pub max_pixels: u64,
    /// [`MAX_EXPORT_SIDE_PX`].
    pub max_side_px: u32,
    /// [`MAX_EXPORT_BYTES`].
    pub max_bytes: usize,
    /// [`MAX_PDF_PAGE_PT`].
    pub max_page_pt: f64,
}

impl ExportLimits {
    pub const DEFAULT: ExportLimits = ExportLimits {
        budget_ns: budget::EXPORT_BUDGET_NS,
        math_wait: EXPORT_MATH_WAIT,
        max_pixels: MAX_EXPORT_PIXELS,
        max_side_px: MAX_EXPORT_SIDE_PX,
        max_bytes: MAX_EXPORT_BYTES,
        max_page_pt: MAX_PDF_PAGE_PT,
    };

    /// Each limit, or its default where it is higher.
    fn tightened(self) -> Self {
        let default = Self::DEFAULT;
        Self {
            budget_ns: if self.budget_ns < default.budget_ns {
                self.budget_ns
            } else {
                default.budget_ns
            },
            math_wait: self.math_wait.min(default.math_wait),
            max_pixels: self.max_pixels.min(default.max_pixels),
            max_side_px: self.max_side_px.min(default.max_side_px),
            max_bytes: self.max_bytes.min(default.max_bytes),
            max_page_pt: if self.max_page_pt < default.max_page_pt {
                self.max_page_pt
            } else {
                default.max_page_pt
            },
        }
    }
}

impl Default for ExportLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Renderer {
    /// Draws `board`, or only the objects of `selection` (in board order, framed to them), into
    /// one file's bytes in `options.format`. Waits for every formula in the math engine (the
    /// worker, for a board from a file) first; see the module docs for what is drawn and the
    /// caps. Nothing is written to disk here.
    pub fn export(
        &mut self,
        board: &Board,
        selection: Option<&[ObjectId]>,
        options: &ExportOptions,
    ) -> Result<Exported, ExportError> {
        self.export_with_limits(board, selection, options, &ExportLimits::DEFAULT)
    }

    /// [`Self::export`] within `limits`, each lowered to its default if above it.
    #[doc(hidden)]
    pub fn export_with_limits(
        &mut self,
        board: &Board,
        selection: Option<&[ObjectId]>,
        options: &ExportOptions,
        limits: &ExportLimits,
    ) -> Result<Exported, ExportError> {
        let limits = limits.tightened();
        let mut budget = Budget::export_within(limits.budget_ns);
        let mut plan = self.plan_export(board, selection, &limits, &mut budget)?;
        let (bytes, shrunk) = match options.format {
            ExportFormat::Png => png::export(self, board, &mut plan, *options, &limits, budget)?,
            ExportFormat::Svg => svg::export(self, board, &mut plan, *options, &limits, budget)?,
            ExportFormat::Pdf => pdf::export(self, board, &mut plan, *options, &limits, budget)?,
        };
        tracing::debug!(
            bytes = bytes.len(),
            objects = plan.objects.len(),
            failed_formulas = plan.failed,
            ?shrunk,
            math_waited = ?plan.math.waited,
            "exported"
        );
        Ok(Exported {
            bytes,
            report: ExportReport {
                failed_formulas: plan.failed,
                shrunk,
            },
        })
    }

    /// Picks the objects to export, waits for their formulas and finds their bounds: what
    /// drawing them takes, apart from the target.
    fn plan_export(
        &mut self,
        board: &Board,
        selection: Option<&[ObjectId]>,
        limits: &ExportLimits,
        budget: &mut Budget,
    ) -> Result<ExportPlan, ExportError> {
        let wanted: Option<HashSet<&ObjectId>> = selection.map(|ids| ids.iter().collect());
        let mut objects = Vec::new();
        let mut hashed = 0;
        for (index, object) in board.objects().iter().enumerate() {
            if wanted
                .as_ref()
                .is_some_and(|wanted| !wanted.contains(&object.id))
            {
                continue;
            }
            let key = source(object).map(|text| {
                hashed += text.len();
                self.text.key(text)
            });
            objects.push(Planned {
                index,
                key,
                look: Look::Drawn,
                bounds: Rect::ZERO,
            });
        }
        budget.charge(budget::hashing(hashed));
        if objects.is_empty() {
            return Err(ExportError::Empty);
        }

        // Every formula, waited for in turn; only its box is kept.
        let mut plan = ExportPlan {
            objects,
            frame: Rect::ZERO,
            failed: 0,
            math: MathClock {
                waited: Duration::ZERO,
                limit: limits.math_wait,
            },
        };
        for planned in &mut plan.objects {
            let Some(object) = board.objects().get(planned.index) else {
                continue;
            };
            let Shape::Math { src, .. } = &object.shape else {
                continue;
            };
            planned.look = match plan.math.layout(&self.math, src)? {
                Ok(layout) => Look::Math(layout_box(&layout)),
                Err(err) => {
                    tracing::debug!(id = %object.id, %err, "exporting math source instead of math");
                    Look::Failed
                }
            };
        }

        let mut frame: Option<Rect> = None;
        let mut drawn = Vec::with_capacity(plan.objects.len());
        for mut planned in std::mem::take(&mut plan.objects) {
            let Some(object) = board.objects().get(planned.index) else {
                continue;
            };
            let Some(bounds) = self.planned_bounds(object, &planned, budget)? else {
                tracing::debug!(id = %object.id, "not exported: no finite bounds");
                continue;
            };
            if planned.look == Look::Failed {
                plan.failed += 1;
            }
            planned.bounds = bounds;
            frame = Some(frame.map_or(bounds, |frame| frame.union(bounds)));
            drawn.push(planned);
        }
        plan.objects = drawn;
        let frame = frame
            .map(|frame| frame.inflate(EXPORT_MARGIN_UNITS, EXPORT_MARGIN_UNITS))
            .filter(|frame| frame.is_finite() && frame.width() > 0.0 && frame.height() > 0.0)
            .ok_or(ExportError::Empty)?;
        plan.frame = frame;
        Ok(plan)
    }

    /// The bounds of what `object` draws in an export, laying its text out if it must; `None`
    /// when they aren't finite.
    fn planned_bounds(
        &mut self,
        object: &Object,
        planned: &Planned,
        budget: &mut Budget,
    ) -> Result<Option<Rect>, ExportError> {
        let bounds = match (&object.shape, planned.look, planned.key) {
            (Shape::Text { at, text, size }, _, Some(key))
            | (
                Shape::Math {
                    at,
                    src: text,
                    size,
                },
                Look::Failed,
                Some(key),
            ) => {
                let (width, height) = self.lay_out_for_export(key, text, budget)?;
                Some(text_box(*at, *size, width, height))
            }
            (Shape::Math { at, size, .. }, Look::Math(placed), _) => Some(
                self.math_to_board(*at, *size, placed.baseline)
                    .transform_rect_bbox(placed.bounds),
            ),
            (shape, ..) => scene::shape_bounds(shape, Space::Board),
        };
        Ok(bounds.filter(Rect::is_finite))
    }

    /// Lays `text` out unless its layout is cached, charging what that is estimated to cost to
    /// `budget`, and returns its size in layout units.
    fn lay_out_for_export(
        &mut self,
        key: TextKey,
        text: &str,
        budget: &mut Budget,
    ) -> Result<(f32, f32), ExportError> {
        let known = self.text.known(key, text);
        if !known.shaped && !budget.spend(known.facts.layout_cost()) {
            return Err(ExportError::OverBudget);
        }
        Ok(self.text.lay_out(key, text, &mut self.glyphs))
    }

    /// Draws every object of `plan` into `sink`, in board order, one at a time.
    fn draw_export<S: ExportSink>(
        &mut self,
        board: &Board,
        plan: &mut ExportPlan,
        sink: &mut S,
    ) -> Result<(), ExportError> {
        for index in 0..plan.objects.len() {
            let Some(planned) = plan.objects.get(index).copied() else {
                continue;
            };
            let Some(object) = board.objects().get(planned.index) else {
                continue;
            };
            self.draw_planned(object, planned, plan, sink)?;
        }
        Ok(())
    }

    /// Hands `object`'s items to `sink`.
    fn draw_planned<S: ExportSink>(
        &mut self,
        object: &Object,
        planned: Planned,
        plan: &mut ExportPlan,
        sink: &mut S,
    ) -> Result<(), ExportError> {
        let (at, size, text, role) = match (&object.shape, planned.look, planned.key) {
            (Shape::Text { at, text, size }, _, Some(key)) => {
                (*at, *size, (key, text), TextRole::Text)
            }
            (Shape::Math { at, src, size }, Look::Failed, Some(key)) => {
                (*at, *size, (key, src), TextRole::Failed)
            }
            (Shape::Math { at, src, size }, Look::Math(_), _) => {
                // Compiled again if the engine dropped it since; held only while it is drawn.
                let layout = plan.math.layout(&self.math, src)?.map_err(|err| {
                    // The frame was made for the formula, so its source drawn in its place could
                    // be cut off: no file, rather than a wrong one.
                    tracing::warn!(id = %object.id, %err, "math failed when drawn for export");
                    ExportError::MathFailedWhileDrawing
                })?;
                let transform = self.math_transform(Space::Board, *at, *size, layout.baseline);
                let mut tools = self.tools();
                sink.begin(&mut tools, &object.id, planned.bounds)?;
                for item in scene::math_items(&layout, transform) {
                    sink.item(&mut tools, &item)?;
                }
                return Ok(());
            }
            (shape, ..) => {
                let mut tools = self.tools();
                sink.begin(&mut tools, &object.id, planned.bounds)?;
                for item in scene::shape_items(shape, Space::Board) {
                    sink.item(&mut tools, &item)?;
                }
                return Ok(());
            }
        };
        let (key, text) = text;
        self.lay_out_for_export(key, text, sink.budget())?;
        let transform = text_transform(Space::Board, at, size);
        let Renderer {
            text: cache,
            glyphs,
            math_fonts,
            palette,
            ..
        } = self;
        let mut tools = Tools {
            glyphs,
            fonts: math_fonts,
            palette,
        };
        let Some(shaped) = cache.shaped(key, text) else {
            // Laid out just above; a text cache too small to hold one text would get here.
            tracing::warn!(id = %object.id, "text not laid out for export");
            return Ok(());
        };
        sink.begin(&mut tools, &object.id, planned.bounds)?;
        let paint = scene::text_paint(role);
        for item in scene::text_items(&shaped.layout, text, transform, paint) {
            sink.item(&mut tools, &item)?;
        }
        Ok(())
    }

    /// What sinks may use of this renderer's while they draw.
    fn tools(&mut self) -> Tools<'_> {
        Tools {
            glyphs: &mut self.glyphs,
            fonts: &self.math_fonts,
            palette: &self.palette,
        }
    }

    /// What exporting `board` (or only the objects of `selection`) hands its sink, object by
    /// object, and the frame it exports, in grid units: for tests that hold the exports to the
    /// window's draw list ([`Self::record_frame`]). Waits for math as [`Self::export`] does, but
    /// draws no file.
    #[doc(hidden)]
    pub fn record_export(
        &mut self,
        board: &Board,
        selection: Option<&[ObjectId]>,
    ) -> Result<(Rect, Vec<SceneObject>), ExportError> {
        let mut budget = Budget::export();
        let mut plan = self.plan_export(board, selection, &ExportLimits::DEFAULT, &mut budget)?;
        let mut sink = Recording {
            sink: Tally::default(),
            budget,
        };
        self.draw_export(board, &mut plan, &mut sink)?;
        Ok((plan.frame, sink.sink.objects))
    }
}

/// A sink an export draws into: it charges what it draws to the export's budget, which the
/// export also charges for laying out text it must.
trait ExportSink: Sink<Error = ExportError> {
    fn budget(&mut self) -> &mut Budget;
}

/// An export sink that hands what it is given to `sink` (a [`Tally`], or in tests a
/// `scene::Recorder`), drawing nothing, with the export's budget.
struct Recording<S> {
    sink: S,
    budget: Budget,
}

impl<S: Sink<Error = Infallible>> Sink for Recording<S> {
    type Error = ExportError;

    fn begin(
        &mut self,
        tools: &mut Tools<'_>,
        id: &ObjectId,
        bounds: Rect,
    ) -> Result<(), ExportError> {
        let Ok(()) = self.sink.begin(tools, id, bounds);
        Ok(())
    }

    fn item(&mut self, tools: &mut Tools<'_>, item: &Item<'_>) -> Result<(), ExportError> {
        let Ok(()) = self.sink.item(tools, item);
        Ok(())
    }
}

impl<S: Sink<Error = Infallible>> ExportSink for Recording<S> {
    fn budget(&mut self) -> &mut Budget {
        &mut self.budget
    }
}

/// What an export draws, found before it draws anything.
struct ExportPlan {
    /// The objects drawn, in board order.
    objects: Vec<Planned>,
    /// The bounds of all of them, with the margin, in grid units.
    frame: Rect,
    /// Formulas drawn as their source because they failed.
    failed: usize,
    math: MathClock,
}

/// How long an export has waited for math, and may.
#[derive(Debug, Clone, Copy)]
struct MathClock {
    waited: Duration,
    limit: Duration,
}

impl MathClock {
    /// Waits for `src` in `engine`, no longer than what is left of the limit
    /// ([`MathEngine::layout_until`]), counting the wait; [`ExportError::MathWait`] once the
    /// waits add up to the limit. A formula cut short by the limit is not cached as failed: the
    /// math engine finishes compiling it, so the next export finds it.
    fn layout(
        &mut self,
        engine: &MathEngine,
        src: &str,
    ) -> Result<Result<Arc<MathLayout>, MathError>, ExportError> {
        let started = Instant::now();
        let left = self.limit.saturating_sub(self.waited);
        let layout = match started.checked_add(left) {
            Some(deadline) => engine.layout_until(src, deadline),
            // A limit too far off to be an instant: none.
            None => engine.layout_blocking(src),
        };
        self.waited = self.waited.saturating_add(started.elapsed());
        // A wait the deadline ended took all that was left, so the waits reach the limit.
        if self.waited >= self.limit {
            return Err(ExportError::MathWait);
        }
        Ok(layout)
    }
}

/// One object an export draws.
#[derive(Debug, Clone, Copy)]
struct Planned {
    /// Its index in the board.
    index: usize,
    /// The key of its text or source.
    key: Option<TextKey>,
    look: Look,
    /// What it draws, in grid units.
    bounds: Rect,
}

/// How an object is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Look {
    /// As itself: a shape, a text.
    Drawn,
    /// A formula that laid out, placed by its box.
    Math(MathBox),
    /// A formula that failed: its source as text, in the error colour.
    Failed,
}

/// The text an object draws, or a formula's source.
fn source(object: &Object) -> Option<&str> {
    match &object.shape {
        Shape::Text { text, .. } => Some(text),
        Shape::Math { src, .. } => Some(src),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Outline, Paint, Recorded, RecordedItem, Recorder};

    /// What exporting `selection` of `board` hands its sink, and the plan's frame and failures.
    fn record(board: &Board, selection: Option<&[ObjectId]>) -> (Vec<Recorded>, Rect, usize) {
        let mut renderer = Renderer::new_trusted();
        let mut budget = Budget::export();
        let mut plan = renderer
            .plan_export(board, selection, &ExportLimits::DEFAULT, &mut budget)
            .unwrap();
        let mut sink = Recording {
            sink: Recorder::default(),
            budget,
        };
        renderer.draw_export(board, &mut plan, &mut sink).unwrap();
        (sink.sink.objects, plan.frame, plan.failed)
    }

    fn id(name: &str) -> ObjectId {
        ObjectId::new(name).unwrap()
    }

    #[test]
    fn an_export_hands_every_object_to_its_sink_in_board_order() {
        let board = lawh_core::from_lawh(
            "lawh version=1\n\
             arrow id=a1 x1=0 y1=0 x2=4 y2=0\n\
             text id=t1 x=0 y=1 \"two\\nlines\"\n\
             math id=m1 x=0 y=3 \"A = pi r^2\"\n\
             math id=m2 x=0 y=5 \"frac(1,\"\n\
             circle id=c1 x=10 y=10 r=1\n",
        )
        .unwrap();
        let (objects, frame, failed) = record(&board, None);
        let ids: Vec<_> = objects.iter().map(|object| object.id.as_str()).collect();
        assert_eq!(ids, ["a1", "t1", "m1", "m2", "c1"]);
        assert_eq!(failed, 1);

        // The arrow: its shaft stroked at the zoom-1 width, its head filled; its bounds hold
        // the head's corners.
        let arrow = &objects[0];
        assert!(matches!(
            arrow.items[..],
            [
                RecordedItem::Stroke {
                    outline: Outline::Line(_),
                    paint: Paint::Ink,
                    ..
                },
                RecordedItem::Fill {
                    paint: Paint::Ink,
                    ..
                }
            ]
        ));
        assert!(arrow.bounds.y1 >= 0.4 * EXPORT_ARROW_HEAD_UNITS - 1e-9);
        // The text: a glyph run a line, its glyphs and its text there for every format.
        let text = &objects[1];
        assert_eq!(text.items.len(), 2);
        for item in &text.items {
            let RecordedItem::Glyphs {
                paint,
                glyphs,
                text,
                ..
            } = item
            else {
                panic!("{item:?}");
            };
            assert_eq!((*paint, text.as_deref()), (Paint::Ink, Some("two\nlines")));
            assert!(glyphs.len() >= 3);
        }
        // The formula: its own items, in ink; the failed one: its source as text, in red.
        let math = &objects[2];
        assert!(math.items.iter().any(|item| matches!(
            item,
            RecordedItem::Glyphs {
                paint: Paint::Ink,
                text: None,
                ..
            }
        )));
        let failed = &objects[3];
        assert!(failed.items.iter().all(|item| matches!(
            item,
            RecordedItem::Glyphs {
                paint: Paint::Error,
                text: Some(text),
                ..
            } if text == "frac(1,"
        )));
        // The frame holds every object's bounds, and the margin.
        for object in &objects {
            let inner = frame.inflate(1e-9 - EXPORT_MARGIN_UNITS, 1e-9 - EXPORT_MARGIN_UNITS);
            assert!(inner.contains_rect(object.bounds), "{object:?}");
        }
        assert!((frame.x1 - (11.0 + EXPORT_STROKE_UNITS / 2.0 + EXPORT_MARGIN_UNITS)).abs() < 1e-9);
    }

    #[test]
    fn a_selection_hands_only_its_objects_over() {
        let board = lawh_core::from_lawh(
            "lawh version=1\n\
             circle id=c1 x=0 y=0 r=1\n\
             circle id=c2 x=10 y=0 r=1\n\
             circle id=c3 x=20 y=0 r=1\n",
        )
        .unwrap();
        let (objects, frame, _) = record(&board, Some(&[id("c3"), id("c1")]));
        let ids: Vec<_> = objects.iter().map(|object| object.id.as_str()).collect();
        assert_eq!(ids, ["c1", "c3"]);
        let half = EXPORT_STROKE_UNITS / 2.0;
        let margin = EXPORT_MARGIN_UNITS;
        let expected =
            Rect::new(-1.0 - half, -1.0 - half, 21.0 + half, 1.0 + half).inflate(margin, margin);
        assert!((frame.x0 - expected.x0).abs() < 1e-9 && (frame.x1 - expected.x1).abs() < 1e-9);
        assert!((frame.y0 - expected.y0).abs() < 1e-9 && (frame.y1 - expected.y1).abs() < 1e-9);
    }

    #[test]
    fn a_formula_that_fails_when_it_is_drawn_stops_the_export() {
        // A formula that lays out while the export is framed, then fails when it is fetched
        // again to be drawn (its layout dropped, the worker crashed compiling it again): a fresh
        // engine whose every compile times out stands in for that second fetch.
        let board = lawh_core::from_lawh(
            "lawh version=1\n\
             math id=m1 x=0 y=0 \"sum_(i=1)^n i\"\n\
             circle id=c1 x=5 y=0 r=1\n",
        )
        .unwrap();
        let limits = ExportLimits::DEFAULT;
        for format in [ExportFormat::Png, ExportFormat::Svg, ExportFormat::Pdf] {
            let mut renderer = Renderer::new_trusted();
            let mut budget = Budget::export();
            let mut plan = renderer
                .plan_export(&board, None, &limits, &mut budget)
                .unwrap();
            assert_eq!(plan.failed, 0);
            assert!(matches!(plan.objects[0].look, Look::Math(_)));
            renderer.math = MathEngine::new().with_timeout(Duration::ZERO);
            let options = ExportOptions {
                format,
                ..ExportOptions::default()
            };
            let written = match format {
                ExportFormat::Png => {
                    png::export(&mut renderer, &board, &mut plan, options, &limits, budget)
                }
                ExportFormat::Svg => {
                    svg::export(&mut renderer, &board, &mut plan, options, &limits, budget)
                }
                ExportFormat::Pdf => {
                    pdf::export(&mut renderer, &board, &mut plan, options, &limits, budget)
                }
            };
            assert_eq!(
                written.map(|_| ()),
                Err(ExportError::MathFailedWhileDrawing),
                "{format:?}"
            );
            assert_eq!(plan.failed, 0, "not drawn as failed");
        }
        assert_eq!(
            ExportError::MathFailedWhileDrawing.to_string(),
            "export stopped: math failed while drawing; export again"
        );
    }

    #[test]
    fn the_math_wait_counts_only_waiting_for_math() {
        let engine = MathEngine::new();
        let mut clock = MathClock {
            waited: Duration::ZERO,
            limit: Duration::from_secs(10),
        };
        assert!(clock.layout(&engine, "x^2").unwrap().is_ok());
        assert!(clock.waited > Duration::ZERO);
        // Time spent elsewhere doesn't count.
        let waited = clock.waited;
        std::thread::sleep(Duration::from_millis(20));
        assert!(clock.layout(&engine, "x^2").unwrap().is_ok());
        assert!(clock.waited.checked_sub(waited).unwrap() < Duration::from_millis(20));
        // Past the limit, the export stops.
        clock.limit = clock.waited;
        assert_eq!(
            clock.layout(&engine, "y^2").map(|_| ()),
            Err(ExportError::MathWait)
        );
    }
}
