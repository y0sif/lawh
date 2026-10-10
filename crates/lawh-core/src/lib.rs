//! Core of lawh: the board model, the Vim-style modal engine, ex-commands
//! and the plain-text `.lawh` file format.
//!
//! This crate has no rendering or windowing dependencies, so the engine can be
//! tested as plain functions: keys in, board changes out.
//!
//! Coordinates are grid units (`f64`) with y growing down.

pub mod board;
pub mod format;

pub use board::{Board, BoardError, Object, ObjectId, Shape};
pub use format::{FormatError, from_lawh, to_lawh};
pub use kurbo::Point;
