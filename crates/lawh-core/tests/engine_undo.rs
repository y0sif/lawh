//! Random key scripts of real commands on random small boards: undoing everything gives back
//! the starting board exactly, redoing everything the board after the last change, and no
//! script panics.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use common::run;
use lawh_core::{Board, Cursor, Object, Point, Shape};
use proptest::prelude::*;

/// Cases for the property tests: `LAWH_PROPTEST_CASES` if set, otherwise `default`.
fn cases(default: u32) -> u32 {
    std::env::var("LAWH_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// Small coordinates in half units, so objects meet, overlap and line up in rows.
fn coordinate() -> impl Strategy<Value = f64> {
    (-12_i32..12).prop_map(|v| f64::from(v) / 2.0)
}

fn size() -> impl Strategy<Value = f64> {
    (1_i32..8).prop_map(|v| f64::from(v) / 2.0)
}

fn shape() -> impl Strategy<Value = Shape> {
    let point = || (coordinate(), coordinate()).prop_map(|(x, y)| Point::new(x, y));
    prop_oneof![
        (point(), "[a-z ]{0,6}").prop_map(|(at, text)| Shape::Text { at, text }),
        (point(), "[a-z^ ]{1,6}").prop_map(|(at, src)| Shape::Math { at, src }),
        (point(), size(), size()).prop_map(|(center, rx, ry)| Shape::Ellipse { center, rx, ry }),
        (point(), size()).prop_map(|(center, r)| Shape::Ellipse {
            center,
            rx: r,
            ry: r
        }),
        (point(), size(), size()).prop_map(|(origin, w, h)| Shape::Rect { origin, w, h }),
        (point(), point()).prop_map(|(a, b)| Shape::Line { a, b }),
        (point(), point()).prop_map(|(a, b)| Shape::Arrow { a, b }),
    ]
}

fn small_board() -> impl Strategy<Value = Board> {
    prop::collection::vec(shape(), 0..7).prop_map(|shapes| {
        let mut board = Board::new();
        for shape in shapes {
            let id = board.fresh_id("o");
            board.insert(Object::new(id, shape)).unwrap();
        }
        board
    })
}

/// Commands as typed: motions, counts, operators, puts, Visual mode, `:` commands, insert
/// sessions and block-Normal edits, undo, redo and repeats, and Shift with `<CR>` and `<Esc>`.
const VOCABULARY: &[&str] = &[
    "h",
    "j",
    "k",
    "l",
    "w",
    "b",
    "H",
    "J",
    "K",
    "L",
    "2",
    "3",
    "dd",
    "x",
    "yy",
    "p",
    "P",
    "d",
    "y",
    "dw",
    "d2w",
    "db",
    "dJ",
    "dK",
    "yL",
    "v",
    "<Esc>",
    ":ci 1<CR>",
    ":el 2 1<CR>",
    ":re 2 1<CR>",
    ":li 1 2<CR>",
    ":ar -1 1<CR>",
    ":te hi<CR>",
    ":ma x^2<CR>",
    "ifoo<Esc>",
    "oab<Esc>",
    "<CR>x<Esc>",
    "Ax<BS><Esc>",
    "u",
    "<C-r>",
    ".",
    "@:",
    "gg",
    "G",
    "<S-CR>",
    "<S-Esc>",
];

fn script() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(VOCABULARY), 0..40).prop_map(|words| words.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(256)))]

    #[test]
    fn undo_and_redo_restore_every_board(start in small_board(), script in script()) {
        let mut r = run(start.clone(), &script);
        // Back to Normal mode, whatever the script left pending.
        r.feed("<Esc><Esc>");
        if let Cursor::Object(id) = r.engine.cursor() {
            prop_assert!(r.engine.board().contains(id), "cursor on a missing object {id}");
        }
        // Redo whatever the script undid, so the last board is the newest one.
        r.feed("10000<C-r>");
        let newest = r.engine.board().clone();
        r.feed("10000u");
        prop_assert_eq!(r.engine.board(), &start);
        prop_assert!(!r.engine.is_modified());
        r.feed("10000<C-r>");
        prop_assert_eq!(r.engine.board(), &newest);
        if let Cursor::Object(id) = r.engine.cursor() {
            prop_assert!(r.engine.board().contains(id), "cursor on a missing object {id}");
        }
    }
}
