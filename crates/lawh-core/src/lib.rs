//! Core of lawh: the board model and the `.lawh` reader and writer.
//!
//! The Vim engine and `:` commands come in milestone 2. This crate has no
//! rendering or windowing code, so they can be tested as plain functions:
//! keys in, board changes out.
//!
//! Coordinates are grid units (`f64`) with y growing down.

pub mod board;
pub mod format;

pub use board::{Board, BoardError, Object, ObjectId, Shape};
pub use format::{FormatError, from_lawh, to_lawh};
pub use kurbo::Point;
