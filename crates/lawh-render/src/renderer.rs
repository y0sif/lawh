//! Board → pixels with `vello_cpu`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use kurbo::{Affine, BezPath, Cap, Join, Point, Rect, Shape as _, Stroke};
use lawh_core::{Board, Object, Shape};
use lawh_math::{MATH_FONT_SIZE_PT, MathEngine, MathItem, MathLayout, MathStatus, MathSummary};
use parley::PositionedLayoutItem;
use vello_cpu::color::{AlphaColor, Srgb};
use vello_cpu::peniko::{Blob, FontData};
use vello_cpu::{Glyph, Pixmap, RenderContext, Resources};

use crate::budget::{self, Budget, Estimate, MIN_VISIBLE_PX};
use crate::glyphs::GlyphTable;
use crate::text::{LAYOUT_EM, TextShaper};
use crate::{RenderError, TEXT_SIZE_UNITS, Viewport, draw};

/// Board background.
const PAPER: AlphaColor<Srgb> = AlphaColor::from_rgb8(0xFA, 0xFA, 0xF7);
/// Shapes, text and math.
const INK: AlphaColor<Srgb> = AlphaColor::from_rgb8(0x1F, 0x1F, 0x1F);
/// Grid dots.
const GRID: AlphaColor<Srgb> = AlphaColor::from_rgb8(0xD6, 0xD6, 0xD0);
/// The source of math that failed to compile.
const ERROR: AlphaColor<Srgb> = AlphaColor::from_rgb8(0xC0, 0x39, 0x2B);
/// The source of math that is still compiling, as a placeholder.
const PENDING: AlphaColor<Srgb> = AlphaColor::from_rgb8(0x9A, 0x9A, 0x94);
/// The outline of an object drawn in its place when the frame budget is spent.
const OUTLINE: AlphaColor<Srgb> = AlphaColor::from_rgb8(0xC8, 0xC8, 0xC2);

/// Width of those outlines in logical pixels.
const OUTLINE_PX: f64 = 1.0;

/// How far from its origin a glyph's outline may reach, in ems. Glyphs whose
/// origin is further than this outside the image aren't drawn.
const GLYPH_REACH_EM: f64 = 3.0;

/// Glyphs whose em is larger than this many physical pixels aren't drawn. A
/// text em at the largest drawable zoom is 5,000 pixels; only math that asks
/// for an absurd font size gets near this. It covers math glyph runs too.
const MAX_GLYPH_EM_PX: f64 = 100_000.0;

/// Math strokes wider than this many physical pixels aren't drawn: their
/// round joins and caps would be arcs that size, which `vello_cpu` flattens
/// whole. A 1 pt stroke at the largest drawable zoom is 250 pixels.
const MAX_STROKE_PX: f64 = 100_000.0;

/// Typst's default text colour; math drawn in it is drawn in [`INK`] instead.
const TYPST_BLACK: [u8; 4] = [0, 0, 0, 255];

/// Grid units per Typst pt of math.
const MATH_SCALE: f64 = TEXT_SIZE_UNITS / MATH_FONT_SIZE_PT;

/// Whether a frame waits for math.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MathWait {
    /// Never: math still compiling is drawn as a placeholder. For the window.
    No,
    /// For every formula, so the image is the same every time. For exports.
    Yes,
}

/// What the last frame drew, from [`Renderer::last_frame`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameStats {
    /// Objects on screen drawn in full.
    pub drawn: usize,
    /// Objects on screen that didn't fit in the frame's work budget: each is drawn as a faint
    /// outline of its bounds instead, or left out once the outlines' own budget is spent too.
    pub downgraded: usize,
    /// How many of the downgraded objects were left out, outline and all.
    pub left_out: usize,
    /// Objects on screen smaller than half a pixel, not drawn.
    pub too_small: usize,
    /// Math blocks drawn as placeholders because they were still compiling.
    pub math_pending: usize,
    /// The frame's estimated work: what it drew (outlines and grid included) and the work of
    /// estimating what it didn't. A window frame stays within a fixed budget of this.
    pub estimated: Duration,
}

/// The bounds of a formula that has laid out, in pt within its layout.
struct KnownMath {
    bounds: Rect,
    /// The frame that last looked at it.
    last_used: u64,
}

/// Draws boards into a pixel buffer it owns, at any pan and zoom.
///
/// Keep one per window: it caches text layouts, compiled math and glyph
/// outlines between frames.
///
/// A window frame never does more than a fixed budget of work, whatever the
/// board holds: each object's cost is estimated before it is drawn, and an
/// object that doesn't fit in what is left is drawn as a faint outline of its
/// bounds instead (see [`FrameStats::downgraded`]). Objects are still drawn in
/// board order, so nothing changes places. Everyday boards are nowhere near
/// the budget.
pub struct Renderer {
    ctx: RenderContext,
    resources: Resources,
    pixmap: Pixmap,
    text: TextShaper,
    math: MathEngine,
    /// `vello_cpu` handles for [`lawh_math::fonts`], by the same index.
    math_fonts: Vec<FontData>,
    /// Outline sizes of the glyphs of [`lawh_math::fonts`], for the budget.
    glyphs: GlyphTable,
    /// Formulas known to have laid out, by source, so bounds and culling can
    /// use their real extent without asking the math engine for them. Entries
    /// a frame doesn't look at are dropped at its end.
    known_math: HashMap<String, KnownMath>,
    /// Counts frames, for [`KnownMath::last_used`].
    frame: u64,
    /// The current frame's image has a new size (or is the first).
    resized: bool,
    last: FrameStats,
}

impl Renderer {
    /// A renderer that compiles math in this process ([`MathEngine::new`]).
    /// **For trusted boards only**, such as tests and goldens: a formula that
    /// overflows the stack takes this process down. Draw boards from files
    /// with [`Renderer::with_math`] and a worker engine.
    pub fn new_trusted() -> Self {
        Self::with_math(MathEngine::new())
    }

    /// A renderer that lays out math with `math`, typically
    /// [`MathEngine::with_worker`] so untrusted formulas compile in a process
    /// that can be killed.
    pub fn with_math(math: MathEngine) -> Self {
        let math_fonts = lawh_math::fonts()
            .iter()
            .map(|asset| FontData::new(Blob::new(Arc::new(asset.data)), asset.index))
            .collect();
        Self {
            ctx: RenderContext::new(1, 1),
            resources: Resources::new(),
            pixmap: Pixmap::new(1, 1),
            text: TextShaper::new(),
            math,
            math_fonts,
            glyphs: GlyphTable::new(),
            known_math: HashMap::new(),
            frame: 0,
            resized: false,
            last: FrameStats::default(),
        }
    }

    /// Draws `board` through `vp` and returns the image: premultiplied RGBA8,
    /// row-major, `vp.width * vp.height * 4` bytes.
    ///
    /// Never waits for math. Only formulas on screen are asked for, nearest
    /// the centre of the view first, so a slow formula elsewhere on the board
    /// can't hold up the ones in view. A formula not compiled yet is queued
    /// and its source drawn in grey as a placeholder; the math engine's waker
    /// says when to draw again. A formula that failed shows its source in red.
    ///
    /// The frame's work is budgeted (see [`Renderer`]); [`Self::last_frame`]
    /// says what was left out.
    pub fn render(&mut self, board: &Board, vp: &Viewport) -> &[u8] {
        self.draw_frame(board, vp, MathWait::No)
    }

    fn draw_frame(&mut self, board: &Board, vp: &Viewport, wait: MathWait) -> &[u8] {
        if !self.begin_frame(vp) {
            self.last = FrameStats::default();
            return self.pixmap.data_as_u8_slice();
        }
        let mut stats = FrameStats::default();
        let mut budget = match (wait, self.resized) {
            (MathWait::No, false) => Budget::frame(),
            (MathWait::No, true) => Budget::resized_frame(),
            (MathWait::Yes, _) => Budget::export(),
        };
        if vp.is_drawable() {
            if let Some((path, dots)) = draw::grid_dots(vp) {
                budget.charge(budget::grid(dots, vp.scale_factor));
                self.ctx.set_paint(GRID);
                self.ctx.fill_path(&path);
            }
            let clip = draw::clip_rect(vp);
            let to_screen = vp.transform();
            let statuses = self.math_statuses(board, vp, clip, wait);
            for (object, math) in board.objects().iter().zip(statuses) {
                if matches!(object.shape, Shape::Math { .. }) && math.is_none() {
                    // Off screen, so not asked for.
                    continue;
                }
                if matches!(math, Some(MathStatus::Pending)) {
                    stats.math_pending += 1;
                }
                let Some(screen) = self
                    .bounds_with(object, math.as_ref())
                    .map(|bounds| to_screen.transform_rect_bbox(bounds))
                    .filter(|bounds| bounds.is_finite() && bounds.overlaps(clip))
                else {
                    continue;
                };
                if screen.width().max(screen.height()) < MIN_VISIBLE_PX {
                    stats.too_small += 1;
                } else if self.draw_object(object, math, vp, clip, &mut budget) {
                    stats.drawn += 1;
                } else {
                    stats.downgraded += 1;
                    if !self.draw_outline(screen, vp, clip, &mut budget) {
                        stats.left_out += 1;
                    }
                }
            }
        } else {
            tracing::debug!(?vp, "viewport not drawable; drawing the background only");
        }
        if stats.math_pending > 0 {
            tracing::debug!(
                pending = stats.math_pending,
                "math still compiling; drawn as placeholders"
            );
        }
        if stats.downgraded > 0 {
            tracing::debug!(
                downgraded = stats.downgraded,
                left_out = stats.left_out,
                drawn = stats.drawn,
                "frame budget spent; drew objects over it as outlines"
            );
        }
        stats.estimated = Duration::from_secs_f64(budget.spent.max(0.0) / 1e9);
        self.last = stats;
        self.end_frame(vp.is_drawable());
        self.finish_frame()
    }

    /// Sizes the image for `vp` and paints it with paper; `false` for an
    /// empty image, which has nothing to draw.
    fn begin_frame(&mut self, vp: &Viewport) -> bool {
        let (width, height) = (vp.width, vp.height);
        if self.pixmap.width() != width || self.pixmap.height() != height {
            self.pixmap.resize(width, height);
        }
        if width == 0 || height == 0 {
            return false;
        }
        self.resized = self.ctx.width() != width || self.ctx.height() != height;
        if self.resized {
            self.ctx.reset_and_resize(width, height);
        } else {
            self.ctx.reset();
        }
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_paint(PAPER);
        self.ctx
            .fill_rect(&Rect::new(0.0, 0.0, f64::from(width), f64::from(height)));
        true
    }

    /// Drops what the frame didn't use from the caches and starts the next one.
    /// A frame that `drew` nothing looked at no formula, so the formulas it
    /// knows are kept.
    fn end_frame(&mut self, drew: bool) {
        self.text.end_frame();
        if drew {
            let frame = self.frame;
            self.known_math.retain(|_, known| known.last_used == frame);
        }
        self.frame = self.frame.wrapping_add(1);
    }

    /// Renders what was drawn since [`Self::begin_frame`] into the image.
    fn finish_frame(&mut self) -> &[u8] {
        self.ctx.flush();
        self.ctx.render(&mut self.pixmap, &mut self.resources);
        self.pixmap.data_as_u8_slice()
    }

    /// Draws `board` through `vp` and encodes the image as PNG.
    ///
    /// Waits for every formula on the board first ([`Self::resolve_math`]),
    /// so the image never has placeholders: math that doesn't compile in
    /// time shows its source in red. Exports have a work budget too, far
    /// larger than a window frame's.
    pub fn render_png(&mut self, board: &Board, vp: &Viewport) -> Result<Vec<u8>, RenderError> {
        if vp.width == 0 || vp.height == 0 {
            return Err(RenderError::EmptyImage {
                width: vp.width,
                height: vp.height,
            });
        }
        self.resolve_math(board);
        self.draw_frame(board, vp, MathWait::Yes);
        self.pixmap
            .clone()
            .into_png()
            .map_err(|err| RenderError::Png(err.to_string()))
    }

    /// Compiles every formula on `board` that isn't compiled yet, waiting
    /// for each (up to the math engine's timeout and restart margin). After
    /// it, [`Self::board_bounds`] and [`Self::render`] see the final math.
    /// For exports and tests; the window never waits.
    pub fn resolve_math(&mut self, board: &Board) {
        for object in board.objects() {
            if let Shape::Math { src, .. } = &object.shape {
                match self.math.layout_blocking(src) {
                    Ok(layout) => self.record_math(src, &layout),
                    Err(err) => tracing::debug!(id = %object.id, %err, "math did not lay out"),
                }
            }
        }
    }

    /// How many math blocks the last [`Self::render`] drew as placeholders
    /// because they were still compiling.
    pub fn math_pending(&self) -> usize {
        self.last.math_pending
    }

    /// What the last frame drew and what it left out.
    pub fn last_frame(&self) -> FrameStats {
        self.last
    }

    /// The smallest rectangle, in grid units, holding every object on the
    /// board, text and math extents included; `None` for an empty board.
    ///
    /// Never waits for math, and never asks for it: a formula this renderer
    /// hasn't seen laid out counts with the extents of its source text. A
    /// laid-out formula counts with everything it draws, which can reach
    /// outside its box. Call [`Self::resolve_math`] first for final bounds.
    /// Strokes and arrowheads have a fixed pixel size, so they aren't
    /// included; [`Viewport::fit`] leaves a margin for them.
    pub fn board_bounds(&mut self, board: &Board) -> Option<Rect> {
        board
            .objects()
            .iter()
            .filter_map(|object| self.object_bounds(object))
            .reduce(|a, b| a.union(b))
    }

    /// The bounds of one object in grid units, as in [`Self::board_bounds`];
    /// `None` for an object with no finite bounds.
    pub fn object_bounds(&mut self, object: &Object) -> Option<Rect> {
        self.bounds_with(object, None)
    }

    /// The math status of every object on `board`, by index: `None` for
    /// objects that aren't math, and for formulas off screen when the frame
    /// doesn't wait. Formulas on screen (by their known bounds, or their
    /// placeholder's) are asked for nearest the centre of the view first, so
    /// they queue ahead of the rest.
    fn math_statuses(
        &mut self,
        board: &Board,
        vp: &Viewport,
        clip: Rect,
        wait: MathWait,
    ) -> Vec<Option<MathStatus>> {
        let objects = board.objects();
        let mut statuses = vec![None; objects.len()];
        let to_screen = vp.transform();
        let center = Point::new(f64::from(vp.width) / 2.0, f64::from(vp.height) / 2.0);
        let mut visible = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            let Shape::Math { src, .. } = &object.shape else {
                continue;
            };
            if wait == MathWait::Yes {
                let status = match self.math.layout_blocking(src) {
                    Ok(layout) => MathStatus::Ready(layout),
                    Err(err) => MathStatus::Failed(err),
                };
                statuses[index] = Some(self.noted(src, status));
                continue;
            }
            let on_screen = self
                .bounds_with(object, None)
                .map(|bounds| to_screen.transform_rect_bbox(bounds))
                .filter(|bounds| bounds.is_finite() && bounds.overlaps(clip));
            if let Some(bounds) = on_screen {
                visible.push((distance(bounds, center), index));
            }
        }
        // Stable, so formulas the same distance away are asked for in board order.
        visible.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, index) in visible {
            if let Some(Shape::Math { src, .. }) = objects.get(index).map(|object| &object.shape) {
                let status = self.math.request(src);
                statuses[index] = Some(self.noted(src, status));
            }
        }
        statuses
    }

    /// `status`, after remembering the bounds of a laid-out formula (or one
    /// whose layout the engine pushed out of its cache).
    fn noted(&mut self, src: &str, status: MathStatus) -> MathStatus {
        match &status {
            MathStatus::Ready(layout) => self.record_math(src, layout),
            MathStatus::Evicted(summary) => self.record_bounds(src, summary_bounds(summary)),
            MathStatus::Pending | MathStatus::Failed(_) => {}
        }
        status
    }

    fn record_math(&mut self, src: &str, layout: &MathLayout) {
        self.record_bounds(src, layout_bounds(layout));
    }

    fn record_bounds(&mut self, src: &str, bounds: Rect) {
        let frame = self.frame;
        if let Some(known) = self.known_math.get_mut(src) {
            known.bounds = bounds;
            known.last_used = frame;
        } else {
            self.known_math.insert(
                src.to_owned(),
                KnownMath {
                    bounds,
                    last_used: frame,
                },
            );
        }
    }

    /// An object's bounds, given its math status from [`Self::math_statuses`]
    /// (`None`: what this renderer knows, without asking the engine).
    fn bounds_with(&mut self, object: &Object, math: Option<&MathStatus>) -> Option<Rect> {
        let bounds = match &object.shape {
            Shape::Ellipse { center, rx, ry } => {
                let (rx, ry) = (rx.abs(), ry.abs());
                Some(Rect::new(
                    center.x - rx,
                    center.y - ry,
                    center.x + rx,
                    center.y + ry,
                ))
            }
            Shape::Rect { origin, w, h } => Some(Rect::from_origin_size(*origin, (*w, *h)).abs()),
            Shape::Line { a, b } | Shape::Arrow { a, b } => Some(Rect::from_points(*a, *b)),
            Shape::Text { at, text } => self.text_bounds(*at, text),
            Shape::Math { at, src } => {
                let laid_out = match math {
                    Some(MathStatus::Ready(layout)) => Some(layout_bounds(layout)),
                    Some(MathStatus::Evicted(summary)) => Some(summary_bounds(summary)),
                    // Still compiling or failed: the source is drawn as text.
                    Some(MathStatus::Pending | MathStatus::Failed(_)) => None,
                    None => {
                        let frame = self.frame;
                        self.known_math.get_mut(src.as_str()).map(|known| {
                            known.last_used = frame;
                            known.bounds
                        })
                    }
                };
                match laid_out {
                    Some(pt) => Some(math_to_board(*at).transform_rect_bbox(pt)),
                    None => self.text_bounds(*at, src),
                }
            }
        };
        bounds.filter(Rect::is_finite)
    }

    fn text_bounds(&mut self, at: Point, text: &str) -> Option<Rect> {
        let layout = self.text.layout(text)?;
        Some(Rect::from_origin_size(
            at,
            (
                f64::from(layout.width()) * text_scale(),
                f64::from(layout.height()) * text_scale(),
            ),
        ))
    }

    /// Draws one object whose bounds reach into `clip` (screen pixels), if
    /// its estimated cost fits in `budget`; `false`, with nothing drawn, when
    /// it doesn't.
    fn draw_object(
        &mut self,
        object: &Object,
        math: Option<MathStatus>,
        vp: &Viewport,
        clip: Rect,
        budget: &mut Budget,
    ) -> bool {
        match &object.shape {
            Shape::Ellipse { center, rx, ry } => {
                self.stroke_px(draw::ellipse(vp, *center, *rx, *ry, clip), vp, clip, budget)
            }
            Shape::Rect { origin, w, h } => {
                self.stroke_px(draw::rect(vp, *origin, *w, *h, clip), vp, clip, budget)
            }
            Shape::Line { a, b } => self.stroke_px(draw::line(vp, *a, *b, clip), vp, clip, budget),
            Shape::Arrow { a, b } => {
                let (shaft, head) = draw::arrow(vp, *a, *b, clip);
                let width = draw::stroke_width_px(vp);
                let mut cost = Estimate::new(clip, vp.width, vp.height, budget.left());
                if let Some(shaft) = &shaft {
                    cost.stroke(shaft, width);
                }
                if let Some(head) = &head {
                    cost.fill(head);
                }
                if !budget.settle(&cost) {
                    return false;
                }
                if let Some(shaft) = shaft {
                    self.stroke_ink(&shaft, width);
                }
                if let Some(head) = head {
                    self.ctx.set_transform(Affine::IDENTITY);
                    self.ctx.set_paint(INK);
                    self.ctx.fill_path(&head);
                }
                true
            }
            Shape::Text { at, text } => self.draw_text(*at, text, INK, vp, clip, budget),
            Shape::Math { at, src } => match math {
                Some(MathStatus::Ready(layout)) => self.draw_math(*at, &layout, vp, clip, budget),
                Some(MathStatus::Failed(err)) => {
                    tracing::debug!(id = %object.id, %err, "drawing math source instead of math");
                    self.draw_text(*at, src, ERROR, vp, clip, budget)
                }
                Some(MathStatus::Pending) | None => {
                    self.draw_text(*at, src, PENDING, vp, clip, budget)
                }
                // Laid out before, but pushed out of the math cache and not compiled again
                // for now: drawn as the faint outline of its bounds, like an object over the
                // budget (and counted with them in `FrameStats::downgraded`).
                Some(MathStatus::Evicted(_)) => false,
            },
        }
    }

    /// Strokes a path given in screen pixels with the fixed outline width, if
    /// it fits in `budget`. Nothing to stroke always fits.
    fn stroke_px(
        &mut self,
        path: Option<BezPath>,
        vp: &Viewport,
        clip: Rect,
        budget: &mut Budget,
    ) -> bool {
        let Some(path) = path else {
            return true;
        };
        let width = draw::stroke_width_px(vp);
        let mut cost = Estimate::new(clip, vp.width, vp.height, budget.left());
        cost.stroke(&path, width);
        if !budget.settle(&cost) {
            return false;
        }
        self.stroke_ink(&path, width);
        true
    }

    /// Strokes a path given in screen pixels in ink, `width` pixels wide.
    fn stroke_ink(&mut self, path: &BezPath, width: f64) {
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_paint(INK);
        self.ctx.set_stroke(
            Stroke::new(width)
                .with_caps(Cap::Round)
                .with_join(Join::Round),
        );
        self.ctx.stroke_path(path);
    }

    /// Draws the faint outline of `screen`, an object's bounds in screen
    /// pixels, in place of an object over the budget, from the outlines' own
    /// budget; `false` when that is spent too.
    fn draw_outline(
        &mut self,
        screen: Rect,
        vp: &Viewport,
        clip: Rect,
        budget: &mut Budget,
    ) -> bool {
        let around_clip = screen.x0 < clip.x0
            && screen.y0 < clip.y0
            && screen.x1 > clip.x1
            && screen.y1 > clip.y1;
        if around_clip {
            // The whole outline is off screen.
            return true;
        }
        // An edge moved onto the clip's border is outside the image.
        let path = screen.intersect(clip).to_path(draw::TOLERANCE_PX);
        let width = OUTLINE_PX * vp.scale_factor;
        let mut cost = Estimate::new(clip, vp.width, vp.height, f64::INFINITY);
        cost.stroke(&path, width);
        if !budget.spend_outline(cost.ns) {
            return false;
        }
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_paint(OUTLINE);
        self.ctx.set_stroke(Stroke::new(width));
        self.ctx.stroke_path(&path);
        true
    }

    /// Draws text with its top-left corner at `at`, skipping glyphs too far
    /// outside `clip` to reach into it, if it fits in `budget`.
    fn draw_text(
        &mut self,
        at: Point,
        text: &str,
        color: AlphaColor<Srgb>,
        vp: &Viewport,
        clip: Rect,
        budget: &mut Budget,
    ) -> bool {
        let em_px = f64::from(LAYOUT_EM) * text_scale() * vp.px_per_unit();
        if em_px < MIN_VISIBLE_PX {
            return true;
        }
        let Some(near) = glyph_area(f64::from(LAYOUT_EM) * text_scale(), vp, clip) else {
            return true;
        };
        let Some(layout) = self.text.layout(text) else {
            return true;
        };
        let transform =
            vp.transform() * Affine::translate(at.to_vec2()) * Affine::scale(text_scale());
        let shown = |baseline: f32| {
            // Text is only scaled and moved, so a line's screen height depends on its baseline.
            let y = (transform * Point::new(0.0, f64::from(baseline))).y;
            (near.y0..=near.y1).contains(&y)
        };
        let mut cost = Estimate::new(clip, vp.width, vp.height, budget.left());
        'lines: for line in layout.lines() {
            if !shown(line.metrics().baseline) {
                continue;
            }
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                cost.run();
                for glyph in glyph_run.positioned_glyphs() {
                    if cost.over() {
                        break 'lines;
                    }
                    let at = transform * Point::new(f64::from(glyph.x), f64::from(glyph.y));
                    // Text is laid out with the bundled text font only, `fonts()[0]`.
                    let drawn = near
                        .contains(at)
                        .then(|| (self.glyphs.stats(0, glyph.id), em_px));
                    cost.glyph(drawn);
                }
            }
        }
        if !budget.settle(&cost) {
            return false;
        }
        self.ctx.set_transform(transform);
        self.ctx.set_paint(color);
        for line in layout.lines() {
            if !shown(line.metrics().baseline) {
                continue;
            }
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let coords = bytemuck::cast_slice::<_, i16>(run.normalized_coords());
                let drawn = self
                    .ctx
                    .glyph_run(&mut self.resources, run.font())
                    .font_size(run.font_size())
                    .hint(false)
                    .normalized_coords(coords)
                    .fill_glyphs(
                        glyph_run
                            .positioned_glyphs()
                            .filter(|glyph| {
                                near.contains(
                                    transform * Point::new(f64::from(glyph.x), f64::from(glyph.y)),
                                )
                            })
                            .map(|glyph| Glyph {
                                id: glyph.id,
                                x: glyph.x,
                                y: glyph.y,
                            }),
                    );
                if let Err(err) = drawn {
                    tracing::debug!(?err, "some text glyphs failed to draw");
                }
            }
        }
        true
    }

    /// The estimated cost of [`Self::draw_math`] with the same arguments,
    /// counted until it is past `limit`.
    fn math_cost(
        &mut self,
        at: Point,
        layout: &MathLayout,
        vp: &Viewport,
        clip: Rect,
        limit: f64,
    ) -> Estimate {
        let transform = math_transform(at, vp);
        let px_per_pt = vp.px_per_unit() * MATH_SCALE;
        let mut cost = Estimate::new(clip, vp.width, vp.height, limit);
        for item in &layout.items {
            if cost.over() {
                break;
            }
            match item {
                MathItem::Glyphs {
                    font, size, glyphs, ..
                } => {
                    let Some(near) = self.glyph_run_area(*font, *size, vp, clip) else {
                        continue;
                    };
                    cost.run();
                    let em_px = (size * px_per_pt).abs();
                    for glyph in glyphs {
                        if cost.over() {
                            break;
                        }
                        let drawn = near
                            .contains(transform * Point::new(glyph.x, glyph.y))
                            .then(|| (self.glyphs.stats(*font, glyph.id), em_px));
                        cost.glyph(drawn);
                    }
                }
                MathItem::Path { path, fill, stroke } => {
                    let stroke = stroke_px(*stroke, px_per_pt);
                    cost.bound(path);
                    if path_area(path, transform, stroke, clip).is_none() {
                        continue;
                    }
                    if fill.is_some() {
                        cost.path(path, transform, None);
                    }
                    if let Some((width, _)) = stroke {
                        cost.path(path, transform, Some(width));
                    }
                }
            }
        }
        cost
    }

    /// Where on screen a math glyph run's glyph origins must be to be drawn;
    /// `None` when none of it is drawn (unknown font, glyphs under half a
    /// pixel or absurdly large).
    fn glyph_run_area(&self, font: usize, size: f64, vp: &Viewport, clip: Rect) -> Option<Rect> {
        if self.math_fonts.get(font).is_none() {
            tracing::warn!(font, "math glyphs reference an unknown font");
            return None;
        }
        if (size * MATH_SCALE * vp.px_per_unit()).abs() < MIN_VISIBLE_PX {
            return None;
        }
        glyph_area(size * MATH_SCALE, vp, clip)
    }

    /// Draws a formula with its top-left corner at `at`, skipping glyphs and
    /// paths that can't reach into `clip` and cutting paths down to it, if it
    /// fits in `budget`.
    fn draw_math(
        &mut self,
        at: Point,
        layout: &MathLayout,
        vp: &Viewport,
        clip: Rect,
        budget: &mut Budget,
    ) -> bool {
        let cost = self.math_cost(at, layout, vp, clip, budget.left());
        if !budget.settle(&cost) {
            return false;
        }
        let transform = math_transform(at, vp);
        let px_per_pt = vp.px_per_unit() * MATH_SCALE;
        for item in &layout.items {
            match item {
                MathItem::Glyphs {
                    font,
                    size,
                    fill,
                    glyphs,
                } => {
                    let Some(near) = self.glyph_run_area(*font, *size, vp, clip) else {
                        continue;
                    };
                    let Some(font_data) = self.math_fonts.get(*font) else {
                        continue;
                    };
                    self.ctx.set_transform(transform);
                    self.ctx.set_paint(math_color(*fill));
                    let drawn = self
                        .ctx
                        .glyph_run(&mut self.resources, font_data)
                        .font_size(to_f32(*size))
                        .hint(false)
                        .fill_glyphs(
                            glyphs
                                .iter()
                                .filter(|glyph| {
                                    near.contains(transform * Point::new(glyph.x, glyph.y))
                                })
                                .map(|glyph| Glyph {
                                    id: glyph.id,
                                    x: to_f32(glyph.x),
                                    y: to_f32(glyph.y),
                                }),
                        );
                    if let Err(err) = drawn {
                        tracing::debug!(?err, "some math glyphs failed to draw");
                    }
                }
                MathItem::Path { path, fill, stroke } => {
                    let stroke = stroke_px(*stroke, px_per_pt);
                    let Some(near) = path_area(path, transform, stroke, clip) else {
                        continue;
                    };
                    let mut screen = path.clone();
                    screen.apply_affine(transform);
                    self.ctx.set_transform(Affine::IDENTITY);
                    if let Some(fill) = fill {
                        self.ctx.set_paint(math_color(*fill));
                        self.ctx.fill_path(&draw::cut_path(&screen, near, true));
                    }
                    if let Some((width, color)) = stroke {
                        self.ctx.set_paint(math_color(color));
                        self.ctx.set_stroke(Stroke::new(width));
                        self.ctx.stroke_path(&draw::cut_path(&screen, near, false));
                    }
                }
            }
        }
        true
    }
}

/// A math path's stroke as its width in screen pixels and its colour; `None`
/// when it has none, or one too wide to draw ([`MAX_STROKE_PX`]).
fn stroke_px(stroke: Option<(f64, [u8; 4])>, px_per_pt: f64) -> Option<(f64, [u8; 4])> {
    stroke.and_then(|(width, color)| {
        let width = width.abs() * px_per_pt;
        if width.is_finite() && width <= MAX_STROKE_PX {
            Some((width, color))
        } else {
            tracing::debug!(width, "math stroke too wide to draw");
            None
        }
    })
}

/// The screen rectangle a math path is cut to: `clip` grown by a whole stroke
/// width, so a stroke's joins and caps are inside too. `None` when the path
/// doesn't reach into it, so isn't drawn.
fn path_area(
    path: &BezPath,
    transform: Affine,
    stroke: Option<(f64, [u8; 4])>,
    clip: Rect,
) -> Option<Rect> {
    let reach = stroke.map_or(0.0, |(width, _)| width);
    let near = clip.inflate(reach, reach);
    // The transform only scales and moves, so this is the screen path's control box.
    let bounds = transform.transform_rect_bbox(path.control_box());
    (bounds.is_finite() && bounds.overlaps(near)).then_some(near)
}

/// A formula's layout (pt, origin at its top-left) → screen pixels, for a
/// formula at `at`.
fn math_transform(at: Point, vp: &Viewport) -> Affine {
    vp.transform() * math_to_board(at)
}

/// A formula's layout (pt) → grid units, for a formula at `at`.
fn math_to_board(at: Point) -> Affine {
    Affine::translate(at.to_vec2()) * Affine::scale(MATH_SCALE)
}

/// The bounds of everything a layout covers, in pt: its box joined with its
/// ink, which can reach outside the box.
fn layout_bounds(layout: &MathLayout) -> Rect {
    Rect::new(0.0, 0.0, layout.width, layout.height)
        .abs()
        .union(layout.ink.abs())
}

/// [`layout_bounds`] of a layout the math engine no longer holds.
fn summary_bounds(summary: &MathSummary) -> Rect {
    Rect::new(0.0, 0.0, summary.width, summary.height)
        .abs()
        .union(summary.ink.abs())
}

/// How far `p` is from `rect`; 0 inside it.
fn distance(rect: Rect, p: Point) -> f64 {
    let dx = (rect.x0 - p.x).max(p.x - rect.x1).max(0.0);
    let dy = (rect.y0 - p.y).max(p.y - rect.y1).max(0.0);
    dx.hypot(dy)
}

/// Where on screen a glyph's origin must be for any of it to reach into
/// `clip`, for glyphs of `em_units` grid units; `None` when glyphs that size
/// aren't drawn at all (non-finite, or past [`MAX_GLYPH_EM_PX`]).
fn glyph_area(em_units: f64, vp: &Viewport, clip: Rect) -> Option<Rect> {
    let em_px = (em_units * vp.px_per_unit()).abs();
    if !(em_px.is_finite() && em_px <= MAX_GLYPH_EM_PX) {
        tracing::debug!(em_px, "glyphs too large to draw");
        return None;
    }
    let reach = GLYPH_REACH_EM * em_px;
    Some(clip.inflate(reach, reach))
}

/// Grid units per layout unit of text.
fn text_scale() -> f64 {
    TEXT_SIZE_UNITS / f64::from(LAYOUT_EM)
}

/// A Typst colour, with Typst's default black drawn as board ink.
fn math_color(rgba: [u8; 4]) -> AlphaColor<Srgb> {
    if rgba == TYPST_BLACK {
        INK
    } else {
        let [r, g, b, a] = rgba;
        AlphaColor::from_rgba8(r, g, b, a)
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
mod calibrate;

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use kurbo::{BezPath, Circle};
    use lawh_math::{MAX_LAYOUT_EXTENT_PT, MathItem, MathLayout};

    use super::*;

    const BLACK: [u8; 4] = [0, 0, 0, 255];
    const MAX: f64 = MAX_LAYOUT_EXTENT_PT;

    /// Draws only `layout`, its top-left corner at the board origin, as an
    /// export draws a math object, and returns the image and how long it took.
    fn render_layout(layout: &MathLayout, vp: &Viewport) -> (Vec<u8>, Duration) {
        let mut renderer = Renderer::new_trusted();
        let started = Instant::now();
        assert!(renderer.begin_frame(vp));
        let mut budget = Budget::export();
        assert!(renderer.draw_math(Point::ZERO, layout, vp, draw::clip_rect(vp), &mut budget));
        let image = renderer.finish_frame().to_vec();
        (image, started.elapsed())
    }

    /// A layout just under the extent cap: a circle as large as the cap
    /// allows, stroked thin; a rectangle filling the cap's left half; and a
    /// line stroked as wide as the cap allows.
    fn huge_layout() -> MathLayout {
        let circle = Circle::new((MAX / 2.0, MAX / 2.0), MAX * 0.49).to_path(0.01);
        let rect = Rect::new(-MAX, -MAX, 0.0, MAX).to_path(0.0);
        let mut line = BezPath::new();
        line.move_to((-MAX, -MAX));
        line.line_to((MAX, -MAX));
        let layout = MathLayout {
            width: MAX,
            height: MAX,
            baseline: MAX,
            // The line's stroke reaches half its width past it.
            ink: Rect::new(-1.5 * MAX, -1.5 * MAX, 1.5 * MAX, MAX),
            items: vec![
                MathItem::Path {
                    path: circle,
                    fill: None,
                    stroke: Some((0.04, BLACK)),
                },
                MathItem::Path {
                    path: rect,
                    fill: Some(BLACK),
                    stroke: None,
                },
                MathItem::Path {
                    path: line,
                    fill: None,
                    stroke: Some((MAX, BLACK)),
                },
            ],
        };
        for item in &layout.items {
            let MathItem::Path { path, .. } = item else {
                unreachable!()
            };
            let bounds = path.control_box();
            let reach = [bounds.x0, bounds.y0, bounds.x1, bounds.y1].map(f64::abs);
            assert!(reach.iter().all(|v| *v <= MAX), "{bounds:?}");
        }
        layout
    }

    /// Ink at the centre of `image`, and paper `away` pixels to either side
    /// along `normal`.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the pixels looked at are a few pixels from the centre of a small image"
    )]
    fn ink_at_center_only(image: &[u8], vp: &Viewport, normal: (f64, f64), away: f64) {
        let (cx, cy) = (f64::from(vp.width) / 2.0, f64::from(vp.height) / 2.0);
        let at = |dx: f64, dy: f64| {
            let (x, y) = ((cx + dx) as usize, (cy + dy) as usize);
            let i = (y * usize::from(vp.width) + x) * 4;
            [image[i], image[i + 1], image[i + 2], image[i + 3]]
        };
        let paper = PAPER.to_rgba8().to_u8_array();
        assert!(at(0.0, 0.0)[0] < 100, "no ink at the centre of {vp:?}");
        for side in [-1.0, 1.0] {
            let px = at(side * away * normal.0, side * away * normal.1);
            assert_eq!(
                px, paper,
                "{vp:?}: expected paper {away} px off the outline"
            );
        }
    }

    #[test]
    fn a_huge_layout_under_the_cap_draws_fast_and_exactly_at_max_zoom() {
        let layout = huge_layout();
        let units = |pt: f64| pt * MATH_SCALE;
        // 40 * 125 * 2 = 10,000 pixels per unit, the most a viewport may show:
        // 250 pixels per pt, so the circle is 2.5e7 pixels across and its 0.04 pt
        // stroke 10 pixels wide.
        let max_zoom = |center: Point| Viewport::new(center, 125.0, 400, 300, 2.0);
        assert!(max_zoom(Point::ZERO).is_drawable());

        for degrees in [0.0_f64, 30.0, 135.0, 270.0] {
            let (sin, cos) = degrees.to_radians().sin_cos();
            let on_circle = Point::new(
                units(MAX / 2.0 + MAX * 0.49 * cos),
                units(MAX / 2.0 + MAX * 0.49 * sin),
            );
            let vp = max_zoom(on_circle);
            let (image, took) = render_layout(&layout, &vp);
            assert!(
                took < Duration::from_millis(500),
                "{degrees} degrees took {took:?}"
            );
            ink_at_center_only(&image, &vp, (cos, sin), 15.0);
        }

        // The rectangle's right edge, at x = 0: ink to its left, paper to its right.
        let vp = max_zoom(Point::new(0.0, units(MAX / 2.0)));
        let (image, took) = render_layout(&layout, &vp);
        assert!(took < Duration::from_millis(500), "took {took:?}");
        let row = usize::from(vp.height / 2) * usize::from(vp.width);
        let left = (row + usize::from(vp.width / 2) - 20) * 4;
        let right = (row + usize::from(vp.width / 2) + 20) * 4;
        assert!(image[left] < 100, "no fill left of the edge");
        assert_eq!(image[right..right + 4], PAPER.to_rgba8().to_u8_array());

        // On the line, whose stroke is too wide to draw at this zoom.
        let vp = max_zoom(Point::new(0.0, units(-MAX)));
        let (_, took) = render_layout(&layout, &vp);
        assert!(took < Duration::from_millis(500), "took {took:?}");
    }

    /// A circle of radius `r` pt with its top-left at the origin, as Typst
    /// draws one: four cubics.
    fn typst_circle(r: f64) -> BezPath {
        let k = 0.552_284_749_8 * r;
        let mut circle = BezPath::new();
        circle.move_to((2.0 * r, r));
        circle.curve_to((2.0 * r, r + k), (r + k, 2.0 * r), (r, 2.0 * r));
        circle.curve_to((r - k, 2.0 * r), (0.0, r + k), (0.0, r));
        circle.curve_to((0.0, r - k), (r - k, 0.0), (r, 0.0));
        circle.curve_to((r + k, 0.0), (2.0 * r, r - k), (2.0 * r, r));
        circle.close_path();
        circle
    }

    /// The cut, not the extent cap, is what keeps this cheap: drawn whole,
    /// this circle took 1.2 s and 1.3 GB in a debug build (measured on Linux
    /// `x86_64`, 2026-10-10); the verifier measured about 51 GB for one of
    /// 1e18 pt.
    #[test]
    fn a_layout_far_past_the_cap_still_draws_fast_and_exactly() {
        let r = 1e12;
        let layout = MathLayout {
            width: 2.0 * r,
            height: 2.0 * r,
            baseline: r,
            ink: Rect::new(-0.5, -0.5, 2.0 * r + 0.5, 2.0 * r + 0.5),
            items: vec![MathItem::Path {
                path: typst_circle(r),
                fill: None,
                stroke: Some((1.0, BLACK)),
            }],
        };
        // The circle's leftmost point, at max zoom: its 1 pt stroke is 250
        // pixels wide.
        let vp = Viewport::new(Point::new(0.0, r * MATH_SCALE), 125.0, 400, 300, 2.0);
        let (image, took) = render_layout(&layout, &vp);
        assert!(took < Duration::from_millis(500), "took {took:?}");
        ink_at_center_only(&image, &vp, (1.0, 0.0), 150.0);
    }
}
