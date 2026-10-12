//! Jump labels, keys in and board out: `s` in Normal mode, Visual mode and after `d` and `y`,
//! the order and the split between one and two characters, `<Esc>`, `<BS>`, keys that start no
//! label, clicks, mappings, the view, `.`, undo and `:set jumplabels`. Text and math are measured
//! with `FixedMeasure`: 0.3 units a character and 0.6 a line.
//!
//! Snapshots add the labels to the header, one a line: the label, its object and where it is
//! drawn (the object's anchor), nearest first.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use std::collections::HashSet;
use std::fmt::Write as _;

use common::{Run, board, run, run_with_rc};
use insta::assert_snapshot;
use lawh_core::engine::{
    DEFAULT_JUMP_LABELS, Effect, LabelChars, MAX_LABEL_CHARS, MAX_LABEL_KEYS, MIN_LABEL_CHARS,
};
use lawh_core::{Board, Cursor, Mode, ObjectId, Point, Rect, to_lawh};
use proptest::prelude::*;

const LAWHRC: &str = include_str!("../../../examples/lawhrc");

/// Cases for the property tests: `LAWH_PROPTEST_CASES` if set, otherwise `default`.
fn cases(default: u32) -> u32 {
    std::env::var("LAWH_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn id(s: &str) -> ObjectId {
    ObjectId::new(s).unwrap()
}

fn grid(x: f64, y: f64) -> Cursor {
    Cursor::Grid(Point::new(x, y))
}

fn object(name: &str) -> Cursor {
    Cursor::Object(id(name))
}

/// The message and whether it is an error.
fn message(r: &Run) -> Option<(&str, bool)> {
    r.engine.message().map(|m| (m.text.as_str(), m.error))
}

/// The labels showing as `label=object`, nearest first, and the typed prefix.
fn labels(r: &Run) -> Option<(Vec<String>, String)> {
    let (labels, typed) = r.engine.jump_labels()?;
    let shown = labels
        .iter()
        .map(|l| format!("{}={}", l.text, l.id))
        .collect();
    Some((shown, typed.to_owned()))
}

/// The label texts showing, nearest first.
fn texts(r: &Run) -> Vec<String> {
    let (labels, _) = r.engine.jump_labels().unwrap();
    labels.iter().map(|l| l.text.clone()).collect()
}

/// The header, the pending keys and the labels showing, then the board.
fn snapshot(r: &Run) -> String {
    let mut out = r.header();
    let _ = write!(out, " pending={:?}", r.engine.pending());
    if let Some((labels, typed)) = r.engine.jump_labels() {
        let _ = write!(out, "\n// labels typed={typed:?}");
        for label in labels {
            let _ = write!(
                out,
                "\n// {} {} {},{}",
                label.text, label.id, label.at.x, label.at.y
            );
        }
    }
    format!("{out}\n{}", to_lawh(r.engine.board()))
}

/// `n` rectangles in a row, `r1` at x 1 to `r{n}` at x `n`: from the origin, `r{i}` is `i`
/// units away.
fn row(n: usize) -> Board {
    let mut objects = String::new();
    for i in 1..=n {
        writeln!(objects, "rect id=r{i} x={i} y=0 w=0.5 h=0.5").unwrap();
    }
    board(&objects)
}

/// Three circles at 2, 5 and 9 units right of the origin, and a text far below.
fn circles() -> Board {
    board(
        "circle id=c1 x=2 y=0 r=0.5
circle id=c2 x=5 y=0 r=0.5
circle id=c3 x=9 y=0 r=0.5
text id=t1 x=0 y=20 \"far\"
",
    )
}

// Which objects get labels, in what order, and which labels.

#[test]
fn labels_go_nearest_first_ties_by_reading_order_then_z_order() {
    // Six objects one unit from the origin, the cursor, and a text three units away. `c3` and
    // `c5` are the same circle: the one written first comes first.
    let b = board(
        "circle id=c1 x=0 y=-1 r=0.25
circle id=c2 x=-1 y=0 r=0.25
circle id=c3 x=1 y=0 r=0.25
circle id=c4 x=0 y=1 r=0.25
circle id=c5 x=1 y=0 r=0.25
text id=t1 x=3 y=0 \"far\"
rect id=r1 x=0 y=-1 w=0.5 h=0.5
",
    );
    let r = run(b, "s");
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert_eq!(r.engine.cursor(), &grid(0.0, 0.0));
    assert_snapshot!(snapshot(&r));
}

#[test]
fn up_to_the_alphabet_every_label_is_one_character() {
    let r = run(row(26), "s");
    let expected: Vec<String> = DEFAULT_JUMP_LABELS.chars().map(String::from).collect();
    assert_eq!(texts(&r), expected);
    assert_eq!(message(&r), None);
}

#[test]
fn one_object_past_the_alphabet_gives_the_last_character_to_two_labels() {
    // 25 objects get one character; `m`, the 26th, starts the labels of the farthest two.
    let r = run(row(27), "s");
    assert_snapshot!(snapshot(&r));
}

#[test]
fn five_hundred_objects_get_seven_single_labels_and_nineteen_first_letters() {
    // Seven alone and 19 first letters give 7 + 19 * 26 = 501 labels; eight alone would give
    // 8 + 18 * 26 = 476, too few.
    let r = run(row(500), "s");
    let texts = texts(&r);
    assert_eq!(texts.len(), 500);
    assert_eq!(texts[..7], ["a", "s", "d", "f", "g", "h", "j"]);
    assert!(texts[7..].iter().all(|t| t.chars().count() == 2));
    assert_eq!(texts[7..10], ["ka", "ks", "kd"]);
    assert_eq!(texts[33], "la");
    // 493 two-letter labels: 18 first letters of 26, then 25 of `m`'s.
    assert_eq!(texts[499], "mn");
    let firsts: HashSet<char> = texts[7..].iter().filter_map(|t| t.chars().next()).collect();
    assert_eq!(firsts.len(), 19);
    let (labels, _) = r.engine.jump_labels().unwrap();
    for (i, label) in labels.iter().enumerate() {
        assert_eq!(label.id, id(&format!("r{}", i + 1)));
    }
    // The farthest takes two keys.
    let mut r = r;
    r.feed("mn");
    assert_eq!(r.engine.cursor(), &object("r500"));
}

#[test]
fn the_square_of_the_alphabet_is_the_most_labels() {
    // Three characters: nine labels of two, none alone.
    let r = run(row(9), ":set jumplabels=abc<CR>s");
    assert_eq!(
        texts(&r),
        ["aa", "ab", "ac", "ba", "bb", "bc", "ca", "cb", "cc"]
    );
    assert_eq!(message(&r), None);
    // Past it the farthest get none, and the message says how many.
    let r = run(row(10), ":set jumplabels=abc<CR>s");
    assert_snapshot!(snapshot(&r));
    let r = run(row(12), ":set jumplabels=abc<CR>s");
    assert_eq!(texts(&r).len(), 9);
    assert_eq!(
        message(&r),
        Some(("3 objects in view have no label", false))
    );
    // The default alphabet labels a full board: 26 * 26 is past 500.
    let m = DEFAULT_JUMP_LABELS.chars().count();
    assert!(m * m >= 500);
}

#[test]
fn the_current_object_gets_no_label() {
    let mut r = run(circles(), "gg");
    assert_eq!(r.engine.cursor(), &object("c1"));
    r.feed("s");
    // From `c1`'s centre: `c2` 3 units away, `c3` 7, `t1` about 20.
    assert_eq!(
        labels(&r),
        Some((
            vec!["a=c2".into(), "s=c3".into(), "d=t1".into()],
            String::new()
        ))
    );
    // Nothing else on the board: nothing to label.
    let mut r = run(board("circle id=c1 x=0 y=0 r=1\n"), "gg");
    r.feed("s");
    assert_eq!(r.engine.jump_labels(), None);
    assert_eq!(message(&r), Some(("no objects in view", true)));
    let r = run(Board::new(), "s");
    assert_eq!(r.engine.jump_labels(), None);
    assert_eq!(message(&r), Some(("no objects in view", true)));
    assert_eq!(r.engine.pending(), "");
}

// Normal mode.

#[test]
fn s_then_a_label_moves_the_cursor_onto_its_object() {
    let mut r = run(circles(), "s");
    assert_eq!(r.engine.pending(), "s");
    r.feed("s");
    assert_eq!(r.engine.cursor(), &object("c2"));
    assert_eq!(r.engine.jump_labels(), None);
    assert_eq!(r.engine.pending(), "");
    assert_eq!(message(&r), None);
    // From `c2`, labels start again from it, and leave it out: `c1` is 3 units away, `c3` 4.
    r.feed("s");
    assert_snapshot!(snapshot(&r));
    r.feed("d");
    assert_eq!(r.engine.cursor(), &object("t1"));
}

#[test]
fn counts_are_ignored() {
    let r = run(circles(), "3sa");
    assert_eq!(r.engine.cursor(), &object("c1"));
    assert_eq!(r.engine.pending(), "");
    let r = run(circles(), "2d3sa");
    assert_eq!(r.engine.board().len(), 3);
    assert!(!r.engine.board().contains(&id("c1")));
}

#[test]
fn nothing_jumps_until_a_label_is_typed_even_with_one_object() {
    let mut r = run(board("circle id=c1 x=4 y=4 r=1\n"), "s");
    assert_eq!(labels(&r), Some((vec!["a=c1".into()], String::new())));
    assert_eq!(r.engine.cursor(), &grid(0.0, 0.0));
    r.feed("a");
    assert_eq!(r.engine.cursor(), &object("c1"));
}

// Visual mode.

#[test]
fn in_visual_mode_a_label_stretches_the_rectangle() {
    let mut r = run(circles(), "vs");
    assert_eq!(r.engine.mode(), Mode::Visual);
    assert_eq!(r.engine.pending(), "s");
    // `c2`: the rectangle goes from the origin to `c2`'s bounds, taking `c1` too.
    r.feed("s");
    assert_eq!(r.engine.mode(), Mode::Visual);
    assert_eq!(r.engine.selection(), [id("c1"), id("c2")]);
    assert_snapshot!(snapshot(&r));
    // Again, from `c2`: `a` is `c1`, `s` `c3`. Then `d` deletes all three.
    r.feed("ss");
    assert_eq!(r.engine.selection(), [id("c1"), id("c2"), id("c3")]);
    r.feed("d");
    assert_eq!(r.engine.board().len(), 1);
    assert_eq!(r.engine.mode(), Mode::Normal);
}

#[test]
fn in_visual_mode_esc_and_keys_that_start_no_label_end_the_labels_only() {
    // As `vf<Esc>` in Vim: the `s` ends, Visual mode stays.
    let mut r = run(circles(), "vsas<Esc>");
    assert_eq!(r.engine.mode(), Mode::Visual);
    assert_eq!(r.engine.jump_labels(), None);
    assert_eq!(r.engine.selection(), [id("c1")]);
    r.feed("s!");
    assert_eq!(r.engine.mode(), Mode::Visual);
    assert_eq!(r.engine.jump_labels(), None);
    assert_eq!(message(&r), Some(("no label \"!\"", true)));
    assert_eq!(r.engine.selection(), [id("c1")]);
    // `<Esc>` again leaves Visual mode.
    r.feed("<Esc>");
    assert_eq!(r.engine.mode(), Mode::Normal);
}

// After an operator: remote operations.

#[test]
fn ys_then_a_label_then_p_copies_a_far_object_next_to_the_cursor() {
    let b = board("text id=t1 x=0 y=0 \"here\"\ncircle id=c1 x=10 y=10 r=1\n");
    let mut r = run(b.clone(), "ggys");
    assert_eq!(r.engine.pending(), "ys");
    r.feed("a");
    // The yank leaves the cursor and the board as they were.
    assert_eq!(r.engine.cursor(), &object("t1"));
    assert_eq!(r.engine.board(), &b);
    r.feed("p");
    assert_snapshot!(snapshot(&r));
    // From a grid point, the copy's anchor goes to the point. From 0,3, `a` is `t1`, `s` `c1`.
    let r = run(b, "3jyssp");
    assert_eq!(
        to_lawh(r.engine.board()),
        "lawh version=1\n\
         text id=t1 x=0 y=0 \"here\"\n\
         circle id=c1 x=10 y=10 r=1\n\
         circle id=c2 x=0 y=3 r=1\n"
    );
}

#[test]
fn ds_then_a_label_deletes_a_far_object_and_dot_asks_for_another() {
    let mut r = run(circles(), "ggds");
    assert_eq!(r.engine.pending(), "ds");
    // From `c1`: `a` is `c2`.
    r.feed("a");
    assert_eq!(r.engine.cursor(), &object("c1"));
    assert!(!r.engine.board().contains(&id("c2")));
    // `.` shows the labels again, from where the cursor is now.
    r.feed(".");
    assert_eq!(r.engine.pending(), "ds");
    assert_eq!(
        labels(&r),
        Some((vec!["a=c3".into(), "s=t1".into()], String::new()))
    );
    r.feed("s");
    assert_snapshot!(snapshot(&r));
    // Each delete is one undo step, and puts the cursor back where it was before it, though
    // the cursor has moved since.
    r.feed("5ju");
    assert!(r.engine.board().contains(&id("t1")));
    assert_eq!(r.engine.cursor(), &object("c1"));
    r.feed("u");
    assert_eq!(r.engine.board(), &circles());
    assert_eq!(r.engine.cursor(), &object("c1"));
    // The register holds the last object deleted: `t1` again, as `t2`.
    r.feed("p");
    assert!(r.engine.board().contains(&id("t2")));
}

#[test]
fn remote_operations_work_from_a_grid_cursor() {
    // `dd` on a grid point has no object to act on; `ds` does.
    let mut r = run(circles(), "dd");
    assert_eq!(message(&r), Some(("no object under the cursor", true)));
    r.feed("dss");
    assert_eq!(r.engine.cursor(), &grid(0.0, 0.0));
    assert!(!r.engine.board().contains(&id("c2")));
    r.feed("u");
    assert_eq!(r.engine.cursor(), &grid(0.0, 0.0));
    assert_eq!(r.engine.board(), &circles());
}

#[test]
fn esc_after_an_operator_ends_the_labels_and_the_operator() {
    // As `df<Esc>` in Vim.
    let mut r = run(circles(), "ggds<Esc>");
    assert_eq!(r.engine.jump_labels(), None);
    assert_eq!(r.engine.pending(), "");
    // `w` is a motion again, not `dw`.
    r.feed("w");
    assert_eq!(r.engine.board(), &circles());
    assert_eq!(r.engine.cursor(), &object("c2"));
    // In Normal mode, `<Esc>` ends the labels alone: nothing moves.
    let r = run(circles(), "s<Esc>");
    assert_eq!(r.engine.jump_labels(), None);
    assert_eq!(r.engine.cursor(), &grid(0.0, 0.0));
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert_eq!(message(&r), None);
}

// Keys that start no label, `<BS>` and clicks.

#[test]
fn a_key_that_starts_no_label_ends_them_and_is_dropped() {
    // `j` is no label here (three objects take `a`, `s`, `d`): it does not move down.
    let mut r = run(circles(), "sj");
    assert_eq!(r.engine.jump_labels(), None);
    assert_eq!(r.engine.cursor(), &grid(0.0, 0.0));
    assert_eq!(message(&r), Some(("no label \"j\"", true)));
    r.feed("s<C-a>");
    assert_eq!(message(&r), Some(("no label \"<C-a>\"", true)));
    r.feed("s<Space>");
    assert_eq!(message(&r), Some(("no label \"<Space>\"", true)));
    // After an operator, the operator ends too.
    r.feed("ggdsj");
    assert_eq!(r.engine.pending(), "");
    r.feed("w");
    assert_eq!(r.engine.board(), &circles());
    // A first letter, then a second that makes no label.
    let mut r = run(row(27), "smz");
    assert_eq!(r.engine.jump_labels(), None);
    assert_eq!(message(&r), Some(("no label \"mz\"", true)));
    r.feed("l");
    assert_eq!(r.engine.cursor(), &grid(1.0, 0.0));
}

#[test]
fn backspace_takes_back_a_first_letter() {
    let mut r = run(row(27), "sm");
    assert_eq!(r.engine.pending(), "sm");
    assert_eq!(r.engine.jump_labels().map(|(_, typed)| typed), Some("m"));
    assert_snapshot!(snapshot(&r));
    r.feed("<BS>");
    assert_eq!(r.engine.pending(), "s");
    assert_eq!(r.engine.jump_labels().map(|(_, typed)| typed), Some(""));
    // `<S-BS>` as `<BS>`: with nothing typed, it ends the labels.
    r.feed("<S-BS>");
    assert_eq!(r.engine.jump_labels(), None);
    assert_eq!(r.engine.cursor(), &grid(0.0, 0.0));
    assert_eq!(message(&r), None);
    // Typed again: `ms`.
    r.feed("sm<BS>ms");
    assert_eq!(r.engine.cursor(), &object("r27"));
    r.feed("sa");
    assert_eq!(r.engine.cursor(), &object("r26"));
}

#[test]
fn a_click_ends_the_labels() {
    let mut r = run(circles(), "s");
    r.click(4.4, 1.6, None);
    assert_eq!(r.engine.jump_labels(), None);
    assert_eq!(r.engine.cursor(), &grid(4.0, 2.0));
    assert_eq!(r.engine.pending(), "");
    // After `d`, the operator goes with them, and in Visual mode the click leaves it.
    r.feed("ds");
    r.click(9.0, 0.0, Some("c3"));
    assert_eq!(r.engine.jump_labels(), None);
    r.feed("w");
    assert_eq!(r.engine.board(), &circles());
    r.feed("vs");
    r.click(0.0, 0.0, None);
    assert_eq!(r.engine.jump_labels(), None);
    assert_eq!(r.engine.mode(), Mode::Normal);
    // The first letter of a label is dropped too.
    let mut r = run(row(27), "sm");
    r.click(0.0, 0.0, None);
    assert_eq!(r.engine.jump_labels(), None);
    assert_eq!(r.engine.pending(), "");
}

// Mappings.

#[test]
fn label_keys_skip_mappings() {
    // The sample lawhrc moves with `jkl;`; with them as labels, `;` picks a label.
    let rc = format!("{LAWHRC}set jumplabels=;lkj\n");
    let r = run_with_rc(&rc, circles(), ";");
    assert_eq!(r.engine.cursor(), &grid(1.0, 0.0));
    let mut r = run_with_rc(&rc, circles(), "s");
    assert_eq!(
        labels(&r),
        Some((
            vec![";=c1".into(), "l=c2".into(), "k=c3".into(), "j=t1".into()],
            String::new()
        ))
    );
    r.feed(";");
    assert_eq!(r.engine.cursor(), &object("c1"));
    // From `c1`: `;` is `c2`, `l` `c3`, `k` `t1`.
    r.feed("sk");
    assert_eq!(r.engine.cursor(), &object("t1"));
    // After `d` (operator-pending mappings) and in Visual mode (Visual mappings) too. From
    // `t1`, far below: `;` is `c1`, `l` `c2`, `k` `c3`.
    r.feed("dsl");
    assert!(!r.engine.board().contains(&id("c2")));
    assert_eq!(r.engine.cursor(), &object("t1"));
    r.feed("vsl");
    assert_eq!(r.engine.cursor(), &object("c3"));
    assert_eq!(r.engine.mode(), Mode::Visual);
}

#[test]
fn label_keys_never_wait_for_a_mapping_and_a_timeout_runs_none() {
    // `a` starts a longer mapping, but picks a label at once.
    let rc = "nnoremap ax :ci 1<CR>\n";
    let mut r = run_with_rc(rc, circles(), "s");
    r.feed("<Timeout>");
    assert!(r.engine.jump_labels().is_some());
    assert_eq!(r.engine.cursor(), &grid(0.0, 0.0));
    r.feed("a");
    assert_eq!(r.effects, Vec::<Effect>::new());
    assert_eq!(r.engine.cursor(), &object("c1"));
    assert_eq!(r.engine.board(), &circles());
    // Keys a mapping types after `s` pick a label too.
    let r = run_with_rc("nnoremap Q sd\n", circles(), "Q");
    assert_eq!(r.engine.cursor(), &object("c3"));
}

// The view.

#[test]
fn only_objects_in_view_get_labels() {
    let b = board(
        "circle id=c1 x=1 y=1 r=0.5
circle id=c2 x=10 y=10 r=0.5
circle id=c3 x=5 y=0 r=0.5
line id=l1 x1=20 y1=0 x2=30 y2=0
",
    );
    let mut r = run(b, "");
    // `c3`'s left edge touches the view's right edge: it counts.
    r.engine.set_view(Some(Rect::new(0.0, 0.0, 4.5, 5.0)));
    r.feed("s");
    assert_eq!(
        labels(&r),
        Some((vec!["a=c1".into(), "s=c3".into()], String::new()))
    );
    // Labels showing stay as they are when the view moves.
    r.engine
        .set_view(Some(Rect::new(100.0, 100.0, 110.0, 110.0)));
    assert_eq!(texts(&r), ["a", "s"]);
    r.feed("s");
    assert_eq!(r.engine.cursor(), &object("c3"));
    // A view with nothing in it.
    r.feed("s");
    assert_eq!(r.engine.jump_labels(), None);
    assert_eq!(message(&r), Some(("no objects in view", true)));
    // A view given back to front is the same view.
    r.engine.set_view(Some(Rect::new(4.5, 5.0, 0.0, 0.0)));
    r.click(0.0, 0.0, None);
    r.feed("s");
    assert_eq!(texts(&r), ["a", "s"]);
    // No view, or one that is not a number: every object.
    r.feed("<Esc>");
    r.engine.set_view(None);
    r.feed("s");
    assert_eq!(texts(&r).len(), 4);
    r.feed("<Esc>");
    r.engine.set_view(Some(Rect::new(f64::NAN, 0.0, 1.0, 1.0)));
    r.feed("s");
    assert_eq!(texts(&r).len(), 4);
}

// `:set jumplabels`.

#[test]
fn set_jumplabels_takes_two_to_fifty_two_different_characters() {
    let mut r = run(circles(), ":set jumplabels?<CR>");
    assert_eq!(
        message(&r),
        Some(("jumplabels=asdfghjklqwertyuiopzxcvbnm", false))
    );
    r.feed(":set jumplabels=qp<CR>s");
    // Two characters, four objects: four labels of two.
    assert_eq!(texts(&r), ["qq", "qp", "pq", "pp"]);
    r.feed("<Esc>:set jumplabels&<CR>:set jumplabels?<CR>");
    assert_eq!(
        message(&r),
        Some(("jumplabels=asdfghjklqwertyuiopzxcvbnm", false))
    );
    let letters: String = ('a'..='z').chain('A'..='Z').collect();
    assert_eq!(letters.chars().count(), MAX_LABEL_CHARS);
    r.feed(&format!(":set jumplabels={letters}<CR>s"));
    assert_eq!(texts(&r), ["a", "b", "c", "d"]);
    r.feed("<Esc>");
    // Bad values are refused, and the option stays as it was.
    let usage = "it takes 2 to 52 different printable characters, such as `asdfghjkl`";
    let too_many = format!("{letters}0");
    for bad in ["a", "", "abca", too_many.as_str()] {
        r.feed(&format!(":set jumplabels={bad}<CR>"));
        let expected = format!("bad value `{bad}` for `jumplabels`: {usage}");
        assert_eq!(message(&r), Some((expected.as_str(), true)), "{bad}");
        assert_eq!(r.engine.settings().jumplabels.to_string(), letters);
    }
    assert_eq!(MIN_LABEL_CHARS, 2);
    assert_eq!(LabelChars::new("x"), None);
}

#[test]
fn jumplabels_load_from_a_lawhrc() {
    let r = run_with_rc("set jumplabels=hjkl\n", circles(), "s");
    assert_eq!(texts(&r), ["h", "j", "k", "l"]);
}

// Random alphabets and boards.

/// Characters labels may be made of: more than [`MAX_LABEL_CHARS`] of them.
fn pool() -> Vec<char> {
    ('a'..='z')
        .chain('A'..='Z')
        .chain('0'..='9')
        .chain(";,./'[]-=".chars())
        .chain("éßλж€".chars())
        .collect()
}

/// An alphabet of 2 to 52 different characters, in any order.
fn alphabet() -> impl Strategy<Value = String> {
    proptest::sample::subsequence(pool(), MIN_LABEL_CHARS..=MAX_LABEL_CHARS)
        .prop_shuffle()
        .prop_map(|chars| chars.into_iter().collect())
}

/// `n` small rectangles scattered over a 41 by 37 area around the origin, some on the same
/// spot.
fn scattered(n: usize) -> Board {
    let mut objects = String::new();
    for i in 0..n {
        let x = (i * 7919) % 41;
        let y = (i * 104_729) % 37;
        writeln!(
            objects,
            "rect id=r{i} x={} y={} w=0.5 h=0.5",
            x as f64 - 20.0,
            y as f64 - 18.0
        )
        .unwrap();
    }
    board(&objects)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(256)))]

    /// Labels are unique and prefix-free, one or two characters of the alphabet, nearest first,
    /// with as many of one character as fit; past the square of the alphabet the rest have
    /// none. Typing any of them goes to its object.
    #[test]
    fn labels_are_unique_prefix_free_and_reach_their_objects(
        chars in alphabet(),
        n in 0_usize..=500,
        on_object in any::<bool>(),
        pick in any::<prop::sample::Index>(),
    ) {
        let mut r = run(scattered(n), &format!(":set jumplabels={chars}<CR>"));
        prop_assert_eq!(message(&r), None);
        if on_object {
            r.feed("gg");
        }
        let from = r.engine.cursor_point();
        let targets = n - usize::from(matches!(r.engine.cursor(), Cursor::Object(_)));
        r.feed("s");
        let Some((labels, typed)) = r.engine.jump_labels() else {
            prop_assert_eq!(targets, 0);
            prop_assert_eq!(message(&r), Some(("no objects in view", true)));
            return Ok(());
        };
        prop_assert_eq!(typed, "");
        let alphabet: Vec<char> = chars.chars().collect();
        let m = alphabet.len();
        let labelled = targets.min(m * m);
        prop_assert_eq!(labels.len(), labelled);
        match targets - labelled {
            0 => prop_assert_eq!(message(&r), None),
            1 => prop_assert_eq!(message(&r), Some(("1 object in view has no label", false))),
            left => {
                let text = format!("{left} objects in view have no label");
                prop_assert_eq!(message(&r), Some((text.as_str(), false)));
            }
        }
        let texts: Vec<Vec<char>> = labels.iter().map(|l| l.text.chars().collect()).collect();
        let unique: HashSet<&Vec<char>> = texts.iter().collect();
        prop_assert_eq!(unique.len(), texts.len());
        for (i, text) in texts.iter().enumerate() {
            prop_assert!((1..=MAX_LABEL_KEYS).contains(&text.len()), "{text:?}");
            prop_assert!(text.iter().all(|c| alphabet.contains(c)), "{text:?}");
            for (j, other) in texts.iter().enumerate() {
                prop_assert!(i == j || !other.starts_with(text), "{text:?} starts {other:?}");
            }
        }
        // One character first, then two; as many of one as leave enough first letters.
        let singles = texts.iter().take_while(|t| t.len() == 1).count();
        prop_assert!(texts[singles..].iter().all(|t| t.len() == 2));
        if labelled <= m {
            prop_assert_eq!(singles, labelled);
        } else {
            prop_assert!(singles + (m - singles) * m >= labelled);
            prop_assert!(singles == m || singles + 1 + (m - singles - 1) * m < labelled);
        }
        // Nearest first, the current object left out.
        let distances: Vec<f64> = labels.iter().map(|l| from.distance(l.at)).collect();
        prop_assert!(distances.windows(2).all(|w| w[0] <= w[1]), "{distances:?}");
        if let Cursor::Object(current) = r.engine.cursor() {
            prop_assert!(labels.iter().all(|l| &l.id != current));
        }
        // Any label goes to its object.
        let label = pick.get(labels).clone();
        r.feed(&label.text.chars().map(|c| match c {
            '<' => "<lt>".to_owned(),
            c => c.to_string(),
        }).collect::<String>());
        prop_assert_eq!(r.engine.cursor(), &Cursor::Object(label.id));
        prop_assert_eq!(r.engine.jump_labels(), None);
    }
}
