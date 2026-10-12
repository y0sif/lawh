//! `:sym`, keys in and board out: the completions and their order, the completion keys, `<CR>`
//! and `<Esc>`, picks into math, text and the board, undo, `@:`, `<C-k>` from Insert mode and
//! `.` after it, mappings, the query cap, and the symbols the engine keeps. The symbols are a
//! small hand-made list of Typst names (`SYMBOLS`).
//!
//! Snapshots add the command line to the header (` line="sym al"`), then the completions, one
//! row a line, `>` on the row selected.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use std::fmt::Write as _;
use std::sync::Arc;

use common::{Run, board, run, run_engine, run_with_symbols, symbol};
use insta::assert_snapshot;
use lawh_core::engine::{Effect, Engine, MAX_SYM_QUERY_CHARS, MAX_SYMBOLS, SYM_ROWS, Symbol};
use lawh_core::key::format_keys;
use lawh_core::{Board, Cursor, Mode, ObjectId, Point, Shape, parse_keys, to_lawh};

/// Typst names and their values, not in order of name: the engine sorts them.
const SYMBOLS: &[(&str, &str)] = &[
    ("alpha", "α"),
    ("Alpha", "Α"),
    ("angle", "∠"),
    ("approx", "≈"),
    ("arrow.r", "→"),
    ("arrow.l", "←"),
    ("arrow.l.r", "↔"),
    ("arrow.r.long", "⟶"),
    ("beta", "β"),
    ("dots.h", "…"),
    ("gamma", "γ"),
    ("harpoon.rt", "⇀"),
    ("infinity", "∞"),
    ("integral", "∫"),
    ("integral.double", "∬"),
    ("lambda", "λ"),
    ("parallel", "∥"),
    ("pi", "π"),
    ("Pi", "Π"),
    ("plus.minus", "±"),
    ("star", "⋆"),
    ("star.filled", "★"),
    ("sum", "∑"),
    ("times", "×"),
];

fn symbols() -> Arc<[Symbol]> {
    SYMBOLS.iter().map(|(n, v)| symbol(n, v)).collect()
}

/// Types `script` into an engine for `board` that has [`SYMBOLS`].
fn sym_run(board: Board, script: &str) -> Run {
    run_with_symbols("", symbols(), board, script)
}

/// [`sym_run`] after reading `rc` as a lawhrc.
fn sym_rc(rc: &str, board: Board, script: &str) -> Run {
    run_with_symbols(rc, symbols(), board, script)
}

fn id(s: &str) -> ObjectId {
    ObjectId::new(s).unwrap()
}

/// A board of one math block, `m1` at the origin.
fn math(src: &str) -> Board {
    board(&format!("math id=m1 x=0 y=0 {src:?}\n"))
}

/// A board of one text block, `t1` at the origin.
fn text(t: &str) -> Board {
    board(&format!("text id=t1 x=0 y=0 {t:?}\n"))
}

/// The message and whether it is an error.
fn message(r: &Run) -> Option<(&str, bool)> {
    r.engine.message().map(|m| (m.text.as_str(), m.error))
}

/// The names of the completion rows, `>` before the one selected: `alpha >Alpha`; empty for no
/// rows, `None` with no completions.
fn rows(r: &Run) -> Option<String> {
    let (rows, selected) = r.engine.completions()?;
    let names: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mark = if Some(i) == selected { ">" } else { "" };
            format!("{mark}{}", s.name)
        })
        .collect();
    Some(names.join(" "))
}

/// The command line's text.
fn line(r: &Run) -> Option<&str> {
    r.engine.command_line().map(|(line, _)| line)
}

/// The text or formula of object `name`.
fn source(r: &Run, name: &str) -> String {
    match &r.engine.board().get(&id(name)).unwrap().shape {
        Shape::Text { text, .. } => text.clone(),
        Shape::Math { src, .. } => src.clone(),
        other => panic!("{name} has no text: {other:?}"),
    }
}

/// The header, the command line and the completions.
fn status(r: &Run) -> String {
    let mut out = r.header();
    if let Some(line) = line(r) {
        let _ = write!(out, " line={line:?}");
    }
    if let Some((rows, selected)) = r.engine.completions() {
        for (i, row) in rows.iter().enumerate() {
            let mark = if Some(i) == selected { '>' } else { ' ' };
            let _ = write!(out, "\n// {mark} {} {}", row.name, row.value);
        }
    }
    out
}

/// [`status`], then the board as a `.lawh` file.
fn snapshot(r: &Run) -> String {
    format!("{}\n{}", status(r), to_lawh(r.engine.board()))
}

/// Types `script` one key at a time into an engine for `board` with [`SYMBOLS`]: one line per
/// key, the key and [`status`] after it, then the board.
fn trace(board: Board, script: &str) -> String {
    let mut r = sym_run(board, "");
    let mut lines = Vec::new();
    for key in parse_keys(script).unwrap() {
        let key = format_keys(&[key]);
        r.feed(&key);
        lines.push(format!("{key:>7} {}", status(&r)));
    }
    format!("{}\n{}", lines.join("\n"), to_lawh(r.engine.board()))
}

// The completions.

#[test]
fn completions_show_the_best_matches_best_first() {
    let r = sym_run(Board::new(), ":sym al");
    assert_eq!(r.engine.completions().unwrap().0.len(), SYM_ROWS);
    assert_snapshot!(snapshot(&r));
}

#[test]
fn the_exact_name_comes_first_then_the_score_then_the_name() {
    let rows = |query: &str| rows(&sym_run(Board::new(), &format!(":sym {query}"))).unwrap();
    // `alpha` and `Alpha` both match `alpha` as well: the name typed comes first.
    assert_eq!(rows("alpha"), "alpha Alpha");
    assert_eq!(rows("Alpha"), "Alpha");
    assert_eq!(rows("pi"), "pi Pi plus.minus");
    // Ties go by name, a small letter before its capital.
    assert_eq!(rows("alp"), "alpha Alpha");
    // The four arrows score alike and go by name; `harpoon.rt` has a gap in its match.
    assert_eq!(
        rows("arr"),
        "arrow.l arrow.l.r arrow.r arrow.r.long harpoon.rt"
    );
    // Fuzzy: `int` starts `integral` and `integral.double`, and is spread over `infinity`.
    assert_eq!(rows("int"), "integral integral.double infinity");
    // A capital matches case: only `Alpha`.
    assert_eq!(rows("Al"), "Alpha");
    // Every word must match.
    assert_eq!(rows("arrow r"), "arrow.l.r arrow.r arrow.r.long arrow.l");
    // fzf's syntax: `!a` leaves out every name with an `a`.
    assert_eq!(rows("!a"), "dots.h infinity pi Pi plus.minus sum times");
    // No match, or no query: no rows.
    assert_eq!(rows("xyz"), "");
    assert_eq!(rows(""), "");
    assert_eq!(rows("^"), "");
}

#[test]
fn completions_show_on_a_sym_line_and_follow_its_query() {
    let mut r = sym_run(Board::new(), ":sym");
    assert_eq!(rows(&r), None);
    r.feed(" ");
    assert_eq!(rows(&r).as_deref(), Some(""));
    r.feed("p");
    let p = rows(&r).unwrap();
    assert!(p.starts_with("parallel pi Pi "), "{p}");
    r.feed("l");
    assert_eq!(rows(&r).as_deref(), Some("plus.minus parallel"));
    r.feed("<BS>");
    assert_eq!(rows(&r), Some(p));
    // `:sy` too, after any `:` and spaces.
    let r = sym_run(Board::new(), ": sy  st");
    assert_eq!(rows(&r).as_deref(), Some("star star.filled"));
    // Not on other lines, nor once the command line closes.
    for script in [":set ", ":syms st", ":sym st<Esc>", ":sym st<BS><BS><BS>"] {
        assert_eq!(rows(&sym_run(Board::new(), script)), None, "{script}");
    }
}

#[test]
fn matching_runs_again_only_when_the_query_changes() {
    let mut r = sym_run(Board::new(), ":sym al");
    let al = rows(&r);
    assert!(r.engine.key_work() > 0);
    // Moving on the line, and spaces after the query, leave it as it was.
    for key in ["<Left>", "<End>", "<Space>", "<Home>"] {
        r.feed(key);
        assert_eq!((r.engine.key_work(), rows(&r)), (0, al.clone()), "{key}");
    }
    r.feed("<End><BS>p");
    assert!(r.engine.key_work() > 0);
    assert_eq!(rows(&r).as_deref(), Some("alpha Alpha"));
}

// The completion keys.

#[test]
fn tab_and_ctrl_n_put_the_next_name_on_the_line_shift_tab_and_ctrl_p_the_one_before() {
    let mut r = sym_run(Board::new(), ":sym arr");
    // Past either end the query comes back as typed, as in Vim's wildmenu.
    for (key, expected, selected) in [
        ("<Tab>", "sym arrow.l", Some(0)),
        ("<Tab>", "sym arrow.l.r", Some(1)),
        ("<C-n>", "sym arrow.r", Some(2)),
        ("<Tab>", "sym arrow.r.long", Some(3)),
        ("<C-n>", "sym harpoon.rt", Some(4)),
        ("<Tab>", "sym arr", None),
        ("<Tab>", "sym arrow.l", Some(0)),
        ("<S-Tab>", "sym arr", None),
        ("<S-Tab>", "sym harpoon.rt", Some(4)),
        ("<C-p>", "sym arrow.r.long", Some(3)),
    ] {
        r.feed(key);
        assert_eq!(line(&r), Some(expected), "{key}");
        // The rows stay: the name put on the line is not matched again.
        let (rows, now) = r.engine.completions().unwrap();
        assert_eq!((rows.len(), now), (5, selected), "{key}");
        assert!(r.engine.key_work() < 100, "{key}");
    }
    assert_eq!(
        rows(&r).as_deref(),
        Some("arrow.l arrow.l.r arrow.r >arrow.r.long harpoon.rt")
    );
    // The cursor goes to the end of the line.
    assert_eq!(r.engine.command_line(), Some(("sym arrow.r.long", 16)));
    // What comes before the query stays as typed.
    let r = sym_run(Board::new(), ":  sy  arr<Tab>");
    assert_eq!(line(&r), Some("  sy  arrow.l"));
    // With no rows, and on other lines, the keys do nothing.
    let r = sym_run(Board::new(), ":sym xyz<Tab><C-n><S-Tab>");
    assert_eq!(line(&r), Some("sym xyz"));
    let r = sym_run(Board::new(), ":ci 2<Tab><C-p>");
    assert_eq!(line(&r), Some("ci 2"));
}

#[test]
fn typing_after_a_tab_matches_the_line_as_it_is() {
    let mut r = sym_run(Board::new(), ":sym arr<Tab><Tab>");
    assert_eq!(line(&r), Some("sym arrow.l.r"));
    r.feed("<BS><BS>");
    assert_eq!(line(&r), Some("sym arrow.l"));
    assert_eq!(rows(&r).as_deref(), Some("arrow.l arrow.l.r arrow.r.long"));
    r.feed("<Tab>");
    assert_eq!(line(&r), Some("sym arrow.l"));
    assert_eq!(rows(&r).as_deref(), Some(">arrow.l arrow.l.r arrow.r.long"));
}

// `<CR>` and `<Esc>`.

#[test]
fn enter_on_the_board_makes_a_math_block_of_the_name_at_the_cursor() {
    // Of the `size` option's size, as `:ma`.
    let r = sym_run(Board::new(), "3l2j:set size=2<CR>:sym alpha<CR>");
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert_eq!(r.engine.completions(), None);
    assert_snapshot!(snapshot(&r));
}

#[test]
fn enter_takes_the_exact_name_else_the_row_selected_else_the_best_match() {
    let picked = |script: &str| {
        let r = sym_run(Board::new(), script);
        assert_eq!(message(&r), None, "{script}");
        source(&r, "m1")
    };
    assert_eq!(picked(":sym pi<CR>"), "pi");
    assert_eq!(picked(":sym Pi<CR>"), "Pi");
    assert_eq!(picked(":sym arr<CR>"), "arrow.l");
    assert_eq!(picked(":sym arr<Tab><Tab><CR>"), "arrow.l.r");
    assert_eq!(picked(":sym arr<S-Tab><CR>"), "harpoon.rt");
    assert_eq!(picked(":sy   lam   <CR>"), "lambda");
    // The exact name needs no completions: `@:` and mappings type it whole.
    let r = sym_rc("nnoremap Q :sym beta<CR>\n", Board::new(), "Q");
    assert_eq!(source(&r, "m1"), "beta");
}

#[test]
fn enter_with_no_name_or_no_match_says_so() {
    for (script, expected) in [
        (":sym<CR>", "missing the symbol's name: `:sy[m] <name>`"),
        (":sym   <CR>", "missing the symbol's name: `:sy[m] <name>`"),
        (":sym xyz<CR>", "no symbol matches \"xyz\""),
        (":sym ^<CR>", "no symbol matches \"^\""),
        (":sym! pi<CR>", "`:sym` takes no `!`"),
    ] {
        let r = sym_run(Board::new(), script);
        assert_eq!(message(&r), Some((expected, true)), "{script}");
        assert!(r.engine.board().is_empty());
        assert_eq!(r.engine.mode(), Mode::Normal);
    }
    // An engine the app gave no symbols.
    let r = run(Board::new(), ":sym pi<CR>");
    assert_eq!(message(&r), Some(("no symbols to pick from", true)));
}

#[test]
fn esc_cancels_the_sym_line() {
    let r = sym_run(Board::new(), ":sym al<Tab><Esc>");
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert_eq!(r.engine.completions(), None);
    assert_eq!(message(&r), None);
    assert!(r.engine.board().is_empty());
    assert!(!r.engine.is_modified());
}

// Picks into blocks.

#[test]
fn a_name_put_into_math_is_spaced_from_a_name_it_would_touch() {
    // At the end (`A`) and the start (`I`) of a formula, from Insert mode.
    for (src, entry, expected) in [
        ("x", "A", "x alpha"),
        ("2", "A", "2alpha"),
        ("x2", "A", "x2 alpha"),
        ("(x)", "A", "(x)alpha"),
        ("x + ", "A", "x + alpha"),
        ("é", "A", "é alpha"),
        ("", "A", "alpha"),
        ("x", "I", "alpha x"),
        ("2", "I", "alpha 2"),
        (")", "I", "alpha)"),
    ] {
        let r = sym_run(math(src), &format!("gg{entry}<C-k>alpha<CR>"));
        assert_eq!(source(&r, "m1"), expected, "{src:?} {entry}");
        assert_eq!(message(&r), None);
    }
    // A line break is not a letter: at the end of the first line, and the start of the second.
    let r = sym_run(math("x\ny"), "gg<CR>a<C-k>alpha<CR>");
    assert_eq!(source(&r, "m1"), "x alpha\ny");
    let r = sym_run(math("x\ny"), "gg<CR>ji<C-k>alpha<CR>");
    assert_eq!(source(&r, "m1"), "x\nalpha y");
    assert_snapshot!(snapshot(&r));
}

#[test]
fn a_text_block_takes_the_value() {
    // In block-Normal mode, before the character under the cursor, the cursor on what went in.
    let r = sym_run(text("ab"), "gg<CR>l:sym alpha<CR>");
    assert_eq!(source(&r, "t1"), "aαb");
    assert_eq!(r.engine.mode(), Mode::BlockNormal);
    assert_eq!(r.engine.text_cursor().unwrap().byte, 1);
    // In Insert mode, unspaced, the caret after it.
    let r = sym_run(text("x"), "ggA<C-k>alpha<CR>");
    assert_eq!(source(&r, "t1"), "xα");
    assert_eq!(r.engine.mode(), Mode::Insert);
    assert_eq!(r.engine.text_cursor().unwrap().byte, 3);
    let r = sym_run(text("x"), "ggA<C-k>arr<Tab><Tab><CR>");
    assert_eq!(source(&r, "t1"), "x↔");
}

#[test]
fn a_pick_in_block_normal_mode_is_one_undo_step() {
    // Before `y`, the cursor on the space after the name.
    let mut r = sym_run(math("x + y"), "gg<CR>$:sym alpha<CR>");
    assert_snapshot!(snapshot(&r));
    r.feed("u");
    assert_eq!(source(&r, "m1"), "x + y");
    assert_eq!(
        (r.engine.mode(), r.engine.text_cursor().unwrap().byte),
        (Mode::BlockNormal, 4)
    );
    r.feed("<C-r>");
    assert_eq!(source(&r, "m1"), "x + alpha y");
    assert_eq!(r.engine.text_cursor().unwrap().byte, 9);
    // `.` repeats the change before it, not the pick: `:` commands never go through `.`.
    let r = sym_run(math("abc"), "gg<CR>x:sym pi<CR>l.");
    assert_eq!(source(&r, "m1"), "pi c");
}

#[test]
fn a_pick_on_the_board_is_one_undo_step() {
    let mut r = sym_run(Board::new(), "2l:sym pi<CR>");
    assert_eq!(r.engine.cursor(), &Cursor::Object(id("m1")));
    r.feed("u");
    assert!(r.engine.board().is_empty());
    assert_eq!(r.engine.cursor(), &Cursor::Grid(Point::new(2.0, 0.0)));
    assert!(!r.engine.is_modified());
}

#[test]
fn at_colon_picks_again_where_it_is_typed() {
    // On the board: another math block.
    let r = sym_run(Board::new(), ":sym pi<CR>3j@:");
    assert_snapshot!(snapshot(&r));
    // A pick from Insert mode, again on the board. (Block-Normal mode has no `@:`.)
    let r = sym_run(math("x"), "ggA<C-k>alpha<CR><Esc>@:");
    assert_eq!(source(&r, "m1"), "x alpha");
    assert_eq!(source(&r, "m2"), "alpha");
}

#[test]
fn a_pick_past_a_cap_on_text_is_refused() {
    let full = "a".repeat(5000);
    let r = sym_run(math(&full), "ggA<C-k>pi<CR>");
    assert_eq!(
        message(&r),
        Some(("a text or formula holds at most 5000 characters", true))
    );
    assert_eq!(source(&r, "m1"), full);
    assert_eq!(r.engine.mode(), Mode::Insert);
}

// `<C-k>` from Insert mode.

#[test]
fn ctrl_k_opens_a_sym_line_and_enter_goes_back_to_insert_mode_after_the_pick() {
    assert_snapshot!(trace(math("x"), "ggA<C-k>al<Tab><CR>+<Esc>"));
}

#[test]
fn esc_or_backspace_on_an_empty_line_goes_back_to_insert_mode_with_nothing_put_in() {
    let mut r = sym_run(math("x"), "ggA<C-k>al<Esc>");
    assert_eq!(r.engine.mode(), Mode::Insert);
    assert_eq!(r.engine.text_cursor().unwrap().byte, 1);
    assert_eq!(r.engine.completions(), None);
    r.feed("<C-k><BS><BS><BS><BS>");
    assert_eq!(line(&r), Some(""));
    r.feed("<BS>");
    assert_eq!(r.engine.mode(), Mode::Insert);
    r.feed("+<Esc>");
    assert_eq!(source(&r, "m1"), "x+");
    // The session is one undo step, `<C-k>` lines and all.
    r.feed("u");
    assert_eq!(source(&r, "m1"), "x");
}

#[test]
fn the_pick_and_what_was_typed_around_it_are_one_undo_step() {
    let mut r = sym_run(math("x"), "ggA=<C-k>beta<CR>+1<Esc>");
    assert_eq!(source(&r, "m1"), "x=beta+1");
    r.feed("u");
    assert_eq!(source(&r, "m1"), "x");
    r.feed("<C-r>");
    assert_eq!(source(&r, "m1"), "x=beta+1");
}

#[test]
fn only_sym_runs_from_insert_mode() {
    let r = sym_run(math("x"), "ggA<C-k><BS><BS><BS><BS>q!<CR>");
    assert_eq!(
        message(&r),
        Some(("from Insert mode the command line runs only `:sym`", true))
    );
    assert_eq!(r.engine.mode(), Mode::Insert);
    assert!(!r.effects.contains(&Effect::Quit));
    // `:sym` errors show and go back to Insert mode too.
    let r = sym_run(math("x"), "ggA<C-k><CR>");
    assert_eq!(
        message(&r),
        Some(("missing the symbol's name: `:sy[m] <name>`", true))
    );
    assert_eq!(r.engine.mode(), Mode::Insert);
    // An empty line does nothing.
    let r = sym_run(math("x"), "ggA<C-k><BS><BS><BS><BS><CR>");
    assert_eq!((message(&r), r.engine.mode()), (None, Mode::Insert));
}

#[test]
fn dot_types_what_the_pick_put_in_not_the_picker_keys() {
    let b = board(
        "math id=m1 x=0 y=0 \"x\"
math id=m2 x=0 y=2 \"y\"
",
    );
    let mut r = sym_run(b, "ggA<C-k>alpha<CR><Esc>w.");
    assert_eq!(source(&r, "m1"), "x alpha");
    assert_eq!(source(&r, "m2"), "y alpha");
    // It types the characters, so it needs no symbols, and a count types them again.
    r.engine.set_symbols(Arc::from(Vec::new()));
    r.feed("gg2.");
    assert_eq!(source(&r, "m1"), "x alpha alpha alpha");
    assert_eq!(message(&r), None);
}

#[test]
fn a_click_ends_the_sym_line_and_its_insert_session() {
    let mut r = sym_run(math("x"), "ggAz<C-k>al");
    r.click(5.0, 5.0, None);
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert_eq!(r.engine.completions(), None);
    assert_eq!(source(&r, "m1"), "xz");
    r.feed("u");
    assert_eq!(source(&r, "m1"), "x");
}

// Mappings.

#[test]
fn command_line_mappings_apply_to_sym_lines() {
    let rc = "cnoremap <C-j> <Tab>
cnoremap ;a alpha
cnoremap <Tab> <S-Tab>
inoremap <C-l> <C-k>pi<CR>
";
    // `<C-j>` runs the built-in `<Tab>`, not the mapped one.
    let r = sym_rc(rc, Board::new(), ":sym arr<C-j><C-j><CR>");
    assert_eq!(source(&r, "m1"), "arrow.l.r");
    let r = sym_rc(rc, Board::new(), ":sym arr<Tab><CR>");
    assert_eq!(source(&r, "m1"), "harpoon.rt");
    let r = sym_rc(rc, Board::new(), ":sym ;a<CR>");
    assert_eq!(source(&r, "m1"), "alpha");
    let r = sym_rc(rc, math("x"), "ggA<C-l>");
    assert_eq!(source(&r, "m1"), "x pi");
    assert_eq!(r.engine.mode(), Mode::Insert);
}

// Caps.

#[test]
fn a_query_holds_at_most_max_sym_query_chars() {
    let too_long = "a".repeat(MAX_SYM_QUERY_CHARS + 1);
    let expected = format!("a `:sym` query holds at most {MAX_SYM_QUERY_CHARS} characters");
    let mut r = sym_run(Board::new(), &format!(":sym {too_long}"));
    assert_eq!(message(&r), Some((expected.as_str(), true)));
    assert_eq!(rows(&r).as_deref(), Some(""));
    r.feed("<CR>");
    assert_eq!(message(&r), Some((expected.as_str(), true)));
    assert!(r.engine.board().is_empty());
    // At the cap it matches; spaces at its ends do not count.
    let at_cap = "a".repeat(MAX_SYM_QUERY_CHARS);
    let r = sym_run(Board::new(), &format!(":sym  {at_cap}  "));
    assert_eq!(message(&r), None);
    assert_eq!(rows(&r).as_deref(), Some(""));
}

#[test]
fn the_engine_keeps_at_most_max_symbols() {
    let many: Arc<[Symbol]> = (0..MAX_SYMBOLS + 10)
        .map(|i| symbol(&format!("s{i:05}"), "x"))
        .collect();
    let mut engine = Engine::new(Board::new());
    assert_eq!(engine.set_symbols(many), MAX_SYMBOLS);
    // The first `MAX_SYMBOLS` given are kept.
    let mut r = run_engine(engine, ":sym s04095<CR>");
    assert_eq!(source(&r, "m1"), "s04095");
    r.feed("j:sym s04096");
    assert!(!rows(&r).unwrap().contains("s04096"));
}

#[test]
fn the_engine_keeps_the_symbols_sym_can_use_once_each() {
    let list: Arc<[Symbol]> = Arc::from(vec![
        symbol("beta", "β"),
        symbol("alpha", "α"),
        symbol("alpha", "A"),
        symbol("", "e"),
        symbol("two words", "w"),
        symbol("bell", "\u{7}"),
        symbol("empty", ""),
        symbol(&"n".repeat(MAX_SYM_QUERY_CHARS + 1), "n"),
    ]);
    let mut engine = Engine::new(Board::new());
    assert_eq!(engine.set_symbols(list), 2);
    let r = run_engine(engine, ":sym a");
    let (rows, _) = r.engine.completions().unwrap();
    assert_eq!(rows, [symbol("alpha", "α"), symbol("beta", "β")]);
}
