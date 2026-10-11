//! What one typed key may cost: keys that mappings, `.` and counts multiply into a great deal of
//! work stop within the engine's work budget ([`MAX_KEY_WORK`]), and the costliest single
//! commands on a full board fit in it.
//!
//! The regression tests bound their time generously, since they run in debug in CI: each took
//! seconds to minutes in release before the budget, and takes about 0.1 s in debug now. Each
//! also checks where the work stopped. Two ignored tests measure: `key_costs` times the worst
//! cases, `work_unit_costs` what a unit of work costs for each kind of work:
//!
//! ```text
//! nice -n 19 cargo test --release -p lawh-core --test engine_cost -- --ignored --nocapture --test-threads=1
//! ```

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use common::{Run, board, run, run_with_rc};
use lawh_core::engine::{Engine, MAX_EXPANDED_KEYS, MAX_KEY_WORK, MAX_RC_BYTES};
use lawh_core::{Board, Cursor, ObjectId, Shape};

/// The message when a key runs out of work.
const TOO_MUCH_WORK: &str = "that key did too much work; the rest of it was dropped";

/// A generous bound for one key that runs out of work: about 0.1 s in debug.
fn bound() -> Duration {
    Duration::from_secs(if cfg!(debug_assertions) { 5 } else { 1 })
}

fn message(r: &Run) -> Option<&str> {
    r.engine.message().map(|m| m.text.as_str())
}

fn id(s: &str) -> ObjectId {
    ObjectId::new(s).unwrap()
}

/// The text of object `name`.
fn text(r: &Run, name: &str) -> String {
    match &r.engine.board().get(&id(name)).unwrap().shape {
        Shape::Text { text, .. } => text.clone(),
        other => panic!("{name} is not a text: {other:?}"),
    }
}

/// The key ran out of work: within the budget, with the message, the typeahead dropped, and
/// the next key starting afresh.
fn assert_stopped(r: &mut Run, elapsed: Duration) {
    assert!(elapsed < bound(), "{elapsed:?}");
    assert_eq!(message(r), Some(TOO_MUCH_WORK));
    assert!(r.engine.key_work() <= MAX_KEY_WORK);
    assert_eq!(r.engine.pending(), "");
    r.feed("<Esc>");
    assert_eq!(message(r), None);
}

/// 500 rectangles of one unit in a column, two units apart: `r0` at the top.
fn column() -> Board {
    let mut objects = String::new();
    for i in 0..500 {
        writeln!(objects, "rect id=r{i} x=0 y={} w=1 h=1", 2 * i).unwrap();
    }
    board(&objects)
}

/// One text of 5,000 regional indicators (U+1F1E6): 2,500 flags on one line.
fn flags() -> Board {
    board(&format!(
        "text id=t1 x=0 y=0 \"{}\"\n",
        "\u{1F1E6}".repeat(5000)
    ))
}

/// 500 texts of 80 characters, one a unit below the other: the board's text cap.
fn texts() -> Board {
    let mut objects = String::new();
    let line = "lorem ipsum ".repeat(7);
    let line = &line[..80];
    for i in 0..500 {
        writeln!(objects, "text id=t{i} x=0 y={i} \"{line}\"").unwrap();
    }
    board(&objects)
}

/// 499 circles, `c1` to `c499`: the next circle's id is the 500th tried.
fn circles() -> Board {
    let mut objects = String::new();
    for i in 1..500 {
        writeln!(objects, "circle id=c{i} x=0 y={i} r=0.4").unwrap();
    }
    board(&objects)
}

/// One text of 5,000 characters of two-letter words on one line.
fn words() -> Board {
    board(&format!("text id=t1 x=0 y=0 \"{}\"\n", "ab ".repeat(1666)))
}

/// One text of two lines of 2,499 characters.
fn two_lines() -> Board {
    let line = "x".repeat(2499);
    board(&format!("text id=t1 x=0 y=0 \"{line}\\n{line}\"\n"))
}

/// `nmap Q {keys repeated n times}Q`: a mapping that runs `keys` `n` times, then itself.
fn rc_repeating(keys: &str, n: usize) -> String {
    format!("nmap Q {}Q\n", keys.repeat(n))
}

/// Runs `script` on `board` with `rc`, and how long it took.
fn timed(rc: &str, board: Board, script: &str) -> (Run, Duration) {
    let started = Instant::now();
    let r = run_with_rc(rc, board, script);
    (r, started.elapsed())
}

/// A worst case: its name, and the run to time.
type Case = (&'static str, Box<dyn Fn() -> (Run, Duration)>);

/// Each worst case.
fn cases() -> Vec<Case> {
    vec![
        (
            "10000J10000K x20, 500-rect column",
            Box::new(|| timed(&rc_repeating("10000J10000K", 20), column(), "ggQ")),
        ),
        (
            "$10000h x36, 5000 regional indicators",
            Box::new(|| timed(&rc_repeating("$10000h", 36), flags(), "gg<CR>Q")),
        ),
        (
            "10000u10000<C-r>, ~250 steps of 500 objects",
            Box::new(|| {
                let script = format!("gg{}", "dGp".repeat(200));
                let mut r = run_with_rc("nmap Q 10000u10000<C-r>Q\n", column(), &script);
                let started = Instant::now();
                r.feed("Q");
                (r, started.elapsed())
            }),
        ),
        (
            "w x255, 500 texts",
            Box::new(|| timed(&rc_repeating("w", 255), texts(), "ggQ")),
        ),
        (
            "p x255 after ggyG, 500 texts",
            Box::new(|| {
                let mut r = run_with_rc(&rc_repeating("p", 255), texts(), "ggyG");
                let started = Instant::now();
                r.feed("Q");
                (r, started.elapsed())
            }),
        ),
        (
            "yG x127, 500 texts",
            Box::new(|| timed(&rc_repeating("yG", 127), texts(), "ggQ")),
        ),
        (
            "10000u alone, ~250 steps of 500 objects",
            Box::new(|| {
                let script = format!("gg{}", "dGp".repeat(200));
                let mut r = run_with_rc("", column(), &script);
                let started = Instant::now();
                r.feed("10000u");
                (r, started.elapsed())
            }),
        ),
        (
            "$10000h alone, 5000 regional indicators",
            Box::new(|| {
                let mut r = run_with_rc("", flags(), "gg<CR>$");
                let started = Instant::now();
                r.feed("10000h");
                (r, started.elapsed())
            }),
        ),
        (
            "10000A<Left><Right><Esc>, 5000 regional indicators",
            Box::new(|| {
                let mut r = run_with_rc("", flags(), "gg10000A<Left><Right>");
                let started = Instant::now();
                r.feed("<Esc>");
                (r, started.elapsed())
            }),
        ),
        (
            "lawhrc of 1 MiB, map and unmap at the front of 999 mappings",
            Box::new(|| {
                let rc = crafted_rc();
                let mut engine = Engine::new(Board::new());
                let started = Instant::now();
                let warnings = engine.source(&rc);
                let elapsed = started.elapsed();
                assert!(warnings.is_empty(), "{warnings:?}");
                (common::run_engine(engine, ""), elapsed)
            }),
        ),
    ]
}

/// A lawhrc of nearly [`MAX_RC_BYTES`]: 999 mappings in the Normal, Visual and
/// operator-pending modes, then `map ! x` and `unmap !` over and over. `!` sorts before every
/// other key mapped, so each makes room at the front of all three modes' mappings, or closes it.
fn crafted_rc() -> String {
    let mut rc = String::new();
    for i in 0..999 {
        let c = char::from_u32(0x100 + i).unwrap();
        writeln!(rc, "map {c} h").unwrap();
    }
    while rc.len() + 20 < MAX_RC_BYTES {
        rc.push_str("map ! x\nunmap !\n");
    }
    rc
}

// Mappings that multiply work: each stops within the budget.

#[test]
fn a_mapping_that_walks_a_column_stops_within_the_work_budget() {
    // Before the budget: 7.6 s in release.
    let (mut r, elapsed) = timed(&rc_repeating("10000J10000K", 20), column(), "ggQ");
    // Each `10000J` walks to the bottom: it costs a quarter of the budget or so.
    assert!(matches!(r.engine.cursor(), Cursor::Object(_)));
    assert_stopped(&mut r, elapsed);
    r.feed("gg");
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("r0")));
}

#[test]
fn a_mapping_that_walks_flags_left_stops_within_the_work_budget() {
    // Before the budget (and the walk left through the line once): 227 s in release.
    let (mut r, elapsed) = timed(&rc_repeating("$10000h", 36), flags(), "gg<CR>Q");
    assert_eq!(r.engine.text_cursor().map(|c| c.byte), Some(0));
    assert_stopped(&mut r, elapsed);
}

#[test]
fn a_mapping_that_undoes_and_redoes_a_deep_history_stops_within_the_work_budget() {
    // 120 steps that each take the whole column off or put it back. Before the budget and the
    // batches, the same with 250 steps took 858 s in release.
    let start = column();
    let script = format!("gg{}", "dGp".repeat(60));
    let mut r = run_with_rc("nmap Q 10000u10000<C-r>Q\n", start.clone(), &script);
    let started = Instant::now();
    r.feed("Q");
    let elapsed = started.elapsed();
    // The undo stopped between two steps: the board is whole or empty.
    assert!(matches!(r.engine.board().len(), 0 | 500));
    assert_stopped(&mut r, elapsed);
    // The history is intact: undoing all of it, a key at a time, gives the column back.
    while message(&r) != Some("already at the oldest change") {
        r.feed("10000u");
    }
    assert_eq!(r.engine.board(), &start);
}

#[test]
fn a_mapping_of_w_over_a_full_board_stops_within_the_work_budget() {
    // Before the budget: 2.2 s in release.
    let (mut r, elapsed) = timed(&rc_repeating("w", 255), texts(), "ggQ");
    // About a hundred `w` fit: each measures 40,000 characters of text.
    let Cursor::Object(at) = r.engine.cursor().clone() else {
        panic!("{:?}", r.engine.cursor());
    };
    let n: usize = at.as_str()[1..].parse().unwrap();
    assert!((50..250).contains(&n), "{at}");
    assert_stopped(&mut r, elapsed);
}

#[test]
fn a_put_refused_for_room_does_no_work() {
    // The board is full, so every `p` is refused before the register is copied: the mapping
    // runs until the keys run out, not the work. Before: 1.8 s in release.
    let mut r = run_with_rc(&rc_repeating("p", 255), texts(), "ggyG");
    let started = Instant::now();
    r.feed("Q");
    let elapsed = started.elapsed();
    assert!(elapsed < bound(), "{elapsed:?}");
    assert_eq!(r.engine.key_work(), 0);
    assert_eq!(
        message(&r).unwrap(),
        format!(
            "one key may set off at most {MAX_EXPANDED_KEYS} keys through mappings and `.`; \
             the rest were dropped"
        )
    );
    assert_eq!(r.engine.board().len(), 500);
}

#[test]
fn a_mapping_of_yanks_stops_within_the_work_budget() {
    // Before the budget: 2.7 s in release.
    let (mut r, elapsed) = timed(&rc_repeating("yG", 127), texts(), "ggQ");
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("t0")));
    assert_stopped(&mut r, elapsed);
    // The register holds the last whole yank.
    r.feed("dd");
    r.feed("u");
    assert_eq!(r.engine.board(), &texts());
}

#[test]
fn a_mapping_that_types_into_a_full_text_stops_within_the_work_budget() {
    // Every `b` is refused, the text being at its cap, but a refused edit still copies and
    // counts the text: 30,000 of them took 3 s in debug.
    let full = "a".repeat(5000);
    let board = board(&format!("text id=t1 x=0 y=0 \"{full}\"\n"));
    let mut r = run_with_rc(&format!("inoremap Q {}Q\n", "b".repeat(255)), board, "ggA");
    let started = Instant::now();
    r.feed("Q");
    let elapsed = started.elapsed();
    assert_eq!(text(&r, "t1"), full);
    assert_eq!(r.engine.mode(), lawh_core::Mode::Insert);
    assert_stopped(&mut r, elapsed);
}

#[test]
fn a_dot_cut_short_ends_its_insert_as_one_undo_step_and_keeps_the_change() {
    // `.` types 2,000 characters at the start of a text of 2,000 or more, then `u` takes them
    // back: each round copies the text 2,000 times, so the work runs out before the keys do.
    let long = "a".repeat(2000);
    let mut r = run_with_rc("nmap Q .uQ\n", Board::new(), &format!("i{long}<Esc>"));
    let started = Instant::now();
    r.feed("Q");
    let elapsed = started.elapsed();
    // The last `.` stopped part way and was not undone.
    let typed = text(&r, "t1").len();
    assert!(typed > 2000 && typed < 4000, "{typed}");
    assert_eq!(r.engine.mode(), lawh_core::Mode::Normal);
    assert_stopped(&mut r, elapsed);
    // What it typed is one undo step, and `.` still types the whole change.
    r.feed("u");
    assert_eq!(text(&r, "t1"), long);
    r.feed(".");
    assert_eq!(text(&r, "t1"), long.repeat(2));
}

#[test]
fn a_count_on_an_insert_that_changes_nothing_types_its_keys_once_more() {
    // `<Left><Right>` leaves the block as it was, so 9,999 more times would too. Before: the
    // `<Esc>` typed them all, 187 ms in release, each `<Left>` reading the whole line twice.
    let mut r = run(flags(), "gg10000A<Left><Right>");
    let started = Instant::now();
    r.feed("<Esc>");
    let elapsed = started.elapsed();
    assert!(elapsed < bound(), "{elapsed:?}");
    assert_eq!(message(&r), None);
    // One more time: two keys, each well under a tenth of the budget.
    assert!(
        r.engine.key_work() < MAX_KEY_WORK / 10,
        "{}",
        r.engine.key_work()
    );
    assert_eq!(text(&r, "t1"), "\u{1F1E6}".repeat(5000));
    // Typing does go on: a count on a key that types repeats it.
    let r = run(Board::new(), "3ia<Left><Right><Esc>");
    assert_eq!(text(&r, "t1"), "aaa");
}

// The costliest single commands fit in the budget whole.

#[test]
fn the_costliest_single_commands_fit_in_the_work_budget() {
    let check = |r: &Run| {
        assert_eq!(message(r), None);
        assert!(r.engine.key_work() <= MAX_KEY_WORK);
    };
    // Down the whole column, one step at a time.
    let r = run(column(), "gg10000J");
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("r499")));
    check(&r);
    // The whole board of text deleted, put back, and both undone.
    let mut r = run(texts(), "ggdG");
    check(&r);
    r.feed("p");
    check(&r);
    assert_eq!(r.engine.board(), &texts());
    r.feed("u");
    check(&r);
    assert!(r.engine.board().is_empty());
    r.feed("u");
    check(&r);
    assert_eq!(r.engine.board(), &texts());
    // Forty steps of the whole column undone at once.
    let mut r = run(column(), &format!("gg{}", "dGp".repeat(20)));
    r.feed("40u");
    check(&r);
    assert_eq!(r.engine.board(), &column());
    // `.` of an insert of 5,000 characters.
    let mut r = run(Board::new(), &format!("i{}<Esc>j", "b".repeat(5000)));
    r.feed(".");
    check(&r);
    assert_eq!(text(&r, "t2"), "b".repeat(5000));
}

#[test]
fn undoing_steps_of_a_whole_board_is_linear() {
    // Undo applies a step's edits as one batch: the board's index is built once a step, not
    // once an edit. Before: 1.7 ms a step in release, far more in debug; now about 1 ms a step
    // in debug.
    let start = column();
    let mut r = run(start.clone(), &format!("gg{}", "dGp".repeat(20)));
    let started = Instant::now();
    r.feed("40u");
    r.feed("40<C-r>");
    let elapsed = started.elapsed();
    assert!(elapsed < bound(), "{elapsed:?}");
    assert_eq!(r.engine.board(), &start);
    r.feed("40u");
    assert_eq!(r.engine.board(), &start);
    assert!(!r.engine.is_modified());
}

#[test]
fn a_lawhrc_of_mappings_made_and_dropped_at_the_front_loads_in_time() {
    // The measured worst case of `MAX_MAPS` (see its doc): 675 ms in release.
    let rc = crafted_rc();
    let mut engine = Engine::new(Board::new());
    let started = Instant::now();
    let warnings = engine.source(&rc);
    let elapsed = started.elapsed();
    assert!(warnings.is_empty(), "{warnings:?}");
    let limit = Duration::from_secs(if cfg!(debug_assertions) { 20 } else { 5 });
    assert!(elapsed < limit, "{elapsed:?}");
}

#[test]
#[ignore = "a measurement: run in release with --nocapture"]
fn key_costs() {
    let only = std::env::var("LAWH_COST_KINDS").unwrap_or_default();
    for (name, case) in cases() {
        if !only.is_empty() && !only.split(',').any(|kind| name.starts_with(kind)) {
            continue;
        }
        let (r, elapsed) = case();
        let message = r
            .engine
            .message()
            .map(|m| m.text.clone())
            .unwrap_or_default();
        println!("{name}: {elapsed:.1?} ({message})");
    }
}

/// Single commands at their costliest on a full board, each as one key: what each unit of work
/// costs (see [`MAX_KEY_WORK`]). Run in debug and release with `--nocapture`.
#[test]
#[ignore = "a measurement: run with --nocapture"]
#[expect(clippy::too_many_lines, reason = "a table of cases, one a row")]
fn work_unit_costs() {
    let half = || {
        let mut objects = String::new();
        for i in 0..250 {
            writeln!(objects, "rect id=r{i} x=0 y={} w=1 h=1", 1.6 * f64::from(i)).unwrap();
        }
        board(&objects)
    };
    let undo_steps = format!("gg{}", "dGp".repeat(200));
    let long_insert = format!("i{}<Esc>", "a".repeat(5000));
    let cases: Vec<(&str, Board, String, &str)> = vec![
        (
            "10000J down a column of 500",
            column(),
            "gg".into(),
            "10000J",
        ),
        ("w over 500 texts", texts(), "gg".into(), "w"),
        ("G over 500 texts", texts(), "gg".into(), "G"),
        ("yG of 500 texts", texts(), "gg".into(), "yG"),
        ("dG of 500 texts", texts(), "gg".into(), "dG"),
        (
            "10000u of ~250 steps of 500",
            column(),
            undo_steps.clone(),
            "10000u",
        ),
        (
            "p of 250 under a column of 250",
            half(),
            "ggyGgg".into(),
            "p",
        ),
        (
            "<Esc> after 10000ia",
            Board::new(),
            "10000ia".into(),
            "<Esc>",
        ),
        (
            "<Esc> after 10000i and a flag half",
            Board::new(),
            "10000i\u{1F1E6}".into(),
            "<Esc>",
        ),
        (
            "10000h at the end of 5000 flag halves",
            flags(),
            "gg<CR>$".into(),
            "10000h",
        ),
        (
            "10000b at the end of 5000 flag halves",
            flags(),
            "gg<CR>$".into(),
            "10000b",
        ),
        ("x in 5000 flag halves", flags(), "gg<CR>".into(), "x"),
        (
            ". of a 5000-character insert",
            Board::new(),
            long_insert,
            "j.",
        ),
        (
            "10000l along 5000 characters of words",
            words(),
            "gg<CR>".into(),
            "10000l",
        ),
        (
            "10000w along 5000 characters of words",
            words(),
            "gg<CR>".into(),
            "10000w",
        ),
        (
            "10000b back along 5000 characters of words",
            words(),
            "gg<CR>$".into(),
            "10000b",
        ),
        (
            "$ on 5000 characters of words",
            words(),
            "gg<CR>".into(),
            "$",
        ),
        (
            "j between two lines of 2500 characters",
            two_lines(),
            "gg<CR>$".into(),
            "j",
        ),
        (
            "<Left> at the end of 5000 characters of words",
            words(),
            "ggA".into(),
            "<Left>",
        ),
        (
            "<End> at the start of 5000 characters of words",
            words(),
            "ggI".into(),
            "<End>",
        ),
        (
            "h at the end of 5000 flag halves",
            flags(),
            "gg<CR>$".into(),
            "h",
        ),
        ("$ on 5000 flag halves", flags(), "gg<CR>".into(), "$"),
        (
            "Q: 30,000 l through a mapping",
            Board::new(),
            String::new(),
            "Q",
        ),
        ("w over 500 rectangles", column(), "gg".into(), "w"),
        ("o beside 500 texts, refused", texts(), "gg".into(), "o"),
        (
            "<Esc> after A on 5000 flag halves",
            flags(),
            "ggA".into(),
            "<Esc>",
        ),
        (
            "u in a block of 5000 flag halves",
            flags(),
            "gg<CR>$x".into(),
            "u",
        ),
        (
            "x at the end of 5000 flag halves",
            flags(),
            "gg<CR>$".into(),
            "x",
        ),
        (
            "10000l along 5000 flag halves",
            flags(),
            "gg<CR>".into(),
            "10000l",
        ),
        (
            "10000w along 5000 flag halves",
            flags(),
            "gg<CR>".into(),
            "10000w",
        ),
        (
            "<Left> at the end of 5000 flag halves",
            flags(),
            "ggA".into(),
            "<Left>",
        ),
        (
            "<BS> at the end of 5000 flag halves",
            flags(),
            "ggA".into(),
            "<BS>",
        ),
        (
            "<Up> between two lines of 2,499 characters",
            two_lines(),
            "ggA".into(),
            "<Up>",
        ),
        (
            ":ci past 499 circles for a fresh id",
            circles(),
            ":ci 1".into(),
            "<CR>",
        ),
        (
            "@: of a :te of 5000 characters",
            Board::new(),
            format!(":te {}<CR>", "x".repeat(5000)),
            "@:",
        ),
    ];
    let ls = rc_repeating("l", 255);
    for (name, board, setup, key) in cases {
        let rc = if key == "Q" { ls.as_str() } else { "" };
        let mut r = run_with_rc(rc, board, &setup);
        let started = Instant::now();
        r.feed(key);
        let elapsed = started.elapsed();
        let work = r.engine.key_work();
        let message = r
            .engine
            .message()
            .map(|m| m.text.clone())
            .unwrap_or_default();
        let per_unit = elapsed.as_secs_f64() * 1e9 / work.max(1) as f64;
        println!("{name}: {elapsed:.1?}, {work} units, {per_unit:.1} ns a unit ({message})");
    }
}
