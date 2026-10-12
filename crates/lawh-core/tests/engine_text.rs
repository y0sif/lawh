//! Text in blocks, keys in and board out: new blocks, Insert mode, block-Normal mode with its
//! motions and `x`, graphemes, the caps on text, the `escape` setting, undo, `.` and clicks.
//! Text and math are measured with `FixedMeasure`: 0.3 units a character, 0.6 a line.
//!
//! Headers show the cursor's byte offset in the block (` byte=3`) in Insert and block-Normal
//! mode.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use common::{Run, board, run, run_engine, trace};
use insta::assert_snapshot;
use lawh_core::board::{MAX_BOARD_TEXT_CHARS, MAX_OBJECTS, MAX_TEXT_CHARS, MAX_TEXT_LINES};
use lawh_core::engine::{Effect, Engine, Escape, MAX_REPEAT_KEYS, TextCursor};
use lawh_core::{Board, Cursor, Mode, Object, ObjectId, Point, Shape};

fn id(s: &str) -> ObjectId {
    ObjectId::new(s).unwrap()
}

/// The text or math source of object `name`.
fn text(r: &Run, name: &str) -> String {
    match &r.engine.board().get(&id(name)).unwrap().shape {
        Shape::Text { text, .. } => text.clone(),
        Shape::Math { src, .. } => src.clone(),
        other => panic!("{name} has no text: {other:?}"),
    }
}

/// An engine whose `<Esc>` in Insert mode always goes to block-Normal mode.
fn escape_to_block(board: Board) -> Engine {
    let mut engine = Engine::new(board);
    engine.settings_mut().escape = Escape::Block;
    engine
}

/// One text block at the origin.
fn one_text(content: &str) -> Board {
    board(&format!("text id=t1 x=0 y=0 \"{content}\"\n"))
}

// New blocks and Insert mode.

#[test]
fn i_a_capital_i_and_a_start_a_new_block_at_a_grid_point() {
    let script = "2l3jiHello<Esc>j4laworld<Esc>jIx<Esc>jAy<Esc>";
    assert_snapshot!(run(Board::new(), script).snapshot());
}

#[test]
fn the_board_shows_the_text_while_typing() {
    // `<CR>` breaks the line with `\n`, never CRLF.
    let r = run(Board::new(), "iab<CR>c");
    assert_eq!(r.engine.mode(), Mode::Insert);
    assert_eq!(
        r.engine.text_cursor(),
        Some(TextCursor {
            id: id("t1"),
            byte: 4,
            block: false
        })
    );
    assert!(r.engine.is_modified());
    assert_snapshot!(r.snapshot());
}

#[test]
fn i_and_a_on_a_text_block_type_at_its_start_and_end() {
    assert_snapshot!(run(one_text("mid"), "wi[<Esc>a]<Esc>I(<Esc>A)<Esc>").snapshot());
}

#[test]
fn only_text_and_math_have_text_to_edit() {
    let b = board("circle id=c1 x=3 y=0 r=1\n");
    for script in ["wi", "wa", "wI", "wA", "w<CR>"] {
        let r = run(b.clone(), script);
        assert_eq!(r.engine.mode(), Mode::Normal, "{script}");
        assert_eq!(r.engine.board(), &b, "{script}");
        assert_snapshot!(r.header());
    }
    // `<CR>` on a grid point.
    assert_snapshot!(run(Board::new(), "<CR>").header());
}

#[test]
fn insert_mode_keys() {
    // `<Tab>` types a tab, Alt with a character types the character, Ctrl chords and other
    // keys type nothing.
    let r = run(Board::new(), "ia<Tab>b<C-x>c<M-d><F1><PageUp><Insert>e");
    assert_eq!(text(&r, "t1"), "a\tbcde");
    // Arrows move within a line, `<Up>` and `<Down>` keep the column, `<Home>` and `<End>`.
    assert_snapshot!(trace(
        one_text("abc\\nde"),
        "wA<Up><End><Down><Home><Left><Right><Right><Right><Up>X"
    ));
}

#[test]
fn backspace_and_delete_join_lines() {
    let r = run(one_text("ab\\ncd"), "wi<Del><Esc>");
    assert_eq!(text(&r, "t1"), "b\ncd");
    // `<BS>` at the start of the second line deletes the break; at the start of the text it
    // does nothing.
    let r = run(one_text("ab\\ncd"), "w<CR>ji<BS><BS><Left><BS><Esc>");
    assert_eq!(text(&r, "t1"), "acd");
    let r = run(one_text("ab"), "wA<Del><Esc>");
    assert_eq!(text(&r, "t1"), "ab");
}

// Graphemes.

#[test]
fn a_combining_accent_is_one_grapheme() {
    // `e` then U+0301 COMBINING ACUTE ACCENT make one `é`: `<BS>` deletes it whole.
    let r = run(Board::new(), "ie\u{301}e\u{301}<BS><Esc>");
    assert_eq!(text(&r, "t1"), "e\u{301}");
    // Columns count graphemes: `a` is in column 1 after `é`, so `j` lands on `y`.
    let r = run(one_text("e\u{301}a\\nxyz"), "w<CR>lj");
    assert_eq!(r.engine.text_cursor().map(|t| t.byte), Some(6));
    // `x` takes it whole into the register; `p` on the board puts it in a new block.
    assert_snapshot!(run(one_text("e\u{301}a"), "w<CR>x<Esc>p").snapshot());
}

#[test]
fn an_emoji_joined_with_zwj_is_one_grapheme() {
    // a, then man ZWJ woman ZWJ girl (18 bytes, from byte 1), then b at byte 19.
    let family = "a\\u{1F468}\\u{200D}\\u{1F469}\\u{200D}\\u{1F467}b";
    assert_snapshot!(trace(one_text(family), "w<CR>llhx"));
    let r = run(one_text(family), "w<CR>lx");
    assert_eq!(text(&r, "t1"), "ab");
    let r = run(one_text(family), "wA<Left><BS><Esc>");
    assert_eq!(text(&r, "t1"), "ab");
    let r = run(one_text(family), "wA<Left><Left><Right><Esc>");
    assert_eq!(r.engine.board(), &one_text(family));
}

// Block-Normal mode.

#[test]
fn enter_edits_a_math_block() {
    let b = board("math id=m1 x=1 y=2 \"x^2 + 1\"\n");
    let mut r = run(b, "w<CR>");
    assert_eq!(r.engine.mode(), Mode::BlockNormal);
    assert_eq!(
        r.engine.text_cursor(),
        Some(TextCursor {
            id: id("m1"),
            byte: 0,
            block: true
        })
    );
    // `2l` onto `2`, `x` it, `i3<Esc>` back here, `$x` the `1`, `a` after the space.
    r.feed("2lxi3<Esc>$xan<Esc>");
    assert_eq!(r.engine.mode(), Mode::BlockNormal);
    assert_snapshot!(r.snapshot());
    r.feed("<Esc>");
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("m1")));
    assert_eq!(r.engine.text_cursor(), None);
}

#[test]
fn j_and_k_keep_the_wanted_column() {
    // Lines: `abcd` (0..4), `x` (5), `abc` (7..10), empty (11), `abcdef` (12..18).
    let b = one_text("abcd\\nx\\nabc\\n\\nabcdef");
    // From column 3 down every line and back up; `$` keeps to the line ends; `0` to column 0.
    assert_snapshot!(trace(b, "w<CR>3ljjjjkkkk$jjjj0k5j"));
}

#[test]
fn h_and_l_stay_in_the_line() {
    assert_snapshot!(trace(one_text("ab\\ncd"), "w<CR>lllhhhj9l9h"));
}

#[test]
fn word_motions_cross_punctuation_and_lines() {
    // `foo.bar  baz` (0..12), an empty line (13), `  q_1` (14..19). `w` and `b` stop on the
    // empty line, `e` does not; `w` at the last word goes to the last grapheme.
    let b = one_text("foo.bar  baz\\n\\n  q_1");
    assert_snapshot!(trace(b, "w<CR>wwwwwwbbbbbbbeeeeebbbbbbb3w2e"));
}

#[test]
fn word_motions_over_words_take_runs_of_non_blanks() {
    // `a.b,,c` (0..6), three spaces, `(x)` (9..12), `y` (13); then `  --  z` (15..22). `W B E`
    // step over punctuation and runs of spaces, across lines, with counts.
    let b = one_text("a.b,,c   (x) y\\n  --  z");
    assert_snapshot!(trace(b, "w<CR>WWWWWBBBBBBEEEEE2W3B2E"));
    // `w` stops at each run of punctuation where `W` does not.
    let r = run(one_text("a.b,,c   (x)"), "w<CR>4w");
    assert_eq!(r.engine.text_cursor().map(|c| c.byte), Some(5));
    let r = run(one_text("a.b,,c   (x)"), "w<CR>W");
    assert_eq!(r.engine.text_cursor().map(|c| c.byte), Some(9));
}

#[test]
fn word_motions_over_words_are_not_bound_on_the_board() {
    for script in ["wW", "wB", "wE", "wvW"] {
        let r = run(one_text("a b"), script);
        assert_eq!(r.engine.cursor(), &Cursor::Object(id("t1")), "{script}");
        assert_eq!(r.engine.board(), &one_text("a b"));
        assert_eq!(r.engine.message(), None);
    }
}

#[test]
fn x_takes_a_count_within_the_line() {
    let mut r = run(one_text("abcdefgh\\nij"), "w<CR>3l2x");
    assert_eq!(text(&r, "t1"), "abcfgh\nij");
    // Past the end of the line it stops there, and the cursor goes back onto the line.
    r.feed("10x");
    assert_eq!(text(&r, "t1"), "abc\nij");
    assert_snapshot!(r.header());
    // `u` puts the text back and the cursor where it was.
    r.feed("u");
    assert_eq!(text(&r, "t1"), "abcfgh\nij");
    assert_snapshot!(r.header());
    // An empty line has nothing to delete.
    let r = run(one_text("a\\n\\nb"), "w<CR>jx");
    assert_eq!(text(&r, "t1"), "a\n\nb");
}

#[test]
fn x_fills_the_register_for_p_on_the_board() {
    assert_snapshot!(run(one_text("hello world"), "w<CR>5x<Esc>p").snapshot());
}

#[test]
fn i_a_capital_i_and_a_in_a_block() {
    // `I` before the first grapheme that is not blank, `A` at the line's end, `a` on an empty
    // line where the cursor is.
    let r = run(
        one_text("  ab\\n\\ncd"),
        "w<CR>$Ix<Esc>jaY<Esc>jAZ<Esc>0lia<Esc>",
    );
    assert_eq!(text(&r, "t1"), "  xab\nY\ncadZ");
    assert_snapshot!(r.header());
}

#[test]
fn other_keys_do_nothing_in_a_block() {
    let r = run(one_text("ab"), "w<CR>ddpPvyJ<Left>");
    assert_eq!(r.engine.mode(), Mode::BlockNormal);
    assert_eq!(r.engine.board(), &one_text("ab"));
    assert_snapshot!(r.header());
}

#[test]
fn the_command_line_comes_back_to_the_block() {
    let mut r = run(one_text("ab"), "w<CR>l:w x.lawh<CR>");
    assert_eq!(
        r.effects,
        [Effect::Write {
            path: "x.lawh".to_owned(),
            quit_after: false
        }]
    );
    assert_snapshot!(r.header());
    r.feed(":ci 3<Esc>");
    assert_eq!(r.engine.mode(), Mode::BlockNormal);
    // A command that makes an object moves the cursor off the block, to the board.
    r.feed(":ci 1<CR>");
    assert_snapshot!(r.snapshot());
}

// Caps.

#[test]
fn a_text_at_max_text_chars_refuses_one_more() {
    let full = "a".repeat(MAX_TEXT_CHARS);
    let mut r = run(one_text(&full), "wAb");
    assert_eq!(text(&r, "t1"), full);
    assert_snapshot!(r.header());
    // Nothing changed, so no undo step.
    r.feed("<Esc>u");
    assert_eq!(
        r.engine.message().map(|m| m.text.as_str()),
        Some("already at the oldest change")
    );
    // A count on `i` stops typing at the cap.
    let r = run(Board::new(), "10000ia<Esc>");
    assert_eq!(text(&r, "t1"), "a".repeat(MAX_TEXT_CHARS));
    assert_snapshot!(r.header());
}

#[test]
fn a_text_at_max_text_lines_refuses_another_line() {
    let lines = "\\n".repeat(MAX_TEXT_LINES - 1);
    let mut r = run(one_text(&lines), "wA<CR>");
    assert_snapshot!(r.header());
    r.feed("x<Esc>");
    assert_eq!(text(&r, "t1"), "\n".repeat(MAX_TEXT_LINES - 1) + "x");
}

#[test]
fn a_board_at_max_board_text_chars_refuses_more_text() {
    let each = MAX_TEXT_CHARS;
    let mut src = String::new();
    for i in 0..MAX_BOARD_TEXT_CHARS / each {
        writeln!(src, "text id=t{} x=0 y={i} \"{}\"", i + 1, "a".repeat(each)).unwrap();
    }
    let full = board(&src);
    // The new block goes on the board empty, then its first character is refused; it is
    // dropped when Insert mode ends, leaving no undo step.
    let mut r = run(full.clone(), "20jia");
    assert_snapshot!(r.header());
    r.feed("<Esc>");
    assert_eq!(r.engine.board(), &full);
    assert_eq!(r.engine.cursor(), &Cursor::Grid(Point::new(0.0, 20.0)));
    assert!(!r.engine.is_modified());
}

#[test]
fn a_full_board_refuses_a_new_block() {
    let mut b = Board::new();
    for i in 1..=MAX_OBJECTS {
        let shape = Shape::Ellipse {
            center: Point::ZERO,
            rx: 1.0,
            ry: 1.0,
        };
        b.insert(Object::new(id(&format!("c{i}")), shape)).unwrap();
    }
    for script in ["9li", "wo"] {
        let r = run(b.clone(), script);
        assert_eq!(r.engine.mode(), Mode::Normal);
        assert_eq!(r.engine.board(), &b);
        assert_snapshot!(r.header());
    }
}

#[test]
fn a_session_past_max_repeat_keys_is_not_repeated() {
    let long = format!("ib{}<Esc>", "<Left><Right>".repeat(MAX_REPEAT_KEYS / 2));
    // `.` repeats nothing, and a count does not type the keys again.
    let r = run(Board::new(), &format!("{long}."));
    assert_eq!(text(&r, "t1"), "b");
    let r = run(Board::new(), &format!("3{long}"));
    assert_eq!(text(&r, "t1"), "b");
}

// Empty blocks, the escape setting, undo.

#[test]
fn an_empty_new_block_is_dropped() {
    let mut r = run(Board::new(), "3lifoo<BS><BS><BS><Esc>");
    assert_snapshot!(r.snapshot());
    r.feed("u");
    assert_eq!(
        r.engine.message().map(|m| m.text.as_str()),
        Some("already at the oldest change")
    );
    // After `o` the cursor goes back to the object.
    let b = board("rect id=r1 x=0 y=0 w=2 h=1\n");
    let r = run(b.clone(), "wo<Esc>");
    assert_eq!(r.engine.board(), &b);
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("r1")));
    // An existing block left empty stays.
    let r = run(one_text("ab"), "wA<BS><BS><Esc>");
    assert_snapshot!(r.snapshot());
}

#[test]
fn esc_goes_back_where_insert_mode_came_from() {
    // From the board back to the board, from block-Normal mode back there.
    let mut r = run(Board::new(), "ifoo<Esc>");
    assert_snapshot!(r.header());
    r.feed("<CR>$ix<Esc>");
    assert_snapshot!(r.header());
    r.feed("<Esc>");
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert_eq!(text(&r, "t1"), "foxo");
}

#[test]
fn escape_block_always_goes_to_block_normal_mode() {
    let mut r = run_engine(escape_to_block(Board::new()), "ifoo<Esc>");
    assert_snapshot!(r.header());
    r.feed("<Esc>");
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("t1")));
    // From the board onto an existing block too; an empty new block still goes to the board.
    r.feed("a!<Esc>");
    assert_snapshot!(r.header());
    r.feed("<Esc>jo<Esc>");
    assert_snapshot!(r.header());
}

#[test]
fn undo_takes_back_a_whole_session_and_its_cursor() {
    let mut r = run(Board::new(), "3lifoo<CR>bar<Esc>u");
    assert_snapshot!(r.snapshot());
    r.feed("<C-r>");
    assert_snapshot!(r.snapshot());
}

#[test]
fn undo_in_a_block_puts_the_cursor_back_in_it() {
    let mut r = run(one_text("hello world"), "w<CR>wiBIG <Esc>0");
    assert_snapshot!(r.header());
    r.feed("u");
    assert_eq!(text(&r, "t1"), "hello world");
    assert_snapshot!(r.header());
    r.feed("<C-r>");
    assert_snapshot!(r.header());
    // Undoing a change made with the cursor off the block leaves the block for the board.
    let r = run(Board::new(), ":te ab<CR><CR>u");
    assert_snapshot!(r.snapshot());
}

// `.`.

#[test]
fn dot_repeats_an_insert() {
    // `.` on the block types at its start, as `i` there does.
    assert_snapshot!(run(Board::new(), "ifoo<Esc>.").snapshot());
}

#[test]
fn a_count_types_an_insert_again() {
    // `3ia<Esc>` types `aaa`; `.` on a grid point makes another block of it; `2.` types `aa`.
    assert_snapshot!(run(Board::new(), "3ia<Esc>j.2.").snapshot());
}

#[test]
fn dot_after_o_makes_another_block_below() {
    let b = board("rect id=r1 x=0 y=0 w=2 h=1\n");
    assert_snapshot!(run(b, "woab<Esc>.").snapshot());
}

#[test]
fn dot_repeats_x_and_inserts_in_a_block() {
    let mut r = run(one_text("abcdef"), "w<CR>x..");
    assert_eq!(text(&r, "t1"), "def");
    // An insert in the block, then `.` after moving.
    r.feed("i-<Esc>2l.");
    assert_eq!(text(&r, "t1"), "-d-ef");
    // On the board, `x` made in a block is not repeated: `x` there would delete the object.
    r.feed("x<Esc>.");
    assert_snapshot!(r.snapshot());
}

#[test]
fn an_insert_repeats_on_the_board_and_in_a_block() {
    // With `<Esc>` to block-Normal mode, `.` right after `ifoo<Esc>` types in the block, at
    // the cursor (on the last `o`); back on the board it types at the block's start.
    let mut r = run_engine(escape_to_block(Board::new()), "ifoo<Esc>.");
    assert_eq!(text(&r, "t1"), "fofooo");
    r.feed("<Esc>.");
    assert_snapshot!(r.snapshot());
    // `o` is a board change: not repeated in a block.
    let r = run(Board::new(), "oab<Esc><CR>.");
    assert_eq!(r.engine.board().len(), 1);
    assert_eq!(text(&r, "t1"), "ab");
}

#[test]
fn dot_stops_when_its_insert_is_refused() {
    // `idd<Esc>` makes t1 at the origin; `L` goes onto the rectangle, where `i` is refused.
    // `.` shows why and stops: the `dd` it typed in Insert mode does not delete r1.
    let b = board("rect id=r1 x=3 y=0 w=1 h=1\n");
    assert_snapshot!(run(b, "idd<Esc>L.").snapshot());
}

#[test]
fn dot_stops_when_a_full_board_refuses_its_block() {
    // `ijl<Esc>` fills the board. On a grid point `.` cannot make a block: the `j` and `l` it
    // typed do not move the cursor.
    let mut b = Board::new();
    for i in 1..MAX_OBJECTS {
        let shape = Shape::Ellipse {
            center: Point::new(0.0, 10.0),
            rx: 1.0,
            ry: 1.0,
        };
        b.insert(Object::new(id(&format!("c{i}")), shape)).unwrap();
    }
    let r = run(b, "ijl<Esc>5l.");
    assert_eq!(r.engine.board().len(), MAX_OBJECTS);
    assert_snapshot!(r.header());
}

#[test]
fn dot_stops_when_o_finds_no_room() {
    // r2 lies on the board's bottom edge, so `o` below it has no room: the cursor stays on it
    // instead of running `jl` from there.
    let b = board("rect id=r1 x=0 y=0 w=1 h=1\nrect id=r2 x=0 y=999999 w=1 h=0.5\n");
    assert_snapshot!(run(b, "wojl<Esc>J.").snapshot());
}

#[test]
fn dot_in_a_block_does_not_repeat_a_board_change() {
    let mut r = run(one_text("ab"), "wyyp<CR>.");
    assert_eq!(r.engine.board().len(), 2);
    assert_eq!(r.engine.mode(), Mode::BlockNormal);
    r.feed("<Esc>.");
    assert_snapshot!(r.snapshot());
}

// `o` and `O`.

/// A rectangle with a wide bar 0.8 below it.
fn shelf() -> Board {
    board(
        "rect id=r1 x=0 y=0 w=2 h=1
rect id=r2 x=-1 y=1.8 w=3 h=0.5
",
    )
}

#[test]
fn o_places_a_new_block_below_past_obstacles() {
    // The empty block (0 by 0.6) would start at 1.5 and hit r2 (1.8..2.3): it goes to 2.8.
    assert_snapshot!(run(shelf(), "wohi<Esc>").snapshot());
}

#[test]
fn capital_o_places_a_new_block_above() {
    // 0 - 0.5 - 0.6 = -1.1.
    assert_snapshot!(run(shelf(), "wOhi<Esc>").snapshot());
}

#[test]
fn a_count_on_o_stacks_blocks_as_one_change() {
    let mut r = run(shelf(), "w3oab<Esc>");
    assert_snapshot!(r.snapshot());
    r.feed("u");
    assert_eq!(r.engine.board(), &shelf());
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("r1")));
}

#[test]
fn o_and_capital_o_on_a_grid_point() {
    // The grid point is an object of no size: below it at 0.5, then above that block.
    assert_snapshot!(run(Board::new(), "ox<Esc>Oy<Esc>").snapshot());
}

// Clicks.

#[test]
fn a_click_commits_the_session() {
    let mut r = run(Board::new(), "ifoo");
    r.click(5.0, 5.0, None);
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert_eq!(r.engine.text_cursor(), None);
    assert_snapshot!(r.snapshot());
    // One undo step, and `.` repeats it.
    r.feed("u");
    assert!(r.engine.board().is_empty());
    r.feed("<C-r>.");
    assert_eq!(text(&r, "t1"), "foofoo");
    assert_eq!(r.engine.mode(), Mode::Normal);
    // A new block left empty is dropped; a click in a block leaves it.
    let mut r = run(Board::new(), "i");
    r.click(1.0, 1.0, None);
    assert!(r.engine.board().is_empty());
    let mut r = run(one_text("ab"), "w<CR>");
    r.click(0.0, 0.0, Some("t1"));
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("t1")));
}

#[test]
fn a_click_during_a_counted_insert_keeps_it_once() {
    // The count would type `hi` again at `<Esc>`; a click commits it once, and `.` then
    // repeats what was committed: one `hi`.
    let mut r = run(Board::new(), "3ihi");
    r.click(5.0, 5.0, None);
    assert_eq!(text(&r, "t1"), "hi");
    r.feed(".");
    assert_eq!(text(&r, "t2"), "hi");
}

// Cost.

#[test]
fn insert_arrows_look_only_at_their_line() {
    // 495 lines of 9 characters (4,949 in all). Each `<Left>` and `<Up>` used to split the
    // whole text into lines: 10,000 of them took over a second in release.
    let content = vec!["abcdefghi"; 495].join("\\n");
    let started = Instant::now();
    let r = run(
        one_text(&content),
        "w10000A<Left><Esc>10000A<Up><Esc>10000A<Home><End><Esc>",
    );
    let elapsed = started.elapsed();
    assert_eq!(text(&r, "t1").len(), 4949);
    assert!(!r.engine.is_modified());
    let limit = if cfg!(debug_assertions) { 10 } else { 1 };
    assert!(elapsed < Duration::from_secs(limit), "{elapsed:?}");
}
