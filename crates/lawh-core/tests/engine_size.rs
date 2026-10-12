//! Sizes of text and math, keys in and board out: the size keys `<C-a>` and `<C-x>` with
//! counts, `.`, undo and Visual mode, `:si[ze]` on the current object and on a selection,
//! `:set size` and `++size=` for new blocks, and `o` and `O` keeping a block's size. Text and math
//! are measured with `FixedMeasure`: 0.3 units a character and 0.6 a line, times the size.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use common::{Run, board, run, run_engine, run_with_rc};
use insta::assert_snapshot;
use lawh_core::engine::{Engine, SIZE_STEPS};
use lawh_core::{Board, Cursor, Mode, ObjectId, Point};

fn id(s: &str) -> ObjectId {
    ObjectId::new(s).unwrap()
}

/// The size of text or math object `name`.
fn size(r: &Run, name: &str) -> f64 {
    let object = r.engine.board().get(&id(name)).unwrap();
    object.shape.size().unwrap()
}

/// The message and whether it is an error.
fn message(r: &Run) -> Option<(&str, bool)> {
    r.engine.message().map(|m| (m.text.as_str(), m.error))
}

/// A text, a formula below it and a circle to the right of the text.
fn mixed() -> Board {
    board(
        "text id=t1 x=0 y=0 \"ab\"
math id=m1 x=0 y=1 \"x^2\"
circle id=c1 x=3 y=0.5 r=0.5
text id=t2 x=0 y=10 \"far\"
",
    )
}

/// Two texts in a column.
fn two_texts() -> Board {
    board("text id=t1 x=0 y=0 \"one\"\ntext id=t2 x=0 y=2 \"two\"\n")
}

// The size keys on the board.

#[test]
fn size_keys_step_text_and_math_along_the_steps() {
    // `gg` is `t1`, then `J` the formula below it.
    let mut r = run(mixed(), "gg<C-a>");
    assert_eq!(size(&r, "t1"), 1.25);
    r.feed("<C-a><C-a>");
    assert_eq!(size(&r, "t1"), 2.0);
    r.feed("<C-x>");
    assert_eq!(size(&r, "t1"), 1.5);
    r.feed("J<C-x><C-x>");
    assert_eq!(size(&r, "m1"), 0.5);
    // The cursor stays on the block; no message on the way.
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("m1")));
    assert_eq!(message(&r), None);
    assert_snapshot!(r.snapshot());
}

#[test]
fn every_step_is_reached_in_turn() {
    let mut r = run(two_texts(), "gg15<C-x>");
    let mut seen = vec![size(&r, "t1")];
    for _ in 1..SIZE_STEPS.len() {
        r.feed("<C-a>");
        seen.push(size(&r, "t1"));
    }
    assert_eq!(seen, SIZE_STEPS);
}

#[test]
fn a_count_steps_that_many_times_and_stops_at_an_end() {
    let mut r = run(two_texts(), "gg5<C-a>");
    assert_eq!(size(&r, "t1"), 3.0);
    assert_eq!(message(&r), None);
    // Past the end: the largest size, and information that it is.
    r.feed("100<C-a>");
    assert_eq!(size(&r, "t1"), 16.0);
    assert_eq!(message(&r), Some(("16 is the largest size", false)));
    // At the end: no change, no undo step, and an error.
    r.feed("<C-a>");
    assert_eq!(size(&r, "t1"), 16.0);
    assert_eq!(message(&r), Some(("16 is the largest size", true)));
    r.feed("u");
    assert_eq!(size(&r, "t1"), 3.0);
    r.feed("10000<C-x>");
    assert_eq!(size(&r, "t1"), 0.25);
    assert_eq!(message(&r), Some(("0.25 is the smallest size", false)));
    r.feed("<C-x>");
    assert_eq!(message(&r), Some(("0.25 is the smallest size", true)));
    // Onto an end exactly is no news.
    r.feed("15<C-a>");
    assert_eq!(size(&r, "t1"), 16.0);
    assert_eq!(message(&r), None);
}

#[test]
fn a_size_between_steps_goes_to_the_next_step() {
    let b = board("text id=t1 x=0 y=0 \"a\" size=1.1\nmath id=m1 x=0 y=2 \"x\" size=13\n");
    let mut r = run(b, "gg<C-a>");
    assert_eq!(size(&r, "t1"), 1.25);
    r.feed("u<C-x>");
    assert_eq!(size(&r, "t1"), 1.0);
    r.feed("J2<C-x>");
    assert_eq!(size(&r, "m1"), 10.0);
    r.feed("u<C-a>");
    assert_eq!(size(&r, "m1"), 16.0);
}

#[test]
fn dot_repeats_the_size_keys_with_a_new_count() {
    let mut r = run(two_texts(), "gg2<C-a>");
    assert_eq!(size(&r, "t1"), 1.5);
    // On the next block, with the same count.
    r.feed("w.");
    assert_eq!(size(&r, "t2"), 1.5);
    // A new count, which sticks.
    r.feed("3.");
    assert_eq!(size(&r, "t2"), 3.0);
    r.feed("b.");
    assert_eq!(size(&r, "t1"), 3.0);
    // `<C-x>` repeats as `<C-x>`.
    r.feed("<C-x>w.");
    assert_eq!((size(&r, "t1"), size(&r, "t2")), (2.5, 2.5));
    assert_snapshot!(r.snapshot());
    // Each `.` is one undo step.
    r.feed("u");
    assert_eq!(size(&r, "t2"), 3.0);
}

#[test]
fn undo_and_redo_take_back_one_size_change() {
    let mut r = run(two_texts(), "gg5<C-a>");
    assert!(r.engine.is_modified());
    r.feed("w");
    r.feed("u");
    assert_eq!(size(&r, "t1"), 1.0);
    assert!(!r.engine.is_modified());
    // The cursor goes back to the block.
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("t1")));
    r.feed("<C-r>");
    assert_eq!(size(&r, "t1"), 3.0);
    assert_snapshot!(r.snapshot());
}

#[test]
fn size_keys_on_a_shape_or_a_grid_point_say_why() {
    // `w` from `t1` is the circle beside it.
    let mut r = run(mixed(), "gg<C-a>w<C-a>");
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("c1")));
    assert_eq!(message(&r), Some(("only text and math have a size", true)));
    r.feed("l<C-x>");
    assert_eq!(message(&r), Some(("no object under the cursor", true)));
    assert_eq!(size(&r, "t1"), 1.25);
    // Neither became the change `.` repeats.
    r.feed("gg.");
    assert_eq!(size(&r, "t1"), 1.5);
}

#[test]
fn size_keys_do_nothing_in_block_normal_mode_or_after_an_operator() {
    let r = run(two_texts(), "gg<CR><C-a><C-x>");
    assert_eq!(r.engine.mode(), Mode::BlockNormal);
    assert_eq!(r.engine.board(), &two_texts());
    assert_eq!(message(&r), None);
    let r = run(two_texts(), "ggd<C-a>");
    assert_eq!(r.engine.board(), &two_texts());
    assert_eq!(r.engine.pending(), "");
    // Insert mode types nothing for them.
    let r = run(two_texts(), "ggA<C-a><C-x><Esc>");
    assert_eq!(r.engine.board(), &two_texts());
}

// The size keys in Visual mode.

#[test]
fn size_keys_in_visual_mode_resize_the_selected_text_and_math() {
    // From `t1` to the grid point (4, 2): `t1`, `m1` and `c1` lie inside; `t2` does not.
    let mut r = run(mixed(), "ggv4l2j<C-a>");
    assert_eq!(r.engine.selection(), [id("t1"), id("m1"), id("c1")]);
    assert_eq!((size(&r, "t1"), size(&r, "m1")), (1.25, 1.25));
    // Visual mode stays, the circle is skipped, and a count steps each block.
    assert_eq!(r.engine.mode(), Mode::Visual);
    r.feed("2<C-a>");
    assert_eq!((size(&r, "t1"), size(&r, "m1")), (2.0, 2.0));
    // `m1` grew past the rectangle's bottom (y 1 + 0.6 x 2 = 2.2) and stays selected.
    assert_eq!(r.engine.selection(), [id("t1"), id("m1"), id("c1")]);
    assert_snapshot!(r.snapshot());
    // The next motion selects from the rectangle again.
    r.feed("l");
    assert_eq!(r.engine.selection(), [id("t1"), id("c1")]);
    // Each key was one undo step.
    r.feed("<Esc>u");
    assert_eq!((size(&r, "t1"), size(&r, "m1")), (1.25, 1.25));
    r.feed("u");
    assert_eq!(r.engine.board(), &mixed());
}

#[test]
fn size_keys_in_visual_mode_report_the_ends_and_what_has_no_size() {
    let b = board("text id=t1 x=0 y=0 \"a\" size=12\ntext id=t2 x=0 y=8 \"b\"\n");
    // One block reaches the end, the other goes on: information.
    let mut r = run(b.clone(), "ggvG3<C-a>");
    assert_eq!((size(&r, "t1"), size(&r, "t2")), (16.0, 2.0));
    assert_eq!(message(&r), Some(("16 is the largest size", false)));
    // Only `t2` changes now; `t1` stays at the end, which is still news.
    r.feed("<C-a>");
    assert_eq!((size(&r, "t1"), size(&r, "t2")), (16.0, 2.5));
    assert_eq!(message(&r), Some(("16 is the largest size", false)));
    // Nothing selected, and a selection of shapes alone.
    let r = run(Board::new(), "v<C-a>");
    assert_eq!(message(&r), Some(("nothing selected", false)));
    assert_eq!(r.engine.mode(), Mode::Visual);
    let r = run(mixed(), "ggwv<C-x>");
    assert_eq!(r.engine.selection(), [id("c1")]);
    assert_eq!(message(&r), Some(("only text and math have a size", true)));
    // Every block at the end: an error and no change.
    let b = board("text id=t1 x=0 y=0 \"a\" size=0.25\nmath id=m1 x=0 y=1 \"b\" size=0.25\n");
    let r = run(b.clone(), "ggvG<C-x>");
    assert_eq!(message(&r), Some(("0.25 is the smallest size", true)));
    assert_eq!(r.engine.board(), &b);
    assert!(!r.engine.is_modified());
}

// What is highlighted after a Visual resize is what `d`, `x`, `y` and `:si` act on.

/// A text and a small circle below its second half: the circle is outside the rectangle from
/// the grid's origin to the text until the text grows over it.
fn text_over_circle() -> Board {
    board("text id=t2 x=2 y=0 \"ab\"\ncircle id=c1 x=2.3 y=1 r=0.2\n")
}

/// A long text and a short one above it, to its right: `vK` from the long one selects both.
fn long_and_short() -> Board {
    board("text id=t1 x=0 y=0 \"abcdefgh\"\ntext id=t2 x=1 y=-2 \"x\"\n")
}

#[test]
fn visual_d_after_a_resize_leaves_what_the_rectangle_gained() {
    // The cursor's text grows to 1.2 units tall, so the rectangle now holds the circle too;
    // only the text is highlighted, and only the text goes.
    let mut r = run(text_over_circle(), "vw<C-a><C-a><C-a>");
    assert_eq!(r.engine.selection(), [id("t2")]);
    assert_eq!(size(&r, "t2"), 2.0);
    r.feed("d");
    assert_eq!(r.engine.board().len(), 1);
    assert!(r.engine.board().contains(&id("c1")));
    assert_snapshot!(r.snapshot());
    // `x` the same.
    let mut r = run(text_over_circle(), "vw<C-a><C-a><C-a>x");
    assert!(r.engine.board().contains(&id("c1")));
    // The next motion selects from the rectangle again: the circle with it.
    r = run(text_over_circle(), "vw<C-a><C-a><C-a>wb");
    assert_eq!(r.engine.selection(), [id("t2"), id("c1")]);
    r.feed("d");
    assert!(r.engine.board().is_empty());
}

#[test]
fn visual_d_after_a_resize_takes_what_grew_out_of_the_rectangle() {
    // `t1` grows past where `v` was pressed, out of the rectangle; it is still highlighted,
    // and goes with `t2`.
    let mut r = run(long_and_short(), "");
    r.click(0.1, 0.1, Some("t1"));
    r.feed("vK<C-a>");
    assert_eq!(r.engine.selection(), [id("t1"), id("t2")]);
    r.feed("d");
    assert!(r.engine.board().is_empty());
    assert_snapshot!(r.snapshot());
    // One undo step puts both back, at their new size.
    r.feed("u");
    assert_eq!((size(&r, "t1"), size(&r, "t2")), (1.25, 1.25));
}

#[test]
fn visual_y_and_si_after_a_resize_act_on_what_is_highlighted() {
    // `y` yanks both, and `p` puts both.
    let mut r = run(long_and_short(), "");
    r.click(0.1, 0.1, Some("t1"));
    r.feed("vK<C-a>y");
    assert_eq!(r.engine.mode(), Mode::Normal);
    r.feed("p");
    assert_eq!(r.engine.board().len(), 4);
    assert_snapshot!(r.snapshot());
    // `:si` from Visual mode keeps the same selection.
    let mut r = run(long_and_short(), "");
    r.click(0.1, 0.1, Some("t1"));
    r.feed("vK<C-a>:si 3<CR>");
    assert_eq!((size(&r, "t1"), size(&r, "t2")), (3.0, 3.0));
}

#[test]
fn dot_does_not_repeat_a_visual_resize() {
    // `.` would repeat the `<C-a>` before it on one block, not the Visual change: it forgets.
    let mut r = run(two_texts(), "gg<C-a>vG<C-a><Esc>");
    assert_eq!((size(&r, "t1"), size(&r, "t2")), (1.5, 1.25));
    r.feed(".");
    assert_eq!((size(&r, "t1"), size(&r, "t2")), (1.5, 1.25));
}

// `:si[ze]`.

#[test]
fn size_command_sets_adds_and_takes() {
    let mut r = run(two_texts(), "gg:si 3<CR>");
    assert_eq!(size(&r, "t1"), 3.0);
    r.feed(":si +1.5<CR>");
    assert_eq!(size(&r, "t1"), 4.5);
    r.feed(":size -4<CR>");
    assert_eq!(size(&r, "t1"), 0.5);
    // Out of range: refused, the size as it was.
    r.feed(":si -0.5<CR>");
    assert_eq!(
        message(&r),
        Some(("size 0 is out of range: sizes go from 0.25 to 16", true))
    );
    r.feed(":si 17<CR>");
    assert_eq!(
        message(&r),
        Some(("size 17 is out of range: sizes go from 0.25 to 16", true))
    );
    assert_eq!(size(&r, "t1"), 0.5);
    // The same size: no change, no undo step.
    r.feed(":si .5<CR>");
    assert_eq!(message(&r), None);
    // Each command is one undo step.
    r.feed("u");
    assert_eq!(size(&r, "t1"), 4.5);
    assert_snapshot!(r.snapshot());
    // `@:` runs the last line again, on the current object.
    r.feed("w@:");
    assert_eq!(size(&r, "t2"), 0.5);
    r.feed(":si +2<CR>@:");
    assert_eq!(size(&r, "t2"), 4.5);
}

#[test]
fn sizes_worked_out_are_rounded_as_coordinates_are() {
    // 1.1 + 0.2 is 1.3000000000000003 in floating point.
    let b = board("text id=t1 x=0 y=0 \"a\" size=1.1\n");
    let r = run(b, "gg:si +0.2<CR>");
    assert_eq!(size(&r, "t1"), 1.3);
    assert_snapshot!(r.snapshot());
}

#[test]
fn size_command_on_a_shape_or_a_grid_point_says_why() {
    let r = run(mixed(), "ggw:si 2<CR>");
    assert_eq!(message(&r), Some(("only text and math have a size", true)));
    let r = run(mixed(), ":si 2<CR>");
    assert_eq!(message(&r), Some(("no object under the cursor", true)));
    assert_eq!(r.engine.board(), &mixed());
}

#[test]
fn size_command_from_visual_mode_resizes_the_selection() {
    let mut r = run(mixed(), "ggv4l2j:");
    // The command line keeps the selection.
    assert_eq!(r.engine.selection(), [id("t1"), id("m1"), id("c1")]);
    r.feed("si 2<CR>");
    assert_eq!((size(&r, "t1"), size(&r, "m1")), (2.0, 2.0));
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert_eq!(r.engine.selection(), []);
    assert_snapshot!(r.snapshot());
    // One undo step.
    r.feed("u");
    assert_eq!(r.engine.board(), &mixed());
    // `@:` has no selection: it acts on the current object, here a grid point.
    r.feed("@:");
    assert_eq!(message(&r), Some(("no object under the cursor", true)));
    r.feed("gg@:");
    assert_eq!((size(&r, "t1"), size(&r, "m1")), (2.0, 1.0));
}

#[test]
fn size_command_on_a_selection_is_all_or_nothing() {
    let b = board("text id=t1 x=0 y=0 \"a\"\nmath id=m1 x=0 y=1 \"b\" size=12\n");
    let mut r = run(b.clone(), "ggvG:si +5<CR>");
    assert_eq!(
        message(&r),
        Some(("size 17 is out of range: sizes go from 0.25 to 16", true))
    );
    assert_eq!(r.engine.board(), &b);
    r.feed("ggvG:si +4<CR>");
    assert_eq!((size(&r, "t1"), size(&r, "m1")), (5.0, 16.0));
    // Shapes alone, and nothing selected.
    let r = run(mixed(), "ggwv:si 2<CR>");
    assert_eq!(message(&r), Some(("only text and math have a size", true)));
    let r = run(mixed(), "9jv:si 2<CR>");
    assert_eq!(message(&r), Some(("nothing selected", false)));
}

#[test]
fn size_command_in_a_block_comes_back_to_it() {
    let r = run(two_texts(), "gg<CR>l:si 2<CR>");
    assert_eq!(size(&r, "t1"), 2.0);
    assert_eq!(r.engine.mode(), Mode::BlockNormal);
    assert_eq!(r.engine.text_cursor().map(|c| c.byte), Some(1));
}

// The size of new blocks.

#[test]
fn set_size_is_the_size_of_new_blocks() {
    let mut r = run(Board::new(), ":set size=2<CR>iab<Esc>");
    assert_eq!(size(&r, "t1"), 2.0);
    r.feed("5j:te Title<CR>5j:ma x^2<CR>");
    assert_eq!((size(&r, "t2"), size(&r, "m1")), (2.0, 2.0));
    // `++size=` wins over it.
    r.feed("5j:te ++size=3 Big<CR>5j:ma ++size=0.5 y<CR>");
    assert_eq!((size(&r, "t3"), size(&r, "m2")), (3.0, 0.5));
    r.feed(":set size?<CR>");
    assert_eq!(message(&r), Some(("size=2", false)));
    // Back to the default.
    r.feed(":set size&<CR>5jic<Esc>");
    assert_eq!(size(&r, "t4"), 1.0);
    assert_snapshot!(r.snapshot());
}

#[test]
fn o_and_capital_o_keep_a_blocks_size() {
    let b = board("text id=t1 x=0 y=0 \"Head\" size=3\ncircle id=c1 x=10 y=5 r=1\n");
    // Below a size-3 block (0.6 x 3 = 1.8 high): at y 2.3, size 3, whatever `:set size` says.
    let mut r = run(b, ":set size=0.5<CR>ggoSub<Esc>");
    assert_eq!(size(&r, "t2"), 3.0);
    // A count makes each block at that size.
    r.feed("2oX<Esc>");
    assert_eq!((size(&r, "t3"), size(&r, "t4")), (3.0, 3.0));
    // Above it too.
    r.feed("ggOTop<Esc>");
    assert_eq!(size(&r, "t5"), 3.0);
    // From a shape (`L` from `t1` is the circle) or a grid point: the `size` option.
    r.feed("ggLoc<Esc>");
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("t6")));
    assert_eq!(size(&r, "t6"), 0.5);
    r.feed("ggkkkOg<Esc>");
    assert_eq!(size(&r, "t7"), 0.5);
    assert_snapshot!(r.snapshot());
}

#[test]
fn typing_into_a_sized_block_keeps_its_size() {
    let b = board("text id=t1 x=0 y=0 \"ab\" size=2.5\n");
    let r = run(b, "ggAcd<Esc><CR>x");
    assert_eq!(size(&r, "t1"), 2.5);
    assert_snapshot!(r.snapshot());
}

#[test]
fn text_deleted_in_a_block_puts_at_the_size_option() {
    let b = board("text id=t1 x=0 y=0 \"hello\" size=4\n");
    let r = run(b, ":set size=1.5<CR>gg<CR>2x<Esc>5jp");
    assert_eq!(size(&r, "t2"), 1.5);
}

#[test]
fn a_lawhrc_sets_the_size_of_new_blocks() {
    let r = run_with_rc("set size=1.5\n", Board::new(), "ia<Esc>");
    assert_eq!(size(&r, "t1"), 1.5);
}

#[test]
fn a_size_option_out_of_range_is_refused_by_the_board() {
    // `:set` refuses such a size; only the settings' field can hold one.
    let mut engine = Engine::new(Board::new());
    engine.settings_mut().size_hundredths = 2000;
    let mut r = run_engine(engine, "ia");
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert!(r.engine.board().is_empty());
    assert_eq!(message(&r), Some(("sizes go from 0.25 to 16", true)));
    r.feed(":te x<CR>");
    assert_eq!(message(&r), Some(("sizes go from 0.25 to 16", true)));
    assert_eq!(r.engine.cursor(), &Cursor::Grid(Point::ZERO));
}
