//! Core of lawh: the board model, the `.lawh` reader and writer, Vim key notation and the
//! editing engine.
//!
//! The engine so far has the board grammar: grid and object motions with counts, the operators
//! `d` and `y` and the unnamed register, puts with the place rule, Visual mode, `.` and `@:`,
//! undo and redo, and the `:` command line (shapes, text, math, write and quit). Text editing
//! inside blocks and key mappings come next during milestone 2. This crate has no rendering or
//! windowing code, so the engine is tested as plain functions: keys in, board out.
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
