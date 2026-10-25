//! The shared draw list: what each object of a board draws, so the window and every export draw
//! the same things the same way.
//!
//! An object is a list of [`Item`]s: shape outlines to stroke, arrowheads to fill, glyph runs
//! (board text, and math's glyphs) and math paths. Items borrow what they draw from where it
//! already is (a text's cached parley layout, a formula's [`MathLayout`]), so building them
//! allocates nothing; the builders here are iterators, which a sink can walk twice (to estimate
//! its cost, then to draw it). Colours are [`Paint`]s, roles the sink looks up in its
//! [`Palette`], or a colour math asked for.
//!
//! Items are built in a [`Space`]: grid units, independent of any view, for exports, which map
//! them on to their own units; or screen pixels through a viewport for the window, whose shapes
//! keep their size on screen at every zoom ([`Style`]). The builders are the same for both, so a
//! shape's geometry (where an arrow's shaft stops, its head's corners) is worked out in one
//! place, in the units it is drawn in.
//!
//! Objects are built one at a time, handed to a [`Sink`], then dropped, so drawing a board never
//! holds more than one formula's layout of its own.

use std::ops::Range;

use kurbo::{Affine, BezPath, Ellipse, Line, Point, Rect, Shape as _, Vec2};
use lawh_core::{ObjectId, Shape};
use lawh_math::{MathGlyph, MathItem, MathLayout};
use parley::{GlyphRun, Layout, PositionedLayoutItem};
use vello_cpu::peniko::FontData;

use crate::export::{EXPORT_ARROW_HEAD_UNITS, EXPORT_STROKE_UNITS};
use crate::glyphs::GlyphTable;
use crate::palette::{Palette, math_paint};
use crate::text::font_index;
use crate::{Viewport, draw};

/// A colour: a role the palette gives a colour to, or a colour of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Paint {
    /// Shapes, text and math in Typst's default black ([`Palette::ink`]).
    Ink,
    /// The source of a formula that failed ([`Palette::error`]).
    Error,
    /// The source of a formula still compiling, in the window ([`Palette::pending`]).
    Pending,
    /// A colour math asked for, as Typst gave it: sRGB bytes, red, green, blue and alpha, not
    /// premultiplied.
    Rgba([u8; 4]),
}

/// Where items are built, and so what units they are in.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Space<'v> {
    /// Grid units, independent of any view, with shapes at the window's zoom-1 sizes
    /// ([`Style::EXPORT`]): for exports, which map items on to their own units.
    Board,
    /// Physical pixels through a viewport ([`Viewport::to_screen`]), with shapes at sizes that
    /// stay the same on screen at every zoom ([`Style::screen`]): for the window.
    Screen(&'v Viewport),
}

impl Space<'_> {
    /// Board point `p` in this space.
    fn point(self, p: Point) -> Point {
        match self {
            Space::Board => p,
            Space::Screen(vp) => vp.to_screen(p),
        }
    }

    /// A length of `units` grid units in this space.
    fn length(self, units: f64) -> f64 {
        match self {
            Space::Board => units,
            Space::Screen(vp) => units * vp.px_per_unit(),
        }
    }

    /// The sizes shapes are drawn at in this space, in its units.
    pub(crate) fn style(self) -> Style {
        match self {
            Space::Board => Style::EXPORT,
            Space::Screen(vp) => Style::screen(vp),
        }
    }

    /// `to_board`, which maps something's own units (a text's layout units, a formula's pt) to
    /// grid units, on to this space: what a glyph run or a path built here carries.
    pub(crate) fn map(self, to_board: Affine) -> Affine {
        match self {
            Space::Board => to_board,
            Space::Screen(vp) => vp.transform() * to_board,
        }
    }
}

/// How wide shape outlines are and how long arrowheads are, in the units of the [`Space`] they
/// are built in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Style {
    pub(crate) stroke: f64,
    pub(crate) head: f64,
}

impl Style {
    /// The sizes exports draw with, in grid units: the window's at zoom 1.
    pub(crate) const EXPORT: Style = Style {
        stroke: EXPORT_STROKE_UNITS,
        head: EXPORT_ARROW_HEAD_UNITS,
    };

    /// The window's sizes through `vp`, in physical pixels: outlines
    /// [`draw::STROKE_PX`] logical pixels wide at every zoom, and arrowheads
    /// [`draw::ARROW_HEAD_UNITS`] long, kept between their pixel limits.
    pub(crate) fn screen(vp: &Viewport) -> Style {
        Style {
            stroke: draw::stroke_width_px(vp),
            head: draw::arrow_head_px(vp),
        }
    }
}

/// A shape's outline, in the units of the [`Space`] it was built in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Outline {
    /// Axis-aligned, its radii above or at zero.
    Ellipse {
        center: Point,
        radii: Vec2,
    },
    /// Its corners in order: `x0 <= x1`, `y0 <= y1`.
    Rect(Rect),
    Line(Line),
}

impl Outline {
    /// The smallest rectangle holding the outline itself, without its stroke.
    pub(crate) fn bounds(self) -> Rect {
        match self {
            Outline::Ellipse { center, radii } => Rect::new(
                center.x - radii.x,
                center.y - radii.y,
                center.x + radii.x,
                center.y + radii.y,
            ),
            Outline::Rect(rect) => rect,
            Outline::Line(line) => Rect::from_points(line.p0, line.p1),
        }
    }

    /// The outline mapped by `transform`, which only scales (the same along both axes) and
    /// moves, as a path flattened `tolerance` units finely where it curves (units after the
    /// transform).
    pub(crate) fn to_path(self, transform: Affine, tolerance: f64) -> BezPath {
        match self {
            Outline::Ellipse { center, radii } => {
                let scale = uniform_scale(transform);
                Ellipse::new(transform * center, radii * scale, 0.0).to_path(tolerance)
            }
            Outline::Rect(rect) => transform.transform_rect_bbox(rect).to_path(tolerance),
            Outline::Line(line) => draw::segment(transform * line.p0, transform * line.p1),
        }
    }
}

/// One thing an object draws, in the units of the [`Space`] it was built in, or in units its
/// `transform` maps to them.
#[derive(Clone)]
pub(crate) enum Item<'a> {
    /// A shape's outline, stroked `width` units wide with round caps and joins.
    Stroke {
        outline: Outline,
        width: f64,
        paint: Paint,
    },
    /// A filled triangle: an arrowhead, tip first.
    Fill { corners: [Point; 3], paint: Paint },
    /// A run of glyphs from one bundled font at one size.
    Glyphs(Glyphs<'a>),
    /// A path of a formula, in pt within its layout, filled, stroked or both. A stroke has
    /// round caps and joins (`kurbo::Stroke::new`'s), and is `width` pt wide.
    Path {
        path: &'a BezPath,
        /// The layout's pt to the space's units: only scales (the same along both axes) and
        /// moves.
        transform: Affine,
        fill: Option<Paint>,
        stroke: Option<(f64, Paint)>,
    },
}

/// A run of glyphs from one bundled font at one size: a glyph run of a text's layout (board
/// text, or a formula's source drawn as text), or one of a formula's glyph runs.
#[derive(Clone)]
pub(crate) struct Glyphs<'a> {
    /// The em size, in the run's units: layout units for text (`crate::text::LAYOUT_EM`), pt
    /// for math.
    pub(crate) size: f64,
    /// The run's units to the space's units: only scales (the same along both axes) and moves.
    pub(crate) transform: Affine,
    pub(crate) paint: Paint,
    pub(crate) run: Run<'a>,
}

/// Where a run's glyphs come from.
#[derive(Clone)]
pub(crate) enum Run<'a> {
    /// A glyph run of a text's parley layout, and the text laid out (the whole text: the run's
    /// glyphs show byte ranges of it, [`Glyphs::glyphs_with_text`]). Board text has one style,
    /// so each glyph run is a whole run of parley's.
    Text {
        glyphs: GlyphRun<'a, ()>,
        text: &'a str,
    },
    /// A formula's glyphs, in the bundled font `font` (an index into [`lawh_math::fonts`]).
    /// Math carries no text: its glyphs may have no character (stretched delimiters, size
    /// variants).
    Math {
        font: usize,
        glyphs: &'a [MathGlyph],
    },
}

/// A glyph and where its origin is, in its run's units, y down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Placed {
    /// The glyph's id in its run's font: from parley, or from the math worker's reply, which is
    /// checked against the font's glyph count when it is read.
    pub(crate) id: u32,
    pub(crate) x: f64,
    pub(crate) y: f64,
}

impl Glyphs<'_> {
    /// The run's font, an index into [`lawh_math::fonts`]: a text run's is looked up from its
    /// parley font when asked (the window, which draws from parley's font, never asks).
    pub(crate) fn font(&self) -> usize {
        match &self.run {
            Run::Text { glyphs, .. } => font_index(glyphs.run().font()),
            Run::Math { font, .. } => *font,
        }
    }

    /// The run's glyphs in the order they are drawn (visual order, for text).
    pub(crate) fn glyphs(&self) -> impl Iterator<Item = Placed> + Clone + '_ {
        let (text, math) = match &self.run {
            Run::Text { glyphs, .. } => (Some(glyphs.positioned_glyphs().map(text_glyph)), None),
            Run::Math { glyphs, .. } => (None, Some(glyphs.iter().map(math_glyph))),
        };
        text.into_iter().flatten().chain(math.into_iter().flatten())
    }

    /// The text the run's glyphs show, for a text run; `None` for math.
    pub(crate) fn text(&self) -> Option<&str> {
        match &self.run {
            Run::Text { text, .. } => Some(text),
            Run::Math { .. } => None,
        }
    }

    /// The glyphs of a text run as [`Self::glyphs`] gives them, each with the byte range of
    /// [`Self::text`] it shows: its cluster's, so every glyph of a cluster (a letter and its
    /// marks set as two glyphs) has the same range. Nothing for math.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "for the PDF exporter's text, which comes next")
    )]
    pub(crate) fn glyphs_with_text(&self) -> impl Iterator<Item = (Placed, Range<usize>)> + '_ {
        let run = match &self.run {
            Run::Text { glyphs, .. } => Some(glyphs),
            Run::Math { .. } => None,
        };
        run.into_iter().flat_map(|glyphs| {
            // A cluster's glyphs, in the order the run's glyphs are walked: both go over the
            // run's shaped clusters in visual order, and a ligature's glyphs come with its
            // first cluster.
            let ranges = glyphs.run().visual_clusters().flat_map(|cluster| {
                let range = cluster.text_range();
                cluster.glyphs().map(move |_| range.clone())
            });
            glyphs.positioned_glyphs().map(text_glyph).zip(ranges)
        })
    }
}

fn text_glyph(glyph: parley::Glyph) -> Placed {
    Placed {
        id: glyph.id,
        x: f64::from(glyph.x),
        y: f64::from(glyph.y),
    }
}

fn math_glyph(glyph: &MathGlyph) -> Placed {
    Placed {
        id: glyph.id,
        x: glyph.x,
        y: glyph.y,
    }
}

/// What a sink may use of the renderer's while it draws: outline sizes of the bundled fonts'
/// glyphs (for estimates), the fonts as `vello_cpu` takes them, and the colours.
pub(crate) struct Tools<'r> {
    pub(crate) glyphs: &'r mut GlyphTable,
    /// [`lawh_math::fonts`], by the same index.
    pub(crate) fonts: &'r [FontData],
    pub(crate) palette: &'r Palette,
}

/// Where a board's draw list goes, one object at a time: its id and bounds, then its items.
pub(crate) trait Sink {
    type Error;

    /// Starts object `id`, whose items lie within `bounds` (in the units of the space they are
    /// built in, strokes and arrowheads included).
    fn begin(
        &mut self,
        tools: &mut Tools<'_>,
        id: &ObjectId,
        bounds: Rect,
    ) -> Result<(), Self::Error>;

    /// One item of the object begun last.
    fn item(&mut self, tools: &mut Tools<'_>, item: &Item<'_>) -> Result<(), Self::Error>;
}

/// What a shape (an ellipse, a rectangle, a line or an arrow) draws in `space`, at the space's
/// sizes ([`Space::style`]): its outline, and an arrow's head. At most two items. Nothing for
/// text or math, or for a shape whose numbers aren't finite.
///
/// The shape is mapped into the space first and its geometry worked out there: an arrow's
/// shaft and head in screen pixels for the window (where the head's length is clamped in
/// pixels), in grid units for an export.
pub(crate) fn shape_items(shape: &Shape, space: Space<'_>) -> impl Iterator<Item = Item<'static>> {
    let style = space.style();
    let stroke = |outline| Item::Stroke {
        outline,
        width: style.stroke,
        paint: Paint::Ink,
    };
    let items = match shape {
        Shape::Ellipse { center, rx, ry } => [
            Some(stroke(Outline::Ellipse {
                center: space.point(*center),
                radii: Vec2::new(space.length(rx.abs()), space.length(ry.abs())),
            })),
            None,
        ],
        Shape::Rect { origin, w, h } => [
            Some(stroke(Outline::Rect(
                Rect::from_origin_size(space.point(*origin), (space.length(*w), space.length(*h)))
                    .abs(),
            ))),
            None,
        ],
        Shape::Line { a, b } => [
            Some(stroke(Outline::Line(Line::new(
                space.point(*a),
                space.point(*b),
            )))),
            None,
        ],
        Shape::Arrow { a, b } => {
            let (a, b) = (space.point(*a), space.point(*b));
            match draw::arrow_parts(a, b, style.head) {
                Some((end, corners)) => [
                    Some(stroke(Outline::Line(Line::new(a, end)))),
                    Some(Item::Fill {
                        corners,
                        paint: Paint::Ink,
                    }),
                ],
                // No length, so no head.
                None => [Some(stroke(Outline::Line(Line::new(a, b)))), None],
            }
        }
        Shape::Text { .. } | Shape::Math { .. } => [None, None],
    };
    let finite = shape_finite(shape);
    items.into_iter().flatten().filter(move |_| finite)
}

/// Whether every number of a shape is finite, so it can be drawn.
fn shape_finite(shape: &Shape) -> bool {
    match shape {
        Shape::Ellipse { center, rx, ry } => center.is_finite() && rx.is_finite() && ry.is_finite(),
        Shape::Rect { origin, w, h } => origin.is_finite() && w.is_finite() && h.is_finite(),
        Shape::Line { a, b } | Shape::Arrow { a, b } => a.is_finite() && b.is_finite(),
        Shape::Text { .. } | Shape::Math { .. } => true,
    }
}

/// The bounds of what a shape draws in `space`, strokes (half their width past the outline, as
/// round caps and joins reach) and arrowheads included; `None` for text or math, or a shape
/// whose numbers aren't finite.
pub(crate) fn shape_bounds(shape: &Shape, space: Space<'_>) -> Option<Rect> {
    shape_items(shape, space)
        .filter_map(|item| item_bounds(&item))
        .reduce(|a, b| a.union(b))
        .filter(Rect::is_finite)
}

/// The bounds of a shape's item in its space's units; `None` for glyphs and math paths, whose
/// bounds come from their layout.
fn item_bounds(item: &Item<'_>) -> Option<Rect> {
    match item {
        Item::Stroke { outline, width, .. } => {
            let reach = width.abs() / 2.0;
            Some(outline.bounds().inflate(reach, reach))
        }
        Item::Fill { corners, .. } => {
            let [a, b, c] = *corners;
            Some(Rect::from_points(a, b).union_pt(c))
        }
        Item::Glyphs(_) | Item::Path { .. } => None,
    }
}

/// What a text laid out as `layout` draws in `paint`, `transform` mapping its layout units to
/// the space's units (`crate::renderer::text_transform`): one glyph run item per glyph run,
/// line by line.
pub(crate) fn text_items<'a>(
    layout: &'a Layout<()>,
    text: &'a str,
    transform: Affine,
    paint: Paint,
) -> impl Iterator<Item = Item<'a>> {
    layout
        .lines()
        .flat_map(move |line| line_items(line, text, transform, paint))
}

/// The glyph run items of one line of a text's layout, as [`text_items`] gives them.
fn line_items<'a>(
    line: parley::Line<'a, ()>,
    text: &'a str,
    transform: Affine,
    paint: Paint,
) -> impl Iterator<Item = Item<'a>> {
    line.items().filter_map(move |item| {
        let PositionedLayoutItem::GlyphRun(glyphs) = item else {
            return None;
        };
        Some(run_item(glyphs, text, transform, paint))
    })
}

/// The item of one glyph run of a text's layout, `text` the whole text laid out, as
/// [`text_items`] gives them: for the window, which builds items only for the runs that reach
/// its view.
pub(crate) fn run_item<'a>(
    glyphs: GlyphRun<'a, ()>,
    text: &'a str,
    transform: Affine,
    paint: Paint,
) -> Item<'a> {
    Item::Glyphs(Glyphs {
        size: f64::from(glyphs.run().font_size()),
        transform,
        paint,
        run: Run::Text { glyphs, text },
    })
}

/// What a formula laid out as `layout` draws, `transform` mapping its pt to the space's units
/// (`Renderer::math_transform`). Typst's default black is [`Paint::Ink`].
pub(crate) fn math_items(layout: &MathLayout, transform: Affine) -> impl Iterator<Item = Item<'_>> {
    layout.items.iter().map(move |item| match item {
        MathItem::Glyphs {
            font,
            size,
            fill,
            glyphs,
        } => Item::Glyphs(Glyphs {
            size: *size,
            transform,
            paint: math_paint(*fill),
            run: Run::Math {
                font: *font,
                glyphs,
            },
        }),
        MathItem::Path { path, fill, stroke } => Item::Path {
            path,
            transform,
            fill: fill.map(math_paint),
            stroke: stroke.map(|(width, color)| (width, math_paint(color))),
        },
    })
}

/// How much `transform` (which only scales, the same along both axes, and moves) scales.
pub(crate) fn uniform_scale(transform: Affine) -> f64 {
    transform.determinant().abs().sqrt()
}

/// What a sink was handed for one object: for tests that hold the window and the exports to the
/// same draw list (`Renderer::record_frame`, `Renderer::record_export`).
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct SceneObject {
    pub id: ObjectId,
    /// Its bounds as the sink was given them: screen pixels for a window frame (the bounds it
    /// is culled by), grid units for an export (strokes and arrowheads included).
    pub bounds: Rect,
    pub items: Vec<SceneItem>,
}

/// One item a sink was handed: its kind and size.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub enum SceneItem {
    /// A shape's outline, stroked `width` wide (in the sink's units).
    Stroke { width: f64 },
    /// An arrowhead.
    Fill,
    /// A glyph run: its font (an index into `lawh_math::fonts()`), how many glyphs it holds, and
    /// the text it shows (`None` for math).
    Glyphs {
        font: usize,
        glyphs: usize,
        text: Option<String>,
    },
    /// A formula's path: how many elements, and whether it is filled and stroked.
    Path {
        elements: usize,
        fill: bool,
        stroke: bool,
    },
}

/// A sink that tallies what it is handed, as [`SceneObject`]s: for tests, through an export
/// (`Renderer::record_export`), or as a copy of what the window's sink is handed
/// (`Renderer::record_frame`).
#[derive(Debug, Default)]
pub(crate) struct Tally {
    pub(crate) objects: Vec<SceneObject>,
}

impl Sink for Tally {
    type Error = std::convert::Infallible;

    fn begin(&mut self, _: &mut Tools<'_>, id: &ObjectId, bounds: Rect) -> Result<(), Self::Error> {
        self.objects.push(SceneObject {
            id: id.clone(),
            bounds,
            items: Vec::new(),
        });
        Ok(())
    }

    fn item(&mut self, _: &mut Tools<'_>, item: &Item<'_>) -> Result<(), Self::Error> {
        let tallied = match item {
            Item::Stroke { width, .. } => SceneItem::Stroke { width: *width },
            Item::Fill { .. } => SceneItem::Fill,
            Item::Glyphs(glyphs) => SceneItem::Glyphs {
                font: glyphs.font(),
                glyphs: glyphs.glyphs().count(),
                text: glyphs.text().map(str::to_owned),
            },
            Item::Path {
                path, fill, stroke, ..
            } => SceneItem::Path {
                elements: path.elements().len(),
                fill: fill.is_some(),
                stroke: stroke.is_some(),
            },
        };
        if let Some(object) = self.objects.last_mut() {
            object.items.push(tallied);
        }
        Ok(())
    }
}

/// A sink that keeps what it is handed, for tests: the same objects and items, whoever draws
/// them.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct Recorder {
    pub(crate) objects: Vec<Recorded>,
}

/// One object a [`Recorder`] was handed.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Recorded {
    pub(crate) id: ObjectId,
    pub(crate) bounds: Rect,
    pub(crate) items: Vec<RecordedItem>,
}

/// An [`Item`], owning what it held.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RecordedItem {
    Stroke {
        outline: Outline,
        width: f64,
        paint: Paint,
    },
    Fill {
        corners: [Point; 3],
        paint: Paint,
    },
    Glyphs {
        font: usize,
        size: f64,
        transform: Affine,
        paint: Paint,
        glyphs: Vec<Placed>,
        text: Option<String>,
    },
    Path {
        elements: usize,
        transform: Affine,
        fill: Option<Paint>,
        stroke: Option<(f64, Paint)>,
    },
}

#[cfg(test)]
impl Sink for Recorder {
    type Error = std::convert::Infallible;

    fn begin(&mut self, _: &mut Tools<'_>, id: &ObjectId, bounds: Rect) -> Result<(), Self::Error> {
        self.objects.push(Recorded {
            id: id.clone(),
            bounds,
            items: Vec::new(),
        });
        Ok(())
    }

    fn item(&mut self, _: &mut Tools<'_>, item: &Item<'_>) -> Result<(), Self::Error> {
        let recorded = match item {
            Item::Stroke {
                outline,
                width,
                paint,
            } => RecordedItem::Stroke {
                outline: *outline,
                width: *width,
                paint: *paint,
            },
            Item::Fill { corners, paint } => RecordedItem::Fill {
                corners: *corners,
                paint: *paint,
            },
            Item::Glyphs(glyphs) => RecordedItem::Glyphs {
                font: glyphs.font(),
                size: glyphs.size,
                transform: glyphs.transform,
                paint: glyphs.paint,
                glyphs: glyphs.glyphs().collect(),
                text: glyphs.text().map(str::to_owned),
            },
            Item::Path {
                path,
                transform,
                fill,
                stroke,
            } => RecordedItem::Path {
                elements: path.elements().len(),
                transform: *transform,
                fill: *fill,
                stroke: *stroke,
            },
        };
        if let Some(object) = self.objects.last_mut() {
            object.items.push(recorded);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use lawh_math::MathEngine;

    use super::*;
    use crate::text::TextShaper;

    fn shape(src: &str) -> Shape {
        let board = lawh_core::from_lawh(&format!("lawh version=1\n{src}\n")).unwrap();
        board.objects()[0].shape.clone()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn export_strokes_and_heads_are_the_window_at_zoom_1() {
        assert!(close(Style::EXPORT.stroke, 0.05), "{:?}", Style::EXPORT);
        assert!(close(Style::EXPORT.head, 0.3), "{:?}", Style::EXPORT);
    }

    #[test]
    fn shape_bounds_include_the_stroke_and_the_head() {
        let style = Space::Board;
        let half = Style::EXPORT.stroke / 2.0;
        let ellipse = shape_bounds(&shape("ellipse id=e1 x=0 y=0 rx=3 ry=2"), style).unwrap();
        assert!(
            close(ellipse.x0, -3.0 - half) && close(ellipse.y1, 2.0 + half),
            "{ellipse:?}"
        );
        // A rectangle made without the board's checks, its width below zero.
        let rect = Shape::Rect {
            origin: Point::new(1.0, 1.0),
            w: -2.0,
            h: 4.0,
        };
        let rect = shape_bounds(&rect, style).unwrap();
        assert!(
            close(rect.x0, -1.0 - half) && close(rect.x1, 1.0 + half),
            "{rect:?}"
        );
        // An arrow pointing right: the head's corners spread 0.4 of its length to either side,
        // past the shaft's stroke.
        let arrow = shape_bounds(&shape("arrow id=a1 x1=0 y1=0 x2=4 y2=0"), style).unwrap();
        assert!(close(arrow.x1, 4.0), "{arrow:?}");
        assert!(
            close(arrow.y0, -0.4 * 0.3) && close(arrow.y1, 0.4 * 0.3),
            "{arrow:?}"
        );
        assert!(close(arrow.x0, -half), "{arrow:?}");
        // The shaft stops halfway into the head.
        let items: Vec<_> = shape_items(&shape("arrow id=a1 x1=0 y1=0 x2=4 y2=0"), style).collect();
        let Item::Stroke {
            outline: Outline::Line(shaft),
            ..
        } = &items[0]
        else {
            panic!("no shaft");
        };
        assert!(close(shaft.p1.x, 4.0 - 0.15), "{shaft:?}");
        assert!(matches!(items[1], Item::Fill { .. }));
        // An arrow shorter than its head has a head as long as itself.
        let short = shape_bounds(&shape("arrow id=a1 x1=0 y1=0 x2=0.1 y2=0"), style).unwrap();
        assert!(close(short.y1, 0.4 * 0.1), "{short:?}");
        // Text and math draw no shape items.
        assert_eq!(
            shape_bounds(&shape("text id=t1 x=0 y=0 \"x\""), style),
            None
        );
    }

    #[test]
    fn shapes_with_numbers_that_are_not_finite_draw_nothing() {
        let shape = Shape::Line {
            a: Point::new(f64::NAN, 0.0),
            b: Point::ZERO,
        };
        assert_eq!(shape_items(&shape, Space::Board).count(), 0);
        assert_eq!(shape_bounds(&shape, Space::Board), None);
    }

    #[test]
    fn on_screen_shapes_are_in_pixels_at_the_window_sizes() {
        // 80 pixels a unit, scale factor 2: outlines 4 pixels wide.
        let vp = Viewport::new(Point::new(1.0, 2.0), 1.0, 400, 300, 2.0);
        let items: Vec<_> = shape_items(
            &shape("ellipse id=e1 x=3 y=2 rx=1 ry=0.5"),
            Space::Screen(&vp),
        )
        .collect();
        let [
            Item::Stroke {
                outline: Outline::Ellipse { center, radii },
                width,
                paint: Paint::Ink,
            },
        ] = items[..]
        else {
            panic!("{}", items.len());
        };
        assert_eq!(center, vp.to_screen(Point::new(3.0, 2.0)));
        assert_eq!(radii, Vec2::new(80.0, 40.0));
        assert!(close(width, 4.0), "{width}");
        // A rectangle keeps its corner and size through the view.
        let items: Vec<_> =
            shape_items(&shape("rect id=r1 x=1 y=2 w=3 h=0.5"), Space::Screen(&vp)).collect();
        let [
            Item::Stroke {
                outline: Outline::Rect(rect),
                ..
            },
        ] = items[..]
        else {
            panic!("{}", items.len());
        };
        assert_eq!(rect, Rect::new(200.0, 150.0, 440.0, 190.0));
        // Arrowheads are 0.3 units long, kept between 8 and 24 logical pixels: 24 pixels at
        // zoom 1 (here 0.3 x 80), 16 when zoomed far out, 48 when zoomed far in.
        for (zoom, head) in [(1.0, 24.0), (0.05, 16.0), (20.0, 48.0)] {
            let vp = Viewport { zoom, ..vp };
            let arrow = shape("arrow id=a1 x1=0 y1=0 x2=1000 y2=0");
            let items: Vec<_> = shape_items(&arrow, Space::Screen(&vp)).collect();
            let [
                Item::Stroke {
                    outline: Outline::Line(shaft),
                    ..
                },
                Item::Fill { corners, .. },
            ] = items[..]
            else {
                panic!("{}", items.len());
            };
            let tip = vp.to_screen(Point::new(1000.0, 0.0));
            assert_eq!(corners[0], tip);
            assert!(close(tip.x - corners[1].x, head), "{zoom}: {corners:?}");
            assert!(close(tip.x - shaft.p1.x, head / 2.0), "{zoom}: {shaft:?}");
        }
    }

    /// The glyph runs of `text` as [`text_items`] gives them.
    fn runs<'a>(layout: &'a Layout<()>, text: &'a str) -> Vec<Glyphs<'a>> {
        text_items(layout, text, Affine::IDENTITY, Paint::Ink)
            .map(|item| match item {
                Item::Glyphs(glyphs) => glyphs,
                _ => panic!("text draws only glyphs"),
            })
            .collect()
    }

    #[test]
    fn text_glyphs_map_to_the_text_they_show() {
        let mut shaper = TextShaper::new();
        // Words, two lines, a letter with two combining marks, right-to-left Hebrew, and `∧`,
        // which Inter lacks and Libertinus Math has.
        for text in [
            "one two",
            "one\ntwo",
            "a\u{301}\u{302}b",
            "ab \u{5D0}\u{5D1} cd",
            "p \u{2227} q",
        ] {
            let layout = shaper.build(text);
            let mut covered = 0;
            for run in runs(&layout, text) {
                let with_text: Vec<_> = run.glyphs_with_text().collect();
                let glyphs: Vec<_> = run.glyphs().collect();
                assert_eq!(with_text.len(), glyphs.len(), "{text:?}");
                for ((placed, range), glyph) in with_text.iter().zip(&glyphs) {
                    assert_eq!(placed, glyph, "{text:?}");
                    assert!(
                        text.get(range.clone()).is_some_and(|s| !s.is_empty()),
                        "{text:?} {range:?}"
                    );
                }
                assert_eq!(run.text(), Some(text));
                covered += glyphs.len();
            }
            assert!(covered > 0, "{text:?}");
        }
        // A letter and its marks: every glyph of the cluster shows all of it.
        let text = "a\u{301}\u{302}";
        let layout = shaper.build(text);
        for run in runs(&layout, text) {
            for (_, range) in run.glyphs_with_text() {
                assert_eq!(range, 0..text.len());
            }
        }
        // The fallback symbol is set in the math font, the letters in the text font.
        let text = "p \u{2227} q";
        let layout = shaper.build(text);
        let fonts: Vec<_> = runs(&layout, text)
            .iter()
            .flat_map(|run| run.glyphs_with_text().map(|(_, range)| (range, run.font())))
            .collect();
        let wedge = text.find('\u{2227}').unwrap();
        assert!(fonts.contains(&(wedge..wedge + 3, 1)), "{fonts:?}");
        assert!(fonts.contains(&(0..1, 0)), "{fonts:?}");
    }

    #[test]
    fn math_items_keep_the_layout_and_draw_typst_black_in_ink() {
        let engine = MathEngine::new();
        let layout = engine.layout_blocking("A = sqrt(pi) r^2").unwrap();
        let transform = Affine::scale(0.5);
        let items: Vec<_> = math_items(&layout, transform).collect();
        assert_eq!(items.len(), layout.items.len());
        let mut glyphs = 0;
        for item in &items {
            match item {
                Item::Glyphs(run) => {
                    assert_eq!(run.paint, Paint::Ink);
                    assert_eq!(run.transform, transform);
                    assert_eq!(run.text(), None);
                    assert_eq!(run.glyphs_with_text().count(), 0);
                    glyphs += run.glyphs().count();
                }
                Item::Path { fill, stroke, .. } => {
                    assert!(fill.is_some() || stroke.is_some());
                }
                _ => panic!("math draws only glyphs and paths"),
            }
        }
        assert!(glyphs >= 5, "{glyphs}");
    }
}
