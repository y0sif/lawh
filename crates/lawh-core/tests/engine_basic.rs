//! The engine's first slice, keys in and board out: grid motions with counts, the `:` command
//! line, `:circle`, undo and redo, and the caps on each.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use common::run;
use insta::assert_snapshot;
use lawh_core::board::MAX_OBJECTS;
use lawh_core::engine::{MAX_COMMAND_LINE_CHARS, MAX_COUNT};
use lawh_core::{Board, Cursor, Mode, Object, ObjectId, Point, Shape};

/// The example from `docs/testing.md`: move, then make a circle at the cursor.
#[test]
fn circle_at_the_cursor() {
    assert_snapshot!(run(Board::new(), "3l2j:ci 2<CR>").snapshot());
}

#[test]
fn counts_move_by_grid_units() {
    // `0` alone is not a count; after another digit it is.
    assert_snapshot!(run(Board::new(), "12l3k0l10j2h").snapshot());
}

#[test]
fn a_count_shows_as_pending_until_its_command() {
    let mut r = run(Board::new(), "12");
    assert_eq!(r.engine.pending(), "12");
    assert_eq!(r.engine.cursor(), &Cursor::Grid(Point::ZERO));
    r.feed("0");
    assert_eq!(r.engine.pending(), "120");
    // An unbound key drops the count.
    r.feed("<Esc>l");
    assert_eq!(r.engine.pending(), "");
    assert_eq!(r.engine.cursor(), &Cursor::Grid(Point::new(1.0, 0.0)));
    // `0` does not start a count.
    r.feed("0");
    assert_eq!(r.engine.pending(), "");
    // A count before `:` is dropped.
    r.feed("3:ci 1<CR>");
    assert_eq!(r.engine.pending(), "");
    assert_snapshot!(r.snapshot());
}

#[test]
fn counts_stop_at_max_count() {
    let mut r = run(Board::new(), "99999999");
    assert_eq!(r.engine.pending(), MAX_COUNT.to_string());
    r.feed("j");
    assert_snapshot!(r.snapshot());
}

#[test]
fn the_cursor_stops_at_max_coordinate() {
    // 101 moves of 10,000 units run past 1,000,000 in each direction.
    let script = format!("{}{}:ci 1<CR>", "10000l".repeat(101), "10000k".repeat(101));
    assert_snapshot!(run(Board::new(), &script).snapshot());
}

#[test]
fn motions_from_an_object_start_at_its_anchor() {
    assert_snapshot!(run(Board::new(), ":ci 2 x=5 y=-1.5<CR>2l").snapshot());
}

#[test]
fn undo_takes_back_a_circle_and_its_cursor() {
    assert_snapshot!(run(Board::new(), "3l:ci 1<CR>u").snapshot());
}

#[test]
fn redo_makes_it_again() {
    assert_snapshot!(run(Board::new(), "3l:ci 1<CR>u<C-r>").snapshot());
}

#[test]
fn a_count_undoes_several_steps() {
    assert_snapshot!(run(Board::new(), ":ci 1<CR>l:ci 2<CR>l:ci 3<CR>2u").snapshot());
}

#[test]
fn a_count_redoes_several_steps() {
    assert_snapshot!(run(Board::new(), ":ci 1<CR>l:ci 2<CR>l:ci 3<CR>3u2<C-r>").snapshot());
}

#[test]
fn undo_past_the_start_says_so() {
    assert_snapshot!(run(Board::new(), "u").snapshot());
}

#[test]
fn redo_past_the_end_says_so() {
    assert_snapshot!(run(Board::new(), ":ci 1<CR><C-r>").snapshot());
}

#[test]
fn a_count_past_the_history_stops_quietly() {
    let r = run(Board::new(), ":ci 1<CR>l:ci 2<CR>9u");
    assert!(r.engine.board().is_empty());
    assert_eq!(r.engine.message(), None);
    let r = run(Board::new(), ":ci 1<CR>u9<C-r>");
    assert_eq!(r.engine.board().len(), 1);
    assert_eq!(r.engine.message(), None);
}

#[test]
fn undo_and_redo_commands() {
    let mut r = run(Board::new(), ":ci 1<CR>:u<CR>");
    assert!(r.engine.board().is_empty());
    r.feed(":red<CR>");
    assert_snapshot!(r.snapshot());
}

#[test]
fn modified_follows_undo() {
    let mut r = run(Board::new(), "");
    assert!(!r.engine.is_modified());
    r.feed(":ci 1<CR>");
    assert!(r.engine.is_modified());
    r.feed("u");
    assert!(!r.engine.is_modified());
    r.feed("<C-r>");
    assert!(r.engine.is_modified());
    // Motions change nothing.
    let r = run(Board::new(), "3l2j");
    assert!(!r.engine.is_modified());
}

#[test]
fn new_circles_get_fresh_ids() {
    assert_snapshot!(run(Board::new(), ":ci 1<CR>:circle 2<CR>j:cir r=3<CR>").snapshot());
}

#[test]
fn command_line_shows_while_typing() {
    let r = run(Board::new(), ":ci 2");
    assert_eq!(r.engine.mode(), Mode::CommandLine);
    assert_eq!(r.engine.command_line(), Some(("ci 2", 4)));
    assert_snapshot!(r.snapshot());
}

#[test]
fn command_line_edits() {
    // `<Home>` puts the missing `c` in front; `<Left>` makes the radius 12.
    let r = run(Board::new(), ":i 2<Home>c<End><Left>1<CR>");
    assert_eq!(r.engine.command_line(), None);
    assert_snapshot!(r.snapshot());
}

#[test]
fn esc_cancels_the_command_line() {
    let r = run(Board::new(), ":ci 2<Esc>");
    assert_eq!(r.engine.command_line(), None);
    assert_snapshot!(r.snapshot());
}

#[test]
fn backspace_on_an_empty_command_line_cancels_it() {
    let r = run(Board::new(), ":<BS>");
    assert_eq!(r.engine.mode(), Mode::Normal);
    let r = run(Board::new(), ":c<BS>");
    assert_eq!(r.engine.mode(), Mode::CommandLine);
    let r = run(Board::new(), ":c<BS><BS>l");
    assert_snapshot!(r.snapshot());
}

#[test]
fn an_empty_command_does_nothing() {
    let r = run(Board::new(), ":<CR>:  <CR>");
    assert_eq!(r.engine.message(), None);
    assert_eq!(r.engine.mode(), Mode::Normal);
}

#[test]
fn an_unknown_command_says_so() {
    assert_snapshot!(run(Board::new(), ":foo 3<CR>").snapshot());
}

#[test]
fn a_circle_with_a_bad_number_says_so() {
    assert_snapshot!(run(Board::new(), ":ci 1e3<CR>").snapshot());
}

#[test]
fn a_circle_without_a_radius_says_so() {
    assert_snapshot!(run(Board::new(), ":ci x=1<CR>").snapshot());
}

#[test]
fn the_board_refuses_a_negative_radius() {
    assert_snapshot!(run(Board::new(), "l:ci -2<CR>").snapshot());
}

#[test]
fn a_full_board_refuses_a_circle() {
    let mut board = Board::new();
    for i in 1..=MAX_OBJECTS {
        let shape = Shape::Ellipse {
            center: Point::ZERO,
            rx: 1.0,
            ry: 1.0,
        };
        board
            .insert(Object::new(ObjectId::new(format!("c{i}")).unwrap(), shape))
            .unwrap();
    }
    let full = board.clone();
    let mut r = run(board, "2l:ci 1<CR>");
    // The board is 500 lines long: the header says it all.
    assert_snapshot!(r.header());
    assert_eq!(r.engine.board(), &full);
    assert!(!r.engine.is_modified());
    // Nothing to undo.
    r.feed("u");
    assert_eq!(
        r.engine.message().map(|m| m.text.as_str()),
        Some("already at the oldest change")
    );
}

#[test]
fn the_command_line_holds_at_most_max_command_line_chars() {
    let full = "x".repeat(MAX_COMMAND_LINE_CHARS);
    let mut r = run(Board::new(), &format!(":{full}yz"));
    assert_snapshot!(r.header());
    assert_eq!(r.engine.command_line(), Some((full.as_str(), full.len())));
    // Making room lets one more in.
    r.feed("<BS>z");
    let line = r.engine.command_line().unwrap().0;
    assert!(line.ends_with("xz") && line.chars().count() == MAX_COMMAND_LINE_CHARS);
    assert_eq!(r.engine.message(), None);
    r.feed("<Esc>");
    assert_eq!(r.engine.mode(), Mode::Normal);
}
