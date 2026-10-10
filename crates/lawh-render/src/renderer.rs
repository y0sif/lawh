//! Board → pixels with `vello_cpu`.

use std::cell::Cell;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use kurbo::{Affine, BezPath, Cap, Join, Point, Rect, Shape as _, Stroke};
use lawh_core::{Board, Object, Shape};
use lawh_math::{MATH_FONT_SIZE_PT, MathEngine, MathItem, MathLayout, MathStatus, MathSummary};
use parley::PositionedLayoutItem;
use vello_cpu::color::{AlphaColor, Srgb};
use vello_cpu::peniko::{Blob, FontData};
use vello_cpu::{Glyph, Pixmap, RenderContext, Resources};

use crate::budget::{self, Budget, Estimate, GlyphSums, MIN_VISIBLE_PX};
use crate::glyphs::GlyphTable;
use crate::text::{Chunk, KeyMap, Known, LAYOUT_EM, Plan, TextKey, TextShaper};
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

/// How far past a text glyph run's advance its glyphs' origins may be, in ems (mark offsets).
/// A run that can't reach the image by this much isn't set up at all.
const RUN_SLACK_EM: f64 = 2.0;

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

/// How a text's size is found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextSize {
    /// From its cached layout, or estimated from its characters and lines when it has none:
    /// never lays it out. For frames, which lay out only the texts they draw.
    Known,
    /// From its layout, laying it out if needed. For [`Renderer::board_bounds`].
    Exact,
}

/// What became of one object on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Drawn,
    /// Over the frame's budget: drawn as an outline.
    OverBudget,
    /// Text not laid out yet: drawn as an outline.
    Pending,
    /// A formula whose layout the math engine dropped: drawn as an outline.
    Evicted,
}

/// What the last frame drew, from [`Renderer::last_frame`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameStats {
    /// Objects on screen drawn in full.
    pub drawn: usize,
    /// Objects on screen that didn't fit in the frame's work budget: each is drawn as a faint
    /// outline of its bounds instead, or left out once the outlines' own budget is spent too.
    pub downgraded: usize,
    /// How many of the objects drawn as outlines (downgraded, [`Self::text_pending`] and
    /// [`Self::math_evicted`]) were left out, outline and all.
    pub left_out: usize,
    /// Objects on screen smaller than half a pixel, not drawn.
    pub too_small: usize,
    /// Math blocks drawn as placeholders because they were still compiling.
    pub math_pending: usize,
    /// Formulas on screen whose layout the math engine pushed out of its cache and isn't
    /// compiling again for now ([`MathStatus::Evicted`]): drawn as faint outlines of their
    /// bounds, like downgraded objects, and counted in neither [`Self::drawn`] nor
    /// [`Self::downgraded`]. Nothing is asked of the engine for them.
    pub math_evicted: usize,
    /// Texts (and sources of math drawn as text) the frame laid out, charged to its budget.
    pub laid_out: usize,
    /// Texts on screen the frame had no budget left to lay out: drawn as faint outlines of
    /// their estimated bounds until a later frame lays them out (see [`Self::incomplete`]).
    pub text_pending: usize,
    /// The frame's estimated work: what it drew (outlines and grid included), the work of
    /// estimating what it didn't, and laying out text. A window frame stays within a fixed
    /// budget of this, but for laying out one text when it must.
    pub estimated: Duration,
}

impl FrameStats {
    /// True when drawing the same view again would draw more: text was left to lay out, or
    /// laying text out took budget that objects then went without. The window draws another
    /// frame; each one lays out at least one text, so this ends. Math still compiling or
    /// pushed out of the engine's cache doesn't count: the engine's waker says when to draw.
    pub fn incomplete(&self) -> bool {
        self.text_pending > 0 || (self.laid_out > 0 && self.downgraded > 0)
    }
}

/// What a frame knows about one object of the board.
#[derive(Debug, Clone, Default)]
struct Seen {
    /// The key of its text, or of a formula's source.
    key: Option<TextKey>,
    /// Its math status: `None` for objects that aren't math, and for formulas off screen when
    /// the frame doesn't wait.
    math: Option<MathStatus>,
}

/// The bounds of a formula that has laid out, in pt within its layout.
struct KnownMath {
    /// The formula's source, compared on lookup: the map is keyed by its hash.
    src: Box<str>,
    bounds: Rect,
    /// The frame that last looked at it.
    last_used: u64,
}

/// What the current frame handed to `vello_cpu` and walked to do so, counted as it drew, and
/// what the estimates of the text it drew charged for the same: for logs, and for tests that
/// hold the estimates to what drawing does.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Work {
    text: TextWork,
    text_charged: TextWork,
    /// Math glyph runs set up and math glyphs drawn.
    math_runs: usize,
    math_glyphs: usize,
}

/// Work drawing text.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct TextWork {
    /// Lines whose glyph runs were walked.
    lines: f64,
    /// Glyph runs parley handed out on those lines, set up or not.
    walked_runs: f64,
    /// Glyphs walked to cull them, drawn or not.
    walked: f64,
    /// Glyph runs set up.
    runs: f64,
    /// Glyphs drawn, in full or culled by `vello_cpu`.
    glyphs: f64,
}

impl TextWork {
    fn add(&mut self, other: &Self) {
        self.lines += other.lines;
        self.walked_runs += other.walked_runs;
        self.walked += other.walked;
        self.runs += other.runs;
        self.glyphs += other.glyphs;
    }
}

/// Draws boards into a pixel buffer it owns, at any pan and zoom.
///
/// Keep one per window: it caches text layouts (within
/// [`MAX_TEXT_CACHE_BYTES`](crate::MAX_TEXT_CACHE_BYTES)), compiled math and glyph outlines
/// between frames.
///
/// A window frame never does more than a fixed budget of work, whatever the
/// board holds: each object's cost is estimated before it is drawn, and an
/// object that doesn't fit in what is left is drawn as a faint outline of its
/// bounds instead (see [`FrameStats::downgraded`]). Objects are still drawn in
/// board order, so nothing changes places. Laying text out is charged to the
/// frame that does it, and text a frame can't afford to lay out waits for the
/// next ([`FrameStats::text_pending`]). Everyday boards are nowhere near the
/// budget.
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
    /// Formulas known to have laid out, by the hash of their source, so bounds
    /// and culling can use their real extent without asking the math engine
    /// for them. Entries a frame doesn't look at are dropped at its end.
    known_math: KeyMap<KnownMath>,
    /// Counts frames, for [`KnownMath::last_used`].
    frame: u64,
    /// The current frame's image has a new size (or is the first).
    resized: bool,
    last: FrameStats,
    work: Work,
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
            known_math: KeyMap::default(),
            frame: 0,
            resized: false,
            last: FrameStats::default(),
            work: Work::default(),
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
    /// says what was left out, and [`FrameStats::incomplete`] when to draw again.
    pub fn render(&mut self, board: &Board, vp: &Viewport) -> &[u8] {
        self.draw_frame(board, vp, MathWait::No)
    }

    fn draw_frame(&mut self, board: &Board, vp: &Viewport, wait: MathWait) -> &[u8] {
        self.work = Work::default();
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
            let clip = draw::clip_rect(vp);
            let mut seen = self.look_up(board, &mut budget);
            self.math_statuses(board, &mut seen, vp, clip, wait);
            self.lay_out_on_screen(board, &seen, vp, clip, &mut budget, &mut stats);
            if let Some((path, dots)) = draw::grid_dots(vp) {
                budget.charge(budget::grid(dots, vp.scale_factor));
                self.ctx.set_paint(GRID);
                self.ctx.fill_path(&path);
            }
            self.draw_objects(board, &seen, vp, clip, &mut budget, &mut stats);
        } else {
            tracing::debug!(?vp, "viewport not drawable; drawing the background only");
        }
        if stats.math_pending > 0 || stats.math_evicted > 0 {
            tracing::debug!(
                pending = stats.math_pending,
                evicted = stats.math_evicted,
                "math still compiling or pushed out of the math cache; drawn as placeholders"
            );
        }
        if stats.downgraded > 0 || stats.text_pending > 0 {
            tracing::debug!(
                downgraded = stats.downgraded,
                text_pending = stats.text_pending,
                left_out = stats.left_out,
                drawn = stats.drawn,
                laid_out = stats.laid_out,
                "frame budget spent; drew objects over it as outlines"
            );
        }
        tracing::trace!(
            work = ?self.work,
            texts_hashed = self.text.hashed,
            texts_laid_out = self.text.built,
            "frame work"
        );
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

    /// Drops the formulas the frame didn't look at and starts the next one. A
    /// frame that `drew` nothing looked at no formula, so the formulas it knows
    /// are kept. Text layouts are kept either way: the text cache drops the
    /// least recently used ones when it is full.
    fn end_frame(&mut self, drew: bool) {
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
                    Ok(layout) => {
                        let key = self.text.key(src);
                        self.record_bounds(key, src, layout_bounds(&layout));
                    }
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
    ///
    /// Lays out every text it hasn't laid out before, so the first call on a
    /// board takes as long as laying out all its text (under 100 ms for any
    /// board within lawh-core's text caps, in release). The layouts are cached
    /// for the frames that follow, and later calls only look them up.
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
        let key = source(object).map(|text| self.text.key(text));
        self.bounds_with(object, key, None, TextSize::Exact)
    }

    /// Looks up the text of every object (or a formula's source), hashing each once, and charges
    /// the hashing to `budget`.
    fn look_up(&mut self, board: &Board, budget: &mut Budget) -> Vec<Seen> {
        let mut hashed = 0;
        let seen = board
            .objects()
            .iter()
            .map(|object| Seen {
                key: source(object).map(|text| {
                    hashed += text.len();
                    self.text.key(text)
                }),
                math: None,
            })
            .collect();
        budget.charge(budget::hashing(hashed));
        seen
    }

    /// Fills in the math status of every formula in `seen`: for formulas off screen too when the
    /// frame waits, else only for formulas on screen (by their known bounds, or their
    /// placeholder's), which are asked for nearest the centre of the view first, so they queue
    /// ahead of the rest.
    fn math_statuses(
        &mut self,
        board: &Board,
        seen: &mut [Seen],
        vp: &Viewport,
        clip: Rect,
        wait: MathWait,
    ) {
        let to_screen = vp.transform();
        let center = Point::new(f64::from(vp.width) / 2.0, f64::from(vp.height) / 2.0);
        let mut visible = Vec::new();
        for (index, (object, seen)) in board.objects().iter().zip(seen.iter_mut()).enumerate() {
            let (Shape::Math { src, .. }, Some(key)) = (&object.shape, seen.key) else {
                continue;
            };
            if wait == MathWait::Yes {
                let status = match self.math.layout_blocking(src) {
                    Ok(layout) => MathStatus::Ready(layout),
                    Err(err) => MathStatus::Failed(err),
                };
                seen.math = Some(self.noted(key, src, status));
                continue;
            }
            let on_screen = self
                .bounds_with(object, Some(key), None, TextSize::Known)
                .map(|bounds| to_screen.transform_rect_bbox(bounds))
                .filter(|bounds| bounds.is_finite() && bounds.overlaps(clip));
            if let Some(bounds) = on_screen {
                visible.push((distance(bounds, center), index));
            }
        }
        // Stable, so formulas the same distance away are asked for in board order.
        visible.sort_by(|a, b| a.0.total_cmp(&b.0));
        let objects = board.objects();
        for (_, index) in visible {
            let (Some(Shape::Math { src, .. }), Some(seen)) = (
                objects.get(index).map(|object| &object.shape),
                seen.get_mut(index),
            ) else {
                continue;
            };
            let Some(key) = seen.key else {
                continue;
            };
            let status = self.math.request(src);
            seen.math = Some(self.noted(key, src, status));
        }
    }

    /// Lays out the texts on screen that have no layout cached, nearest the centre of the view
    /// first, while their estimated cost fits in `budget`. The nearest is laid out (and charged)
    /// even when it doesn't fit, so a text over the budget still gets laid out and repeated
    /// frames always finish. Texts left are drawn as outlines this frame.
    fn lay_out_on_screen(
        &mut self,
        board: &Board,
        seen: &[Seen],
        vp: &Viewport,
        clip: Rect,
        budget: &mut Budget,
        stats: &mut FrameStats,
    ) {
        if text_view(Point::ZERO, vp, clip).is_none() {
            // Text isn't drawn at this zoom.
            return;
        }
        let to_screen = vp.transform();
        let center = Point::new(f64::from(vp.width) / 2.0, f64::from(vp.height) / 2.0);
        let mut wanted = Vec::new();
        for (index, (object, seen)) in board.objects().iter().zip(seen).enumerate() {
            let (Some((at, text)), Some(key)) =
                (drawn_as_text(object, seen.math.as_ref()), seen.key)
            else {
                continue;
            };
            let known = self.text.known(key, text);
            if known.shaped {
                continue;
            }
            let screen = to_screen.transform_rect_bbox(self.text_rect(at, &known));
            let shown = screen.is_finite()
                && screen.overlaps(clip)
                && screen.width().max(screen.height()) >= MIN_VISIBLE_PX;
            if shown {
                wanted.push((distance(screen, center), index));
            }
        }
        wanted.sort_by(|a, b| a.0.total_cmp(&b.0));
        let objects = board.objects();
        let mut first = true;
        for (_, index) in wanted {
            let (Some(object), Some(seen)) = (objects.get(index), seen.get(index)) else {
                continue;
            };
            let (Some((_, text)), Some(key)) =
                (drawn_as_text(object, seen.math.as_ref()), seen.key)
            else {
                continue;
            };
            let known = self.text.known(key, text);
            if known.shaped {
                // The same text again, laid out above.
                continue;
            }
            let facts = known.facts;
            let cost = budget::layout(facts.chars, facts.non_ascii, facts.changes, facts.lines);
            if first {
                budget.charge(cost);
            } else if !budget.spend(cost) {
                continue;
            }
            first = false;
            self.text.lay_out(key, text, &mut self.glyphs);
            stats.laid_out += 1;
        }
    }

    /// Draws every object on screen in board order, or its outline when it doesn't fit in
    /// `budget`, counting what became of each in `stats`.
    fn draw_objects(
        &mut self,
        board: &Board,
        seen: &[Seen],
        vp: &Viewport,
        clip: Rect,
        budget: &mut Budget,
        stats: &mut FrameStats,
    ) {
        let to_screen = vp.transform();
        for (object, seen) in board.objects().iter().zip(seen) {
            let math = seen.math.as_ref();
            if matches!(object.shape, Shape::Math { .. }) && math.is_none() {
                // Off screen, so not asked for.
                continue;
            }
            if matches!(math, Some(MathStatus::Pending)) {
                stats.math_pending += 1;
            }
            let Some(screen) = self
                .bounds_with(object, seen.key, math, TextSize::Known)
                .map(|bounds| to_screen.transform_rect_bbox(bounds))
                .filter(|bounds| bounds.is_finite() && bounds.overlaps(clip))
            else {
                continue;
            };
            if screen.width().max(screen.height()) < MIN_VISIBLE_PX {
                stats.too_small += 1;
                continue;
            }
            let outcome = self.draw_object(object, seen, vp, clip, budget);
            match outcome {
                Outcome::Drawn => {
                    stats.drawn += 1;
                    continue;
                }
                Outcome::OverBudget => stats.downgraded += 1,
                Outcome::Pending => stats.text_pending += 1,
                Outcome::Evicted => stats.math_evicted += 1,
            }
            if !self.draw_outline(screen, vp, clip, budget) {
                stats.left_out += 1;
            }
        }
    }

    /// `status`, after remembering the bounds of a laid-out formula (or one
    /// whose layout the engine pushed out of its cache).
    fn noted(&mut self, key: TextKey, src: &str, status: MathStatus) -> MathStatus {
        match &status {
            MathStatus::Ready(layout) => self.record_bounds(key, src, layout_bounds(layout)),
            MathStatus::Evicted(summary) => self.record_bounds(key, src, summary_bounds(summary)),
            MathStatus::Pending | MathStatus::Failed(_) => {}
        }
        status
    }

    fn record_bounds(&mut self, key: TextKey, src: &str, bounds: Rect) {
        let frame = self.frame;
        if let Some(known) = self
            .known_math
            .get_mut(&key.0)
            .filter(|known| *known.src == *src)
        {
            known.bounds = bounds;
            known.last_used = frame;
        } else {
            self.known_math.insert(
                key.0,
                KnownMath {
                    src: src.into(),
                    bounds,
                    last_used: frame,
                },
            );
        }
    }

    /// The bounds of a formula known to have laid out, in pt within its layout.
    fn known_math_bounds(&mut self, key: TextKey, src: &str) -> Option<Rect> {
        let frame = self.frame;
        self.known_math
            .get_mut(&key.0)
            .filter(|known| *known.src == *src)
            .map(|known| {
                known.last_used = frame;
                known.bounds
            })
    }

    /// An object's bounds, given the key of its text or source (hashed here when `None`) and
    /// its math status from [`Self::math_statuses`] (`None`: what this renderer knows, without
    /// asking the engine).
    fn bounds_with(
        &mut self,
        object: &Object,
        key: Option<TextKey>,
        math: Option<&MathStatus>,
        size: TextSize,
    ) -> Option<Rect> {
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
            Shape::Text { at, text } => {
                let key = key.unwrap_or_else(|| self.text.key(text));
                Some(self.text_bounds(*at, key, text, size))
            }
            Shape::Math { at, src } => {
                let key = key.unwrap_or_else(|| self.text.key(src));
                let laid_out = match math {
                    Some(MathStatus::Ready(layout)) => Some(layout_bounds(layout)),
                    Some(MathStatus::Evicted(summary)) => Some(summary_bounds(summary)),
                    // Still compiling or failed: the source is drawn as text.
                    Some(MathStatus::Pending | MathStatus::Failed(_)) => None,
                    None => self.known_math_bounds(key, src),
                };
                match laid_out {
                    Some(pt) => Some(math_to_board(*at).transform_rect_bbox(pt)),
                    None => Some(self.text_bounds(*at, key, src, size)),
                }
            }
        };
        bounds.filter(Rect::is_finite)
    }

    /// The bounds of text with its top-left corner at `at`, in grid units.
    fn text_bounds(&mut self, at: Point, key: TextKey, text: &str, size: TextSize) -> Rect {
        match size {
            TextSize::Exact => {
                let (width, height) = self.text.lay_out(key, text, &mut self.glyphs);
                text_box(at, width, height)
            }
            TextSize::Known => {
                let known = self.text.known(key, text);
                self.text_rect(at, &known)
            }
        }
    }

    /// The bounds of a text the cache knows, with its top-left corner at `at`: of its layout,
    /// or estimated when it has none yet.
    fn text_rect(&self, at: Point, known: &Known) -> Rect {
        let (width, height) = self.text.size(known);
        text_box(at, width, height)
    }

    /// Draws one object whose bounds reach into `clip` (screen pixels), if
    /// its estimated cost fits in `budget`.
    fn draw_object(
        &mut self,
        object: &Object,
        seen: &Seen,
        vp: &Viewport,
        clip: Rect,
        budget: &mut Budget,
    ) -> Outcome {
        let fits = |drawn: bool| {
            if drawn {
                Outcome::Drawn
            } else {
                Outcome::OverBudget
            }
        };
        match &object.shape {
            Shape::Ellipse { center, rx, ry } => {
                fits(self.stroke_px(draw::ellipse(vp, *center, *rx, *ry, clip), vp, clip, budget))
            }
            Shape::Rect { origin, w, h } => {
                fits(self.stroke_px(draw::rect(vp, *origin, *w, *h, clip), vp, clip, budget))
            }
            Shape::Line { a, b } => {
                fits(self.stroke_px(draw::line(vp, *a, *b, clip), vp, clip, budget))
            }
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
                    return Outcome::OverBudget;
                }
                if let Some(shaft) = shaft {
                    self.stroke_ink(&shaft, width);
                }
                if let Some(head) = head {
                    self.ctx.set_transform(Affine::IDENTITY);
                    self.ctx.set_paint(INK);
                    self.ctx.fill_path(&head);
                }
                Outcome::Drawn
            }
            Shape::Text { at, text } => match seen.key {
                Some(key) => self.draw_text(*at, key, text, INK, vp, clip, budget),
                None => Outcome::Pending,
            },
            Shape::Math { at, src } => match (&seen.math, seen.key) {
                (Some(MathStatus::Ready(layout)), _) => {
                    fits(self.draw_math(*at, layout, vp, clip, budget))
                }
                // Laid out before, but pushed out of the math cache and not compiled again for
                // now: drawn as the faint outline of its bounds, and nothing is asked for.
                (Some(MathStatus::Evicted(_)), _) => Outcome::Evicted,
                (Some(MathStatus::Failed(err)), Some(key)) => {
                    tracing::debug!(id = %object.id, %err, "drawing math source instead of math");
                    self.draw_text(*at, key, src, ERROR, vp, clip, budget)
                }
                (Some(MathStatus::Pending) | None, Some(key)) => {
                    self.draw_text(*at, key, src, PENDING, vp, clip, budget)
                }
                (_, None) => Outcome::Pending,
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

    /// Draws text with its top-left corner at `at`, if its estimated cost fits in `budget`:
    /// only the lines whose baseline is near `clip`, and of those only the glyph runs and glyphs
    /// that can reach into it. The estimate comes from the text's plan, so a text over the
    /// budget is never walked. [`Outcome::Pending`] when the text has no layout cached.
    #[expect(
        clippy::too_many_arguments,
        reason = "the text, where and how to draw it, and the budget"
    )]
    fn draw_text(
        &mut self,
        at: Point,
        key: TextKey,
        text: &str,
        color: AlphaColor<Srgb>,
        vp: &Viewport,
        clip: Rect,
        budget: &mut Budget,
    ) -> Outcome {
        let Some(view) = text_view(at, vp, clip) else {
            // Nothing of it is drawn at this zoom.
            return Outcome::Drawn;
        };
        let Some(shaped) = self.text.shaped(key, text) else {
            return Outcome::Pending;
        };
        let plan = &shaped.plan;
        let lines = view.lines(plan);
        let (cost, charged) = text_cost(plan, lines.clone(), &view, vp, clip, budget.left());
        if !budget.settle(&cost) {
            return Outcome::OverBudget;
        }
        self.work.text_charged.add(&charged);
        let TextView {
            transform, near, ..
        } = view;
        let slack = RUN_SLACK_EM * f64::from(LAYOUT_EM);
        let screen_x = |x: f64| (transform * Point::new(x, 0.0)).x;
        self.ctx.set_transform(transform);
        self.ctx.set_paint(color);
        // Only lines with glyphs have chunks, so lines without are never looked at, and a line
        // is walked only when a run of it can reach the image ([`text_cost`] charges the same).
        let mut last = None;
        for chunk in plan.chunks(lines) {
            if last == Some(chunk.line) || !view.reaches(chunk) {
                continue;
            }
            last = Some(chunk.line);
            let Some(line) = shaped.layout.get(chunk.line as usize) else {
                continue;
            };
            self.work.text.lines += 1.0;
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                self.work.text.walked_runs += 1.0;
                let start = f64::from(glyph_run.offset());
                let end = start + f64::from(glyph_run.advance());
                if screen_x(end + slack) < near.x0 || screen_x(start - slack) > near.x1 {
                    // None of its glyphs can reach the image.
                    continue;
                }
                let run = glyph_run.run();
                let coords = bytemuck::cast_slice::<_, i16>(run.normalized_coords());
                // Counted as `vello_cpu` takes them: it goes over the glyphs once.
                let (walked, kept) = (Cell::new(0_u32), Cell::new(0_u32));
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
                                walked.set(walked.get() + 1);
                                near.contains(
                                    transform * Point::new(f64::from(glyph.x), f64::from(glyph.y)),
                                )
                            })
                            .map(|glyph| {
                                kept.set(kept.get() + 1);
                                Glyph {
                                    id: glyph.id,
                                    x: glyph.x,
                                    y: glyph.y,
                                }
                            }),
                    );
                self.work.text.runs += 1.0;
                self.work.text.walked += f64::from(walked.get());
                self.work.text.glyphs += f64::from(kept.get());
                if let Err(err) = drawn {
                    tracing::debug!(?err, "some text glyphs failed to draw");
                }
            }
        }
        Outcome::Drawn
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
                    let kept = Cell::new(0);
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
                                .map(|glyph| {
                                    kept.set(kept.get() + 1);
                                    Glyph {
                                        id: glyph.id,
                                        x: to_f32(glyph.x),
                                        y: to_f32(glyph.y),
                                    }
                                }),
                        );
                    self.work.math_runs += 1;
                    self.work.math_glyphs += kept.get();
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

/// The em size of text in grid units.
fn text_em_units() -> f64 {
    f64::from(LAYOUT_EM) * text_scale()
}

/// The box of a text `width` x `height` layout units with its top-left corner at `at`, in grid
/// units.
fn text_box(at: Point, width: f32, height: f32) -> Rect {
    Rect::from_origin_size(
        at,
        (
            f64::from(width) * text_scale(),
            f64::from(height) * text_scale(),
        ),
    )
}

/// The text an object draws, or a formula's source: what the text cache is looked up by.
fn source(object: &Object) -> Option<&str> {
    match &object.shape {
        Shape::Text { text, .. } => Some(text),
        Shape::Math { src, .. } => Some(src),
        _ => None,
    }
}

/// Where and what an object draws as text, given its math status: a text, or the source of a
/// formula still compiling or that failed.
fn drawn_as_text<'a>(object: &'a Object, math: Option<&MathStatus>) -> Option<(Point, &'a str)> {
    match (&object.shape, math) {
        (Shape::Text { at, text }, _) => Some((*at, text)),
        (Shape::Math { at, src }, Some(MathStatus::Pending | MathStatus::Failed(_))) => {
            Some((*at, src))
        }
        _ => None,
    }
}

/// How a text lands on screen.
#[derive(Debug, Clone, Copy)]
struct TextView {
    /// Layout units → screen pixels.
    transform: Affine,
    /// Where a glyph's origin must be for the glyph to be drawn.
    near: Rect,
    /// Pixels to the em.
    em_px: f64,
}

impl TextView {
    /// Whether the origins of `chunk`'s glyphs are near enough the image for them to be drawn
    /// (on a line near it).
    fn shows(&self, chunk: &Chunk) -> bool {
        self.spans(chunk, 0.0)
    }

    /// Whether a glyph run with glyphs in `chunk` can reach the image: a run reaches a glyph's
    /// advance past its last glyph's origin, advances are under 2 ems, and [`Renderer::draw_text`]
    /// keeps runs that come within [`RUN_SLACK_EM`] of the image.
    fn reaches(&self, chunk: &Chunk) -> bool {
        self.spans(chunk, (RUN_SLACK_EM + 2.0) * f64::from(LAYOUT_EM))
    }

    /// Whether `chunk`'s glyph origins, `reach` layout units further out to either side, overlap
    /// the image's glyph area horizontally.
    fn spans(&self, chunk: &Chunk, reach: f64) -> bool {
        let screen_x = |x: f64| (self.transform * Point::new(x, 0.0)).x;
        screen_x(f64::from(chunk.x1) + reach) >= self.near.x0
            && screen_x(f64::from(chunk.x0) - reach) <= self.near.x1
    }

    /// The lines of `plan` whose baseline is near the image: text is only scaled and moved, so
    /// a line's screen height depends on its baseline alone.
    fn lines(&self, plan: &Plan) -> Range<usize> {
        let transform = self.transform;
        plan.lines_within(
            |baseline| (transform * Point::new(0.0, f64::from(baseline))).y,
            self.near.y0,
            self.near.y1,
        )
    }
}

/// How text with its top-left corner at `at` lands on screen through `vp`; `None` when no text
/// is drawn at this zoom (glyphs under half a pixel, or absurdly large).
fn text_view(at: Point, vp: &Viewport, clip: Rect) -> Option<TextView> {
    let em = text_em_units();
    let em_px = em * vp.px_per_unit();
    if em_px < MIN_VISIBLE_PX {
        return None;
    }
    let near = glyph_area(em, vp, clip)?;
    Some(TextView {
        transform: vp.transform() * Affine::translate(at.to_vec2()) * Affine::scale(text_scale()),
        near,
        em_px,
    })
}

/// The estimated cost of [`Renderer::draw_text`] drawing `lines` of a text planned as `plan`
/// through `view`: walking those lines, and drawing the chunks of their glyphs whose origins
/// reach into `view.near`. Runs are set up for chunks a little further out too, as
/// [`Renderer::draw_text`] keeps every run that comes within [`RUN_SLACK_EM`] of the image.
/// Counted until it is past `limit`; with what it charged for.
fn text_cost(
    plan: &Plan,
    lines: Range<usize>,
    view: &TextView,
    vp: &Viewport,
    clip: Rect,
    limit: f64,
) -> (Estimate, TextWork) {
    let TextView {
        transform, em_px, ..
    } = *view;
    let mut cost = Estimate::new(clip, vp.width, vp.height, limit);
    let mut charged = TextWork::default();
    let (mut drawn, mut culled) = (GlyphSums::default(), GlyphSums::default());
    let mut walked = None;
    for chunk in plan.chunks(lines) {
        if cost.over() {
            break;
        }
        cost.chunk();
        if !view.reaches(chunk) {
            continue;
        }
        if walked != Some(chunk.line) {
            // The first chunk of its line that reaches the image: the line is walked.
            walked = Some(chunk.line);
            let (glyphs, runs) = plan.on_line(chunk.line as usize);
            cost.text_line(glyphs, runs);
            charged.lines += 1.0;
            charged.walked += glyphs;
            charged.walked_runs += runs;
        }
        charged.runs += f64::from(chunk.runs);
        if view.shows(chunk) {
            // Drawn: in full where their ink can reach the image, else only flattened.
            let [left, top, right, bottom] = chunk.ink.map(f64::from);
            let ink = transform.transform_rect_bbox(Rect::new(left, top, right, bottom));
            if ink.overlaps(clip) {
                drawn.merge(&chunk.glyphs);
            } else {
                culled.merge(&chunk.glyphs);
            }
        }
    }
    cost.glyphs(&drawn, &culled, charged.runs, em_px);
    charged.glyphs = drawn.count() + culled.count();
    (cost, charged)
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

    /// A board of `count` texts at the origin, the `i`th made by `text(i)`.
    fn text_board(count: usize, text: impl Fn(usize) -> String) -> Board {
        let mut board = Board::new();
        for i in 0..count {
            board
                .insert(Object::new(
                    lawh_core::ObjectId::new(format!("t{i}")).unwrap(),
                    Shape::Text {
                        at: Point::ZERO,
                        text: text(i),
                    },
                ))
                .unwrap();
        }
        board
    }

    /// `n` characters of `alphabet` after `tag`, in lines of `per_line` characters.
    fn cycled(tag: usize, n: usize, per_line: usize, alphabet: &str) -> String {
        let mut out = format!("{tag} ");
        let mut column = out.len();
        let mut chars = alphabet.chars().cycle();
        while out.chars().count() < n {
            if column + 1 >= per_line {
                out.push('\n');
                column = 0;
            } else {
                out.push(chars.next().unwrap());
                column += 1;
            }
        }
        out
    }

    #[test]
    fn a_frame_hashes_each_text_once() {
        let mut renderer = Renderer::new_trusted();
        // A formula that failed and one still compiling are drawn from their source as text.
        assert!(renderer.math.layout_blocking("frac(1,").is_err());
        let board = lawh_core::from_lawh(
            "lawh version=1\n\
             text id=t1 x=0 y=0 \"one\"\n\
             text id=t2 x=0 y=1 \"two\\nlines\"\n\
             math id=m1 x=0 y=3 \"frac(1,\"\n\
             math id=m2 x=0 y=4 \"A = pi r^2\"\n\
             rect id=r1 x=0 y=0 w=1 h=1\n",
        )
        .unwrap();
        let vp = Viewport::new(Point::new(2.0, 2.0), 2.0, 400, 300, 1.0);
        for _ in 0..3 {
            let hashed = renderer.text.hashed;
            renderer.render(&board, &vp);
            assert_eq!(renderer.text.hashed - hashed, 4);
        }
    }

    #[test]
    fn text_draws_only_the_lines_and_runs_that_reach_the_image() {
        // 499 lines of words, and one line of 5,000 characters whose direction changes at every
        // character: a glyph run each.
        let tall = cycled(0, 5_000, 11, "lorem ipsum ");
        let wide = cycled(0, 5_000, usize::MAX, "a\u{5D0}");
        for (name, text, most_lines, most_runs) in
            [("tall", tall, 8.0, 8.0), ("wide", wide, 1.0, 64.0)]
        {
            let board = text_board(1, |_| text.clone());
            let mut renderer = Renderer::new_trusted();
            renderer.board_bounds(&board);
            // Zoom 20 on the text's first line: a few lines, a few dozen characters.
            let vp = Viewport::new(Point::new(2.0, 0.3), 20.0, 400, 300, 1.0);
            renderer.render(&board, &vp);
            let (frame, work) = (renderer.last_frame(), renderer.work.text);
            assert_eq!((frame.drawn, frame.laid_out), (1, 0), "{name}: {frame:?}");
            assert!(work.glyphs > 0.0, "{name}: {work:?}");
            assert!(work.lines <= most_lines, "{name}: {work:?}");
            assert!(work.runs <= most_runs, "{name}: {work:?}");
            // Only glyphs of the runs kept are walked.
            assert!(work.walked <= 200.0, "{name}: {work:?}");
        }
    }

    /// Every piece of drawing work the text of the last frame did was charged for by its estimates.
    fn assert_text_work_charged(renderer: &Renderer, what: &str) {
        let (done, charged) = (renderer.work.text, renderer.work.text_charged);
        let fits = [
            done.lines <= charged.lines,
            done.walked_runs <= charged.walked_runs,
            done.walked <= charged.walked,
            done.runs <= charged.runs,
            done.glyphs <= charged.glyphs,
        ];
        assert!(
            fits.iter().all(|fits| *fits),
            "{what}: did {done:?}, charged for {charged:?}"
        );
    }

    #[test]
    fn drawing_does_no_work_its_estimate_did_not_charge_for() {
        let words = |i| cycled(i, 5_000, 11, "lorem ipsum ");
        let bidi = |i| cycled(i, 5_000, usize::MAX, "a\u{5D0}");
        let small = |i| cycled(i, 80, usize::MAX, "lorem ipsum ");
        let marks = |i| cycled(i, 5_000, 75, "a\u{301}\u{302}\u{303}\u{304}");
        let newlines = |i: usize| format!("{i}{}", "\n".repeat(498));
        let boards = [
            ("words in lines", text_board(8, words)),
            ("direction changing", text_board(8, bidi)),
            ("small texts", text_board(500, small)),
            ("stacked marks", text_board(8, marks)),
            ("newlines", text_board(80, newlines)),
        ];
        for (name, board) in boards {
            let mut renderer = Renderer::new_trusted();
            let bounds = renderer.board_bounds(&board).unwrap();
            let mut glyphs = 0.0;
            for vp in [
                Viewport::fit(bounds, 800, 600, 1.0),
                Viewport::new(Point::new(5.0, 3.0), 1.0, 800, 600, 1.0),
                Viewport::new(Point::new(2.0, 40.0), 1.0, 800, 600, 1.0),
                Viewport::new(Point::new(1.0, 1.0), 20.0, 800, 600, 2.0),
                Viewport::new(Point::new(2.0, 40.3), 20.0, 800, 600, 1.0),
            ] {
                // The second frame at this size has the full budget.
                renderer.render(&board, &vp);
                renderer.render(&board, &vp);
                glyphs += renderer.work.text.glyphs;
                assert_text_work_charged(&renderer, &format!("{name} at {vp:?}"));
                // And all the work counted is within the frame's estimate.
                let work = renderer.work.text;
                let least = budget::least_cost(
                    work.lines,
                    work.walked,
                    work.walked_runs,
                    work.runs,
                    work.glyphs,
                );
                let frame = renderer.last_frame();
                assert!(least <= frame.estimated.as_secs_f64() * 1e9, "{frame:?}");
            }
            assert!(glyphs > 0.0, "{name}: nothing drawn");
        }
        // A pile of math glyphs, drawn whole at zoom 1 and in part at zoom 20.
        let board = lawh_core::from_lawh(
            "lawh version=1\nmath id=m1 x=0 y=0 \"#for i in range(300) { box(width: 0pt)[x] }\"\n",
        )
        .unwrap();
        let mut renderer = Renderer::new_trusted();
        renderer.resolve_math(&board);
        let bounds = renderer.board_bounds(&board).unwrap();
        for zoom in [1.0, 20.0] {
            let vp = Viewport::new(bounds.center(), zoom, 800, 600, 1.0);
            renderer.render(&board, &vp);
            let (work, frame) = (renderer.work, renderer.last_frame());
            assert_eq!(frame.drawn, 1, "{frame:?}");
            let least = budget::least_cost(
                0.0,
                0.0,
                0.0,
                work.math_runs as f64,
                work.math_glyphs as f64,
            );
            assert!(least <= frame.estimated.as_secs_f64() * 1e9, "{frame:?}");
        }
    }

    #[test]
    fn evicted_math_is_an_outline_and_asks_for_nothing() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let wakes = Arc::new(AtomicUsize::new(0));
        let engine = MathEngine::new();
        let woken = Arc::clone(&wakes);
        engine.set_waker(Arc::new(move || {
            woken.fetch_add(1, Ordering::SeqCst);
        }));
        let mut renderer = Renderer::with_math(engine);
        let src = "A = pi r^2";
        let board =
            lawh_core::from_lawh(&format!("lawh version=1\nmath id=m1 x=0 y=0 \"{src}\"\n"))
                .unwrap();
        // What the engine answers for a formula whose layout it pushed out of its cache: here
        // 200 x 40 pt, 5 x 1 grid units, 200 x 40 pixels at zoom 1.
        let summary = MathSummary {
            width: 200.0,
            height: 40.0,
            baseline: 30.0,
            ink: Rect::new(0.0, 0.0, 200.0, 40.0),
            glyphs: 8,
            path_elements: 0,
        };
        let vp = Viewport::new(Point::new(2.5, 0.5), 1.0, 400, 200, 1.0);
        let clip = draw::clip_rect(&vp);
        assert!(renderer.begin_frame(&vp));
        let seen = [Seen {
            key: Some(renderer.text.key(src)),
            math: Some(MathStatus::Evicted(summary)),
        }];
        let mut budget = Budget::frame();
        let mut stats = FrameStats::default();
        renderer.draw_objects(&board, &seen, &vp, clip, &mut budget, &mut stats);
        let image = renderer.finish_frame().to_vec();
        assert_eq!(
            (
                stats.math_evicted,
                stats.drawn,
                stats.downgraded,
                stats.left_out
            ),
            (1, 0, 0, 0),
            "{stats:?}"
        );
        assert!(!stats.incomplete());
        // Its outline, in the faint outline colour, on the left edge of its box: x = 100 px,
        // y from 80 to 120 px. Nothing else is drawn.
        let at = |x: usize, y: usize| {
            let i = (y * 400 + x) * 4;
            [image[i], image[i + 1], image[i + 2], image[i + 3]]
        };
        let faint = |px: [u8; 4]| px[0] == px[1] && (0xC0..0xFA).contains(&px[0]);
        assert!(
            faint(at(99, 100)) || faint(at(100, 100)),
            "{:?}",
            at(100, 100)
        );
        let paper = PAPER.to_rgba8().to_u8_array();
        assert_eq!(at(200, 100), paper);
        assert!(
            image
                .as_chunks::<4>()
                .0
                .iter()
                .all(|px| *px == paper || faint(*px)),
            "only paper and the outline"
        );
        // Nothing was asked of the engine.
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(wakes.load(Ordering::SeqCst), 0);
        assert_eq!(renderer.math.compiles(), 0);
    }
}
