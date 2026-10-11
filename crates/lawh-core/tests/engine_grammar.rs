//! The board grammar, keys in and board out: object motions, operators and the register, puts
//! and the place rule, Visual mode, `.` and `@:`, the `:` commands, writing and quitting, and
//! clicks. Text and math are measured with `FixedMeasure`: 0.3 units a character, 0.6 a line.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use common::{board, run, run_engine, trace};
use insta::assert_snapshot;
use lawh_core::board::{MAX_COORDINATE, MAX_OBJECTS};
use lawh_core::engine::{Effect, Engine, FixedMeasure};
use lawh_core::{Board, Cursor, Mode, Object, ObjectId, Point, Rect, Shape};

/// Three rows, written out of reading order:
///
/// ```text
/// y 0.5   r1 (0..2)     c1 (4.5..5.5)   t1 (8..8.9)
/// y 3     l1 (0..2)     r2 (4..6)
/// y 5.3   m1 (1..1.9)
/// ```
fn rows() -> Board {
    board(
        "text id=t1 x=8 y=0.2 \"abc\"
math id=m1 x=1 y=5 \"x^2\"
rect id=r1 x=0 y=0 w=2 h=1
rect id=r2 x=4 y=2.5 w=2 h=1
circle id=c1 x=5 y=0.5 r=0.5
line id=l1 x1=0 y1=3 x2=2 y2=3
",
    )
}

fn id(s: &str) -> ObjectId {
    ObjectId::new(s).unwrap()
}

// Motions.

#[test]
fn hjkl_from_an_object_start_at_its_anchor() {
    // `w` lands on r1, whose anchor is its top-left corner (0, 0); from (2, 0), between r1
    // and c1, `w` lands on c1, whose anchor is its centre (5, 0.5).
    assert_snapshot!(trace(rows(), "w2lwj"));
}

#[test]
fn w_and_b_walk_the_reading_order() {
    // From the grid origin, `w` visits rows top to bottom, left to right, and stops at the
    // last object; `b` walks back and stops at the first.
    assert_snapshot!(trace(rows(), "wwwwwwwbbbbbbb"));
}

#[test]
fn w_and_b_from_a_grid_point_and_with_counts() {
    // From (3, 0.5), between r1 and c1 in the first row: `w` is c1, `b` is r1.
    let r = run(rows(), "3lw");
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("c1")));
    let r = run(rows(), "3lb");
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("r1")));
    // Counts: four steps from the origin, then back two; a count past the end stops there.
    assert_snapshot!(trace(rows(), "4w2b99w"));
}

#[test]
fn gg_and_capital_g_go_to_the_first_and_last_object() {
    // Reading order: r1 c1 t1 l1 r2 m1. With a count, the count-th object, or the last when
    // there are fewer. `g` then another key does nothing; in Visual mode they move the cursor.
    assert_snapshot!(trace(rows(), "Ggg3gg3G99G2gggh2lvG<Esc>"));
    // An empty board: the cursor stays.
    let r = run(Board::new(), "3lGgg");
    assert_eq!(r.engine.cursor(), &Cursor::Grid(Point::new(3.0, 0.0)));
}

#[test]
fn d_and_y_with_gg_and_capital_g_take_a_run_in_reading_order() {
    // From t1, third in reading order: `dG` takes t1 to m1, `dgg` r1 to t1, `d2G` c1 and t1.
    assert_snapshot!(run(rows(), "3ggdG").snapshot());
    assert_snapshot!(run(rows(), "3ggdgg").snapshot());
    assert_snapshot!(run(rows(), "3ggd2G").snapshot());
    // `yG` from the last object yanks it alone; `.` repeats `dG` from the new cursor.
    let r = run(rows(), "GyGggp");
    assert_eq!(r.engine.board().len(), 7);
    let r = run(rows(), "5ggdGgg.");
    assert_eq!(r.engine.board().len(), 0);
}

#[test]
fn go_first_puts_the_cursor_on_the_first_object() {
    // A board far from the grid's origin.
    let far = board("circle id=c1 x=500 y=300 r=2\ntext id=t1 x=480 y=290 \"far\"\n");
    let mut r = run(far.clone(), "");
    r.engine.go_first(&mut FixedMeasure);
    assert_snapshot!(r.header());
    assert!(!r.engine.is_modified());
    // An empty board keeps the origin.
    let mut r = run(Board::new(), "");
    r.engine.go_first(&mut FixedMeasure);
    assert_eq!(r.engine.cursor(), &Cursor::Grid(Point::ZERO));
    // No undo step, and `.` still repeats the last change.
    let mut r = run(rows(), "wdd");
    r.engine.go_first(&mut FixedMeasure);
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("c1")));
    r.feed(".");
    assert_eq!(r.engine.board().len(), 4);
    r.feed("uu");
    assert_eq!(r.engine.board(), &rows());
    r.feed("u");
    assert_eq!(
        r.engine.message().map(|m| m.text.as_str()),
        Some("already at the oldest change")
    );
    // Outside Normal mode it does nothing.
    let mut r = run(far, "i");
    r.engine.go_first(&mut FixedMeasure);
    assert_eq!(r.engine.mode(), Mode::Insert);
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("t2")));
}

/// An origin with a rectangle to its right that overlaps it across, and a circle nearer by
/// centre down to the right that does not.
fn compass() -> Board {
    board(
        "rect id=o x=0 y=0 w=2 h=2
circle id=diag x=4 y=4 r=0.5
rect id=far x=6 y=1.5 w=1 h=3
",
    )
}

#[test]
fn shift_hjkl_prefer_objects_that_overlap_across() {
    // `L` from o: `far` overlaps o's rows (1.5..4.5 against 0..2), so it wins over `diag`,
    // whose centre is closer (4.2 against 5.7) but which only lies in the cone.
    let mut r = run(compass(), "wL");
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("far")));
    // `H` from far: o and diag both overlap its rows; diag's facing edge is 1.5 away, o's 4.
    r.feed("H");
    assert_snapshot!(r.snapshot());
}

#[test]
fn shift_hjkl_fall_back_to_the_cone() {
    // `J` from o: nothing below overlaps its columns (0..2). `diag` lies in the cone (3 across
    // for 3 along) and scores 3 + 2 x 3 = 9; `far` is 5.5 across for 2 along, outside it.
    assert_snapshot!(trace(compass(), "wJ"));
}

#[test]
fn shift_hjkl_without_a_candidate_leave_the_cursor() {
    // Nothing is above o or to its left; nothing above a grid point far up either. From
    // there, `J` finds o in the cone (the point is on o's left edge, which does not overlap).
    assert_snapshot!(trace(compass(), "wKH9k9kKJ"));
}

#[test]
fn a_label_inside_a_circle() {
    // The label's centre (4.85, 5) is left of the circle's (5, 5): `H` from the circle reaches
    // it (the overlap and the gap clamped at 0 win), `L` from the label comes back. Their
    // centres share a y, so `J` and `K` find nothing; `w` reads the circle first (x0 3 < 4.4).
    let b = board(
        "circle id=c1 x=5 y=5 r=2
text id=t1 x=4.4 y=4.7 \"r=2\"
",
    );
    assert_snapshot!(trace(b, "wHJKLw"));
}

#[test]
fn shift_hjkl_counts_repeat_from_each_target() {
    let b = board(
        "rect id=a x=0 y=0 w=1 h=1
rect id=b x=3 y=0 w=1 h=1
rect id=c x=6 y=0 w=1 h=1
",
    );
    assert_snapshot!(trace(b, "w2L5H"));
}

// Operators.

#[test]
fn dd_deletes_the_object_and_leaves_the_cursor_on_its_anchor() {
    let mut r = run(rows(), "wwdd");
    assert_eq!(r.engine.pending(), "");
    assert_snapshot!(r.snapshot());
    r.feed("u");
    assert_eq!(r.engine.board(), &rows());
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("c1")));
}

#[test]
fn x_is_dd() {
    assert_eq!(
        run(rows(), "wwx").engine.board(),
        run(rows(), "wwdd").engine.board()
    );
    assert_eq!(
        run(rows(), "w2x").engine.board(),
        run(rows(), "w2dd").engine.board()
    );
}

#[test]
fn yy_keeps_the_board_and_the_cursor() {
    let r = run(rows(), "wwyy");
    assert_eq!(r.engine.board(), &rows());
    assert!(!r.engine.is_modified());
    assert_snapshot!(r.snapshot());
}

#[test]
fn a_count_on_dd_takes_the_next_objects_in_reading_order() {
    // r1, c1, t1: the whole first row.
    assert_snapshot!(run(rows(), "w3dd").snapshot());
}

#[test]
fn d3w_is_3dd() {
    let r = run(rows(), "wd3w");
    assert_eq!(r.engine.board(), run(rows(), "w3dd").engine.board());
    assert_snapshot!(r.snapshot());
}

#[test]
fn counts_on_an_operator_and_its_motion_multiply() {
    // From c1, `2d3w` is `d6w`: c1 and the four after it, as many as there are.
    let mut r = run(rows(), "ww2d3");
    assert_eq!(r.engine.pending(), "2d3");
    r.feed("w");
    assert_snapshot!(r.snapshot());
}

#[test]
fn db_takes_the_objects_before_this_one() {
    // From r2: `db` takes l1 alone, `d2b` l1 and t1. r2 stays.
    assert_snapshot!(run(rows(), "5wdb").snapshot());
    assert_snapshot!(run(rows(), "5wd2b").snapshot());
    // Nothing before the first object: nothing happens.
    let r = run(rows(), "wdb");
    assert_eq!(r.engine.board(), &rows());
    assert!(!r.engine.is_modified());
}

#[test]
fn dj_takes_this_object_and_the_one_below() {
    // J from r1 is l1 (sharing columns 0..2, 2 below); from l1 it is m1.
    assert_snapshot!(run(rows(), "wdJ").snapshot());
    assert_snapshot!(run(rows(), "wd2J").snapshot());
    // Nothing above r1: `dK` does nothing.
    let r = run(rows(), "wdK");
    assert_eq!(r.engine.board(), &rows());
}

#[test]
fn an_operator_then_a_grid_motion_cancels() {
    // `dj` deletes nothing and moves nothing; the next `j` moves.
    let mut r = run(rows(), "wdj");
    assert_eq!(r.engine.board(), &rows());
    assert_eq!(r.engine.pending(), "");
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("r1")));
    r.feed("j");
    assert_snapshot!(r.snapshot());
    // So do other keys, `<Esc>` included.
    let r = run(rows(), "wd<Esc>y3<Esc>dz");
    assert_eq!(r.engine.board(), &rows());
    assert_eq!(r.engine.pending(), "");
}

#[test]
fn operators_need_an_object() {
    assert_snapshot!(run(rows(), "dd").snapshot());
    let r = run(rows(), "x");
    assert_eq!(
        r.engine.message().map(|m| m.text.as_str()),
        Some("no object under the cursor")
    );
}

// Puts.

#[test]
fn p_at_a_grid_point_puts_the_first_anchor_at_the_cursor() {
    // r1 is yanked, then put with its top-left at (9, 1). Its id is taken: r3.
    assert_snapshot!(run(rows(), "wyyj9lp").snapshot());
}

#[test]
fn dd_then_p_restores_with_the_same_id() {
    // The circle comes back where it was, keeping its id, on top of the board. A second `p`
    // puts a copy below it (from y 1 + 0.5) with a fresh id.
    let mut r = run(rows(), "wwddp");
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("c1")));
    let mut objects: Vec<Object> = r.engine.board().objects().to_vec();
    objects.sort_by(|a, b| a.id.cmp(&b.id));
    let mut original = rows().objects().to_vec();
    original.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(objects, original);
    r.feed("p");
    assert_snapshot!(r.snapshot());
}

/// A rectangle with a circle under its right end and a note further down on its left.
fn blocked() -> Board {
    board(
        "rect id=a x=0 y=0 w=4 h=1
circle id=b x=3 y=2 r=0.6
text id=c x=-1 y=2.9 \"note\"
",
    )
}

#[test]
fn the_place_rule_moves_past_every_blocker() {
    // `p` under a (4 by 1): first spot y 1.5, the box 0..4 by 1.5..2.5 hits b (2.4..3.6 by
    // 1.4..2.6), so y = 2.6 + 0.5 = 3.1. That box (3.1..4.1) hits c (-1..0.2 by 2.9..3.5), so
    // y = 3.5 + 0.5 = 4. Nothing is there: the copy goes to (0, 4).
    assert_snapshot!(run(blocked(), "wyyp").snapshot());
}

#[test]
fn the_place_rule_counts_flat_boxes() {
    // A copy of l1 below it would start at y 0.5, right on l2: it goes past l2, to y 1.
    let b = board(
        "line id=l1 x1=0 y1=0 x2=5 y2=0
line id=l2 x1=0 y1=0.5 x2=5 y2=0.5
",
    );
    assert_snapshot!(run(b, "wyyp").snapshot());
    // A copy of the upright line l1 would run down r1's left edge (x 0, y 2.5..4.5): it goes
    // past r1, to y 5.
    let b = board(
        "line id=l1 x1=0 y1=0 x2=0 y2=2
rect id=r1 x=0 y=2.5 w=3 h=2
",
    );
    assert_snapshot!(run(b, "wyyp").snapshot());
}

#[test]
fn p_and_capital_p_round_y_outward() {
    // "hello" at (0.1234, 0.25) is 1.5 by 0.6: below, 0.85 + 0.5 = 1.35 rounds up to 1.4;
    // above (from the original, `b` goes back to it), 0.25 - 0.5 - 0.6 = -0.85 rounds down to
    // -0.9. x is rounded to thousandths.
    let b = board("text id=t1 x=0.1234 y=0.25 \"hello\"\n");
    assert_snapshot!(run(b, "wyypbP").snapshot());
}

#[test]
fn no_room_near_max_coordinate() {
    let b = board(&format!(
        "rect id=r1 x=0 y={} w=2 h=1\n",
        MAX_COORDINATE - 1.5
    ));
    let r = run(b.clone(), "wyyp");
    assert_eq!(r.engine.board(), &b);
    assert!(!r.engine.is_modified());
    assert_snapshot!(r.snapshot());
    // Above there is room.
    assert_eq!(run(b, "wyyP").engine.board().len(), 2);
}

#[test]
fn a_count_on_p_stacks_copies() {
    // Each copy goes 0.5 below the one before: centres at y 2.5, 5 and 7.5. (The circle is
    // right of the origin, so `w` reaches it: a point inside it would come after it.)
    let b = board("circle id=c1 x=3 y=0 r=1\n");
    assert_snapshot!(run(b, "wyy3p").snapshot());
}

#[test]
fn a_count_on_p_at_a_grid_point_stacks_below_the_first() {
    let b = board("rect id=r1 x=0 y=0 w=1 h=1\n");
    assert_snapshot!(run(b, "wyy5l2p").snapshot());
}

#[test]
fn puts_are_all_or_nothing_against_max_objects() {
    let mut b = Board::new();
    for i in 1..MAX_OBJECTS {
        let shape = Shape::Ellipse {
            center: Point::new(i as f64 * 3.0, 0.0),
            rx: 1.0,
            ry: 1.0,
        };
        b.insert(Object::new(id(&format!("c{i}")), shape)).unwrap();
    }
    // 499 objects: two more do not fit, so nothing is put.
    let mut r = run(b.clone(), "w2yyp");
    assert_eq!(r.engine.board(), &b);
    assert_snapshot!(r.header());
    r.feed("u");
    assert_eq!(
        r.engine.message().map(|m| m.text.as_str()),
        Some("already at the oldest change")
    );
    // One does.
    r.feed("yyp");
    assert_eq!(r.engine.board().len(), MAX_OBJECTS);
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("c500")));
    // A huge count is refused at once.
    r.feed("u9999p");
    assert_eq!(r.engine.board(), &b);
}

#[test]
fn p_with_an_empty_register_says_so() {
    assert_snapshot!(run(rows(), "wp").snapshot());
}

// Visual mode.

#[test]
fn visual_selects_objects_inside_the_rectangle() {
    // From the origin to (7, 3): r1, c1 and l1 (its y 3 is on the edge) lie inside; r2 ends
    // at y 3.5 and t1 starts at x 8.
    let mut r = run(rows(), "v3j7l");
    assert_eq!(r.engine.mode(), Mode::Visual);
    assert_eq!(
        r.engine.visual_rect(&mut FixedMeasure),
        Some(Rect::new(0.0, 0.0, 7.0, 3.0))
    );
    assert_eq!(r.engine.selection(), [id("r1"), id("c1"), id("l1")]);
    assert_snapshot!(r.header());
    // `d` deletes them as one step and leaves Visual mode.
    r.feed("d");
    assert_snapshot!(r.snapshot());
    r.feed("u");
    assert_eq!(r.engine.board(), &rows());
}

#[test]
fn visual_yank_then_put() {
    // The yank leaves the cursor on the first object yanked, r1; the put at (0, 8) moves the
    // group so r1's anchor is there, with fresh ids.
    let mut r = run(rows(), "v3j7ly");
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert_eq!(r.engine.selection(), []);
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("r1")));
    r.feed("8jp");
    assert_snapshot!(r.snapshot());
}

#[test]
fn visual_from_a_circle_takes_its_label_and_radius() {
    let b = board(
        "circle id=c1 x=5 y=5 r=2
text id=t1 x=5.2 y=4.4 \"r\"
line id=l1 x1=5 y1=5 x2=7 y2=5
rect id=r9 x=10 y=10 w=1 h=1
",
    );
    // `v` on the circle spans its bounds, which hold the label and the radius.
    let mut r = run(b, "wv");
    assert_eq!(r.engine.selection(), [id("c1"), id("t1"), id("l1")]);
    assert_snapshot!(r.header());
    // Deleted, moved 10 to the right and put back: the same ids, the same shapes 10 over.
    r.feed("d10lp");
    assert_snapshot!(r.snapshot());
}

#[test]
fn leaving_visual_mode() {
    for leave in ["<Esc>", "v"] {
        let r = run(rows(), &format!("v3j7l{leave}"));
        assert_eq!(r.engine.mode(), Mode::Normal, "{leave}");
        assert_eq!(r.engine.selection(), []);
        assert_eq!(r.engine.visual_rect(&mut FixedMeasure), None);
        assert_eq!(r.engine.board(), &rows());
    }
    // `:` leaves Visual mode for the command line.
    let r = run(rows(), "v3j:");
    assert_eq!(r.engine.mode(), Mode::CommandLine);
    assert_eq!(r.engine.selection(), []);
    // An empty selection deletes nothing.
    let r = run(rows(), "9k9kvjd");
    assert_snapshot!(r.header());
}

// `.` and `@:`.

#[test]
fn dot_repeats_the_last_change() {
    // `dd` on r1, then on c1 by `.`.
    assert_snapshot!(run(rows(), "wddw.").snapshot());
}

#[test]
fn dot_reuses_the_count_or_takes_a_new_one() {
    // `d2w` takes r1 and c1; `.` takes the next two (t1, l1).
    assert_snapshot!(run(rows(), "wd2ww.").snapshot());
    // `3.` takes t1, l1 and r2 instead, and the 3 sticks: the next `.` takes m1, all there is.
    let mut r = run(rows(), "wd2ww3.");
    assert_eq!(r.engine.board().len(), 1);
    r.feed("w.");
    assert_snapshot!(r.snapshot());
}

#[test]
fn dot_repeats_puts_but_not_yanks_or_motions() {
    // `p` puts r2 below r1. The yank and the motion back to r1 are not changes, so `.` puts
    // again below r1: r2 is in the way (1.5..2.5), so r3 goes to 2.5 + 0.5 = 3.
    let b = board("rect id=r1 x=0 y=0 w=1 h=1\n");
    assert_snapshot!(run(b, "wyypyyb.").snapshot());
}

#[test]
fn dot_after_a_visual_change_does_nothing() {
    let r = run(rows(), "wddv3j7ldw.");
    let before_dot = run(rows(), "wddv3j7ldw");
    assert_eq!(r.engine.board(), before_dot.engine.board());
}

#[test]
fn at_colon_repeats_the_last_command_line() {
    // `@:` runs `:ci 1` again at the new cursor; a count does not repeat it.
    assert_snapshot!(run(Board::new(), ":ci 1<CR>l@:l3@:").snapshot());
    assert_snapshot!(run(Board::new(), "@:").snapshot());
}

// `:` commands.

#[test]
fn shape_commands_place_relative_to_the_cursor() {
    // Ellipse around the cursor, rectangle from it, line and arrow from it by (DX, DY), and
    // keys that place them exactly.
    let script =
        "2l3j:el 2 1<CR>:re 3 2<CR>:li 2 -1<CR>:ar 1 1 y2=0<CR>:li x1=0 y1=0 x2=1 y2=1<CR>";
    assert_snapshot!(run(Board::new(), script).snapshot());
}

#[test]
fn text_and_math_commands() {
    let script = ":te  two spaces | \"kept\" <CR>j:ma A = pi r^2<CR>j:text x<CR>";
    assert_snapshot!(run(Board::new(), script).snapshot());
}

#[test]
fn shape_commands_have_their_own_messages() {
    assert_snapshot!(run(Board::new(), ":el 1 -1<CR>").snapshot());
    assert_snapshot!(run(Board::new(), ":li 1<CR>").snapshot());
    // Past the board's edge, from the cursor (at x 1000000, a line 5 long ends past it) or
    // exactly. A rectangle there is fine: the board limits points, not sizes.
    let far = format!("{}:re 2 1<CR>:li 5 0<CR>", "10000l".repeat(100));
    assert_snapshot!(run(Board::new(), &far).header());
    assert_snapshot!(run(Board::new(), ":ci 1 x=-1000001<CR>").snapshot());
    // Text past the board's caps.
    let long = "x".repeat(lawh_core::board::MAX_TEXT_CHARS + 1);
    assert_snapshot!(run(Board::new(), &format!(":te {long}<CR>")).header());
}

#[test]
fn new_objects_get_ids_by_kind() {
    let script =
        ":ci 1<CR>:el 1 2<CR>:el 2 2<CR>:re 1 1<CR>:li 1 1<CR>:ar 1 1<CR>:te a<CR>:ma b<CR>";
    assert_snapshot!(run(Board::new(), script).snapshot());
}

// Writing and quitting.

#[test]
fn write_needs_a_file_name() {
    let r = run(Board::new(), ":ci 1<CR>:w<CR>");
    assert_eq!(r.effects, []);
    assert_snapshot!(r.header());
}

#[test]
fn write_names_an_unnamed_board_then_reports() {
    let mut r = run(Board::new(), ":ci 1<CR>:w a.lawh<CR>");
    assert_eq!(
        r.effects,
        [Effect::Write {
            path: "a.lawh".to_owned(),
            quit_after: false
        }]
    );
    assert_eq!(r.engine.file_name(), Some("a.lawh"));
    assert!(r.engine.is_modified());
    r.write_done("a.lawh", Ok(()));
    assert!(!r.engine.is_modified());
    assert_eq!(r.effects.len(), 1);
    assert_snapshot!(r.header());
    // `:w` now writes to it; a failure says so and leaves the board modified.
    r.feed("l:ci 2<CR>:w<CR>");
    r.write_done("a.lawh", Err("permission denied"));
    assert!(r.engine.is_modified());
    assert_snapshot!(r.header());
}

#[test]
fn write_to_another_file_is_a_copy() {
    let engine = Engine::new(Board::new()).with_file_name(Some("main.lawh".to_owned()));
    let mut r = run_engine(engine, ":ci 1<CR>:w copy.lawh<CR>");
    r.write_done("copy.lawh", Ok(()));
    assert_eq!(r.engine.file_name(), Some("main.lawh"));
    assert!(r.engine.is_modified());
}

#[test]
fn quit_refuses_while_modified() {
    let mut r = run(Board::new(), ":q<CR>");
    assert_eq!(r.effects, [Effect::Quit]);
    r.feed(":ci 1<CR>:q<CR>");
    assert_eq!(r.effects, [Effect::Quit]);
    assert_snapshot!(r.header());
    r.feed(":q!<CR>");
    assert_eq!(r.effects, [Effect::Quit, Effect::Quit]);
}

#[test]
fn wq_writes_then_quits() {
    let engine = Engine::new(Board::new()).with_file_name(Some("b.lawh".to_owned()));
    let mut r = run_engine(engine, ":ci 1<CR>:wq<CR>");
    let write = Effect::Write {
        path: "b.lawh".to_owned(),
        quit_after: true,
    };
    assert_eq!(r.effects, std::slice::from_ref(&write));
    r.write_done("b.lawh", Ok(()));
    assert_eq!(r.effects, [write, Effect::Quit]);
    // A failed write does not quit.
    let engine = Engine::new(Board::new()).with_file_name(Some("b.lawh".to_owned()));
    let mut r = run_engine(engine, ":wq!<CR>");
    r.write_done("b.lawh", Err("disk full"));
    assert_eq!(r.effects.len(), 1);
    assert!(r.engine.message().is_some_and(|m| m.error));
}

#[test]
fn x_writes_only_when_modified() {
    let named = || Engine::new(Board::new()).with_file_name(Some("c.lawh".to_owned()));
    let r = run_engine(named(), ":x<CR>");
    assert_eq!(r.effects, [Effect::Quit]);
    let mut r = run_engine(named(), ":ci 1<CR>:xit<CR>");
    assert_eq!(
        r.effects,
        [Effect::Write {
            path: "c.lawh".to_owned(),
            quit_after: true
        }]
    );
    r.write_done("c.lawh", Ok(()));
    assert_eq!(r.effects.last(), Some(&Effect::Quit));
    // Unnamed and modified: no file to write.
    let r = run(Board::new(), ":ci 1<CR>:x<CR>");
    assert_eq!(r.effects, []);
}

#[test]
fn modified_follows_undo_back_to_the_saved_point() {
    let engine = Engine::new(Board::new()).with_file_name(Some("d.lawh".to_owned()));
    let mut r = run_engine(engine, ":ci 1<CR>:w<CR>");
    r.write_done("d.lawh", Ok(()));
    assert!(!r.engine.is_modified());
    r.feed("l:ci 2<CR>");
    assert!(r.engine.is_modified());
    r.feed("u");
    assert!(!r.engine.is_modified());
    r.feed("u");
    assert!(r.engine.is_modified());
    r.feed("<C-r>");
    assert!(!r.engine.is_modified());
    // A change made while the write was out: the write does not count as saving it.
    let engine = Engine::new(Board::new()).with_file_name(Some("d.lawh".to_owned()));
    let mut r = run_engine(engine, ":ci 1<CR>:w<CR>l:ci 2<CR>");
    r.write_done("d.lawh", Ok(()));
    assert!(r.engine.is_modified());
}

#[test]
fn the_same_file_written_another_way_saves_the_board() {
    let named = || Engine::new(Board::new()).with_file_name(Some("a.lawh".to_owned()));
    for path in ["./a.lawh", "dir/../a.lawh", "./dir/./sub/../.././a.lawh"] {
        let mut r = run_engine(named(), &format!(":ci 1<CR>:w {path}<CR>"));
        r.write_done(path, Ok(()));
        assert!(!r.engine.is_modified(), "{path}");
        assert_eq!(r.engine.file_name(), Some("a.lawh"));
    }
    // Other files: copies.
    for path in ["../a.lawh", "dir/a.lawh", "/a.lawh", "a.lawh/.."] {
        let mut r = run_engine(named(), &format!(":ci 1<CR>:w {path}<CR>"));
        r.write_done(path, Ok(()));
        assert!(r.engine.is_modified(), "{path}");
    }
}

#[test]
fn wq_to_another_file_writes_a_copy_and_stays() {
    // The board's own file still lacks the changes, so `:wq other` does not quit after the
    // copy (as Vim); `:wq!` does.
    let named = || Engine::new(Board::new()).with_file_name(Some("b.lawh".to_owned()));
    for command in ["wq", "x"] {
        let mut r = run_engine(named(), &format!(":ci 1<CR>:{command} other.lawh<CR>"));
        assert_eq!(
            r.effects,
            [Effect::Write {
                path: "other.lawh".to_owned(),
                quit_after: true
            }]
        );
        r.write_done("other.lawh", Ok(()));
        assert_eq!(r.effects.len(), 1, "{command}");
        assert_snapshot!(r.header());
        r.feed(":q!<CR>");
        assert_eq!(r.effects.last(), Some(&Effect::Quit));
        // Forced, or with nothing left unsaved, it quits.
        let mut r = run_engine(named(), &format!(":ci 1<CR>:{command}! other.lawh<CR>"));
        r.write_done("other.lawh", Ok(()));
        assert_eq!(r.effects.last(), Some(&Effect::Quit), "{command}!");
    }
    let mut r = run_engine(named(), ":wq other.lawh<CR>");
    r.write_done("other.lawh", Ok(()));
    assert_eq!(r.effects.last(), Some(&Effect::Quit));
}

// Shift on named keys.

#[test]
fn shift_with_esc_cr_bs_or_del_is_the_plain_key() {
    // On the command line: `<S-BS>` deletes, `<S-CR>` runs.
    let r = run(Board::new(), ":ci 12<S-BS><S-CR>");
    assert_snapshot!(r.snapshot());
    // `<S-Esc>` leaves Visual mode, the command line, Insert mode and block-Normal mode;
    // `<S-CR>` enters a block.
    let r = run(rows(), "v<S-Esc>");
    assert_eq!(r.engine.mode(), Mode::Normal);
    let r = run(Board::new(), ":ci 1<S-Esc>");
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert!(r.engine.board().is_empty());
    let mut r = run(Board::new(), "iab<S-BS>c<S-Esc>");
    assert_eq!(r.engine.mode(), Mode::Normal);
    r.feed("<S-CR>");
    assert_eq!(r.engine.mode(), Mode::BlockNormal);
    r.feed("<S-Esc>");
    assert_snapshot!(r.snapshot());
}

// Clicks.

#[test]
fn click_on_an_object_and_on_empty_space() {
    let mut r = run(rows(), "");
    r.click(5.2, 0.4, Some("c1"));
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("c1")));
    // Empty space: the nearest whole grid point.
    r.click(2.6, -1.4, None);
    assert_eq!(r.engine.cursor(), &Cursor::Grid(Point::new(3.0, -1.0)));
    // A click ends Visual mode, the command line and pending keys.
    r.feed("v3j");
    r.click(0.0, 0.0, Some("r1"));
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert_eq!(r.engine.selection(), []);
    r.feed(":ci 3");
    r.click(0.0, 0.0, None);
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert_eq!(r.engine.command_line(), None);
    r.feed("2d");
    r.click(9.0, 9.0, Some("nope"));
    assert_eq!(r.engine.pending(), "");
    assert_eq!(r.engine.cursor(), &Cursor::Grid(Point::new(9.0, 9.0)));
    assert_snapshot!(r.snapshot());
}
