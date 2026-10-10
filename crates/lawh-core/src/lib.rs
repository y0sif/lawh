//! Core of lawh: the board model, the Vim-style modal engine, ex-commands
//! and the plain-text `.lawh` file format.
//!
//! This crate has no rendering or windowing dependencies, so the engine can be
//! tested as plain functions: keys in, board changes out.
