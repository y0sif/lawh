//! The shared draw list drawn with `vello_cpu`: the window's sink ([`Screen`]), and how every
//! item is handed to `vello_cpu` ([`Vello`]), which the PNG export's sink shares, so the window
//! and PNGs stroke, fill and set glyphs the same way.

use std::cell::Cell;
use std::convert::Infallible;

use kurbo::{Affine, BezPath, Cap, Join, Point, Rect, Stroke};
use lawh_core::ObjectId;
use vello_cpu::color::{AlphaColor, Srgb};
use vello_cpu::peniko::FontData;
use vello_cpu::{Glyph, RenderContext, Resources};

use super::{Work, glyph_area, math_run_area, path_area, stroke_px, to_f32};
use crate::draw;
use crate::scene::{Glyphs, Item, Paint, Run, Sink, Tally, Tools, uniform_scale};

/// A `vello_cpu` context and its resources: strokes, fills and glyph runs in pixels.
pub(crate) struct Vello<'c> {
    pub(crate) ctx: &'c mut RenderContext,
    pub(crate) resources: &'c mut Resources,
}

impl Vello<'_> {
    /// Strokes `path`, given in pixels, `width` pixels wide with round caps and joins, as every
    /// outline and math stroke is drawn.
    pub(crate) fn stroke(&mut self, path: &BezPath, width: f64, color: AlphaColor<Srgb>) {
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_paint(color);
        self.ctx.set_stroke(
            Stroke::new(width)
                .with_caps(Cap::Round)
                .with_join(Join::Round),
        );
        self.ctx.stroke_path(path);
    }

    /// Fills `path`, given in pixels.
    pub(crate) fn fill(&mut self, path: &BezPath, color: AlphaColor<Srgb>) {
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_paint(color);
        self.ctx.fill_path(path);
    }

    /// Sets what glyph runs are drawn with: `transform`, from a run's units to pixels, and
    /// `color`.
    pub(crate) fn glyph_state(&mut self, transform: Affine, color: AlphaColor<Srgb>) {
        self.ctx.set_transform(transform);
        self.ctx.set_paint(color);
    }

    /// Fills the glyphs of `run` that `keep` keeps, by their origin in the run's units, as
    /// unhinted outlines of the run's font at its size, through the transform and in the colour
    /// [`Self::glyph_state`] set. A text run's glyphs are set from its own parley font; a math
    /// run's font is looked up in `fonts` (`lawh_math::fonts`), and a run of a font that isn't
    /// there draws nothing. Returns how many glyphs it walked and how many it kept.
    pub(crate) fn glyphs(
        &mut self,
        fonts: &[FontData],
        run: &Glyphs<'_>,
        keep: impl Fn(Point) -> bool,
    ) -> (u32, u32) {
        // Counted as `vello_cpu` takes them: it goes over the glyphs once.
        let (walked, kept) = (Cell::new(0_u32), Cell::new(0_u32));
        let kept_at = |at: Point| {
            walked.set(walked.get() + 1);
            let shown = keep(at);
            if shown {
                kept.set(kept.get() + 1);
            }
            shown
        };
        let drawn = match &run.run {
            Run::Text { glyphs, .. } => {
                let font = glyphs.run();
                let coords = bytemuck::cast_slice::<_, i16>(font.normalized_coords());
                self.ctx
                    .glyph_run(self.resources, font.font())
                    .font_size(font.font_size())
                    .hint(false)
                    .normalized_coords(coords)
                    .fill_glyphs(
                        glyphs
                            .positioned_glyphs()
                            .filter(|glyph| {
                                kept_at(Point::new(f64::from(glyph.x), f64::from(glyph.y)))
                            })
                            .map(|glyph| Glyph {
                                id: glyph.id,
                                x: glyph.x,
                                y: glyph.y,
                            }),
                    )
            }
            Run::Math { font, glyphs } => {
                let Some(font) = fonts.get(*font) else {
                    tracing::warn!(font, "math glyphs reference an unknown font");
                    return (0, 0);
                };
                self.ctx
                    .glyph_run(self.resources, font)
                    .font_size(to_f32(run.size))
                    .hint(false)
                    .fill_glyphs(
                        glyphs
                            .iter()
                            .filter(|glyph| kept_at(Point::new(glyph.x, glyph.y)))
                            .map(|glyph| Glyph {
                                id: glyph.id,
                                x: to_f32(glyph.x),
                                y: to_f32(glyph.y),
                            }),
                    )
            }
        };
        if let Err(err) = drawn {
            tracing::debug!(?err, "some glyphs failed to draw");
        }
        (walked.get(), kept.get())
    }
}

/// The window's sink: items built in screen pixels (`crate::scene::Space::Screen`), drawn into
/// the renderer's own context, culled and cut to the frame's clip (the image grown by the cull
/// padding) so huge shapes and formulas cost what their visible part does. The frame has already
/// estimated each object and charged it to its budget; this only draws, and counts what it drew
/// in the frame's [`Work`].
///
/// - Shape outlines and arrowheads are cut to the clip (`draw::cut_item`). The frame cuts them
///   once, for its estimate, and hands the cut paths over with [`Screen::shape`].
/// - Of a text's glyph runs (the frame hands over only those that can reach the image), only
///   glyphs whose origin is near the image are drawn: the text's [`Screen::text_near`], as its
///   estimate counted them. A math glyph run finds its own area from its size on screen.
/// - Math paths are cut to the clip grown by their stroke (`draw::cut_path`).
pub(crate) struct Screen<'f> {
    vello: Vello<'f>,
    pub(crate) work: &'f mut Work,
    clip: Rect,
    /// For a text: where its glyphs' origins must be for them to be drawn
    /// (`TextView::near`). `None` works it out from each run's size on screen.
    text_near: Option<Rect>,
    /// What it is handed is copied here too, for tests (`Renderer::record_frame`).
    tally: Option<&'f mut Tally>,
    /// The transform and paint it last set for glyph runs ([`Vello::glyph_state`]), while
    /// nothing else has set the context's since: an object's runs share them, and setting them
    /// once is enough.
    glyph_state: Option<(Affine, Paint)>,
}

impl<'f> Screen<'f> {
    pub(crate) fn new(
        vello: Vello<'f>,
        work: &'f mut Work,
        clip: Rect,
        text_near: Option<Rect>,
        tally: Option<&'f mut Tally>,
    ) -> Self {
        Self {
            vello,
            work,
            clip,
            text_near,
            tally,
            glyph_state: None,
        }
    }

    /// Strokes or fills a shape's `item` along `cut`, the part of it inside the clip
    /// (`draw::cut_item`), which the frame's estimate counted.
    pub(crate) fn shape(&mut self, tools: &mut Tools<'_>, item: &Item<'_>, cut: &BezPath) {
        self.tallied(tools, item);
        self.glyph_state = None;
        match item {
            Item::Stroke { width, paint, .. } => {
                self.vello.stroke(cut, *width, tools.palette.color(*paint));
            }
            Item::Fill { paint, .. } => self.vello.fill(cut, tools.palette.color(*paint)),
            Item::Glyphs(_) | Item::Path { .. } => {}
        }
    }

    fn tallied(&mut self, tools: &mut Tools<'_>, item: &Item<'_>) {
        if let Some(tally) = self.tally.as_deref_mut() {
            let Ok(()) = tally.item(tools, item);
        }
    }

    /// Draws a glyph run's glyphs near the image.
    fn glyphs(&mut self, tools: &Tools<'_>, run: &Glyphs<'_>) {
        let transform = run.transform;
        let near = match &run.run {
            Run::Text { .. } => {
                let near = self
                    .text_near
                    .or_else(|| glyph_area(run.size * uniform_scale(transform), self.clip));
                let Some(near) = near else {
                    return;
                };
                near
            }
            Run::Math { font, .. } => {
                let em_px = (run.size * uniform_scale(transform)).abs();
                let Some(near) = math_run_area(tools.fonts, *font, em_px, self.clip) else {
                    return;
                };
                near
            }
        };
        let state = Some((transform, run.paint));
        if self.glyph_state != state {
            self.vello
                .glyph_state(transform, tools.palette.color(run.paint));
            self.glyph_state = state;
        }
        let (walked, kept) = self
            .vello
            .glyphs(tools.fonts, run, |at| near.contains(transform * at));
        if matches!(run.run, Run::Text { .. }) {
            self.work.text.runs += 1.0;
            self.work.text.walked += f64::from(walked);
            self.work.text.glyphs += f64::from(kept);
        } else {
            self.work.math_runs += 1;
            self.work.math_glyphs += kept as usize;
        }
    }

    /// Draws a formula's path, cut to the clip grown by its stroke; a stroke too wide to draw
    /// is left out (`stroke_px`).
    fn path(
        &mut self,
        tools: &Tools<'_>,
        path: &BezPath,
        transform: Affine,
        fill: Option<Paint>,
        stroke: Option<(f64, Paint)>,
    ) {
        let stroke = stroke_px(stroke, uniform_scale(transform));
        let Some(near) = path_area(path, transform, stroke, self.clip) else {
            return;
        };
        self.glyph_state = None;
        let mut screen = path.clone();
        screen.apply_affine(transform);
        if let Some(fill) = fill {
            self.vello.fill(
                &draw::cut_path(&screen, near, true),
                tools.palette.color(fill),
            );
        }
        if let Some((width, paint)) = stroke {
            self.vello.stroke(
                &draw::cut_path(&screen, near, false),
                width,
                tools.palette.color(paint),
            );
        }
    }
}

impl Sink for Screen<'_> {
    type Error = Infallible;

    fn begin(
        &mut self,
        tools: &mut Tools<'_>,
        id: &ObjectId,
        bounds: Rect,
    ) -> Result<(), Infallible> {
        if let Some(tally) = self.tally.as_deref_mut() {
            tally.begin(tools, id, bounds)?;
        }
        Ok(())
    }

    fn item(&mut self, tools: &mut Tools<'_>, item: &Item<'_>) -> Result<(), Infallible> {
        match item {
            Item::Stroke { .. } | Item::Fill { .. } => {
                if let Some(cut) = draw::cut_item(item, self.clip) {
                    self.shape(tools, item, &cut);
                }
            }
            Item::Glyphs(run) => {
                self.tallied(tools, item);
                self.glyphs(tools, run);
            }
            Item::Path {
                path,
                transform,
                fill,
                stroke,
            } => {
                self.tallied(tools, item);
                self.path(tools, path, *transform, *fill, *stroke);
            }
        }
        Ok(())
    }
}
