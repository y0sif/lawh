//! Math for lawh: compiles Typst math snippets to vector paths the renderer
//! can draw, and lists the symbols Typst math knows by name ([`symbols`]),
//! for `:sym`.
//!
//! Typst runs in a sandbox (see `SECURITY.md`): the only file it can see is
//! the formula itself, fonts come from [`fonts`], there is no clock and no
//! package download, and `plugin` (WebAssembly), `cite` and `bibliography`
//! (CSL parsing), `image` (image decoding), `read` with the data loaders
//! (`json`, `yaml`, `toml`, `cbor`, `csv`, `xml`) and `raw` (theme and syntax
//! parsing) are removed from the library.
//!
//! A formula from a board is untrusted, and Typst can't be stopped, so
//! [`MathEngine::with_worker`] compiles in a separate worker process
//! ([`worker_main`]) with a memory cap, and kills it when a compile takes too
//! long or restarts it when it crashes. [`MathEngine::new`] compiles in this
//! process, for trusted input only. Either way [`MathEngine`] caches outcomes,
//! within [`MAX_CACHED_LAYOUT_BYTES`] and [`MAX_CACHED_FAILURES`], and limits
//! how often it compiles layouts pushed out of the cache again
//! ([`MathStatus::Evicted`], [`MAX_RECOMPILE_BURST`], [`RECOMPILE_INTERVAL`]).
//!
//! A layout that reaches past [`MAX_LAYOUT_EXTENT_PT`], or holds more than
//! [`MAX_LAYOUT_GLYPHS`] glyphs or [`MAX_LAYOUT_PATH_ELEMENTS`] path elements,
//! is refused as [`MathError::TooLarge`], so a formula can't hand the renderer
//! geometry billions of pixels across or hundreds of thousands of things to
//! draw.

mod compile;
mod engine;
mod fonts;
mod layout;
mod process;
mod protocol;
mod symbols;
mod worker;
mod world;

pub use engine::{
    DEFAULT_TIMEOUT, MAX_CACHED_FAILURE_BYTES, MAX_CACHED_FAILURES, MAX_CACHED_LAYOUT_BYTES,
    MAX_ERROR_MESSAGE_BYTES, MAX_FORMULA_RECORDS, MAX_RECOMPILE_BURST, MAX_WORKERS, MathEngine,
    MathStatus, MathWaker, RECOMPILE_COOLDOWN, RECOMPILE_INTERVAL, WANTED_FOR,
};
pub use fonts::{FontAsset, fonts};
pub use process::WorkerCommand;
pub use symbols::{SymbolInfo, symbols};
pub use worker::{WORKER_MEMORY_LIMIT_BYTES, worker_main};

/// Font size of math, in Typst pt. [`MathLayout`] coordinates are at this
/// size; the renderer scales them to the board's text size.
pub const MATH_FONT_SIZE_PT: f64 = 20.0;

/// Largest absolute value, in pt, that any number in a [`MathLayout`] may
/// have: the box size, the baseline, the ink bounds, glyph sizes and
/// positions, path points (control points included) and stroke widths. A
/// layout with a larger number, or one that isn't finite, is
/// [`MathError::TooLarge`], in process and from the worker alike.
///
/// 100,000 pt is about 35 m on paper, or 2,500 grid units (5,000 text ems)
/// on a board. Real formulas are far smaller: a 30x30 matrix is about 1,500
/// pt wide and a chain of a hundred sums about 5,500 (see `tests/layout.rs`).
/// Only a formula built to be huge, such as `#circle(radius: 1e18pt)`, gets
/// near it; drawing that one zoomed in would mean flattening curves billions
/// of pixels long. Under the cap, glyph positions also keep `f32` precision
/// to a hundredth of a pt.
pub const MAX_LAYOUT_EXTENT_PT: f64 = 100_000.0;

/// Most glyphs a [`MathLayout`] may hold, across all its glyph runs (an
/// empty run counts as one). A layout with more is [`MathError::TooLarge`],
/// in process and from the worker alike.
///
/// Measured on Linux `x86_64` in release, 2026-10-10
/// (`measure_heavy_formulas` in `tests/layout.rs`, and
/// `math_complexity_frame_time` in lawh-render's `tests/bench.rs`):
///
/// - The largest real formula measured, a 30x30 matrix of subscripted
///   entries, has 4,932 glyphs; this cap is five times that. A chain of a
///   hundred sums has 701, forty `cases` rows 667.
/// - `#range(700).map(_ => "x" * 700).join(linebreak())`, under 60 bytes,
///   lays out 490,000 glyphs and took 1.4 s a frame to draw.
/// - At the cap, glyphs laid out normally draw in 2-3 ms at the app's
///   highest zoom (most are off screen) and 74 ms fitted to a 1920x1080
///   window. Glyphs stacked on one spot defeat culling: 25,000 of them took
///   1.2 s a frame at zoom 20 (2.3 s at scale factor 2), so the renderer
///   needs a frame budget on top of this cap.
pub const MAX_LAYOUT_GLYPHS: usize = 25_000;

/// Most path elements a [`MathLayout`] may hold: every element (move, line,
/// curve, close) of every path counts once, and the elements of a stroked
/// path count again, since stroking costs about twice what filling does. An
/// empty path counts as one. A layout with more is [`MathError::TooLarge`],
/// in process and from the worker alike.
///
/// Measured like [`MAX_LAYOUT_GLYPHS`]:
///
/// - Real formulas draw few paths: fraction bars, radical lines, boxes. A
///   chain of thirty integrals counts 240, a 20x20 matrix of fractions
///   1,600 and a 30x30 one 3,600. Glyphs that Typst rotates, mirrors or
///   scales become outlines of about 18 elements each: a rotated label
///   counts about 570 and a rotated 10x10 matrix 7,828.
/// - The review's 95-byte `#polygon(stroke: 100pt, ..range(100000)...)`
///   counts 200,002 and took 5.3 s a frame to draw.
/// - Elements that cross the screen cost 15 to 60 us each to draw, so a
///   hostile layout at the cap (polygons, stacked thick lines) still took
///   155 to 305 ms a frame fitted to a 1920x1080 window and 10 to 190 ms at
///   the app's highest zoom: the renderer needs a frame budget on top of this
///   cap.
pub const MAX_LAYOUT_PATH_ELEMENTS: usize = 10_000;

/// A laid-out formula in pt at [`MATH_FONT_SIZE_PT`], origin at the top-left
/// of the formula box, y down.
#[derive(Debug, Clone, PartialEq)]
pub struct MathLayout {
    /// Width of the formula box.
    pub width: f64,
    /// Height of the formula box.
    pub height: f64,
    /// Distance from the top of the box to the baseline of the first line.
    pub baseline: f64,
    /// The bounds of everything the items draw: glyph outlines from their
    /// font, paths with half their stroke width (lawh strokes math with
    /// round joins and caps). Items can reach outside the formula box
    /// (`#place`, `#move`, negative spacing), so cull and fit with this, not
    /// with the box. [`kurbo::Rect::ZERO`] when nothing is drawn.
    pub ink: kurbo::Rect,
    /// What to draw, back to front.
    pub items: Vec<MathItem>,
}

/// What a formula's layout measures, without what it draws: the engine keeps
/// this after dropping the layout itself from its cache
/// ([`MathStatus::Evicted`]), so a formula can still be placed, culled and
/// drawn as an outline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MathSummary {
    /// [`MathLayout::width`].
    pub width: f64,
    /// [`MathLayout::height`].
    pub height: f64,
    /// [`MathLayout::baseline`].
    pub baseline: f64,
    /// [`MathLayout::ink`]: cull and fit with this joined with the box.
    pub ink: kurbo::Rect,
    /// Glyphs, counted as [`MAX_LAYOUT_GLYPHS`] counts them.
    pub glyphs: usize,
    /// Path elements, counted as [`MAX_LAYOUT_PATH_ELEMENTS`] counts them.
    pub path_elements: usize,
}

impl MathSummary {
    /// The summary of `layout`.
    pub fn of(layout: &MathLayout) -> Self {
        layout::summary(layout)
    }
}

/// One drawable piece of a formula.
#[derive(Debug, Clone, PartialEq)]
pub enum MathItem {
    /// A run of glyphs from one font at one size.
    Glyphs {
        /// Index into [`fonts`].
        font: usize,
        /// Font size (em size) in pt.
        size: f64,
        /// RGBA8 fill colour.
        fill: [u8; 4],
        /// Glyphs with absolute positions.
        glyphs: Vec<MathGlyph>,
    },
    /// A vector shape: fraction bars, radical lines, and glyphs that Typst
    /// scaled or rotated (drawn as outlines).
    Path {
        /// The outline in layout coordinates.
        path: kurbo::BezPath,
        /// RGBA8 fill colour, if filled.
        fill: Option<[u8; 4]>,
        /// Stroke width in pt and RGBA8 colour, if stroked.
        stroke: Option<(f64, [u8; 4])>,
    },
}

/// One glyph of a [`MathItem::Glyphs`] run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MathGlyph {
    /// Glyph id in the run's font.
    pub id: u32,
    /// Origin x, in pt within the layout.
    pub x: f64,
    /// Baseline y, in pt within the layout, y down.
    pub y: f64,
}

/// Why a formula has no layout.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MathError {
    /// Typst rejected the formula; the message joins its diagnostics.
    #[error("math error: {0}")]
    Compile(String),
    /// The compile took longer than the engine's timeout (in process mode,
    /// the worker was killed), or, in-process, too many earlier compiles are
    /// still running.
    #[error("math took too long to compile")]
    Timeout,
    /// The compiler panicked.
    #[error("math compiler crashed")]
    Panicked,
    /// The worker process died during the compile (stack overflow, memory
    /// cap) or sent a malformed response. A new worker serves the next
    /// formula.
    #[error("math worker crashed")]
    Crashed,
    /// The worker process can't be started; the message says why.
    #[error("math is unavailable: {0}")]
    Unavailable(String),
    /// The layout reaches past [`MAX_LAYOUT_EXTENT_PT`], holds a number
    /// that isn't finite, or holds more than [`MAX_LAYOUT_GLYPHS`] glyphs or
    /// [`MAX_LAYOUT_PATH_ELEMENTS`] path elements.
    #[error("math is too large to draw")]
    TooLarge,
}
