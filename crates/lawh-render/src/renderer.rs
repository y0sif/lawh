//! Board → pixels with `vello_cpu`.

use std::sync::Arc;

use kurbo::{Affine, BezPath, Cap, Join, Point, Rect, Shape as _, Stroke};
use lawh_core::{Board, Object, Shape};
use lawh_math::{MATH_FONT_SIZE_PT, MathEngine, MathItem, MathLayout, MathStatus};
use parley::PositionedLayoutItem;
use vello_cpu::color::{AlphaColor, Srgb};
use vello_cpu::peniko::{Blob, FontData};
use vello_cpu::{Glyph, Pixmap, RenderContext, Resources};

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

/// Draws boards into a pixel buffer it owns, at any pan and zoom.
///
/// Keep one per window: it caches text layouts, compiled math and glyph
/// outlines between frames.
pub struct Renderer {
    ctx: RenderContext,
    resources: Resources,
    pixmap: Pixmap,
    text: TextShaper,
    math: MathEngine,
    /// `vello_cpu` handles for [`lawh_math::fonts`], by the same index.
    math_fonts: Vec<FontData>,
    /// Math blocks the last frame drew as placeholders.
    math_pending: usize,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    /// A renderer that compiles math in this process ([`MathEngine::new`]).
    /// For trusted boards only, such as tests and goldens: a formula that
    /// overflows the stack takes this process down. Draw boards from files
    /// with [`Renderer::with_math`] and a worker engine.
    pub fn new() -> Self {
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
            math_pending: 0,
        }
    }

    /// Draws `board` through `vp` and returns the image: premultiplied RGBA8,
    /// row-major, `vp.width * vp.height * 4` bytes.
    ///
    /// Never waits for math. A formula not compiled yet is queued and its
    /// source drawn in grey as a placeholder; the math engine's waker says
    /// when to draw again. A formula that failed shows its source in red.
    pub fn render(&mut self, board: &Board, vp: &Viewport) -> &[u8] {
        self.draw_frame(board, vp, MathWait::No)
    }

    fn draw_frame(&mut self, board: &Board, vp: &Viewport, wait: MathWait) -> &[u8] {
        if !self.begin_frame(vp) {
            return self.pixmap.data_as_u8_slice();
        }
        let mut pending = 0;
        if vp.is_drawable() {
            if let Some(dots) = draw::grid_dots(vp) {
                self.ctx.set_paint(GRID);
                self.ctx.fill_path(&dots);
            }
            let clip = draw::clip_rect(vp);
            let to_screen = vp.transform();
            for object in board.objects() {
                // Asked once per object, so bounds and drawing agree.
                let math = self.math_status(object, wait);
                if matches!(math, Some(MathStatus::Pending)) {
                    pending += 1;
                }
                let on_screen = self
                    .bounds_with(object, math.as_ref())
                    .map(|bounds| to_screen.transform_rect_bbox(bounds))
                    .is_some_and(|bounds| bounds.is_finite() && bounds.overlaps(clip));
                if on_screen {
                    self.draw_object(object, math, vp, clip);
                }
            }
        } else {
            tracing::debug!(?vp, "viewport not drawable; drawing the background only");
        }
        if pending > 0 {
            tracing::debug!(pending, "math still compiling; drawn as placeholders");
        }
        self.math_pending = pending;
        self.text.end_frame();
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
        if self.ctx.width() == width && self.ctx.height() == height {
            self.ctx.reset();
        } else {
            self.ctx.reset_and_resize(width, height);
        }
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_paint(PAPER);
        self.ctx
            .fill_rect(&Rect::new(0.0, 0.0, f64::from(width), f64::from(height)));
        true
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
    /// time shows its source in red.
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
    pub fn resolve_math(&self, board: &Board) {
        for object in board.objects() {
            if let Shape::Math { src, .. } = &object.shape
                && let Err(err) = self.math.layout_blocking(src)
            {
                tracing::debug!(id = %object.id, %err, "math did not lay out");
            }
        }
    }

    /// How many math blocks the last [`Self::render`] drew as placeholders
    /// because they were still compiling.
    pub fn math_pending(&self) -> usize {
        self.math_pending
    }

    /// The smallest rectangle, in grid units, holding every object on the
    /// board, text and math extents included; `None` for an empty board.
    ///
    /// Never waits for math: a formula still compiling counts with the
    /// extents of its source text. Call [`Self::resolve_math`] first for
    /// final bounds. Strokes and arrowheads have a fixed pixel size, so they
    /// aren't included; [`Viewport::fit`] leaves a margin for them.
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
        let math = self.math_status(object, MathWait::No);
        self.bounds_with(object, math.as_ref())
    }

    /// Where a math object's formula stands; `None` for other objects.
    fn math_status(&self, object: &Object, wait: MathWait) -> Option<MathStatus> {
        let Shape::Math { src, .. } = &object.shape else {
            return None;
        };
        Some(match wait {
            MathWait::No => self.math.request(src),
            MathWait::Yes => match self.math.layout_blocking(src) {
                Ok(layout) => MathStatus::Ready(layout),
                Err(err) => MathStatus::Failed(err),
            },
        })
    }

    /// An object's bounds, given its math status from [`Self::math_status`].
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
            Shape::Math { at, src } => match math {
                Some(MathStatus::Ready(layout)) => Some(Rect::from_origin_size(
                    *at,
                    (layout.width * MATH_SCALE, layout.height * MATH_SCALE),
                )),
                // Still compiling or failed: the source is drawn as text.
                _ => self.text_bounds(*at, src),
            },
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

    /// Draws one object whose bounds reach into `clip` (screen pixels).
    fn draw_object(
        &mut self,
        object: &Object,
        math: Option<MathStatus>,
        vp: &Viewport,
        clip: Rect,
    ) {
        match &object.shape {
            Shape::Ellipse { center, rx, ry } => {
                self.stroke_px(draw::ellipse(vp, *center, *rx, *ry, clip), vp);
            }
            Shape::Rect { origin, w, h } => {
                self.stroke_px(draw::rect(vp, *origin, *w, *h, clip), vp);
            }
            Shape::Line { a, b } => self.stroke_px(draw::line(vp, *a, *b, clip), vp),
            Shape::Arrow { a, b } => {
                let (shaft, head) = draw::arrow(vp, *a, *b, clip);
                self.stroke_px(shaft, vp);
                if let Some(head) = head {
                    self.ctx.set_transform(Affine::IDENTITY);
                    self.ctx.set_paint(INK);
                    self.ctx.fill_path(&head);
                }
            }
            Shape::Text { at, text } => self.draw_text(*at, text, INK, vp, clip),
            Shape::Math { at, src } => match math {
                Some(MathStatus::Ready(layout)) => self.draw_math(*at, &layout, vp, clip),
                Some(MathStatus::Failed(err)) => {
                    tracing::debug!(id = %object.id, %err, "drawing math source instead of math");
                    self.draw_text(*at, src, ERROR, vp, clip);
                }
                Some(MathStatus::Pending) | None => self.draw_text(*at, src, PENDING, vp, clip),
            },
        }
    }

    /// Strokes a path given in screen pixels with the fixed outline width.
    fn stroke_px(&mut self, path: Option<BezPath>, vp: &Viewport) {
        let Some(path) = path else {
            return;
        };
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_paint(INK);
        self.ctx.set_stroke(
            Stroke::new(draw::stroke_width_px(vp))
                .with_caps(Cap::Round)
                .with_join(Join::Round),
        );
        self.ctx.stroke_path(&path);
    }

    /// Draws text with its top-left corner at `at`, skipping glyphs too far
    /// outside `clip` to reach into it.
    fn draw_text(
        &mut self,
        at: Point,
        text: &str,
        color: AlphaColor<Srgb>,
        vp: &Viewport,
        clip: Rect,
    ) {
        let Some(layout) = self.text.layout(text) else {
            return;
        };
        let transform =
            vp.transform() * Affine::translate(at.to_vec2()) * Affine::scale(text_scale());
        let Some(near) = glyph_area(f64::from(LAYOUT_EM) * text_scale(), vp, clip) else {
            return;
        };
        self.ctx.set_transform(transform);
        self.ctx.set_paint(color);
        for line in layout.lines() {
            // Text is only scaled and moved, so a line's screen height depends on its baseline.
            let baseline = transform * Point::new(0.0, f64::from(line.metrics().baseline));
            if !(near.y0..=near.y1).contains(&baseline.y) {
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
    }

    /// Draws a formula with its top-left corner at `at`, skipping glyphs and
    /// paths that can't reach into `clip` and cutting paths down to it.
    fn draw_math(&mut self, at: Point, layout: &MathLayout, vp: &Viewport, clip: Rect) {
        let transform =
            vp.transform() * Affine::translate(at.to_vec2()) * Affine::scale(MATH_SCALE);
        let px_per_pt = vp.px_per_unit() * MATH_SCALE;
        for item in &layout.items {
            match item {
                MathItem::Glyphs {
                    font,
                    size,
                    fill,
                    glyphs,
                } => {
                    let Some(font_data) = self.math_fonts.get(*font) else {
                        tracing::warn!(font, "math glyphs reference an unknown font");
                        continue;
                    };
                    let Some(near) = glyph_area(size * MATH_SCALE, vp, clip) else {
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
                    let stroke = stroke.and_then(|(width, color)| {
                        let width = width.abs() * px_per_pt;
                        if width.is_finite() && width <= MAX_STROKE_PX {
                            Some((width, color))
                        } else {
                            tracing::debug!(width, "math stroke too wide to draw");
                            None
                        }
                    });
                    // Grown by a whole stroke width, so a stroke's joins and caps
                    // are inside too.
                    let reach = stroke.map_or(0.0, |(width, _)| width);
                    let near = clip.inflate(reach, reach);
                    let mut screen = path.clone();
                    screen.apply_affine(transform);
                    let bounds = screen.bounding_box();
                    if !(bounds.is_finite() && bounds.overlaps(near)) {
                        continue;
                    }
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
    }
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
mod tests {
    use std::time::{Duration, Instant};

    use kurbo::{BezPath, Circle};
    use lawh_math::{MAX_LAYOUT_EXTENT_PT, MathItem, MathLayout};

    use super::*;

    const BLACK: [u8; 4] = [0, 0, 0, 255];
    const MAX: f64 = MAX_LAYOUT_EXTENT_PT;

    /// Draws only `layout`, its top-left corner at the board origin, as a frame
    /// draws a math object, and returns the image and how long it took.
    fn render_layout(layout: &MathLayout, vp: &Viewport) -> (Vec<u8>, Duration) {
        let mut renderer = Renderer::new();
        let started = Instant::now();
        assert!(renderer.begin_frame(vp));
        renderer.draw_math(Point::ZERO, layout, vp, draw::clip_rect(vp));
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
