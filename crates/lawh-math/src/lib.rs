//! Math for lawh: compiles Typst math snippets to vector paths the renderer
//! can draw, and finds symbols by name for `:sym`.
//!
//! Typst runs in a sandbox (see `SECURITY.md`): the only file it can see is
//! the formula itself, fonts come from [`fonts`], there is no clock and no
//! package download, and `plugin` (WebAssembly), `cite` and `bibliography`
//! (CSL parsing) are removed from the library. Each compile runs on its own
//! thread with a time limit, and [`MathEngine`] caches every outcome.

mod engine;
mod fonts;
mod layout;
mod world;

pub use engine::{DEFAULT_TIMEOUT, MAX_WORKERS, MathEngine};
pub use fonts::{FontAsset, fonts};

/// Font size of math, in Typst pt. [`MathLayout`] coordinates are at this
/// size; the renderer scales them to the board's text size.
pub const MATH_FONT_SIZE_PT: f64 = 20.0;

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
    /// The compile took longer than the engine's timeout, or too many
    /// earlier compiles are still running.
    #[error("math took too long to compile")]
    Timeout,
    /// The compiler crashed.
    #[error("math compiler crashed")]
    Panicked,
}
