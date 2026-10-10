//! Board → pixels with `vello_cpu`.

use std::sync::Arc;

use kurbo::{Affine, BezPath, Cap, Join, Point, Rect, Stroke};
use lawh_core::{Board, Object, Shape};
use lawh_math::{MATH_FONT_SIZE_PT, MathEngine, MathItem, MathLayout};
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

/// Typst's default text colour; math drawn in it is drawn in [`INK`] instead.
const TYPST_BLACK: [u8; 4] = [0, 0, 0, 255];

/// Grid units per Typst pt of math.
const MATH_SCALE: f64 = TEXT_SIZE_UNITS / MATH_FONT_SIZE_PT;

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
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        let math_fonts = lawh_math::fonts()
            .iter()
            .map(|asset| FontData::new(Blob::new(Arc::new(asset.data)), asset.index))
            .collect();
        Self {
            ctx: RenderContext::new(1, 1),
            resources: Resources::new(),
            pixmap: Pixmap::new(1, 1),
            text: TextShaper::new(),
            math: MathEngine::new(),
            math_fonts,
        }
    }

    /// Draws `board` through `vp` and returns the image: premultiplied RGBA8,
    /// row-major, `vp.width * vp.height * 4` bytes.
    ///
    /// Math that is not cached yet is compiled here, which blocks for up to
    /// the math engine's timeout. Math that fails shows its source in red.
    pub fn render(&mut self, board: &Board, vp: &Viewport) -> &[u8] {
        let (width, height) = (vp.width, vp.height);
        if self.pixmap.width() != width || self.pixmap.height() != height {
            self.pixmap.resize(width, height);
        }
        if width == 0 || height == 0 {
            return self.pixmap.data_as_u8_slice();
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
        if vp.is_drawable() {
            if let Some(dots) = draw::grid_dots(vp) {
                self.ctx.set_paint(GRID);
                self.ctx.fill_path(&dots);
            }
            let pad = draw::cull_padding_px(vp);
            let screen = Rect::new(0.0, 0.0, f64::from(width), f64::from(height)).inflate(pad, pad);
            let to_screen = vp.transform();
            for object in board.objects() {
                let on_screen = self
                    .object_bounds(object)
                    .is_some_and(|b| to_screen.transform_rect_bbox(b).overlaps(screen));
                if on_screen {
                    self.draw_object(object, vp);
                }
            }
        } else {
            tracing::debug!(?vp, "viewport not drawable; drawing the background only");
        }
        self.text.end_frame();

        self.ctx.flush();
        self.ctx.render(&mut self.pixmap, &mut self.resources);
        self.pixmap.data_as_u8_slice()
    }

    /// Draws `board` through `vp` and encodes the image as PNG.
    pub fn render_png(&mut self, board: &Board, vp: &Viewport) -> Result<Vec<u8>, RenderError> {
        if vp.width == 0 || vp.height == 0 {
            return Err(RenderError::EmptyImage {
                width: vp.width,
                height: vp.height,
            });
        }
        self.render(board, vp);
        self.pixmap
            .clone()
            .into_png()
            .map_err(|err| RenderError::Png(err.to_string()))
    }

    /// The smallest rectangle, in grid units, holding every object on the
    /// board, text and math extents included; `None` for an empty board.
    ///
    /// Strokes and arrowheads have a fixed pixel size, so they aren't
    /// included; [`Viewport::fit`] leaves a margin for them.
    pub fn board_bounds(&mut self, board: &Board) -> Option<Rect> {
        board
            .objects()
            .iter()
            .filter_map(|object| self.object_bounds(object))
            .reduce(|a, b| a.union(b))
    }

    /// The bounds of one object in grid units, as in [`Self::board_bounds`].
    pub fn object_bounds(&mut self, object: &Object) -> Option<Rect> {
        match &object.shape {
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
            Shape::Math { at, src } => match self.math.layout(src) {
                Ok(layout) => Some(Rect::from_origin_size(
                    *at,
                    (layout.width * MATH_SCALE, layout.height * MATH_SCALE),
                )),
                Err(_) => self.text_bounds(*at, src),
            },
        }
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

    fn draw_object(&mut self, object: &Object, vp: &Viewport) {
        match &object.shape {
            Shape::Ellipse { center, rx, ry } => {
                self.stroke_px(&draw::ellipse(vp, *center, *rx, *ry), vp);
            }
            Shape::Rect { origin, w, h } => self.stroke_px(&draw::rect(vp, *origin, *w, *h), vp),
            Shape::Line { a, b } => self.stroke_px(&draw::line(vp, *a, *b), vp),
            Shape::Arrow { a, b } => {
                let (shaft, head) = draw::arrow(vp, *a, *b);
                self.stroke_px(&shaft, vp);
                if let Some(head) = head {
                    self.ctx.set_paint(INK);
                    self.ctx.fill_path(&head);
                }
            }
            Shape::Text { at, text } => self.draw_text(*at, text, INK, vp),
            Shape::Math { at, src } => match self.math.layout(src) {
                Ok(layout) => self.draw_math(*at, &layout, vp),
                Err(err) => {
                    tracing::debug!(id = %object.id, %err, "drawing math source instead of math");
                    self.draw_text(*at, src, ERROR, vp);
                }
            },
        }
    }

    /// Strokes a path given in screen pixels with the fixed outline width.
    fn stroke_px(&mut self, path: &BezPath, vp: &Viewport) {
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_paint(INK);
        self.ctx.set_stroke(
            Stroke::new(draw::stroke_width_px(vp))
                .with_caps(Cap::Round)
                .with_join(Join::Round),
        );
        self.ctx.stroke_path(path);
    }

    fn draw_text(&mut self, at: Point, text: &str, color: AlphaColor<Srgb>, vp: &Viewport) {
        let Some(layout) = self.text.layout(text) else {
            return;
        };
        self.ctx.set_transform(
            vp.transform() * Affine::translate(at.to_vec2()) * Affine::scale(text_scale()),
        );
        self.ctx.set_paint(color);
        for line in layout.lines() {
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
                    .fill_glyphs(glyph_run.positioned_glyphs().map(|glyph| Glyph {
                        id: glyph.id,
                        x: glyph.x,
                        y: glyph.y,
                    }));
                if let Err(err) = drawn {
                    tracing::debug!(?err, "some text glyphs failed to draw");
                }
            }
        }
    }

    fn draw_math(&mut self, at: Point, layout: &MathLayout, vp: &Viewport) {
        self.ctx.set_transform(
            vp.transform() * Affine::translate(at.to_vec2()) * Affine::scale(MATH_SCALE),
        );
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
                    self.ctx.set_paint(math_color(*fill));
                    let drawn = self
                        .ctx
                        .glyph_run(&mut self.resources, font_data)
                        .font_size(to_f32(*size))
                        .hint(false)
                        .fill_glyphs(glyphs.iter().map(|glyph| Glyph {
                            id: glyph.id,
                            x: to_f32(glyph.x),
                            y: to_f32(glyph.y),
                        }));
                    if let Err(err) = drawn {
                        tracing::debug!(?err, "some math glyphs failed to draw");
                    }
                }
                MathItem::Path { path, fill, stroke } => {
                    if let Some(fill) = fill {
                        self.ctx.set_paint(math_color(*fill));
                        self.ctx.fill_path(path);
                    }
                    if let Some((width, color)) = stroke {
                        self.ctx.set_paint(math_color(*color));
                        self.ctx.set_stroke(Stroke::new(*width));
                        self.ctx.stroke_path(path);
                    }
                }
            }
        }
    }
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
