//! What measuring text costs a key when the renderer measures it: laying out a text the
//! renderer has not laid out before (one just edited) counts against the engine's work budget
//! (`lawh_core::engine::MAX_KEY_WORK`, through `Measure::take_work`), so a mapping that edits a
//! long text and measures the board, over and over, stops within the budget.
//!
//! The regression tests bound their time generously, since they run in debug in CI, where this
//! crate builds without optimisation and lays text out several times slower than in release.
//! Two ignored tests measure: `key_costs` times the worst cases, `work_unit_costs` what a unit of
//! measuring work costs:
//!
//! ```text
//! nice -n 19 cargo test --release -p lawh-render --test measure_cost -- \
//!     --ignored --nocapture --test-threads=1
//! ```

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use lawh_core::board::DEFAULT_TEXT_SIZE;
use lawh_core::engine::{Engine, MAX_KEY_WORK};
use lawh_core::{Board, Cursor, Measure, Object, ObjectId, Point, Shape, parse_keys};
use lawh_render::Renderer;

/// The message when a key runs out of work.
const TOO_MUCH_WORK: &str = "that key did too much work; the rest of it was dropped";

/// Characters of the long text the repro cases edit: room for the 1,000 `x`s that the deepest
/// mapping types before [`lawh_core::board::MAX_TEXT_CHARS`].
const LONG: usize = 3_800;

/// A generous bound for one key that runs out of work: the budget is about 0.1 s of measuring
/// in release.
fn bound() -> Duration {
    Duration::from_secs(if cfg!(debug_assertions) { 5 } else { 1 })
}

/// Makes a text of `n` characters: [`latin`] or [`mixed`].
type MakeText = fn(usize) -> String;

/// `n` characters of words.
fn latin(n: usize) -> String {
    "lorem ipsum dolor sit amet "
        .chars()
        .cycle()
        .take(n)
        .collect()
}

/// `n` characters whose script changes at every one (Latin, Greek, Cyrillic, all in the text
/// font): each is a run of its own, the costliest text the text font lays out.
fn mixed(n: usize) -> String {
    "a\u{3B1}\u{430}".chars().cycle().take(n).collect()
}

fn id(s: &str) -> ObjectId {
    ObjectId::new(s).unwrap()
}

/// Text `t1` of `text` at the origin, and rectangle `r1` below it.
fn text_and_rect(text: String) -> Board {
    let mut board = Board::new();
    board
        .insert(Object::new(
            id("t1"),
            Shape::Text {
                at: Point::ZERO,
                text,
                size: DEFAULT_TEXT_SIZE,
            },
        ))
        .unwrap();
    board
        .insert(Object::new(
            id("r1"),
            Shape::Rect {
                origin: Point::new(0.0, 10.0),
                w: 1.0,
                h: 1.0,
            },
        ))
        .unwrap();
    board
}

/// 500 texts of 79 characters, one a unit below the other: 500 characters under the board's
/// text cap, so they can be typed into.
fn texts() -> Board {
    let mut board = Board::new();
    for i in 0..500 {
        board
            .insert(Object::new(
                id(&format!("t{i}")),
                Shape::Text {
                    at: Point::new(0.0, f64::from(i)),
                    text: latin(79),
                    size: DEFAULT_TEXT_SIZE,
                },
            ))
            .unwrap();
    }
    board
}

/// An engine and the renderer that measures for it.
struct Run {
    engine: Engine,
    renderer: Renderer,
}

impl Run {
    /// Reads `rc` as a lawhrc into an engine for `board` (it must load without warnings), and
    /// lays out the board's text, as the app does when it opens a board.
    fn new(rc: &str, board: Board) -> Self {
        let mut renderer = Renderer::new_trusted();
        renderer.board_bounds(&board);
        let mut engine = Engine::new(board);
        let warnings = engine.source(rc);
        assert!(warnings.is_empty(), "{warnings:?}");
        Self { engine, renderer }
    }

    fn feed(&mut self, script: &str) -> &mut Self {
        for key in parse_keys(script).unwrap() {
            self.engine.feed(key, &mut self.renderer);
        }
        self
    }

    /// Types `script` and how long it took.
    fn timed(&mut self, script: &str) -> Duration {
        let started = Instant::now();
        self.feed(script);
        started.elapsed()
    }

    fn message(&self) -> Option<&str> {
        self.engine.message().map(|m| m.text.as_str())
    }

    /// The text of `t1`.
    fn text(&self) -> &str {
        match &self.engine.board().get(&id("t1")).unwrap().shape {
            Shape::Text { text, .. } => text,
            other => panic!("t1 is not a text: {other:?}"),
        }
    }

    /// The `x`s typed at the start of `t1`.
    fn typed(&self) -> usize {
        self.text().chars().take_while(|c| *c == 'x').count()
    }
}

/// `nmap Q {keys}Q` on [`text_and_rect`] of `text`, the cursor on the text: each round edits
/// it, then measures the board.
fn repro(keys: &str, text: String) -> (Run, Duration) {
    let mut r = Run::new(&format!("nmap Q {keys}Q\n"), text_and_rect(text));
    r.feed("gg");
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("t1")));
    let elapsed = r.timed("Q");
    (r, elapsed)
}

/// The key ran out of work after a few rounds: within the bound, with the message, and the
/// next key starting afresh.
fn assert_stopped(r: &mut Run, elapsed: Duration, most_rounds: usize) {
    assert!(elapsed < bound(), "{elapsed:?}");
    assert_eq!(r.message(), Some(TOO_MUCH_WORK));
    assert!(r.engine.key_work() <= MAX_KEY_WORK);
    let rounds = r.typed();
    assert!((1..=most_rounds).contains(&rounds), "{rounds} rounds");
    r.feed("<Esc>");
    assert_eq!(r.message(), None);
}

#[test]
fn a_mapping_that_edits_a_long_text_and_shows_labels_stops_within_the_work_budget() {
    // Before measuring counted: 2.0 s in release with words, 6.4 s with the script changing at
    // every character, stopped only by the mapping depth after 1,000 rounds. Each `s` lays the
    // edited text out again: about 3.6 million units with words, 7.2 million mixed.
    let (mut r, elapsed) = repro("ix<Esc>s<Esc>", latin(LONG));
    assert_stopped(&mut r, elapsed, 40);
    let (mut r, elapsed) = repro("ix<Esc>s<Esc>", mixed(LONG));
    assert_stopped(&mut r, elapsed, 20);
}

#[test]
fn a_mapping_that_edits_a_long_text_and_moves_stops_within_the_work_budget() {
    // Before: 1.1 s in release with words, 3.6 s mixed. `w` lays the text out again, `b` finds
    // it laid out.
    let (mut r, elapsed) = repro("ix<Esc>wb", latin(LONG));
    assert_stopped(&mut r, elapsed, 40);
    let (mut r, elapsed) = repro("ix<Esc>wb", mixed(LONG));
    assert_stopped(&mut r, elapsed, 20);
}

#[test]
fn measuring_text_laid_out_already_costs_only_the_engines_own_count() {
    // The board's text was laid out when it opened: `w` over 500 texts costs what the engine
    // counts for measuring (`MEASURE_WORK`) and putting in order, nothing more.
    let mut r = Run::new("", texts());
    r.feed("gg");
    r.feed("w");
    let warm = r.engine.key_work();
    assert!(warm < MAX_KEY_WORK / 50, "{warm}");
    // An edit lays that text out again on the next measure, estimated at about a microsecond
    // a character (80 here), and only on the next.
    r.feed("ix<Esc>");
    r.feed("w");
    let after_edit = r.engine.key_work();
    r.feed("w");
    let warm_again = r.engine.key_work();
    // The engine counts the character typed too.
    assert!(warm_again <= warm + 100, "{warm_again} {warm}");
    let laid_out = after_edit - warm_again;
    assert!((50_000..200_000).contains(&laid_out), "{laid_out}");
}

#[test]
fn measuring_done_between_keys_is_not_charged_to_the_next_key() {
    // The app measures between keys too (the Visual rectangle, for the overlay): what that lays
    // out is not the next key's work.
    let mut r = Run::new("", text_and_rect(latin(LONG)));
    let unseen = Object::new(
        id("t2"),
        Shape::Text {
            at: Point::ZERO,
            text: mixed(LONG),
            size: DEFAULT_TEXT_SIZE,
        },
    );
    assert!(Measure::bounds(&mut r.renderer, &unseen).is_some());
    r.feed("l");
    assert_eq!(r.engine.key_work(), 0);
}

#[test]
#[ignore = "a measurement: run in release with --nocapture"]
fn key_costs() {
    let cases: [(&str, &str, MakeText); 4] = [
        ("ix<Esc>s<Esc>, words", "ix<Esc>s<Esc>", latin),
        ("ix<Esc>s<Esc>, mixed", "ix<Esc>s<Esc>", mixed),
        ("ix<Esc>wb, words", "ix<Esc>wb", latin),
        ("ix<Esc>wb, mixed", "ix<Esc>wb", mixed),
    ];
    for (name, keys, text) in cases {
        let (r, elapsed) = repro(keys, text(LONG));
        println!(
            "{name}: {elapsed:.1?}, {} rounds, {} units ({})",
            r.typed(),
            r.engine.key_work(),
            r.message().unwrap_or_default()
        );
    }
    let mut r = Run::new(&format!("nmap Q {}Q\n", "w".repeat(255)), texts());
    r.feed("gg");
    let elapsed = r.timed("Q");
    println!(
        "w x255, 500 texts: {elapsed:.1?}, {} units ({})",
        r.engine.key_work(),
        r.message().unwrap_or_default()
    );
}

/// What a unit of measuring work costs: one key that measures a board after an edit, laying the
/// edited text out again. Run in debug and release with `--nocapture`.
#[test]
#[ignore = "a measurement: run with --nocapture"]
fn work_unit_costs() {
    let mut out = String::new();
    let kinds: [(&str, MakeText); 2] = [("words", latin), ("mixed", mixed)];
    for (kind, text) in kinds {
        for n in [100, 1000, LONG] {
            for key in ["s", "w"] {
                let mut r = Run::new("", text_and_rect(text(n)));
                r.feed("ggix<Esc>");
                let elapsed = r.timed(key);
                let work = r.engine.key_work();
                let per_unit = elapsed.as_secs_f64() * 1e9 / work.max(1) as f64;
                writeln!(
                    out,
                    "{key} after an edit of {n} {kind}: {elapsed:.1?}, {work} units, \
                     {per_unit:.2} ns a unit"
                )
                .unwrap();
            }
        }
    }
    let mut r = Run::new("", texts());
    r.feed("gg");
    let elapsed = r.timed("w");
    let work = r.engine.key_work();
    let per_unit = elapsed.as_secs_f64() * 1e9 / work.max(1) as f64;
    writeln!(
        out,
        "w over 500 texts laid out: {elapsed:.1?}, {work} units, {per_unit:.2} ns a unit"
    )
    .unwrap();
    print!("{out}");
}
