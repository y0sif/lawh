//! Shared helpers for the engine tests: type a key script, snapshot the result.
//!
//! A snapshot is a header line, then the board as a `.lawh` file:
//!
//! ```text
//! // mode=normal cursor=object(c1) keys=10
//! lawh version=1
//! circle id=c1 x=3 y=2 r=2
//! ```
//!
//! The header gives the mode, the cursor (`grid(x,y)` or `object(id)`), how many keys the
//! script typed (one per key in Vim notation: `<CR>` is one) and the message, if any:
//! ` message="..."`.

#![allow(
    dead_code,
    reason = "each test file includes this module and uses a different part of it"
)]

use std::fmt::Write as _;

use lawh_core::engine::{Cursor, Effect, Engine, FixedMeasure, Mode};
use lawh_core::{Board, parse_keys, to_lawh};

/// An engine after a key script.
pub struct Run {
    pub engine: Engine,
    /// Every effect the keys asked for, in order.
    pub effects: Vec<Effect>,
    /// Keys typed, counting each `<...>` as one.
    pub keys: usize,
}

/// Types `script` (Vim notation) into an engine for `board`, measuring with [`FixedMeasure`].
pub fn run(board: Board, script: &str) -> Run {
    let mut run = Run {
        engine: Engine::new(board),
        effects: Vec::new(),
        keys: 0,
    };
    run.feed(script);
    run
}

impl Run {
    /// Types more keys into the same engine.
    pub fn feed(&mut self, script: &str) -> &mut Self {
        let keys = parse_keys(script).unwrap_or_else(|e| panic!("bad key script {script:?}: {e}"));
        self.keys += keys.len();
        for key in keys {
            let effects = self.engine.feed(key, &mut FixedMeasure);
            self.effects.extend(effects);
        }
        self
    }

    /// The header line alone: mode, cursor, keys and message.
    pub fn header(&self) -> String {
        let engine = &self.engine;
        let cursor = match engine.cursor() {
            Cursor::Grid(p) => format!("grid({},{})", num(p.x), num(p.y)),
            Cursor::Object(id) => format!("object({id})"),
        };
        let mut header = format!(
            "// mode={} cursor={cursor} keys={}",
            mode_name(engine.mode()),
            self.keys
        );
        if let Some(message) = engine.message() {
            let _ = write!(header, " message={:?}", message.text);
        }
        header
    }

    /// The header line, then the board as a `.lawh` file.
    pub fn snapshot(&self) -> String {
        format!("{}\n{}", self.header(), to_lawh(self.engine.board()))
    }
}

/// A coordinate as the file writes small numbers: `3`, `-0.5`, and `0` for `-0`.
fn num(v: f64) -> String {
    if v == 0.0 {
        "0".to_owned()
    } else {
        v.to_string()
    }
}

/// The mode in kebab case, from its name: `normal`, `command-line`.
fn mode_name(mode: Mode) -> String {
    let mut name = String::new();
    for (i, c) in format!("{mode:?}").chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            name.push('-');
        }
        name.extend(c.to_lowercase());
    }
    name
}
