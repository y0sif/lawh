//! Typst frame → [`MathLayout`]: absolute glyph positions and vector paths in
//! pt, y down, origin at the top-left of the formula box.

use kurbo::{Affine, BezPath, Shape as _};
use typst::foundations::Value;
use typst::introspection::{MetadataElem, Tag};
use typst::layout::{Abs, Frame, FrameItem, Transform};
use typst::text::TextItem;
use typst::visualize::{Color, CurveItem, FixedStroke, Geometry, Paint, Shape};

use crate::fonts::asset_index;
use crate::world::BASELINE_MARKER;
use crate::{MathError, MathGlyph, MathItem, MathLayout};

/// Fallback colour for gradients and tilings, which a layout can't carry.
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// Converts the first page of a compiled formula.
pub(crate) fn convert(page: &Frame) -> Result<MathLayout, MathError> {
    let mut items = Vec::new();
    walk(page, Affine::IDENTITY, &mut items)?;
    Ok(MathLayout {
        width: page.width().to_pt(),
        height: page.height().to_pt(),
        baseline: first_baseline(page, Affine::IDENTITY).unwrap_or_else(|| page.height().to_pt()),
        items,
    })
}

/// The baseline of the first line: where the [`BASELINE_MARKER`] tag sits.
fn first_baseline(frame: &Frame, ts: Affine) -> Option<f64> {
    frame.items().find_map(|(pos, item)| {
        let at = ts * Affine::translate((pos.x.to_pt(), pos.y.to_pt()));
        match item {
            FrameItem::Group(group) => first_baseline(&group.frame, at * affine(group.transform)),
            FrameItem::Tag(Tag::Start(content, _)) => content
                .to_packed::<MetadataElem>()
                .filter(|meta| meta.value == Value::Str(BASELINE_MARKER.into()))
                .map(|_| at.translation().y),
            _ => None,
        }
    })
}

fn walk(frame: &Frame, ts: Affine, out: &mut Vec<MathItem>) -> Result<(), MathError> {
    for (pos, item) in frame.items() {
        let at = ts * Affine::translate((pos.x.to_pt(), pos.y.to_pt()));
        match item {
            FrameItem::Group(group) => walk(&group.frame, at * affine(group.transform), out)?,
            FrameItem::Text(text) => out.push(text_item(text, at)?),
            FrameItem::Shape(shape, _) => out.push(shape_item(shape, at)),
            // Images can only come from inline bytes and aren't drawable
            // here; links and tags carry nothing visible.
            FrameItem::Image(..) | FrameItem::Link(..) | FrameItem::Tag(_) => {}
        }
    }
    Ok(())
}

/// A run of glyphs. Under a plain translation it stays a glyph run; under a
/// scale, rotation or skew the glyphs become outlines, since a glyph run has
/// no transform of its own.
fn text_item(text: &TextItem, at: Affine) -> Result<MathItem, MathError> {
    let size = text.size.to_pt();
    let fill = paint_rgba(&text.fill);
    // Glyph origins relative to the run, y down. Typst's glyph offsets and
    // advances are y-up.
    let mut placed = Vec::with_capacity(text.glyphs.len());
    let (mut x, mut y) = (Abs::zero(), Abs::zero());
    for glyph in &text.glyphs {
        let gx = (x + glyph.x_offset.at(text.size)).to_pt();
        let gy = -(y + glyph.y_offset.at(text.size)).to_pt();
        placed.push((glyph.id, gx, gy));
        x += glyph.x_advance.at(text.size);
        y += glyph.y_advance.at(text.size);
    }

    if is_translation_affine(at) {
        let font = asset_index(text.font.font()).ok_or_else(|| {
            MathError::Compile(format!(
                "internal: glyphs from a font that isn't bundled ({})",
                text.font.font().info().family
            ))
        })?;
        let origin = at.translation();
        let glyphs = placed
            .into_iter()
            .map(|(id, gx, gy)| MathGlyph {
                id: u32::from(id),
                x: origin.x + gx,
                y: origin.y + gy,
            })
            .collect();
        return Ok(MathItem::Glyphs {
            font,
            size,
            fill,
            glyphs,
        });
    }

    let scale = size / text.font.units_per_em();
    let mut path = BezPath::new();
    for (id, gx, gy) in placed {
        let mut outline = Outline(BezPath::new());
        if text
            .font
            .ttf()
            .outline_glyph(ttf_parser::GlyphId(id), &mut outline)
            .is_some()
        {
            let place = at * Affine::translate((gx, gy)) * Affine::scale_non_uniform(scale, -scale);
            path.extend(place * outline.0);
        }
    }
    Ok(MathItem::Path {
        path,
        fill: Some(fill),
        stroke: text.stroke.as_ref().map(|stroke| stroke_of(stroke, at)),
    })
}

fn shape_item(shape: &Shape, at: Affine) -> MathItem {
    let mut path = BezPath::new();
    match &shape.geometry {
        Geometry::Line(to) => {
            path.move_to((0.0, 0.0));
            path.line_to((to.x.to_pt(), to.y.to_pt()));
        }
        Geometry::Rect(size) => {
            path = kurbo::Rect::new(0.0, 0.0, size.x.to_pt(), size.y.to_pt()).to_path(0.0);
        }
        Geometry::Curve(curve) => {
            for item in &curve.0 {
                match item {
                    CurveItem::Move(p) => path.move_to((p.x.to_pt(), p.y.to_pt())),
                    CurveItem::Line(p) => path.line_to((p.x.to_pt(), p.y.to_pt())),
                    CurveItem::Cubic(a, b, p) => path.curve_to(
                        (a.x.to_pt(), a.y.to_pt()),
                        (b.x.to_pt(), b.y.to_pt()),
                        (p.x.to_pt(), p.y.to_pt()),
                    ),
                    CurveItem::Close => path.close_path(),
                }
            }
        }
    }
    path.apply_affine(at);
    MathItem::Path {
        path,
        fill: shape.fill.as_ref().map(paint_rgba),
        stroke: shape.stroke.as_ref().map(|stroke| stroke_of(stroke, at)),
    }
}

/// Stroke width scaled by the transform's area factor, and its colour.
fn stroke_of(stroke: &FixedStroke, at: Affine) -> (f64, [u8; 4]) {
    (
        stroke.thickness.to_pt() * at.determinant().abs().sqrt(),
        paint_rgba(&stroke.paint),
    )
}

fn paint_rgba(paint: &Paint) -> [u8; 4] {
    match paint {
        Paint::Solid(color) => color_rgba(color),
        Paint::Gradient(_) | Paint::Tiling(_) => BLACK,
    }
}

fn color_rgba(color: &Color) -> [u8; 4] {
    let rgb = color.to_rgb();
    [rgb.red, rgb.green, rgb.blue, rgb.alpha].map(unit_to_u8)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=255 before the cast"
)]
fn unit_to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn affine(t: Transform) -> Affine {
    Affine::new([
        t.sx.get(),
        t.ky.get(),
        t.kx.get(),
        t.sy.get(),
        t.tx.to_pt(),
        t.ty.to_pt(),
    ])
}

fn is_translation_affine(at: Affine) -> bool {
    const EPS: f64 = 1e-9;
    let [a, b, c, d, _, _] = at.as_coeffs();
    (a - 1.0).abs() < EPS && b.abs() < EPS && c.abs() < EPS && (d - 1.0).abs() < EPS
}

/// Collects a glyph outline (font units, y-up) as a kurbo path.
struct Outline(BezPath);

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to((f64::from(x), f64::from(y)));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to((f64::from(x), f64::from(y)));
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0
            .quad_to((f64::from(x1), f64::from(y1)), (f64::from(x), f64::from(y)));
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.curve_to(
            (f64::from(x1), f64::from(y1)),
            (f64::from(x2), f64::from(y2)),
            (f64::from(x), f64::from(y)),
        );
    }

    fn close(&mut self) {
        self.0.close_path();
    }
}

#[cfg(test)]
mod tests {
    use typst_layout::PagedDocument;

    use super::*;
    use crate::world::{SandboxWorld, document};

    fn layout_of(world: &SandboxWorld) -> MathLayout {
        let doc = typst::compile::<PagedDocument>(world).output.unwrap();
        convert(&doc.pages()[0].frame).unwrap()
    }

    #[test]
    fn baseline_marker_moves_nothing() {
        let marker = format!("#metadata(\"{BASELINE_MARKER}\")\n");
        for src in [
            "-x",
            "a = -b",
            "x &= 1 \\ y &= -2",
            "lr((a/b))",
            "+ x",
            "f(x)",
            "sum_(i=1)^n i",
        ] {
            let with = layout_of(&SandboxWorld::new(src));
            let plain = document(src).replace(&marker, "");
            let without = layout_of(&SandboxWorld::from_document(plain));
            assert_eq!(with.items, without.items, "{src}");
            assert_eq!(
                (with.width, with.height),
                (without.width, without.height),
                "{src}"
            );
        }
    }

    #[test]
    fn baseline_is_the_first_line() {
        let two_lines = layout_of(&SandboxWorld::new("a \\ b/c"));
        let MathItem::Glyphs { glyphs, .. } = &two_lines.items[0] else {
            panic!("expected the glyph a first");
        };
        assert!((two_lines.baseline - glyphs[0].y).abs() < 1e-9);
    }

    #[test]
    fn rotated_glyphs_become_outlines() {
        let rotated = layout_of(&SandboxWorld::new("#rotate(45deg, $x$)"));
        assert!(matches!(
            rotated.items.as_slice(),
            [MathItem::Path {
                fill: Some(_),
                stroke: None,
                ..
            }]
        ));
    }
}
