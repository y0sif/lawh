//! What the window draws over the board while editing ([`Renderer::render_with`]): the cursor,
//! a Visual selection, the caret and the status bar. With the queries the app makes of the same
//! geometry: [`Renderer::caret_rect`], [`Renderer::hit_test`] and [`Measure`].

use std::collections::HashSet;

use kurbo::{Affine, BezPath, Cap, Join, Point, Rect, Shape as _, Stroke};
use lawh_core::engine::Measure;
use lawh_core::{Board, Object, ObjectId, Shape};
use parley::{Affinity, Cluster, Layout, PositionedLayoutItem, Selection};
use vello_cpu::color::{AlphaColor, Srgb};
use vello_cpu::{Glyph, RenderContext, Resources};

use super::{
    CURSOR, CURSOR_BLOCK, ERROR, INK, MathWait, Renderer, SELECTED, STATUS_BAR, STATUS_EDGE, Seen,
    TextSize, VISUAL_EDGE, VISUAL_FILL, text_scale, text_view,
};
use crate::budget::{Budget, Estimate};
use crate::overlay::{Caret, Overlay, OverlayCursor, STATUS_FONT_PX, status_bar_height_px};
use crate::status::{Keep, window};
use crate::text::{LAYOUT_EM, TextKey};
use crate::{Viewport, draw};

// Sizes in logical pixels, times the scale factor on screen.

/// Half the width of the grid cursor's cross.
const CROSS_ARM_PX: f64 = 7.0;
/// Width of the cross's lines.
const CROSS_LINE_PX: f64 = 1.5;
/// How far the outline around the current object stands off its bounds, and its width.
const CURRENT_GAP_PX: f64 = 5.0;
const CURRENT_LINE_PX: f64 = 1.5;
/// The same for the outline around each selected object: inside the current object's, so an
/// object that is both shows both.
const SELECTED_GAP_PX: f64 = 2.5;
const SELECTED_LINE_PX: f64 = 1.5;
/// Width of the Visual rectangle's edge.
const VISUAL_LINE_PX: f64 = 1.0;
/// Width of the caret bar, in a text block in Insert mode and on the command line.
const CARET_PX: f64 = 2.0;
/// Space between the status bar's ends and its text, and between its left and right text.
const STATUS_PAD_PX: f64 = 8.0;
/// Height of the line along the status bar's top.
const STATUS_EDGE_PX: f64 = 1.0;

/// Width of the block caret where there is no character to cover (an empty line, the end of a
/// text), in ems.
const EMPTY_BLOCK_EM: f64 = 0.5;

impl Renderer {
    /// Draws `board` through `vp` as [`Self::render`] does, then `overlay` over it: the cursor,
    /// a Visual selection, the caret in a text block and the status bar along the bottom (see
    /// [`Overlay`]). A math block with the caret in it is drawn as its source in the board's
    /// text style, without asking the math engine for it; once the caret leaves, the formula is
    /// asked for and drawn again.
    ///
    /// Cost: the overlay is drawn after the board's budgeted work, and isn't charged to the
    /// frame's budget or counted in [`FrameStats`](crate::FrameStats). Its cost is bounded by
    /// the image, not the board: a few outlines and fills cut to the image, a caret, and at most
    /// two status strings of `STATUS_WINDOW_CHARS` characters, laid out only when they change.
    /// Highlighting the objects of a Visual selection has its own budget (`HIGHLIGHT_BUDGET_NS`,
    /// 3 ms of estimated work); past it the rest of the selection isn't highlighted. Measured in
    /// release (`overlay_frame_time` in `tests/bench.rs`, Linux `x86_64`, 2026-10-11): an
    /// everyday overlay adds 0.1 ms to a frame at 1920x1080 and 0.2 ms at 2880x1800; the worst
    /// (a full board selected, a Visual rectangle over the whole view, the longest command line
    /// changing every frame) 1.9 ms and 4.2 ms.
    pub fn render_with(&mut self, board: &Board, vp: &Viewport, overlay: &Overlay<'_>) -> &[u8] {
        self.draw_frame_with(board, vp, MathWait::No, Some(overlay))
    }

    /// Where the caret stands before byte `byte` of the text (or math source) of object `id`,
    /// in grid units: a rectangle of no width at its left edge, one line high. `None` when
    /// there is no such object or it isn't a text or a formula. A byte past the end stands at
    /// the end; one inside a grapheme, at the start of that grapheme.
    ///
    /// Laid out as the board draws it: a formula as its source, as it is drawn while the caret
    /// is in it. Lays the text out if this renderer hasn't yet.
    pub fn caret_rect(&mut self, board: &Board, id: &ObjectId, byte: usize) -> Option<Rect> {
        let (at, text) = block_text(board.get(id)?)?;
        let key = self.text.key(text);
        self.text.lay_out(key, text, &mut self.glyphs);
        self.caret_box(at, key, text, byte, false)
    }

    /// The object a click at `at` selects; `None` when it hits none. Each kind is hit where it
    /// is drawn, within `tolerance` grid units:
    ///
    /// - a line or an arrow near its segment (not anywhere in its bounding box, which for a
    ///   long diagonal covers much of the board);
    /// - an ellipse inside its shape or near its outline (not in the corners of its box);
    /// - a rectangle inside it or near its outline;
    /// - a text or a formula in its box ([`Self::object_bounds`]).
    ///
    /// When several are hit, a text or a formula wins over a shape (a label inside a frame is
    /// what the click is for), then the one covering the smallest area (a line covers none, so
    /// it wins over the shape it crosses), then the topmost (the last in board order). A few
    /// screen pixels of tolerance make thin lines easy to click. A tolerance that isn't a
    /// number above zero counts as 0.
    ///
    /// Lays out only texts whose estimated bounds hold `at`.
    pub fn hit_test(&mut self, board: &Board, at: Point, tolerance: f64) -> Option<ObjectId> {
        if !at.is_finite() {
            return None;
        }
        let reach = if tolerance > 0.0 {
            tolerance.min(f64::MAX)
        } else {
            0.0
        };
        let in_box = |bounds: Rect| {
            let grown = bounds.inflate(reach, reach);
            grown.x0 <= at.x && at.x <= grown.x1 && grown.y0 <= at.y && at.y <= grown.y1
        };
        let mut best: Option<(Hit, &ObjectId)> = None;
        for object in board.objects() {
            // Nothing outside an object's bounds grown by the tolerance can hit it. A text's
            // estimated bounds are no smaller than its layout's, so only texts that may be hit
            // are laid out.
            let Some(near) = self
                .bounds_with(object, None, None, TextSize::Known)
                .filter(|bounds| in_box(*bounds))
            else {
                continue;
            };
            let hit = match &object.shape {
                Shape::Line { a, b } | Shape::Arrow { a, b } => {
                    (segment_distance(at, *a, *b) <= reach).then_some(Hit::shape(0.0))
                }
                Shape::Ellipse { center, rx, ry } => ellipse_hit(at, *center, *rx, *ry, reach)
                    .then(|| Hit::shape(std::f64::consts::PI * rx.abs() * ry.abs())),
                Shape::Rect { .. } => Some(Hit::shape(near.area().abs())),
                Shape::Text { .. } | Shape::Math { .. } => self
                    .object_bounds(object)
                    .filter(|bounds| in_box(*bounds))
                    .map(|bounds| Hit::block(bounds.area().abs())),
            };
            // Later objects are drawn on top, so they win a tie.
            if let Some(hit) = hit
                && best.is_none_or(|(best, _)| !best.beats(hit))
            {
                best = Some((hit, &object.id));
            }
        }
        best.map(|(_, id)| id.clone())
    }

    /// Draws `overlay` over the board drawn from `seen`, cut to `clip`.
    pub(super) fn draw_overlay(
        &mut self,
        board: &Board,
        seen: &[Seen],
        vp: &Viewport,
        clip: Rect,
        overlay: &Overlay<'_>,
    ) {
        let sf = vp.scale_factor;
        if let Some(visual) = overlay.visual {
            let screen = vp.transform().transform_rect_bbox(visual);
            if screen.is_finite() && screen.overlaps(clip) {
                self.fill_screen(screen.intersect(clip), VISUAL_FILL);
                if let Some(edge) = box_path(screen, clip) {
                    self.stroke_screen(&edge, VISUAL_LINE_PX * sf, VISUAL_EDGE);
                }
            }
        }
        if !overlay.selection.is_empty() {
            self.draw_highlights(board, seen, vp, clip, overlay.selection);
        }
        match overlay.cursor {
            OverlayCursor::Grid(point) => self.draw_cross(vp.to_screen(point), sf, clip),
            OverlayCursor::Object(id) => {
                let caret = overlay.caret.filter(|caret| caret.id == id);
                if let Some(screen) = self.current_bounds(board, seen, id, caret, vp) {
                    let gap = CURRENT_GAP_PX * sf;
                    if let Some(edge) = box_path(screen.inflate(gap, gap), clip) {
                        self.stroke_screen(&edge, CURRENT_LINE_PX * sf, CURSOR);
                    }
                }
            }
            OverlayCursor::None => {}
        }
        if let Some(caret) = overlay.caret {
            self.draw_caret(board, seen, vp, clip, caret);
        }
        self.draw_status_bar(vp, overlay);
    }

    /// The bounds of object `id` on screen, as the frame drew it, grown to hold the caret when
    /// `caret` is in it, so the outline always goes around the caret.
    fn current_bounds(
        &mut self,
        board: &Board,
        seen: &[Seen],
        id: &ObjectId,
        caret: Option<Caret<'_>>,
        vp: &Viewport,
    ) -> Option<Rect> {
        let index = board.objects().iter().position(|object| object.id == *id)?;
        let (object, seen) = (board.objects().get(index)?, seen.get(index)?);
        let mut bounds = self.bounds_with(object, seen.key, seen.math.as_ref(), TextSize::Known)?;
        if let (Some(caret), Some((at, text)), Some(key)) = (caret, block_text(object), seen.key)
            && let Some(caret) = self.caret_box(at, key, text, caret.byte, caret.block)
        {
            bounds = bounds.union(caret);
        }
        Some(vp.transform().transform_rect_bbox(bounds)).filter(Rect::is_finite)
    }

    /// Outlines the objects of `selection` that reach into `clip`, in board order, while their
    /// estimated cost fits in the highlights' budget.
    fn draw_highlights(
        &mut self,
        board: &Board,
        seen: &[Seen],
        vp: &Viewport,
        clip: Rect,
        selection: &[ObjectId],
    ) {
        let selected: HashSet<&ObjectId> = selection.iter().collect();
        let to_screen = vp.transform();
        let gap = SELECTED_GAP_PX * vp.scale_factor;
        let width = SELECTED_LINE_PX * vp.scale_factor;
        let mut budget = Budget::highlights();
        let mut path = BezPath::new();
        for (object, seen) in board.objects().iter().zip(seen) {
            if !selected.contains(&object.id) {
                continue;
            }
            let Some(edge) = self
                .bounds_with(object, seen.key, seen.math.as_ref(), TextSize::Known)
                .map(|bounds| to_screen.transform_rect_bbox(bounds).inflate(gap, gap))
                .and_then(|screen| box_path(screen, clip))
            else {
                continue;
            };
            let mut cost = Estimate::new(clip, vp.width, vp.height, budget.left());
            cost.stroke(&edge, width);
            if budget.settle(&cost) {
                path.extend(edge);
                self.work.highlighted += 1;
            } else {
                self.work.not_highlighted += 1;
            }
        }
        if self.work.not_highlighted > 0 {
            tracing::debug!(
                highlighted = self.work.highlighted,
                not_highlighted = self.work.not_highlighted,
                "highlights' budget spent; left the rest of the selection without one"
            );
        }
        if !path.elements().is_empty() {
            self.stroke_screen(&path, width, SELECTED);
        }
    }

    /// Draws the grid cursor's cross at `at`, in screen pixels.
    fn draw_cross(&mut self, at: Point, sf: f64, clip: Rect) {
        if !(at.is_finite() && clip.contains(at)) {
            return;
        }
        let arm = CROSS_ARM_PX * sf;
        let mut cross = BezPath::new();
        cross.move_to((at.x - arm, at.y));
        cross.line_to((at.x + arm, at.y));
        cross.move_to((at.x, at.y - arm));
        cross.line_to((at.x, at.y + arm));
        self.stroke_screen(&cross, CROSS_LINE_PX * sf, CURSOR);
    }

    /// Draws `caret` in the text it is in, when that text is drawn and laid out (a frame that
    /// hasn't laid it out yet lays it out next, [`FrameStats::incomplete`](crate::FrameStats)).
    fn draw_caret(
        &mut self,
        board: &Board,
        seen: &[Seen],
        vp: &Viewport,
        clip: Rect,
        caret: Caret<'_>,
    ) {
        let Some(index) = board
            .objects()
            .iter()
            .position(|object| object.id == *caret.id)
        else {
            return;
        };
        let (Some(object), Some(Seen { key: Some(key), .. })) =
            (board.objects().get(index), seen.get(index))
        else {
            return;
        };
        let Some((at, text)) = block_text(object) else {
            return;
        };
        if text_view(at, vp, clip).is_none() {
            // Its text isn't drawn at this zoom.
            return;
        }
        let Some(rect) = self.caret_box(at, *key, text, caret.byte, caret.block) else {
            return;
        };
        let screen = vp.transform().transform_rect_bbox(rect);
        if !(screen.is_finite() && screen.overlaps(clip)) {
            return;
        }
        if caret.block {
            self.fill_screen(screen.intersect(clip), CURSOR_BLOCK);
        } else {
            let half = CARET_PX * vp.scale_factor / 2.0;
            let bar = Rect::new(screen.x0 - half, screen.y0, screen.x0 + half, screen.y1);
            self.fill_screen(bar.intersect(clip), CURSOR);
        }
    }

    /// The caret before byte `byte` of `text` (a block with its top-left corner at `at`), in grid
    /// units: of no width, or over the grapheme there for a `block` caret. `None` when the text
    /// isn't laid out.
    fn caret_box(
        &self,
        at: Point,
        key: TextKey,
        text: &str,
        byte: usize,
        block: bool,
    ) -> Option<Rect> {
        let shaped = self.text.shaped(key, text)?;
        let caret = caret_in_layout(&shaped.layout, byte, block, self.text.line_height());
        let scale = text_scale();
        Some(Rect::new(
            at.x + caret.x0 * scale,
            at.y + caret.y0 * scale,
            at.x + caret.x1 * scale,
            at.y + caret.y1 * scale,
        ))
    }

    /// Draws the status bar along the bottom of the image: the status line's left text (or the
    /// command line, with its caret) and right text.
    fn draw_status_bar(&mut self, vp: &Viewport, overlay: &Overlay<'_>) {
        let sf = vp.scale_factor;
        let (width, height) = (f64::from(vp.width), f64::from(vp.height));
        let bar_height = f64::from(status_bar_height_px(sf)).min(height);
        let top = height - bar_height;
        self.fill_screen(Rect::new(0.0, top, width, height), STATUS_BAR);
        let edge = (STATUS_EDGE_PX * sf).round().max(1.0);
        self.fill_screen(
            Rect::new(0.0, top, width, (top + edge).min(height)),
            STATUS_EDGE,
        );

        let px_per_layout_unit = STATUS_FONT_PX * sf / f64::from(LAYOUT_EM);
        let pad = STATUS_PAD_PX * sf;
        let status = overlay.status;
        let (left, caret, left_color) = if let Some((text, cursor)) = overlay.command_line {
            let (shown, at) = window(":", text, Keep::Around(cursor));
            (shown, Some(at), INK)
        } else {
            let color = if status.error { ERROR } else { INK };
            (window("", status.left, Keep::Start).0, None, color)
        };
        let right = window("", status.right, Keep::End).0;
        let left = (!left.is_empty()).then(|| self.status.shape(&mut self.text, &left));
        let right = (!right.is_empty()).then(|| self.status.shape(&mut self.text, &right));
        // Text is centred in the bar, below its edge.
        let at_y = |layout: &Layout<()>| {
            top + edge + (bar_height - edge - f64::from(layout.height()) * px_per_layout_unit) / 2.0
        };
        let right_x = right
            .and_then(|slot| self.status.layout(slot))
            .map(|layout| width - pad - f64::from(layout.width()) * px_per_layout_unit);
        // The right text takes at most the right half while there is left text: a longer one
        // shows its end.
        let right_from = right_x.map(|x| {
            if left.is_some() {
                x.max(width / 2.0)
            } else {
                x
            }
        });
        // The left text may run up to here; the right text is drawn over the rest.
        let left_end = right_from.map_or(width - pad, |x| x - pad);

        if let Some(layout) = left.and_then(|slot| self.status.layout(slot)) {
            let caret_bar = caret.map(|byte| caret_in_layout(layout, byte, false, 0.0));
            let mut x = pad;
            if let Some(bar) = caret_bar {
                // Scrolled left as far as it takes to keep the caret in view.
                let past = pad + bar.x0 * px_per_layout_unit + CARET_PX * sf - left_end;
                if past > 0.0 {
                    x -= past;
                }
            }
            let transform =
                Affine::translate((x, at_y(layout))) * Affine::scale(px_per_layout_unit);
            fill_layout(
                &mut self.ctx,
                &mut self.resources,
                layout,
                transform,
                left_color,
            );
            if let Some(bar) = caret_bar.map(|bar| transform.transform_rect_bbox(bar)) {
                // Right of the boundary, where the next character goes: at the end of the
                // line it doesn't touch the last one.
                let bar = Rect::new(bar.x0, bar.y0, bar.x0 + CARET_PX * sf, bar.y1);
                self.ctx.set_transform(Affine::IDENTITY);
                self.ctx.set_paint(CURSOR);
                self.ctx.fill_rect(&bar);
            }
        }
        if let (Some(layout), Some(x), Some(from)) = (
            right.and_then(|slot| self.status.layout(slot)),
            right_x,
            right_from,
        ) {
            // Over any left text that runs this far.
            self.ctx.set_transform(Affine::IDENTITY);
            self.ctx.set_paint(STATUS_BAR);
            self.ctx
                .fill_rect(&Rect::new(from - pad, top + edge, width, height));
            let cut = x < from;
            if cut {
                self.ctx
                    .push_clip_rect(&Rect::new(from, top, width, height));
            }
            let transform =
                Affine::translate((x, at_y(layout))) * Affine::scale(px_per_layout_unit);
            fill_layout(&mut self.ctx, &mut self.resources, layout, transform, INK);
            if cut {
                self.ctx.pop_clip();
            }
        }
    }

    /// Fills `rect`, in screen pixels.
    fn fill_screen(&mut self, rect: Rect, color: AlphaColor<Srgb>) {
        if rect.width() > 0.0 && rect.height() > 0.0 {
            self.ctx.set_transform(Affine::IDENTITY);
            self.ctx.set_paint(color);
            self.ctx.fill_rect(&rect);
        }
    }

    /// Strokes `path`, in screen pixels, `width` pixels wide with square corners.
    fn stroke_screen(&mut self, path: &BezPath, width: f64, color: AlphaColor<Srgb>) {
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_paint(color);
        self.ctx.set_stroke(
            Stroke::new(width)
                .with_join(Join::Miter)
                .with_caps(Cap::Butt),
        );
        self.ctx.stroke_path(path);
    }
}

/// The renderer measures objects for the engine as it draws them: [`Renderer::object_bounds`].
///
/// A formula this renderer hasn't seen laid out yet (one still compiling, as right after
/// `:ma ...`) is measured by its source text, as it is drawn until it compiles; its bounds
/// change once it has. So a command that places by size right after making one (`o` in the
/// same mapping as `:ma ...`) places by the source's size, not the formula's. This is
/// deliberate: measuring never waits for the math engine.
impl Measure for Renderer {
    fn bounds(&mut self, object: &Object) -> Option<Rect> {
        self.object_bounds(object)
    }
}

/// What a click hits of one object, for [`Renderer::hit_test`] to choose between the objects it
/// hits.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Hit {
    /// A text or a formula: these win over shapes, which often frame them.
    block: bool,
    /// The area the object covers, in square grid units (0 for a line or an arrow): the
    /// smallest wins.
    area: f64,
}

impl Hit {
    fn shape(area: f64) -> Self {
        Self { block: false, area }
    }

    fn block(area: f64) -> Self {
        Self { block: true, area }
    }

    /// True when this hit wins over `other` whatever their board order.
    fn beats(self, other: Self) -> bool {
        if self.block != other.block {
            return self.block;
        }
        self.area.total_cmp(&other.area).is_lt()
    }
}

/// The distance from `p` to the segment from `a` to `b`, in grid units.
fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let length_sq = ab.hypot2();
    let t = if length_sq > 0.0 {
        ((p - a).dot(ab) / length_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + ab * t)).hypot()
}

/// An ellipse whose short radius is under this fraction of its long one is hit as the segment
/// along its long axis. The two differ by less than a millionth of the ellipse's size, far less
/// than a pixel at any zoom that shows the whole ellipse, and the exact distance squares the
/// ratio of the radii, which for a flatter ellipse can overflow.
const FLAT_ELLIPSE_RATIO: f64 = 1e-6;

/// Most halvings [`outside_ellipse_distance`] makes. Bisection on an `f64` interval reaches
/// two neighbouring floats within 1074 halvings (53 bits of mantissa and 1021 of exponent),
/// the bound in Eberly's "Distance from a Point to an Ellipse"; real inputs take about 60.
const MAX_ELLIPSE_HALVINGS: u32 = 1074;

/// True when `at` is inside the ellipse at `center` with radii `rx` and `ry`, or within `reach`
/// of its outline (all in grid units).
fn ellipse_hit(at: Point, center: Point, rx: f64, ry: f64, reach: f64) -> bool {
    // By symmetry, the quarter at or above 0 on both axes, with the long radius along the first.
    let (dx, dy) = ((at.x - center.x).abs(), (at.y - center.y).abs());
    let (long, short, along, across) = if rx.abs() >= ry.abs() {
        (rx.abs(), ry.abs(), dx, dy)
    } else {
        (ry.abs(), rx.abs(), dy, dx)
    };
    if !(long > 0.0 && short >= long * FLAT_ELLIPSE_RATIO) {
        // Flat (or a point): its long axis, from -long to long.
        return (along - long).max(0.0).hypot(across) <= reach;
    }
    let (on_long, on_short) = (along / long, across / short);
    if on_long * on_long + on_short * on_short <= 1.0 {
        return true;
    }
    outside_ellipse_distance(long, short, along, across) <= reach
}

/// The distance from the point (`along`, `across`), outside the ellipse with radius `long`
/// along the first axis and `short` along the second (`long >= short > 0`, both coordinates at
/// or above 0), to the ellipse: Eberly's robust bisection ("Distance from a Point to an
/// Ellipse, an Ellipsoid, or a Hyperellipsoid").
fn outside_ellipse_distance(long: f64, short: f64, along: f64, across: f64) -> f64 {
    if across <= 0.0 {
        return along - long;
    }
    if along <= 0.0 {
        return across - short;
    }
    let (z0, z1) = (along / long, across / short);
    let r0 = (long / short) * (long / short);
    let n0 = r0 * z0;
    // The root of g(t) = (n0 / (t + r0))^2 + (z1 / (t + 1))^2 - 1 lies in [lo, hi].
    let (mut lo, mut hi) = (z1 - 1.0, n0.hypot(z1) - 1.0);
    let mut mid = lo;
    for _ in 0..MAX_ELLIPSE_HALVINGS {
        mid = lo / 2.0 + hi / 2.0;
        if mid <= lo || mid >= hi {
            break;
        }
        let (q0, q1) = (n0 / (mid + r0), z1 / (mid + 1.0));
        let excess = q0 * q0 + q1 * q1 - 1.0;
        if excess > 0.0 {
            lo = mid;
        } else if excess < 0.0 {
            hi = mid;
        } else {
            break;
        }
    }
    let nearest = Point::new(r0 * along / (mid + r0), across / (mid + 1.0));
    (Point::new(along, across) - nearest).hypot()
}

/// Where a text or math block's text starts, and its text (a formula's source).
fn block_text(object: &Object) -> Option<(Point, &str)> {
    match &object.shape {
        Shape::Text { at, text } => Some((*at, text)),
        Shape::Math { at, src } => Some((*at, src)),
        _ => None,
    }
}

/// The outline of `rect` (screen pixels) cut to `clip`: edges outside it moved onto its border,
/// where they aren't seen. `None` when it doesn't reach into `clip`, or holds all of it.
fn box_path(rect: Rect, clip: Rect) -> Option<BezPath> {
    if !(rect.is_finite() && rect.overlaps(clip)) {
        return None;
    }
    let around_clip =
        rect.x0 < clip.x0 && rect.y0 < clip.y0 && rect.x1 > clip.x1 && rect.y1 > clip.y1;
    (!around_clip).then(|| rect.intersect(clip).to_path(draw::TOLERANCE_PX))
}

/// The caret before byte `byte` of `layout`, in layout units, from parley's cursor geometry: a
/// rectangle of no width one line high, or for a `block` caret the box of the grapheme there
/// (from parley's selection geometry), [`EMPTY_BLOCK_EM`] wide where there is none. A layout
/// without lines (an empty text) gives a caret at its top-left corner, `line_height` high.
fn caret_in_layout(layout: &Layout<()>, byte: usize, block: bool, line_height: f32) -> Rect {
    let cursor = parley::Cursor::from_byte_index(layout, byte, Affinity::Downstream);
    let bar = cursor.geometry(layout, 0.0);
    let (x, mut y0, mut y1) = (bar.x0, bar.y0, bar.y1);
    if y1 <= y0 {
        (y0, y1) = (0.0, f64::from(line_height));
    }
    if !block {
        return Rect::new(x, y0, x, y1);
    }
    let mut cell: Option<Rect> = None;
    if let Some(cluster) = Cluster::from_byte_index(layout, byte)
        && !cluster.is_hard_line_break()
    {
        let range = cluster.text_range();
        let selection = Selection::new(
            parley::Cursor::from_byte_index(layout, range.start, Affinity::Downstream),
            parley::Cursor::from_byte_index(layout, range.end, Affinity::Upstream),
        );
        selection.geometry_with(layout, |part, _| {
            let part = Rect::new(part.x0, part.y0, part.x1, part.y1);
            cell = Some(cell.map_or(part, |cell| cell.union(part)));
        });
    }
    match cell.filter(|cell| cell.width() > 0.0) {
        Some(cell) => cell,
        None => Rect::new(x, y0, x + EMPTY_BLOCK_EM * f64::from(LAYOUT_EM), y1),
    }
}

/// Fills the glyphs of `layout` (one line or a few, short), which `transform` maps to screen
/// pixels.
fn fill_layout(
    ctx: &mut RenderContext,
    resources: &mut Resources,
    layout: &Layout<()>,
    transform: Affine,
    color: AlphaColor<Srgb>,
) {
    ctx.set_transform(transform);
    ctx.set_paint(color);
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            let run = glyph_run.run();
            let coords = bytemuck::cast_slice::<_, i16>(run.normalized_coords());
            let drawn = ctx
                .glyph_run(resources, run.font())
                .font_size(run.font_size())
                .hint(false)
                .normalized_coords(coords)
                .fill_glyphs(glyph_run.positioned_glyphs().map(|glyph| Glyph {
                    id: glyph.id,
                    x: glyph.x,
                    y: glyph.y,
                }));
            if let Err(err) = drawn {
                tracing::debug!(?err, "some status bar glyphs failed to draw");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::time::Duration;

    use lawh_core::engine::MAX_COMMAND_LINE_CHARS;
    use lawh_core::from_lawh;

    use super::*;
    use crate::overlay::StatusLine;
    use crate::status::STATUS_WINDOW_CHARS;

    fn id(name: &str) -> ObjectId {
        ObjectId::new(name).unwrap()
    }

    fn area() -> Board {
        from_lawh(include_str!("../../../../examples/area.lawh")).unwrap()
    }

    #[test]
    fn a_formula_being_edited_is_its_source_in_ink_and_not_compiled() {
        let board = area();
        let mut renderer = Renderer::new_trusted();
        let vp = Viewport::new(Point::new(6.0, 0.3), 3.0, 600, 300, 1.0);
        let m1 = id("m1");
        let overlay = Overlay {
            cursor: OverlayCursor::Object(&m1),
            caret: Some(Caret {
                id: &m1,
                byte: 4,
                block: true,
            }),
            ..Overlay::default()
        };
        for _ in 0..2 {
            let image = renderer.render_with(&board, &vp, &overlay);
            // Ink, not the grey of a placeholder.
            let ink = image
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|px| px[0] < 80 && px[1] < 80 && px[2] < 80)
                .count();
            assert!(ink > 100, "{ink}");
            let frame = renderer.last_frame();
            assert_eq!((frame.math_pending, frame.drawn), (0, 1), "{frame:?}");
        }
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(renderer.math.compiles(), 0, "the formula was asked for");
        // The outline goes around the source, as drawn.
        let source = renderer.caret_rect(&board, &m1, 0).unwrap();
        let mut seen = renderer.look_up(&board, &mut Budget::frame());
        mark_edited_for_test(&board, &mut seen, &m1);
        let caret = Caret {
            id: &m1,
            byte: 0,
            block: false,
        };
        let outline = renderer
            .current_bounds(&board, &seen, &m1, Some(caret), &vp)
            .unwrap();
        let start = vp.to_screen(Point::new(source.x0, source.y0));
        assert!((outline.x0 - start.x).abs() < 1e-6, "{outline:?} {start:?}");

        // Once the caret leaves, the formula is asked for.
        renderer.render_with(&board, &vp, &Overlay::default());
        assert_eq!(renderer.last_frame().math_pending, 1);
        renderer.resolve_math(&board);
        renderer.render_with(&board, &vp, &Overlay::default());
        assert_eq!(renderer.last_frame().math_pending, 0);
        assert!(renderer.math.compiles() > 0);
    }

    /// What [`Renderer::draw_frame_with`] does with the caret's object, for tests that look at
    /// one step.
    fn mark_edited_for_test(board: &Board, seen: &mut [Seen], id: &ObjectId) {
        super::super::mark_edited(board, seen, id);
        for (object, seen) in board.objects().iter().zip(seen.iter_mut()) {
            if seen.edited && matches!(object.shape, Shape::Math { .. }) {
                seen.math = Some(lawh_math::MathStatus::Pending);
            }
        }
    }

    #[test]
    fn the_text_with_the_caret_is_laid_out_first() {
        // 500 texts on screen: a frame lays out those nearest the centre while its budget lasts.
        let mut src = String::from("lawh version=1\n");
        for i in 0..500 {
            let (x, y) = (f64::from(i % 25) * 4.0, f64::from(i / 25));
            writeln!(
                src,
                "text id=t{i} x={x} y={y} \"{i} {}\"",
                "lorem ipsum ".repeat(6)
            )
            .unwrap();
        }
        let board = from_lawh(&src).unwrap();
        let last = id("t499");
        let vp = Viewport::new(Point::new(48.0, 10.0), 0.4, 1920, 1080, 1.0);
        let overlay = Overlay {
            caret: Some(Caret {
                id: &last,
                byte: 0,
                block: false,
            }),
            ..Overlay::default()
        };
        let mut plain = Renderer::new_trusted();
        plain.render(&board, &vp);
        let mut editing = Renderer::new_trusted();
        editing.render_with(&board, &vp, &overlay);
        let text = |renderer: &mut Renderer| {
            let Some(Shape::Text { text, .. }) = board.get(&last).map(|object| &object.shape)
            else {
                unreachable!()
            };
            let key = renderer.text.key(text);
            renderer.text.shaped(key, text).is_some()
        };
        assert!(
            plain.last_frame().text_pending > 0,
            "{:?}",
            plain.last_frame()
        );
        assert!(!text(&mut plain), "the far corner was laid out anyway");
        assert!(text(&mut editing));
    }

    #[test]
    fn highlights_stop_at_their_budget() {
        // A full board of rectangles almost as large as the screen, all selected.
        let mut src = String::from("lawh version=1\n");
        for i in 0..500 {
            let inset = f64::from(i % 50) * 0.05;
            writeln!(
                src,
                "rect id=r{i} x={} y={} w={} h={}",
                -20.0 + inset,
                -10.0 + inset,
                40.0 - 2.0 * inset,
                20.0 - 2.0 * inset
            )
            .unwrap();
        }
        let board = from_lawh(&src).unwrap();
        let selection: Vec<ObjectId> = board.objects().iter().map(|o| o.id.clone()).collect();
        let vp = Viewport::new(Point::ZERO, 1.0, 1920, 1080, 1.0);
        let mut renderer = Renderer::new_trusted();
        let overlay = Overlay {
            selection: &selection,
            ..Overlay::default()
        };
        renderer.render_with(&board, &vp, &overlay);
        let work = renderer.work;
        assert!(work.highlighted > 0, "{work:?}");
        assert!(work.not_highlighted > 0, "{work:?}");
        assert_eq!(work.highlighted + work.not_highlighted, 500);

        // An everyday selection is highlighted whole.
        let board = area();
        let selection = [id("c1"), id("l1"), id("t2")];
        let overlay = Overlay {
            selection: &selection,
            ..Overlay::default()
        };
        renderer.render_with(
            &board,
            &Viewport::fit(Rect::new(-3.0, -5.0, 8.0, 3.0), 800, 600, 1.0),
            &overlay,
        );
        assert_eq!(
            (renderer.work.highlighted, renderer.work.not_highlighted),
            (3, 0)
        );
    }

    #[test]
    fn the_distance_to_an_ellipse_matches_a_dense_sampling_of_it() {
        let ellipses: [(f64, f64); 4] = [(4.0, 2.0), (1.0, 1.0), (100.0, 0.5), (3.0, 2.999)];
        let points: [(f64, f64); 7] = [
            (5.0, 0.5),
            (0.2, 3.0),
            (4.5, 4.5),
            (0.0, 7.0),
            (9.0, 0.0),
            (101.0, 0.2),
            (50.0, 1.0),
        ];
        for (long, short) in ellipses {
            // The distance to the point at angle `t`.
            let at = |t: f64, x: f64, y: f64| (long * t.cos() - x).hypot(short * t.sin() - y);
            for (x, y) in points {
                if (x / long).powi(2) + (y / short).powi(2) <= 1.0_f64 {
                    continue;
                }
                // Every 1/20,000th of a turn, then 2,000 steps on each side of the nearest.
                let coarse = std::f64::consts::TAU / 20_000.0;
                let best = (0..20_000_u32)
                    .map(|i| f64::from(i) * coarse)
                    .min_by(|a, b| at(*a, x, y).total_cmp(&at(*b, x, y)))
                    .unwrap();
                let sampled = (0..=4000_u32)
                    .map(|i| at(best + (f64::from(i) - 2000.0) * coarse / 2000.0, x, y))
                    .fold(f64::INFINITY, f64::min);
                let exact = outside_ellipse_distance(long, short, x, y);
                assert!(
                    (exact - sampled).abs() < 1e-6 && exact <= sampled + 1e-9,
                    "{long}x{short} at ({x}, {y}): {exact} vs {sampled}"
                );
            }
        }
        // A flat ellipse is hit as its long axis; a point as itself.
        assert!(ellipse_hit(
            Point::new(5.05, 0.0),
            Point::ZERO,
            5.0,
            1e-9,
            0.1
        ));
        assert!(ellipse_hit(
            Point::new(0.0, 0.09),
            Point::ZERO,
            5.0,
            1e-9,
            0.1
        ));
        assert!(!ellipse_hit(
            Point::new(0.0, 0.11),
            Point::ZERO,
            5.0,
            1e-9,
            0.1
        ));
        assert!(ellipse_hit(
            Point::new(0.06, 0.06),
            Point::ZERO,
            0.0,
            0.0,
            0.1
        ));
        assert!(!ellipse_hit(
            Point::new(f64::NAN, 0.0),
            Point::ZERO,
            1.0,
            1.0,
            0.1
        ));
    }

    #[test]
    fn a_long_command_line_lays_out_only_a_window_of_it() {
        let board = area();
        let vp = Viewport::new(Point::ZERO, 1.0, 800, 600, 1.0);
        let mut renderer = Renderer::new_trusted();
        let line = format!("te {}", "x".repeat(MAX_COMMAND_LINE_CHARS - 3));
        for cursor in [line.len(), 0, line.len() / 2] {
            let overlay = Overlay {
                command_line: Some((&line, cursor)),
                status: StatusLine {
                    left: "",
                    right: &line,
                    error: false,
                },
                ..Overlay::default()
            };
            renderer.render_with(&board, &vp, &overlay);
            for text in renderer.status.texts() {
                assert!(
                    text.chars().count() <= STATUS_WINDOW_CHARS + 3,
                    "{}",
                    text.len()
                );
            }
        }
        // The same line again lays nothing out.
        let built = renderer.status.built;
        let overlay = Overlay {
            command_line: Some((&line, 0)),
            ..Overlay::default()
        };
        renderer.render_with(&board, &vp, &overlay);
        renderer.render_with(&board, &vp, &overlay);
        assert_eq!(renderer.status.built, built);
    }
}
