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
//! script typed (one per key in Vim notation: `<CR>` is one), the Visual selection, if any
//! (` selection=c1,t1`), the cursor's byte offset in the text block being edited, in Insert and
//! block-Normal mode (` byte=3`), and the message, if any: ` message="..."`.
//!
//! In a key script, `<Timeout>` stands for the mapping timeout: the harness calls
//! [`Engine::timeout`] there. It is not a key and counts as none.

#![allow(
    dead_code,
    reason = "each test file includes this module and uses a different part of it"
)]

use std::fmt::Write as _;

use lawh_core::engine::{Cursor, Effect, Engine, FixedMeasure, Mode};
use lawh_core::key::format_keys;
use lawh_core::{Board, ObjectId, Point, from_lawh, parse_keys, to_lawh};

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
    run_engine(Engine::new(board), script)
}

/// Where a key script calls [`Engine::timeout`].
pub const TIMEOUT: &str = "<Timeout>";

/// Reads `rc` as a lawhrc into an engine for `board`, then types `script` as [`run`] does. The
/// rc must load without warnings.
pub fn run_with_rc(rc: &str, board: Board, script: &str) -> Run {
    let mut engine = Engine::new(board);
    let warnings = engine.source(rc);
    assert!(warnings.is_empty(), "lawhrc warnings: {warnings:#?}");
    run_engine(engine, script)
}

/// Types `script` into `engine`, as [`run`] does.
pub fn run_engine(engine: Engine, script: &str) -> Run {
    let mut run = Run {
        engine,
        effects: Vec::new(),
        keys: 0,
    };
    run.feed(script);
    run
}

/// A board from the lines of a `.lawh` file after its header.
pub fn board(objects: &str) -> Board {
    let src = format!("lawh version=1\n{objects}");
    from_lawh(&src).unwrap_or_else(|e| panic!("bad board {objects:?}: {e}"))
}

/// Types `script` one key at a time into an engine for `board`: one line per key, the key
/// and the header after it. The board is not shown.
pub fn trace(board: Board, script: &str) -> String {
    let mut run = run(board, "");
    let mut lines = Vec::new();
    for (i, part) in script.split(TIMEOUT).enumerate() {
        if i > 0 {
            run.feed(TIMEOUT);
            lines.push(format!("{TIMEOUT} {}", run.header()));
        }
        for key in parse_keys(part).unwrap_or_else(|e| panic!("bad key script {part:?}: {e}")) {
            let key = format_keys(&[key]);
            run.feed(&key);
            lines.push(format!("{key:>5} {}", run.header()));
        }
    }
    lines.join("\n")
}

impl Run {
    /// Types more keys into the same engine, calling [`Engine::timeout`] at each `<Timeout>`.
    pub fn feed(&mut self, script: &str) -> &mut Self {
        for (i, part) in script.split(TIMEOUT).enumerate() {
            if i > 0 {
                let effects = self.engine.timeout(&mut FixedMeasure);
                self.effects.extend(effects);
            }
            let keys = parse_keys(part).unwrap_or_else(|e| panic!("bad key script {part:?}: {e}"));
            self.keys += keys.len();
            for key in keys {
                let effects = self.engine.feed(key, &mut FixedMeasure);
                self.effects.extend(effects);
            }
        }
        self
    }

    /// Tells the engine the app wrote `path` (or failed to), keeping the effects it returns. The
    /// app found `path` to be another file than the board's own, or did not look.
    pub fn write_done(&mut self, path: &str, result: Result<(), &str>) -> &mut Self {
        let effects = self
            .engine
            .write_done(path, result.map_err(str::to_owned), false);
        self.effects.extend(effects);
        self
    }

    /// [`Run::write_done`] for a write the app found to be to the board's own file, whatever
    /// its path says (a link to it, say).
    pub fn write_done_same_file(&mut self, path: &str, result: Result<(), &str>) -> &mut Self {
        let effects = self
            .engine
            .write_done(path, result.map_err(str::to_owned), true);
        self.effects.extend(effects);
        self
    }

    /// Clicks at `(x, y)`, on object `hit` if any.
    pub fn click(&mut self, x: f64, y: f64, hit: Option<&str>) -> &mut Self {
        let hit = hit.map(|id| ObjectId::new(id).unwrap());
        self.engine.click(Point::new(x, y), hit, &mut FixedMeasure);
        self
    }

    /// The header line alone: mode, cursor, keys, selection and message.
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
        if !engine.selection().is_empty() {
            let ids: Vec<&str> = engine.selection().iter().map(ObjectId::as_str).collect();
            let _ = write!(header, " selection={}", ids.join(","));
        }
        if let Some(text) = engine.text_cursor() {
            let _ = write!(header, " byte={}", text.byte);
        }
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
