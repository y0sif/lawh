//! The frame budget: what each object is estimated to cost to draw, and how much a frame may
//! spend, so no board can make a frame slow.
//!
//! Costs are estimated nanoseconds of `vello_cpu` work, calibrated in release on Linux `x86_64`
//! (the `calibrate_*` tests in `renderer/calibrate.rs`, which assert that nothing takes more than
//! 1.2 times its estimate, and `frame_budget_worst_cases` in `tests/bench.rs`). They follow what
//! `vello_cpu` does with a path: it flattens curves into lines, walks every line across the
//! 4x4-pixel tiles it touches, and paints the area between. So a path costs per line, per pixel
//! of outline on screen (measured along x plus along y, which is what tiles follow) and per pixel
//! of area covered, and a glyph the same for its outline from the font.
//!
//! Text adds parley's own work: laying a text out costs per character, per change of script or
//! direction and per line ([`layout`]), charged to the frame that lays it out; drawing a laid-out
//! text walks every glyph run of each line drawn ([`Estimate::text_line`]). A text's cost comes
//! from the plan made when it was laid out (`crate::text::Plan`), never from walking it.
//!
//! The estimates lean high: a frame that spends its whole budget takes less than the budget.

use std::cmp::Ordering;

use kurbo::{Affine, BezPath, PathSeg, Rect};

use crate::glyphs::GlyphStats;

/// What a window frame may spend drawing objects and the grid. With the outline budget and a
/// frame's fixed cost (the paper and the final render: 1 ms at 1920x1080, 3 ms at 2880x1800)
/// it keeps a frame under about 33 ms, two refreshes at 60 Hz: the worst boards measured,
/// text at lawh-core's caps included, take 25 ms at most (`frame_budget_worst_cases` in
/// `tests/bench.rs`, 2026-10-11).
pub(crate) const FRAME_BUDGET_NS: f64 = 18e6;

/// What a window frame may spend right after the image changed size. Resizing costs `vello_cpu`
/// about 10 ms of its own at 2880x1800 (new buffers, grown on first use), so these frames draw
/// less; the next frame draws the rest.
pub(crate) const RESIZED_FRAME_BUDGET_NS: f64 = 8e6;

/// What a window frame may spend on the outlines of objects over the budget.
pub(crate) const OUTLINE_BUDGET_NS: f64 = 3e6;

/// What an export may spend drawing objects. Exports don't need to be interactive, so this is
/// far above anything a real board needs, but a hostile board still can't make one take minutes.
pub(crate) const EXPORT_BUDGET_NS: f64 = 2e9;

/// Objects whose bounds are smaller than this on screen, in physical pixels, aren't drawn, and
/// neither are glyphs with an em this small.
pub(crate) const MIN_VISIBLE_PX: f64 = 0.5;

/// Setting up one fill or stroke, or one glyph run.
const NS_PER_DRAW: f64 = 1_500.0;
/// Each line `vello_cpu` flattens a path into.
const NS_PER_LINE: f64 = 70.0;
/// Each pixel of outline on screen, along x plus along y, per side of a stroke.
const NS_PER_OUTLINE_PX: f64 = 14.0;
/// Each pixel of area a fill or stroke covers on screen.
const NS_PER_AREA_PX: f64 = 0.3;
/// Each glyph drawn: finding and placing its outline. Glyphs have their own constants, fitted
/// to every glyph size from 8 to 65,536 pixels for the plainest and the most complex glyphs of
/// both bundled fonts (`calibrate_costs`).
const NS_PER_GLYPH: f64 = 300.0;
/// Each line a glyph's outline is flattened into.
const GLYPH_NS_PER_LINE: f64 = 50.0;
/// A glyph's curves are flattened about this finely ([`FLATTEN_PX`] for paths).
const GLYPH_FLATTEN_PX: f64 = 0.25;
/// Each pixel of a glyph's outline on screen, along x plus along y.
const GLYPH_NS_PER_OUTLINE_PX: f64 = 18.0;
/// Each pixel of a glyph's bounding box on screen.
const GLYPH_NS_PER_AREA_PX: f64 = 0.1;
/// Each math glyph looked at to estimate a frame, drawn or not.
const NS_PER_EXAMINED_GLYPH: f64 = 15.0;
/// Each line of text walked: finding it and starting on its glyph runs (`calibrate_text_drawing`).
const NS_PER_TEXT_LINE: f64 = 150.0;
/// Each glyph and each glyph run on the lines of text walked, drawn or not: parley hands out a
/// line's glyph runs only by walking every glyph in them, and the renderer walks the glyphs of
/// the runs it keeps again to cull them.
const NS_PER_WALKED_GLYPH: f64 = 15.0;
const NS_PER_WALKED_RUN: f64 = 100.0;
/// Each chunk of a text's plan looked at to estimate it (`crate::text::Plan`).
const NS_PER_EXAMINED_CHUNK: f64 = 10.0;
/// Laying out one text with parley and planning it (`calibrate_text_layout`): per text, per
/// ASCII character (0.9 us for digits and punctuation, 0.5 us for words), per other character,
/// per change of Unicode block (`crate::text::Facts::changes`: a text whose direction or script
/// changes at every character took 1.8 us a character) and per line.
const NS_PER_LAYOUT: f64 = 5_000.0;
const NS_PER_LAYOUT_ASCII_CHAR: f64 = 950.0;
const NS_PER_LAYOUT_OTHER_CHAR: f64 = 800.0;
const NS_PER_LAYOUT_CHANGE: f64 = 1_100.0;
const NS_PER_LAYOUT_LINE: f64 = 600.0;
/// Each byte of text hashed to look it up in the text cache.
const NS_PER_HASHED_BYTE: f64 = 1.0;
/// Each path element looked at to estimate a frame.
const NS_PER_EXAMINED_ELEMENT: f64 = 70.0;
/// Each grid dot, at scale factor 1.
const NS_PER_GRID_DOT: f64 = 1_100.0;
/// Each object looked at: its bounds, its math status, its transform.
const NS_PER_OBJECT: f64 = 1_000.0;
/// Each path element gone over for a path's bounds.
const NS_PER_BOUNDED_ELEMENT: f64 = 4.0;

/// Curves are flattened into lines this many pixels long, roughly: about one line per this
/// many pixels of a curve's length, at the square root (flattening error falls with the square
/// of the line count).
const FLATTEN_PX: f64 = 1.0;

/// How far past the clip a cut path's curves may reach (`draw::CUT_REACH_PX`): curves are
/// flattened no larger than this.
const MAX_FLATTENED_PX: f64 = 4096.0;

/// How much a frame may still spend.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Budget {
    left: f64,
    outlines_left: f64,
    /// Estimated work spent, outlines and estimating included.
    pub(crate) spent: f64,
}

impl Budget {
    pub(crate) fn frame() -> Self {
        Self::new(FRAME_BUDGET_NS)
    }

    /// A window frame right after the image changed size.
    pub(crate) fn resized_frame() -> Self {
        Self::new(RESIZED_FRAME_BUDGET_NS)
    }

    pub(crate) fn export() -> Self {
        Self::new(EXPORT_BUDGET_NS)
    }

    /// No limit, for measuring.
    #[cfg(test)]
    pub(crate) fn unlimited() -> Self {
        Self {
            left: f64::MAX,
            outlines_left: f64::MAX,
            spent: 0.0,
        }
    }

    fn new(left: f64) -> Self {
        Self {
            left,
            outlines_left: OUTLINE_BUDGET_NS,
            spent: 0.0,
        }
    }

    /// What is left for drawing objects.
    pub(crate) fn left(&self) -> f64 {
        self.left
    }

    /// Takes `cost` from the budget and returns `true`, or `false` when it doesn't fit (or
    /// isn't a number).
    pub(crate) fn spend(&mut self, cost: f64) -> bool {
        take(&mut self.left, &mut self.spent, cost)
    }

    /// Takes `cost` from the budget whether it fits or not: work already done, such as
    /// estimating an object that then didn't fit, or work that has to be done, such as the
    /// grid.
    pub(crate) fn charge(&mut self, cost: f64) {
        if cost.is_finite() && cost > 0.0 {
            self.left = (self.left - cost).max(0.0);
            self.spent += cost;
        }
    }

    /// The same as [`Self::spend`] for the outline of an object over the budget, from the
    /// outlines' own budget.
    pub(crate) fn spend_outline(&mut self, cost: f64) -> bool {
        take(&mut self.outlines_left, &mut self.spent, cost)
    }

    /// Takes an object's estimate from the budget if it fits, and the work of estimating it if
    /// not; `true` when the object may be drawn.
    pub(crate) fn settle(&mut self, estimate: &Estimate) -> bool {
        if self.spend(estimate.ns) {
            true
        } else {
            self.charge(estimate.examined);
            false
        }
    }
}

fn take(left: &mut f64, spent: &mut f64, cost: f64) -> bool {
    if cost.is_finite() && cost <= *left {
        *left -= cost;
        *spent += cost;
        true
    } else {
        false
    }
}

/// The estimated cost of drawing one object into `clip`, the image grown by the cull padding,
/// added up piece by piece. It stops counting once it is past `limit`, what the budget has
/// left, so estimating a huge object that can't be drawn costs little. The work of looking at
/// each piece counts too.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Estimate {
    clip: Rect,
    /// The image size in pixels.
    width: f64,
    height: f64,
    limit: f64,
    /// The estimate so far, `examined` included.
    pub(crate) ns: f64,
    /// The part of `ns` that went into looking at the object.
    examined: f64,
}

impl Estimate {
    /// An estimate for one object, starting at what looking at any object costs, so once the
    /// budget is spent every object is over it at once.
    pub(crate) fn new(clip: Rect, width: u16, height: u16, limit: f64) -> Self {
        Self {
            clip,
            width: f64::from(width),
            height: f64::from(height),
            limit,
            ns: NS_PER_OBJECT,
            examined: NS_PER_OBJECT,
        }
    }

    /// The part of the estimate that went into looking at the object: what it costs when it
    /// isn't drawn.
    #[cfg(test)]
    pub(crate) fn examined(&self) -> f64 {
        self.examined
    }

    /// Adds going over `path` for its bounds.
    pub(crate) fn bound(&mut self, path: &BezPath) {
        self.add(0.0, NS_PER_BOUNDED_ELEMENT * path.elements().len() as f64);
    }

    /// True once the estimate is past what the budget had left: the object won't be drawn,
    /// so there is no need to look further.
    pub(crate) fn over(&self) -> bool {
        !matches!(
            self.ns.partial_cmp(&self.limit),
            Some(Ordering::Less | Ordering::Equal)
        )
    }

    fn add(&mut self, ns: f64, examined: f64) {
        self.ns += ns + examined;
        self.examined += examined;
        if !self.ns.is_finite() {
            self.ns = f64::INFINITY;
        }
    }

    /// Adds looking at one glyph, and drawing it when it is on screen (`Some`): `stats` from
    /// the font, `em_px` pixels to the em.
    pub(crate) fn glyph(&mut self, drawn: Option<(GlyphStats, f64)>) {
        let ns = drawn.map_or(0.0, |(stats, em_px)| {
            glyph(stats, em_px, self.width, self.height)
        });
        self.add(ns, NS_PER_EXAMINED_GLYPH);
    }

    /// Adds setting up a glyph run.
    pub(crate) fn run(&mut self) {
        self.add(NS_PER_DRAW, 0.0);
    }

    /// Adds walking a line of text that holds `glyphs` glyphs in `runs` glyph runs, drawn or
    /// not.
    pub(crate) fn text_line(&mut self, glyphs: f64, runs: f64) {
        self.add(
            NS_PER_TEXT_LINE + NS_PER_WALKED_GLYPH * glyphs + NS_PER_WALKED_RUN * runs,
            0.0,
        );
    }

    /// Adds looking at one chunk of a text's plan.
    pub(crate) fn chunk(&mut self) {
        self.add(0.0, NS_PER_EXAMINED_CHUNK);
    }

    /// Adds drawing glyphs whose outlines add up to `drawn`, and glyphs that add up to `culled`
    /// whose ink can't reach the image, `em_px` pixels to the em, in `runs` glyph runs.
    /// `vello_cpu` still flattens the outlines of culled glyphs, but no tiles or pixels.
    pub(crate) fn glyphs(&mut self, drawn: &GlyphSums, culled: &GlyphSums, runs: f64, em_px: f64) {
        self.add(
            NS_PER_DRAW * runs
                + glyph_sums(drawn, em_px, self.width, self.height)
                + glyph_sums_flattened(culled, em_px),
            0.0,
        );
    }

    /// Adds filling `path`, given in screen pixels.
    pub(crate) fn fill(&mut self, path: &BezPath) {
        self.path(path, Affine::IDENTITY, None);
    }

    /// Adds stroking `path`, given in screen pixels, `width` pixels wide with round joins and
    /// caps.
    pub(crate) fn stroke(&mut self, path: &BezPath, width: f64) {
        self.path(path, Affine::IDENTITY, Some(width));
    }

    /// Adds filling (`stroke` is `None`) or stroking `path`, which `to_screen` maps to screen
    /// pixels: math paths, before they are cut down to the clip.
    pub(crate) fn path(&mut self, path: &BezPath, to_screen: Affine, stroke: Option<f64>) {
        let width = stroke.map_or(0.0, f64::abs);
        let sides = if stroke.is_some() { 2.0 } else { 1.0 };
        self.add(NS_PER_DRAW, 0.0);
        let mut outline = 0.0;
        for seg in path.segments() {
            if self.over() {
                return;
            }
            let (ns, seg_outline) = segment(to_screen * seg, self.clip, width);
            self.add(
                ns * sides + NS_PER_OUTLINE_PX * seg_outline * sides,
                NS_PER_EXAMINED_ELEMENT,
            );
            outline += seg_outline * sides;
        }
        if stroke.is_some() {
            // A round join or cap at every element: half a circle `width` across, flattened
            // and walked like any outline.
            let joins = path.elements().len() as f64;
            let join_outline = (2.0 * width).min(2.0 * (self.clip.width() + self.clip.height()));
            self.add(
                joins * (NS_PER_OUTLINE_PX * join_outline + NS_PER_LINE * flattened(width)),
                0.0,
            );
        }
        let covered = to_screen
            .transform_rect_bbox(path.control_box())
            .inflate(width / 2.0, width / 2.0)
            .intersect(self.clip);
        let mut area = covered.area().max(0.0);
        if stroke.is_some() {
            area = area.min(outline / sides * width);
        }
        self.add(NS_PER_AREA_PX * area, 0.0);
    }
}

/// The estimated cost of drawing `dots` grid dots at `scale_factor` (they are
/// `1.5 * scale_factor` pixels across).
pub(crate) fn grid(dots: usize, scale_factor: f64) -> f64 {
    dots as f64 * NS_PER_GRID_DOT * scale_factor.max(1.0)
}

/// The cost of flattening one segment, and the length of outline it draws into `clip`,
/// counting a stroke `width` wide (0 for a fill) as reaching half that past the segment.
fn segment(seg: PathSeg, clip: Rect, width: f64) -> (f64, f64) {
    let bounds = control_box(seg).inflate(width / 2.0, width / 2.0);
    if !bounds.overlaps(clip) {
        // Cut to a straight line, or left out whole by `vello_cpu`.
        return (NS_PER_LINE, 0.0);
    }
    // A segment reaching far past the clip is halved down to `MAX_FLATTENED_PX` first
    // (`draw::cut_path`), about twice per halving of its size.
    let reach = bounds.width().max(bounds.height()) / MAX_FLATTENED_PX;
    let halvings = if reach > 1.0 { 2.0 * reach.log2() } else { 0.0 };
    let polygon = polygon_length(seg);
    let visible = bounds.intersect(clip);
    // A line goes across its bounds once, a quadratic at most twice and a cubic three times in
    // each direction.
    let crossings = match seg {
        PathSeg::Line(_) => 1.0,
        PathSeg::Quad(_) => 2.0,
        PathSeg::Cubic(_) => 3.0,
    };
    let outline = polygon.min(crossings * (visible.width() + visible.height()));
    let lines = match seg {
        PathSeg::Line(_) => 1.0,
        _ => 1.0 + flattened(polygon),
    };
    (NS_PER_LINE * (lines + halvings), outline)
}

/// Lines a curve about `length` pixels long is flattened into, past the first.
fn flattened(length: f64) -> f64 {
    (length.min(MAX_FLATTENED_PX) / FLATTEN_PX).sqrt()
}

/// The estimated cost of a glyph with these outline statistics, drawn `em_px` pixels to the em
/// into an image of `width` x `height` pixels.
///
/// It grows with the glyph's size three ways: the lines its curves flatten into (with the
/// square root of its size), the tiles its outline crosses (with its size, up to what can be on
/// screen) and the area it covers (with its size squared, up to the image).
pub(crate) fn glyph(stats: GlyphStats, em_px: f64, width: f64, height: f64) -> f64 {
    let em = em_px.abs();
    let elements = stats.elements();
    let outline = stats.outline_em * em;
    let per_curve = if stats.curves > 0.0 {
        outline / elements.max(1.0)
    } else {
        0.0
    };
    let flattened = (per_curve.min(MAX_FLATTENED_PX) / GLYPH_FLATTEN_PX).sqrt();
    let lines = stats.lines + stats.curves * (1.0 + flattened);
    // Each element can cross the image a few times at most.
    let on_screen = outline.min(elements * 3.0 * (width + height));
    let area = (stats.area_em * em * em).min(width * height);
    NS_PER_GLYPH
        + GLYPH_NS_PER_LINE * lines
        + GLYPH_NS_PER_OUTLINE_PX * on_screen
        + GLYPH_NS_PER_AREA_PX * area
}

/// The outline statistics of many glyphs added up, so what drawing them costs can be estimated
/// at any size without going over them again ([`Estimate::glyphs`]).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct GlyphSums {
    count: f32,
    lines: f32,
    curves: f32,
    /// Each glyph's curves times the square root of its outline per element over
    /// [`GLYPH_FLATTEN_PX`]: times the square root of the em in pixels, the lines its curves
    /// flatten into past the first ([`glyph`]).
    flatten: f32,
    outline: f32,
    elements: f32,
    area: f32,
}

impl GlyphSums {
    pub(crate) fn add(&mut self, stats: GlyphStats) {
        let elements = stats.elements();
        let flatten = if stats.curves > 0.0 {
            stats.curves * (stats.outline_em / elements.max(1.0) / GLYPH_FLATTEN_PX).sqrt()
        } else {
            0.0
        };
        self.count += 1.0;
        self.lines += to_f32(stats.lines);
        self.curves += to_f32(stats.curves);
        self.flatten += to_f32(flatten);
        self.outline += to_f32(stats.outline_em);
        self.elements += to_f32(elements);
        self.area += to_f32(stats.area_em);
    }

    /// How many glyphs were added.
    pub(crate) fn count(&self) -> f64 {
        f64::from(self.count)
    }

    pub(crate) fn merge(&mut self, other: &Self) {
        self.count += other.count;
        self.lines += other.lines;
        self.curves += other.curves;
        self.flatten += other.flatten;
        self.outline += other.outline;
        self.elements += other.elements;
        self.area += other.area;
    }
}

/// No less than the sum of [`glyph`] over every glyph in `sums`, drawn `em_px` pixels to the em
/// into an image of `width` x `height` pixels: each term of [`glyph`] adds up over the glyphs,
/// and the limits it puts on a glyph's outline and area are applied to the totals.
fn glyph_sums(sums: &GlyphSums, em_px: f64, width: f64, height: f64) -> f64 {
    let em = em_px.abs();
    let count = f64::from(sums.count);
    let on_screen =
        (f64::from(sums.outline) * em).min(f64::from(sums.elements) * 3.0 * (width + height));
    let area = (f64::from(sums.area) * em * em).min(count * width * height);
    glyph_sums_flattened(sums, em_px)
        + GLYPH_NS_PER_OUTLINE_PX * on_screen
        + GLYPH_NS_PER_AREA_PX * area
}

/// The part of [`glyph_sums`] for finding the glyphs and flattening their outlines.
fn glyph_sums_flattened(sums: &GlyphSums, em_px: f64) -> f64 {
    let lines = f64::from(sums.lines)
        + f64::from(sums.curves)
        + em_px.abs().sqrt() * f64::from(sums.flatten);
    NS_PER_GLYPH * f64::from(sums.count) + GLYPH_NS_PER_LINE * lines
}

/// The least the budget charges for drawing work a frame did: lines of text walked, text glyphs
/// and glyph runs walked, glyph runs set up and glyphs drawn. A frame's estimate covers at least
/// this.
#[cfg(test)]
pub(crate) fn least_cost(
    text_lines: f64,
    walked_glyphs: f64,
    walked_runs: f64,
    runs: f64,
    glyphs: f64,
) -> f64 {
    NS_PER_TEXT_LINE * text_lines
        + NS_PER_WALKED_GLYPH * walked_glyphs
        + NS_PER_WALKED_RUN * walked_runs
        + NS_PER_DRAW * runs
        + NS_PER_GLYPH * glyphs
}

/// The estimated cost of laying out a text of `chars` characters, `non_ascii` of them outside
/// ASCII, with `changes` changes of Unicode block, in `lines` lines, and planning it.
pub(crate) fn layout(chars: usize, non_ascii: usize, changes: usize, lines: usize) -> f64 {
    let ascii = chars.saturating_sub(non_ascii);
    NS_PER_LAYOUT
        + NS_PER_LAYOUT_ASCII_CHAR * ascii as f64
        + NS_PER_LAYOUT_OTHER_CHAR * non_ascii as f64
        + NS_PER_LAYOUT_CHANGE * changes as f64
        + NS_PER_LAYOUT_LINE * lines as f64
}

/// The estimated cost of hashing `bytes` bytes of text.
pub(crate) fn hashing(bytes: usize) -> f64 {
    NS_PER_HASHED_BYTE * bytes as f64
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "glyph statistics are small numbers, summed in f32 to keep text plans small"
)]
fn to_f32(v: f64) -> f32 {
    v as f32
}

/// The smallest rectangle holding a segment's points, control points included.
fn control_box(seg: PathSeg) -> Rect {
    match seg {
        PathSeg::Line(l) => Rect::from_points(l.p0, l.p1),
        PathSeg::Quad(q) => Rect::from_points(q.p0, q.p1).union_pt(q.p2),
        PathSeg::Cubic(c) => Rect::from_points(c.p0, c.p1).union_pt(c.p2).union_pt(c.p3),
    }
}

/// The length of a segment's control polygon along x plus along y: no less than the segment's
/// own.
fn polygon_length(seg: PathSeg) -> f64 {
    let manhattan = |a: kurbo::Point, b: kurbo::Point| (b.x - a.x).abs() + (b.y - a.y).abs();
    match seg {
        PathSeg::Line(l) => manhattan(l.p0, l.p1),
        PathSeg::Quad(q) => manhattan(q.p0, q.p1) + manhattan(q.p1, q.p2),
        PathSeg::Cubic(c) => manhattan(c.p0, c.p1) + manhattan(c.p1, c.p2) + manhattan(c.p2, c.p3),
    }
}

#[cfg(test)]
mod tests {
    use kurbo::{Circle, Shape as _};

    use super::*;

    const CLIP: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);

    fn estimate() -> Estimate {
        Estimate::new(CLIP, 1920, 1080, f64::INFINITY)
    }

    fn fill(path: &BezPath) -> f64 {
        let mut cost = estimate();
        cost.fill(path);
        cost.ns
    }

    const X: GlyphStats = GlyphStats {
        lines: 9.0,
        curves: 8.0,
        outline_em: 3.7,
        area_em: 0.26,
        ink_em: [0.03, 0.0, 0.53, 0.54],
    };

    #[test]
    fn paths_cost_more_on_screen_than_off() {
        let on = Circle::new((960.0, 540.0), 400.0).to_path(0.1);
        let off = Circle::new((-5000.0, 540.0), 400.0).to_path(0.1);
        assert!(
            fill(&on) > 10.0 * fill(&off),
            "{} {}",
            fill(&on),
            fill(&off)
        );
    }

    #[test]
    fn huge_paths_cost_no_more_than_the_screen_allows() {
        let screen = Circle::new((960.0, 540.0), 1000.0).to_path(0.1);
        let huge = Circle::new((960.0, 540.0), 1e12).to_path(1e3);
        assert!(fill(&huge).is_finite());
        assert!(
            fill(&huge) < 20.0 * fill(&screen),
            "{} {}",
            fill(&huge),
            fill(&screen)
        );
    }

    #[test]
    fn wide_strokes_cost_more() {
        let mut line = BezPath::new();
        line.move_to((0.0, 540.0));
        line.line_to((1920.0, 540.0));
        let mut thin = estimate();
        thin.stroke(&line, 2.0);
        let mut wide = estimate();
        wide.stroke(&line, 1000.0);
        assert!(wide.ns > 2.0 * thin.ns, "{} {}", wide.ns, thin.ns);
    }

    #[test]
    fn glyph_cost_grows_with_size() {
        let small = glyph(X, 20.0, 1920.0, 1080.0);
        let large = glyph(X, 800.0, 1920.0, 1080.0);
        assert!(large > 5.0 * small, "{small} {large}");
        // Past the screen, the area stops growing.
        let huge = glyph(X, 1e5, 1920.0, 1080.0);
        let huger = glyph(X, 1e6, 1920.0, 1080.0);
        assert!(huger < 2.0 * huge, "{huge} {huger}");
    }

    #[test]
    fn an_estimate_stops_once_over_its_limit() {
        // A thousand screen-crossing lines, with room for a few.
        let mut zigzag = BezPath::new();
        zigzag.move_to((0.0, 0.0));
        for i in 0..1000 {
            zigzag.line_to((if i % 2 == 0 { 1920.0 } else { 0.0 }, f64::from(i)));
        }
        let mut full = estimate();
        full.fill(&zigzag);
        let mut limited = Estimate::new(CLIP, 1920, 1080, full.ns / 100.0);
        limited.fill(&zigzag);
        assert!(limited.over());
        assert!(limited.ns < full.ns / 50.0, "{} {}", limited.ns, full.ns);
        // What it looked at is charged when it doesn't fit.
        let mut budget = Budget::frame();
        budget.charge(FRAME_BUDGET_NS - full.ns / 100.0);
        assert!(!budget.settle(&limited));
        assert!(budget.left() < full.ns / 100.0);
        assert!(budget.left() > 0.0);
    }

    #[test]
    fn glyphs_off_screen_cost_only_the_look() {
        let mut on = estimate();
        let mut off = estimate();
        for _ in 0..100 {
            on.glyph(Some((X, 20.0)));
            off.glyph(None);
        }
        assert!(on.ns > 100.0 * off.ns, "{} {}", on.ns, off.ns);
        assert!(off.ns > 0.0);
    }

    #[test]
    fn the_budget_refuses_what_does_not_fit() {
        let mut budget = Budget::frame();
        assert!(budget.spend(FRAME_BUDGET_NS / 2.0));
        assert!(!budget.spend(FRAME_BUDGET_NS));
        assert!(!budget.spend(f64::NAN));
        assert!(!budget.spend(f64::INFINITY));
        assert!(budget.spend(FRAME_BUDGET_NS / 2.0));
        assert!(!budget.spend(1.0));
        budget.charge(1e6);
        assert!(budget.left().abs() < f64::EPSILON);
        assert!(budget.spend_outline(OUTLINE_BUDGET_NS));
        assert!(!budget.spend_outline(1.0));
        assert!((budget.spent - FRAME_BUDGET_NS - 1e6 - OUTLINE_BUDGET_NS).abs() < 1.0);
    }
}
