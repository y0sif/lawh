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
//! fit are drawn as faint outlines of their bounds ([`FrameStats`]).
//!
//! Board coordinates are grid units with y down. At zoom 1 and scale factor 1
//! one unit is [`BASE_PX_PER_UNIT`] pixels.

mod budget;
mod draw;
mod error;
mod glyphs;
mod renderer;
mod text;
mod viewport;

pub use error::RenderError;
pub use renderer::{FrameStats, Renderer};
pub use viewport::Viewport;

/// Pixels per grid unit at zoom 1 and scale factor 1.
pub const BASE_PX_PER_UNIT: f64 = 40.0;

/// Em size of text and math, in grid units.
pub const TEXT_SIZE_UNITS: f64 = 0.5;
