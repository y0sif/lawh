//! Core of lawh: the board model, the `.lawh` reader and writer, Vim key notation and the
//! editing engine.
//!
//! The engine so far has Normal mode with grid motions (`hjkl` and counts), the `:` command
//! line with `:circle`, `:undo` and `:redo`, and undo and redo (`u`, `<C-r>`). More of the Vim
//! grammar comes during milestone 2. This crate has no rendering or windowing code, so the
//! engine is tested as plain functions: keys in, board out.
//!
//! Coordinates are grid units (`f64`) with y growing down.

pub mod board;
pub mod engine;
pub mod format;
pub mod key;

pub use board::{Board, BoardError, Object, ObjectId, Shape};
pub use engine::{Cursor, Effect, Engine, FixedMeasure, Measure, Message, Mode};
pub use format::{FormatError, from_lawh, to_lawh};
pub use key::{Key, KeyCode, KeyParseError, Mods, parse_keys};
pub use kurbo::{Point, Rect};
