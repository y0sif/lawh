//! Core of lawh: the board model, the `.lawh` reader and writer, Vim key notation and the
//! editing engine.
//!
//! The engine has the board grammar (grid and object motions with counts, the operators `d` and
//! `y` with the unnamed register, puts with the place rule, Visual mode, `.` and `@:`, undo and
//! redo, and the `:` command line for shapes, text, math, writing, exporting and quitting), Vim
//! editing inside text and math blocks (Insert and block-Normal mode), and key mappings with
//! settings (`:map` and its family, `:set`), read at start from a lawhrc. This crate has no
//! rendering or windowing code, so the engine is tested as plain functions: keys in, board out.
//! Exports are asked of the app as data ([`export`]): the renderer draws them.
//!
//! Coordinates are grid units (`f64`) with y growing down.

pub mod board;
pub mod engine;
pub mod export;
pub mod format;
pub mod key;

pub use board::{Board, BoardError, Object, ObjectId, Shape};
pub use engine::{Cursor, Effect, Engine, FixedMeasure, Measure, Message, Mode, RcWarning};
pub use export::{ExportBackground, ExportFormat, ExportOptions, ExportReport};
pub use format::{FormatError, from_lawh, to_lawh};
pub use key::{Key, KeyCode, KeyParseError, Mods, parse_keys};
pub use kurbo::{Point, Rect};
