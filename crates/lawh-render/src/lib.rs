//! Rendering for lawh: draws a board to pixels for the window and exports it
//! to PNG, SVG and PDF ([`Renderer::export`]).
//!
//! [`Renderer`] draws a [`lawh_core::Board`] through a [`Viewport`] with
//! `vello_cpu`, headless and on the CPU. Text is laid out by parley with the
//! bundled Inter font, and Libertinus Math for the characters Inter lacks (such
//! as the symbols `:sym` puts into text), and math by [`lawh_math::MathEngine`];
//! no system font is ever loaded, so a board looks the same everywhere. Glyphs
//! are drawn as unhinted outlines, so they stay crisp at any zoom.
//!
//! A frame never waits for math: a formula still compiling is drawn as its
//! source in grey until the engine's waker asks for another frame, and only
//! formulas on screen are asked for. Exports ([`Renderer::export`], and
//! [`Renderer::render_png`] of a view) wait for every formula, so they are the
//! same every time. An export frames the whole board (or a selection) at the
//! window's zoom-1 sizes, within caps of its own ([`EXPORT_MATH_WAIT`],
//! [`MAX_EXPORT_PIXELS`], [`MAX_PDF_PAGE_PT`], [`MAX_EXPORT_BYTES`], ...). An
//! SVG is only geometry: glyphs as outlines from the bundled fonts, no board
//! text in the file. A PDF is one page with board text as real text (the
//! bundled fonts embedded as subsets, so it can be searched and copied) and
//! math as paths.
//!
//! The window and every export draw from one draw list: each object's
//! outlines, arrowheads, glyph runs and math paths, built by the same
//! builders, in grid units for an export and in screen pixels for the window
//! (whose outlines and arrowheads keep their size on screen at every zoom).
//! Every colour comes from one [`Palette`].
//!
//! A frame's work is budgeted, so a hostile board can't freeze the window:
//! each object's cost is estimated before it is drawn, and objects that don't
//! fit are drawn as faint outlines of their bounds ([`FrameStats`]). Text is
//! laid out once and cached ([`MAX_TEXT_CACHE_BYTES`]), with what its glyphs
//! cost to draw, so a frame estimates a text without walking its layout.
//!
//! While editing, [`Renderer::render_with`] draws an [`Overlay`] over the board: the cursor, a
//! Visual selection, the caret in a text block, jump labels, `:sym` completions and the status
//! bar. The renderer also measures objects for the engine (`lawh_core::engine::Measure`), as it
//! draws them, sizes included, and reports what laying out text it had not laid out before cost,
//! which counts against the engine's work budget for the key.
//!
//! Text and math have a size (`lawh_core::board::MIN_TEXT_SIZE` to `MAX_TEXT_SIZE`), a
//! multiple of [`TEXT_SIZE_UNITS`]. Math is drawn about 1.27 times as large as text of the
//! same size, so the two have the same x-height (Libertinus Math's is smaller than Inter's), and
//! a formula's first baseline sits on the baseline of a text at the same `y`.
//!
//! Board coordinates are grid units with y down. At zoom 1 and scale factor 1
//! one unit is [`BASE_PX_PER_UNIT`] pixels.

mod budget;
mod draw;
mod error;
mod export;
mod glyphs;
mod overlay;
mod palette;
mod renderer;
mod scene;
mod status;
mod text;
mod viewport;

pub use error::RenderError;
#[doc(hidden)]
pub use export::ExportLimits;
pub use export::{
    EXPORT_ARROW_HEAD_UNITS, EXPORT_MARGIN_UNITS, EXPORT_MATH_WAIT, EXPORT_STROKE_UNITS,
    ExportError, Exported, MAX_EXPORT_BYTES, MAX_EXPORT_PIXELS, MAX_EXPORT_SIDE_PX,
    MAX_PDF_PAGE_PT, PDF_PT_PER_UNIT,
};
pub use overlay::{Caret, Overlay, OverlayCursor, StatusLine, status_bar_height_px};
pub use palette::Palette;
pub use renderer::{FrameStats, Renderer};
#[doc(hidden)]
pub use scene::{SceneItem, SceneObject};
pub use text::MAX_TEXT_CACHE_BYTES;
pub use viewport::Viewport;

/// Pixels per grid unit at zoom 1 and scale factor 1.
pub const BASE_PX_PER_UNIT: f64 = 40.0;

/// Em size of text and math of size 1, in grid units. Text of size `s` has an em `s` times as
/// large; math of size `s` is laid out at the same em and drawn about 1.27 times larger still,
/// to match the text's x-height.
pub const TEXT_SIZE_UNITS: f64 = 0.5;
