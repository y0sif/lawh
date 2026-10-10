//! Math for lawh: compiles Typst math snippets to vector paths the renderer
//! can draw, and finds symbols by name for `:sym`.
//!
//! Typst runs in a sandbox (see `SECURITY.md`): the only file it can see is
//! the formula itself, fonts come from [`fonts`], there is no clock and no
//! package download, and `plugin` (WebAssembly), `cite` and `bibliography`
//! (CSL parsing), `image` (image decoding) and `read` with the data loaders
//! (`json`, `yaml`, `toml`, `cbor`, `csv`, `xml`) are removed from the
//! library.
//!
//! A formula from a board is untrusted, and Typst can't be stopped, so
//! [`MathEngine::with_worker`] compiles in a separate worker process
//! ([`worker_main`]) with a memory cap, and kills it when a compile takes too
//! long or restarts it when it crashes. [`MathEngine::new`] compiles in this
//! process, for trusted input only. Either way [`MathEngine`] caches every
//! outcome.
//!
//! A layout that reaches past [`MAX_LAYOUT_EXTENT_PT`] is refused as
//! [`MathError::TooLarge`], so a formula can't hand the renderer geometry
//! billions of pixels across.

mod compile;
mod engine;
mod fonts;
mod layout;
mod process;
mod protocol;
mod worker;
mod world;

pub use engine::{DEFAULT_TIMEOUT, MAX_WORKERS, MathEngine, MathStatus, MathWaker};
pub use fonts::{FontAsset, fonts};
pub use process::WorkerCommand;
pub use worker::{WORKER_MEMORY_LIMIT_BYTES, worker_main};

/// Font size of math, in Typst pt. [`MathLayout`] coordinates are at this
/// size; the renderer scales them to the board's text size.
pub const MATH_FONT_SIZE_PT: f64 = 20.0;

/// Largest absolute value, in pt, that any number in a [`MathLayout`] may
/// have: the box size, the baseline, glyph sizes and positions, path points
/// (control points included) and stroke widths. A layout with a larger
/// number, or one that isn't finite, is [`MathError::TooLarge`], in process
/// and from the worker alike.
///
/// 100,000 pt is about 35 m on paper, or 2,500 grid units (5,000 text ems)
/// on a board. Real formulas are far smaller: a 30x30 matrix is about 1,500
/// pt wide and a chain of a hundred sums about 5,500 (see `tests/layout.rs`).
/// Only a formula built to be huge, such as `#circle(radius: 1e18pt)`, gets
/// near it; drawing that one zoomed in would mean flattening curves billions
/// of pixels long. Under the cap, glyph positions also keep `f32` precision
/// to a hundredth of a pt.
pub const MAX_LAYOUT_EXTENT_PT: f64 = 100_000.0;

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
    /// What to draw, back to front.
    pub items: Vec<MathItem>,
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
    /// The layout reaches past [`MAX_LAYOUT_EXTENT_PT`] or holds a number
    /// that isn't finite.
    #[error("math is too large to draw")]
    TooLarge,
}
