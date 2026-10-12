//! Rendering for lawh: draws a board to pixels for the window and exports it
//! to PNG (SVG and PDF come later).
//!
//! [`Renderer`] draws a [`lawh_core::Board`] through a [`Viewport`] with
//! `vello_cpu`, headless and on the CPU. Text is laid out by parley with the
//! bundled Inter font and math by [`lawh_math::MathEngine`]; no system font is
//! ever loaded, so a board looks the same everywhere. Glyphs are drawn as
//! unhinted outlines, so they stay crisp at any zoom.
//!
//! A frame never waits for math: a formula still compiling is drawn as its
//! source in grey until the engine's waker asks for another frame, and only
//! formulas on screen are asked for. Exports ([`Renderer::render_png`]) wait
//! for every formula, so they are the same every time.
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
//! draws them, sizes included.
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
mod glyphs;
mod overlay;
mod renderer;
mod status;
mod text;
mod viewport;

pub use error::RenderError;
pub use overlay::{Caret, Overlay, OverlayCursor, StatusLine, status_bar_height_px};
pub use renderer::{FrameStats, Renderer};
pub use text::MAX_TEXT_CACHE_BYTES;
pub use viewport::Viewport;

/// Pixels per grid unit at zoom 1 and scale factor 1.
pub const BASE_PX_PER_UNIT: f64 = 40.0;

/// Em size of text and math of size 1, in grid units. Text of size `s` has an em `s` times as
/// large; math of size `s` is laid out at the same em and drawn about 1.27 times larger still,
/// to match the text's x-height.
pub const TEXT_SIZE_UNITS: f64 = 0.5;
