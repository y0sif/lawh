//! Typst frame → [`MathLayout`]: absolute glyph positions and vector paths in
//! pt, y down, origin at the top-left of the formula box, and the checks every
//! layout passes before it reaches the renderer.

use std::collections::HashMap;

use kurbo::{Affine, BezPath, ParamCurveExtrema, PathEl, Point, Rect, Shape as _};
use typst::foundations::Value;
use typst::introspection::{MetadataElem, Tag};
use typst::layout::{Abs, Frame, FrameItem, Transform};
use typst::text::TextItem;
use typst::visualize::{Color, CurveItem, FixedStroke, Geometry, Paint, Shape};

use crate::fonts::{asset_index, glyph_box};
use crate::world::BASELINE_MARKER;
use crate::{
    MAX_LAYOUT_EXTENT_PT, MAX_LAYOUT_GLYPHS, MAX_LAYOUT_PATH_ELEMENTS, MathError, MathGlyph,
    MathItem, MathLayout, MathSummary,
};

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
        ink: ink(&items),
        items,
    })
}

/// [`MathError::TooLarge`] unless `layout` is within every limit: each
/// number within [`MAX_LAYOUT_EXTENT_PT`] of zero (which also rules out NaN
/// and infinity), the ink bounds included, at most [`MAX_LAYOUT_GLYPHS`]
/// glyphs and at most [`MAX_LAYOUT_PATH_ELEMENTS`] path elements, counted as
/// the constants say.
///
/// Both [`crate::compile::compile`] (in process and in the worker) and the
/// parent's decoding of a worker's reply run this, so no layout reaches the
/// renderer without it.
pub(crate) fn check_limits(layout: &MathLayout) -> Result<(), MathError> {
    let counts = Counts::of(layout);
    if counts.glyphs > MAX_LAYOUT_GLYPHS || counts.weighted_elements > MAX_LAYOUT_PATH_ELEMENTS {
        return Err(MathError::TooLarge);
    }
    check_extent(layout)
}

/// What [`check_limits`] counts, and what [`approx_bytes`] needs.
struct Counts {
    /// Glyphs, an empty run counting as one.
    glyphs: usize,
    /// Path elements as [`MAX_LAYOUT_PATH_ELEMENTS`] counts them.
    weighted_elements: usize,
    /// Path elements, each once.
    elements: usize,
}

impl Counts {
    fn of(layout: &MathLayout) -> Self {
        let mut counts = Self {
            glyphs: 0,
            weighted_elements: 0,
            elements: 0,
        };
        for item in &layout.items {
            match item {
                MathItem::Glyphs { glyphs, .. } => {
                    counts.glyphs = counts.glyphs.saturating_add(glyphs.len().max(1));
                }
                MathItem::Path { path, stroke, .. } => {
                    let elements = path.elements().len();
                    let weight = if stroke.is_some() { 2 } else { 1 };
                    counts.elements = counts.elements.saturating_add(elements);
                    counts.weighted_elements = counts
                        .weighted_elements
                        .saturating_add(elements.max(1).saturating_mul(weight));
                }
            }
        }
        counts
    }
}

/// `layout` without its items.
pub(crate) fn summary(layout: &MathLayout) -> MathSummary {
    let counts = Counts::of(layout);
    MathSummary {
        width: layout.width,
        height: layout.height,
        baseline: layout.baseline,
        ink: layout.ink,
        glyphs: counts.glyphs,
        path_elements: counts.weighted_elements,
    }
}

/// About how many bytes `layout` takes in memory, for the engine's cache.
pub(crate) fn approx_bytes(layout: &MathLayout) -> usize {
    let counts = Counts::of(layout);
    size_of::<MathLayout>()
        .saturating_add(layout.items.len().saturating_mul(size_of::<MathItem>()))
        .saturating_add(counts.glyphs.saturating_mul(size_of::<MathGlyph>()))
        .saturating_add(counts.elements.saturating_mul(size_of::<PathEl>()))
}

/// The union of what every item draws, or [`Rect::ZERO`] if nothing is.
fn ink(items: &[MathItem]) -> Rect {
    // The math font is CFF, where a glyph's bounds mean outlining it, so each
    // glyph is looked up once.
    let mut boxes: HashMap<(usize, u32), Option<Rect>> = HashMap::new();
    let mut ink: Option<Rect> = None;
    let mut add = |rect: Rect| {
        let rect = rect.abs();
        ink = Some(ink.map_or(rect, |ink| ink.union(rect)));
    };
    for item in items {
        match item {
            MathItem::Glyphs {
                font, size, glyphs, ..
            } => {
                for glyph in glyphs {
                    let em = *boxes
                        .entry((*font, glyph.id))
                        .or_insert_with(|| glyph_box(*font, glyph.id));
                    // Font units are y up; the layout is y down.
                    if let Some(em) = em {
                        add(Rect::new(
                            glyph.x + em.x0 * size,
                            glyph.y - em.y1 * size,
                            glyph.x + em.x1 * size,
                            glyph.y - em.y0 * size,
                        ));
                    }
                }
            }
            MathItem::Path { path, fill, stroke } => {
                if fill.is_none() && stroke.is_none() {
                    continue;
                }
                let half = stroke.map_or(0.0, |(width, _)| width.abs() / 2.0);
                let bounds = path
                    .segments()
                    .map(|seg| ParamCurveExtrema::bounding_box(&seg));
                if let Some(bounds) = bounds.reduce(|a, b| a.union(b)) {
                    add(bounds.inflate(half, half));
                }
            }
        }
    }
    ink.unwrap_or(Rect::ZERO)
}

/// [`MathError::TooLarge`] unless every number in `layout` is within
/// [`MAX_LAYOUT_EXTENT_PT`] of zero.
fn check_extent(layout: &MathLayout) -> Result<(), MathError> {
    let ink = layout.ink;
    let box_fits = [
        layout.width,
        layout.height,
        layout.baseline,
        ink.x0,
        ink.y0,
        ink.x1,
        ink.y1,
    ]
    .into_iter()
    .all(fits);
    let items_fit = layout.items.iter().all(|item| match item {
        MathItem::Glyphs { size, glyphs, .. } => {
            fits(*size) && glyphs.iter().all(|glyph| fits(glyph.x) && fits(glyph.y))
        }
        MathItem::Path { path, stroke, .. } => {
            stroke.is_none_or(|(width, _)| fits(width))
                && path.elements().iter().all(|el| match *el {
                    PathEl::MoveTo(p) | PathEl::LineTo(p) => point_fits(p),
                    PathEl::QuadTo(a, p) => point_fits(a) && point_fits(p),
                    PathEl::CurveTo(a, b, p) => point_fits(a) && point_fits(b) && point_fits(p),
                    PathEl::ClosePath => true,
                })
        }
    });
    if box_fits && items_fit {
        Ok(())
    } else {
        Err(MathError::TooLarge)
    }
}

/// False for NaN too.
fn fits(v: f64) -> bool {
    v.abs() <= MAX_LAYOUT_EXTENT_PT
}

fn point_fits(p: Point) -> bool {
    fits(p.x) && fits(p.y)
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
            path = Rect::new(0.0, 0.0, size.x.to_pt(), size.y.to_pt()).to_path(0.0);
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

    fn small() -> MathLayout {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.curve_to((1.0, 2.0), (3.0, 4.0), (5.0, 6.0));
        MathLayout {
            width: 10.0,
            height: 10.0,
            baseline: 8.0,
            ink: Rect::new(-0.5, -0.5, 10.0, 10.0),
            items: vec![
                MathItem::Glyphs {
                    font: 0,
                    size: 20.0,
                    fill: BLACK,
                    glyphs: vec![MathGlyph {
                        id: 1,
                        x: 0.0,
                        y: 8.0,
                    }],
                },
                MathItem::Path {
                    path,
                    fill: None,
                    stroke: Some((1.0, BLACK)),
                },
            ],
        }
    }

    #[test]
    fn every_number_counts_for_the_extent() {
        type Edit = fn(&mut MathLayout, f64);
        assert_eq!(check_limits(&small()), Ok(()));
        let at_cap = MAX_LAYOUT_EXTENT_PT;
        let past_cap = MAX_LAYOUT_EXTENT_PT.next_up();
        let edits: [(&str, Edit); 11] = [
            ("width", |l, v| l.width = v),
            ("height", |l, v| l.height = -v),
            ("baseline", |l, v| l.baseline = v),
            ("ink left", |l, v| l.ink.x0 = -v),
            ("ink bottom", |l, v| l.ink.y1 = v),
            ("glyph size", |l, v| {
                if let MathItem::Glyphs { size, .. } = &mut l.items[0] {
                    *size = v;
                }
            }),
            ("glyph x", |l, v| {
                if let MathItem::Glyphs { glyphs, .. } = &mut l.items[0] {
                    glyphs[0].x = -v;
                }
            }),
            ("glyph y", |l, v| {
                if let MathItem::Glyphs { glyphs, .. } = &mut l.items[0] {
                    glyphs[0].y = v;
                }
            }),
            ("stroke width", |l, v| {
                if let MathItem::Path { stroke, .. } = &mut l.items[1] {
                    *stroke = Some((v, BLACK));
                }
            }),
            ("control point", |l, v| {
                if let MathItem::Path { path, .. } = &mut l.items[1] {
                    let mut els = path.elements().to_vec();
                    els[1] =
                        PathEl::CurveTo((1.0, -v).into(), (3.0, 4.0).into(), (5.0, 6.0).into());
                    *path = BezPath::from_vec(els);
                }
            }),
            ("end point", |l, v| {
                if let MathItem::Path { path, .. } = &mut l.items[1] {
                    path.line_to((v, 0.0));
                }
            }),
        ];
        for (name, edit) in edits {
            let mut layout = small();
            edit(&mut layout, at_cap);
            assert_eq!(check_limits(&layout), Ok(()), "{name} at the cap");
            for bad in [past_cap, f64::NAN, f64::INFINITY] {
                let mut layout = small();
                edit(&mut layout, bad);
                assert_eq!(
                    check_limits(&layout),
                    Err(MathError::TooLarge),
                    "{name} = {bad}"
                );
            }
        }
    }

    fn glyphs(n: usize) -> MathItem {
        MathItem::Glyphs {
            font: 1,
            size: 20.0,
            fill: BLACK,
            glyphs: vec![
                MathGlyph {
                    id: 1,
                    x: 0.0,
                    y: 0.0
                };
                n
            ],
        }
    }

    fn polyline(elements: usize, stroked: bool) -> MathItem {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        for i in 1..elements {
            path.line_to((f64::from(u32::try_from(i % 2).unwrap()), 1.0));
        }
        MathItem::Path {
            path,
            fill: (!stroked).then_some(BLACK),
            stroke: stroked.then_some((1.0, BLACK)),
        }
    }

    fn with_items(items: Vec<MathItem>) -> MathLayout {
        MathLayout { items, ..small() }
    }

    #[test]
    fn glyphs_count_across_runs_up_to_the_cap() {
        let half = MAX_LAYOUT_GLYPHS / 2;
        let at_cap = with_items(vec![glyphs(half), glyphs(MAX_LAYOUT_GLYPHS - half)]);
        assert_eq!(check_limits(&at_cap), Ok(()));
        let over = with_items(vec![glyphs(half), glyphs(MAX_LAYOUT_GLYPHS - half + 1)]);
        assert_eq!(check_limits(&over), Err(MathError::TooLarge));
        // Empty runs count as one glyph each.
        let mut empty_runs = vec![glyphs(0); 2];
        empty_runs.push(glyphs(MAX_LAYOUT_GLYPHS - 1));
        assert_eq!(
            check_limits(&with_items(empty_runs)),
            Err(MathError::TooLarge)
        );
    }

    #[test]
    fn stroked_path_elements_count_twice() {
        let max = MAX_LAYOUT_PATH_ELEMENTS;
        let filled = with_items(vec![polyline(max, false)]);
        assert_eq!(check_limits(&filled), Ok(()));
        let filled_over = with_items(vec![polyline(max - 10, false), polyline(11, false)]);
        assert_eq!(check_limits(&filled_over), Err(MathError::TooLarge));
        let stroked = with_items(vec![polyline(max / 2, true)]);
        assert_eq!(check_limits(&stroked), Ok(()));
        let stroked_over = with_items(vec![polyline(max / 2 + 1, true)]);
        assert_eq!(check_limits(&stroked_over), Err(MathError::TooLarge));
        // An empty path counts as one element.
        let empty = MathItem::Path {
            path: BezPath::new(),
            fill: None,
            stroke: None,
        };
        let empties = with_items(vec![polyline(max, false), empty]);
        assert_eq!(check_limits(&empties), Err(MathError::TooLarge));
    }

    #[test]
    fn cache_size_grows_with_glyphs_and_elements() {
        let empty = approx_bytes(&with_items(Vec::new()));
        let glyph_bytes = approx_bytes(&with_items(vec![glyphs(1000)])) - empty;
        assert!(
            glyph_bytes >= 1000 * size_of::<MathGlyph>(),
            "{glyph_bytes}"
        );
        let path_bytes = approx_bytes(&with_items(vec![polyline(1000, true)])) - empty;
        assert!(path_bytes >= 1000 * size_of::<PathEl>(), "{path_bytes}");
    }

    /// `layout`'s ink, rounded to a hundredth of a pt.
    fn ink_of(src: &str) -> Rect {
        let ink = layout_of(&SandboxWorld::new(src)).ink;
        let round = |v: f64| (v * 100.0).round() / 100.0;
        Rect::new(round(ink.x0), round(ink.y0), round(ink.x1), round(ink.y1))
    }

    #[test]
    fn ink_holds_glyph_outlines_not_their_advances() {
        let layout = layout_of(&SandboxWorld::new("x"));
        let ink = layout.ink;
        // An italic x: its outline sits on the baseline, inside the box, less tall than an em.
        assert!(ink.x0 >= 0.0 && ink.x1 <= layout.width + 1.0, "{ink:?}");
        assert!((ink.y1 - layout.baseline).abs() < 0.5, "{ink:?}");
        assert!(ink.height() > 5.0 && ink.height() < 20.0, "{ink:?}");
        // Nothing drawn, no ink.
        assert_eq!(ink_of("#h(1em)"), Rect::ZERO);
    }

    #[test]
    fn ink_reaches_items_outside_the_box() {
        // Placed and moved content overhangs the formula box.
        let box_only = layout_of(&SandboxWorld::new("x"));
        let placed = layout_of(&SandboxWorld::new("x #place(dx: 300pt, dy: -200pt)[y]"));
        assert!(
            (placed.width - box_only.width).abs() < 1.0,
            "{}",
            placed.width
        );
        assert!(
            placed.ink.x1 > 300.0 && placed.ink.y0 < -190.0,
            "{:?}",
            placed.ink
        );
        let moved = ink_of("#move(dx: -50pt)[$x$]");
        assert!(moved.x0 < -40.0, "{moved:?}");
    }

    #[test]
    fn ink_holds_half_the_stroke_width() {
        // A 100 pt line stroked 10 pt wide: 5 pt more on every side, round caps included.
        let line = ink_of("#line(length: 100pt, stroke: 10pt)");
        assert_eq!((line.width(), line.height()), (110.0, 10.0), "{line:?}");
        // Curves count by their extrema, not their control points.
        let circle = ink_of("#circle(radius: 10pt, stroke: 2pt)");
        assert_eq!(
            (circle.width(), circle.height()),
            (22.0, 22.0),
            "{circle:?}"
        );
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
