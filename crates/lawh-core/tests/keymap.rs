//! Key mappings, keys in and board out: every map mode, `map` against `noremap`, waiting for a
//! longer mapping and the timeout, `<nowait>`, `<Leader>`, the caps on mappings, `:set`, and
//! reading a lawhrc with [`Engine::source`], including `examples/lawhrc`.
//!
//! `<Timeout>` in a key script calls [`Engine::timeout`] (see `common`).

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use common::{Run, board, run, run_engine, run_with_rc};
use insta::assert_snapshot;
use lawh_core::engine::{
    Effect, Engine, Escape, MAX_EXPANDED_KEYS, MAX_MAP_DEPTH, MAX_MAP_KEYS, MAX_MAPS,
    MAX_PENDING_KEYS, MAX_RC_BYTES, MAX_RC_WARNINGS, MAX_REPEAT_KEYS, RcWarning,
};
use lawh_core::{Board, Cursor, Key, Mode, ObjectId, Point, Shape};

/// The sample lawhrc: `jkl;` for moving, as in its author's Neovim.
const LAWHRC: &str = include_str!("../../../examples/lawhrc");

/// Three objects in a row, and a line under the first:
///
/// ```text
/// y 0.5   r1 (0..2)   c1 (4.5..5.5)   r2 (8..10)
/// y 3     l1 (0..2)
/// ```
fn row() -> Board {
    board(
        "rect id=r1 x=0 y=0 w=2 h=1
circle id=c1 x=5 y=0.5 r=0.5
rect id=r2 x=8 y=0 w=2 h=1
line id=l1 x1=0 y1=3 x2=2 y2=3
",
    )
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

/// The text of object `name`.
fn text(r: &Run, name: &str) -> String {
    match &r.engine.board().get(&id(name)).unwrap().shape {
        Shape::Text { text, .. } => text.clone(),
        other => panic!("{name} is not a text: {other:?}"),
    }
}

fn message(r: &Run) -> Option<&str> {
    r.engine.message().map(|m| m.text.as_str())
}

/// The message, which must be an error.
fn error(r: &Run) -> &str {
    let m = r.engine.message().expect("an error message");
    assert!(m.error, "not an error: {m:?}");
    &m.text
}

fn warnings(rc: &str) -> Vec<(usize, String)> {
    Engine::new(Board::new())
        .source(rc)
        .into_iter()
        .map(|RcWarning { line, message }| (line, message))
        .collect()
}

/// The message when one key would set off more than [`MAX_EXPANDED_KEYS`] keys.
fn too_many_keys() -> String {
    format!(
        "one key may set off at most {MAX_EXPANDED_KEYS} keys through mappings and `.`; the \
         rest were dropped"
    )
}

/// A bound for work that takes milliseconds in release: 1 s there, more in a debug build.
fn quick() -> Duration {
    Duration::from_secs(if cfg!(debug_assertions) { 10 } else { 1 })
}

fn arm(ms: u64) -> Effect {
    Effect::ArmTimer { ms }
}

// The sample lawhrc.

#[test]
fn the_sample_lawhrc_loads_without_warnings() {
    let mut engine = Engine::new(Board::new());
    assert_eq!(engine.source(LAWHRC), []);
    assert_eq!(engine.settings().leader, Key::char(' '));
}

#[test]
fn the_sample_lawhrc_moves_with_jkl_semicolon() {
    let cursor = |script: &str| run_with_rc(LAWHRC, row(), script).engine.cursor().clone();
    // On the board: `;` right, `j` left, `k` down, `l` up, with counts.
    assert_eq!(cursor(";"), grid(1.0, 0.0));
    assert_eq!(cursor("3;"), grid(3.0, 0.0));
    assert_eq!(cursor("j"), grid(-1.0, 0.0));
    assert_eq!(cursor("2k"), grid(0.0, 2.0));
    assert_eq!(cursor("2kl"), grid(0.0, 1.0));
    // Object jumps: `H` right, `J` left, `K` down, `L` up.
    assert_eq!(cursor("wH"), object("c1"));
    assert_eq!(cursor("wHJ"), object("r1"));
    assert_eq!(cursor("wK"), object("l1"));
    assert_eq!(cursor("wKL"), object("r1"));
    // Keys not mapped keep their meaning: `h` still moves left.
    assert_eq!(cursor("h"), grid(-1.0, 0.0));
}

#[test]
fn the_sample_lawhrc_in_visual_operator_pending_and_block_normal_mode() {
    // Visual: from r1, `H` jumps right to c1, selecting both.
    let r = run_with_rc(LAWHRC, row(), "wvH");
    assert_eq!(r.engine.selection(), [id("r1"), id("c1")]);
    // After `d`: `H` is the jump right, so `dH` deletes r1 and c1.
    let r = run_with_rc(LAWHRC, row(), "wdH");
    assert_snapshot!(r.snapshot());
    // In a text block: `;` right, `j` left.
    let b = board("text id=t1 x=0 y=0 \"hello\"\n");
    let mut r = run_with_rc(LAWHRC, b, "w<CR>;;");
    assert_eq!(r.engine.text_cursor().unwrap().byte, 2);
    r.feed("j");
    assert_eq!(r.engine.text_cursor().unwrap().byte, 1);
}

#[test]
fn the_sample_lawhrc_redoes_writes_and_leaves_insert_mode() {
    // `U` redoes.
    let r = run_with_rc(LAWHRC, Board::new(), ":ci 1<CR>uU");
    assert_eq!(r.engine.board().len(), 1);
    // `jk` leaves Insert mode.
    let r = run_with_rc(LAWHRC, Board::new(), "ifoojk");
    assert_snapshot!(r.snapshot());
    // `<Space>` waits for the rest of `<Leader>s`, then `s` writes.
    let mut engine = Engine::new(Board::new()).with_file_name(Some("rc.lawh".to_owned()));
    assert_eq!(engine.source(LAWHRC), []);
    let mut r = run_engine(engine, "<Space>");
    assert_eq!(r.effects, [arm(1000)]);
    assert_eq!(r.engine.pending(), "<Space>");
    r.feed("s");
    assert_eq!(
        r.effects.last(),
        Some(&Effect::Write {
            path: "rc.lawh".to_owned(),
            quit_after: false
        })
    );
    assert_eq!(r.engine.pending(), "");
    // `<Space>` alone does nothing once the timeout runs it.
    let r = run_with_rc(LAWHRC, Board::new(), "<Space><Timeout>;");
    assert_eq!(r.engine.cursor(), &grid(1.0, 0.0));
    assert_eq!(r.engine.pending(), "");
}

// Each mode.

#[test]
fn normal_mode_map_and_noremap() {
    let rc = "nnoremap Q :ci 1<CR>\nnmap W Q\n";
    let r = run_with_rc(rc, Board::new(), "Q3lW");
    assert_snapshot!(r.snapshot());
    // Not in other modes: `Q` types itself in Insert mode.
    let r = run_with_rc(rc, Board::new(), "iQ<Esc>");
    assert_eq!(text(&r, "t1"), "Q");
}

#[test]
fn visual_mode_map_and_noremap() {
    let rc = "xnoremap s d\nxmap S s\n";
    let r = run_with_rc(rc, row(), "wvLs");
    assert_snapshot!(r.snapshot());
    let r = run_with_rc(rc, row(), "wvLS");
    assert_eq!(r.engine.board().len(), 2);
    // Not in Normal mode.
    let r = run_with_rc(rc, row(), "ws");
    assert_eq!(r.engine.board(), &row());
}

#[test]
fn operator_pending_mode_map_and_noremap() {
    // After an operator, `a` is `w`: `da` deletes the current object.
    let rc = "onoremap a w\nomap A a\n";
    let r = run_with_rc(rc, row(), "wda");
    assert_snapshot!(r.snapshot());
    let r = run_with_rc(rc, row(), "w2dA");
    assert_eq!(r.engine.board().len(), 2);
    // `a` still starts typing in Normal mode.
    let r = run_with_rc(rc, Board::new(), "ab<Esc>");
    assert_eq!(text(&r, "t1"), "b");
}

#[test]
fn insert_mode_map_and_noremap() {
    let rc = "inoremap jk <Esc>\nimap ;; jk\n";
    let r = run_with_rc(rc, Board::new(), "ifoo;;");
    assert_eq!(r.engine.mode(), Mode::Normal);
    assert_eq!(text(&r, "t1"), "foo");
    // Not in Normal mode: `j` and `k` move.
    let r = run_with_rc(rc, Board::new(), "jk");
    assert_eq!(r.engine.cursor(), &grid(0.0, 0.0));
    assert_eq!(r.effects, []);
}

#[test]
fn command_line_mode_map_and_noremap() {
    let rc = "cnoremap <C-a> <Home>\ncmap <C-e> <C-a>\n";
    let r = run_with_rc(rc, Board::new(), ":i 3<C-a>c<CR>:l 2 1<C-e>e<CR>");
    assert_snapshot!(r.snapshot());
}

#[test]
fn map_bang_is_insert_and_command_line_mode() {
    let rc = "noremap! <C-a> <Home>\n";
    let r = run_with_rc(rc, Board::new(), "ifoo<C-a>x<Esc>:i 1<C-a>c<CR>");
    assert_eq!(text(&r, "t1"), "xfoo");
    assert_eq!(r.engine.board().len(), 2);
}

// How mappings expand.

#[test]
fn map_maps_again_and_noremap_does_not() {
    let rc = "nnoremap j h\nnmap Q j\nnnoremap R j\n";
    // `Q` is `j`, which is `h`: left.
    let r = run_with_rc(rc, Board::new(), "Q");
    assert_eq!(r.engine.cursor(), &grid(-1.0, 0.0));
    // `R` is `j` as typed: down.
    let r = run_with_rc(rc, Board::new(), "R");
    assert_eq!(r.engine.cursor(), &grid(0.0, 1.0));
}

#[test]
fn keys_that_map_to_themselves_and_more_run_the_first_as_typed() {
    // Vim's own example, `:map ab abcd`: `a` starts typing (not mapped again), and `bcd` is
    // typed in Insert mode, where Normal-mode mappings do not apply.
    let r = run_with_rc("nmap ab abcd\n", Board::new(), "ab<Esc>");
    assert_snapshot!(r.snapshot());
    // `:map a aaa` likewise types `aa`, and does not expand forever.
    let r = run_with_rc("map a aaa\n", Board::new(), "a<Esc>");
    assert_eq!(text(&r, "t1"), "aa");
}

#[test]
fn a_recursive_mapping_stops_at_max_map_depth() {
    let started = Instant::now();
    let mut r = run_with_rc("nmap a b\nnmap b a\n", Board::new(), "a");
    assert_eq!(error(&r), "recursive mapping");
    assert_eq!(r.engine.pending(), "");
    // The typeahead was dropped; the next key works.
    r.feed("l");
    assert_eq!(r.engine.cursor(), &grid(1.0, 0.0));
    assert_eq!(message(&r), None);

    // A mapping that runs a key, then itself: exactly MAX_MAP_DEPTH expansions run, each
    // moving one unit right. (Vim counts until a key reaches a command, so it would loop
    // forever.)
    let r = run_with_rc("nmap Q lQ\n", Board::new(), "Q");
    assert_eq!(r.engine.cursor(), &grid(MAX_MAP_DEPTH as f64, 0.0));
    assert_eq!(error(&r), "recursive mapping");
    // One that never grows the typeahead: `yy` (nothing to yank), then `x` again.
    let r = run_with_rc("map x yx\n", Board::new(), "x");
    assert_eq!(error(&r), "recursive mapping");
    assert_eq!(r.engine.mode(), Mode::Normal);
    // `:map a aaa` in Visual mode, where `a` does nothing: one key more each time.
    let r = run_with_rc("map a aaa\n", Board::new(), "va");
    assert_eq!(error(&r), "recursive mapping");
    assert_eq!(r.engine.mode(), Mode::Visual);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_mapping_that_feeds_itself_stops_at_max_pending_keys() {
    // In Visual mode `a` does nothing, and each expansion adds MAX_MAP_KEYS - 2 keys.
    let started = Instant::now();
    let rc = format!("map a {}\n", "a".repeat(MAX_MAP_KEYS));
    let mut r = run_with_rc(&rc, Board::new(), "va");
    assert_eq!(
        error(&r),
        format!("mappings made more than {MAX_PENDING_KEYS} keys at once; they were dropped")
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(r.engine.mode(), Mode::Visual);
    assert_eq!(r.engine.pending(), "");
    r.feed("<Esc>");
    assert_eq!(r.engine.mode(), Mode::Normal);
}

#[test]
fn one_key_sets_off_at_most_max_expanded_keys() {
    // `q` types 255 `l`s and itself again, and 999 mappings start with those 255 `l`s, so
    // each `l` is looked up against all of them. Without a cap, 1,000 expansions of 256 keys
    // each, looked up in every mapping one by one, took 82 s in release.
    let ls = "l".repeat(255);
    let mut rc = format!("nmap q {ls}q\n");
    for i in 0..999 {
        let c = char::from_u32(0x100 + i).unwrap();
        writeln!(rc, "nmap {ls}{c} h").unwrap();
    }
    let started = Instant::now();
    let mut r = run_with_rc(&rc, Board::new(), "q");
    let elapsed = started.elapsed();
    assert_eq!(error(&r), too_many_keys());
    // Every expansion that fits ran: each moves 255 units right.
    let expansions = MAX_EXPANDED_KEYS / 256;
    assert_eq!(r.engine.cursor(), &grid((expansions * 255) as f64, 0.0));
    assert!(elapsed < quick(), "{elapsed:?}");
    // The typeahead was dropped; the next key starts afresh.
    assert_eq!(r.engine.pending(), "");
    r.feed("h");
    assert_eq!(r.engine.cursor(), &grid((expansions * 255 - 1) as f64, 0.0));
    assert_eq!(message(&r), None);
}

#[test]
fn dot_from_a_mapping_counts_against_max_expanded_keys() {
    // `Q` repeats a 2,002-key insert, undoes it and runs itself again. `.` used to replay its
    // keys whatever the mapping had done, so 1,000 rounds of it never finished.
    let long = "a".repeat(2000);
    let started = Instant::now();
    let r = run_with_rc("nmap Q .uQ\n", Board::new(), &format!("i{long}<Esc>Q"));
    let elapsed = started.elapsed();
    assert_eq!(error(&r), too_many_keys());
    assert_eq!(text(&r, "t1"), long);
    assert!(elapsed < quick(), "{elapsed:?}");
    // A `.` still replays a change of nearly MAX_REPEAT_KEYS keys in full, with a count: its
    // keys, then the 9,997 the count types again (MAX_REPEAT_KEYS allows one more time).
    let moves = (MAX_REPEAT_KEYS - 3) / 2;
    let insert = format!("ib{}<Esc>", "<Left><Right>".repeat(moves));
    let mut r = run(Board::new(), &format!("{insert}j"));
    r.feed("3.");
    assert_eq!(text(&r, "t2"), "bb");
    assert_eq!(message(&r), None);
    // So does a mapping that runs it once.
    let mut r = run_with_rc("nnoremap Q 3.\n", Board::new(), &format!("{insert}j"));
    r.feed("Q");
    assert_eq!(text(&r, "t2"), "bb");
    assert_eq!(message(&r), None);
}

#[test]
fn dot_repeats_the_keys_a_mapping_ran_without_mapping_them_again() {
    // `X` runs `dd`; `.` replays `dd` itself, though `d` is mapped to nothing by then.
    let r = run_with_rc("nnoremap X dd\nnnoremap d <Nop>\n", row(), "wXw.");
    assert_snapshot!(r.snapshot());
}

#[test]
fn a_zero_on_a_count_and_the_key_after_at_are_not_mapped() {
    let rc = "nnoremap 0 :ci 1<CR>\n";
    // `10l` moves ten units; `0` alone is mapped.
    let r = run_with_rc(rc, Board::new(), "10l");
    assert_eq!(r.engine.cursor(), &grid(10.0, 0.0));
    let r = run_with_rc(rc, Board::new(), "0");
    assert_eq!(r.engine.board().len(), 1);
    // With `;` and `:` swapped, `@:` still repeats the last command line.
    let rc = "nnoremap ; :\nnnoremap : ;\n";
    let r = run_with_rc(rc, Board::new(), ";ci 1<CR>3l@:");
    assert_snapshot!(r.snapshot());
}

// Waiting for a longer mapping.

#[test]
fn jk_leaves_insert_mode_and_other_keys_after_j_are_typed() {
    let rc = "inoremap jk <Esc>\n";
    let r = run_with_rc(rc, Board::new(), "ifoojk");
    assert_snapshot!(r.snapshot());
    // The timeout types the `j`.
    let r = run_with_rc(rc, Board::new(), "ifooj<Timeout>x<Esc>");
    assert_eq!(text(&r, "t1"), "foojx");
    assert_eq!(r.keys, 7);
    // So does a key that does not go on to `jk`.
    let r = run_with_rc(rc, Board::new(), "ifoojx<Esc>");
    assert_eq!(text(&r, "t1"), "foojx");
    let r = run_with_rc(rc, Board::new(), "ifoojjk");
    assert_eq!(text(&r, "t1"), "fooj");
    assert_eq!(r.engine.mode(), Mode::Normal);
}

#[test]
fn keys_that_start_a_longer_mapping_arm_the_timer() {
    let mut r = run_with_rc("inoremap jk <Esc>\n", Board::new(), "ifooj");
    assert_eq!(r.effects, [arm(1000)]);
    assert_eq!(r.engine.pending(), "j");
    // The board does not show the `j` yet.
    assert_eq!(text(&r, "t1"), "foo");
    r.feed("<Timeout>");
    assert_eq!(text(&r, "t1"), "fooj");
    assert_eq!(r.engine.pending(), "");
    // A timeout with nothing waiting does nothing.
    r.feed("<Timeout><Timeout>");
    assert_eq!(r.effects, [arm(1000)]);
    assert_eq!(text(&r, "t1"), "fooj");
    // The timer follows `timeoutlen`.
    let r = run_with_rc("set tm=250\ninoremap jk <Esc>\n", Board::new(), "ij");
    assert_eq!(r.effects, [arm(250)]);
    // Counts and command keys show before the waiting keys.
    let r = run_with_rc(LAWHRC, Board::new(), "2<Space>");
    assert_eq!(r.engine.pending(), "2<Space>");
}

#[test]
fn the_timeout_takes_the_longest_mapping_typed() {
    let rc = "nnoremap Q :ci 1<CR>\nnnoremap QQ :ci 2<CR>\nnnoremap QQQ :ci 3<CR>\n";
    let r = run_with_rc(rc, Board::new(), "Q");
    assert_eq!(
        (r.engine.board().len(), r.effects.as_slice()),
        (0, &[arm(1000)][..])
    );
    let r = run_with_rc(rc, Board::new(), "QQ<Timeout>");
    assert_snapshot!(r.snapshot());
    // A key that goes on to no mapping takes the longest one, then runs by itself.
    let r = run_with_rc(rc, Board::new(), "QQl");
    assert_eq!(r.engine.board().len(), 1);
    assert_eq!(r.engine.cursor(), &grid(1.0, 0.0));
    // Keys that start a mapping but make none run as typed: `QQ` with only `QQQ` mapped.
    let r = run_with_rc("nnoremap llQ :ci 1<CR>\n", Board::new(), "ll<Timeout>");
    assert_eq!(r.engine.cursor(), &grid(2.0, 0.0));
}

#[test]
fn nowait_does_not_wait_for_longer_mappings() {
    let rc = "nnoremap <nowait> Q :ci 1<CR>\nnnoremap QQ :ci 2<CR>\n";
    let r = run_with_rc(rc, Board::new(), "Q");
    assert_eq!(r.engine.board().len(), 1);
    assert_eq!(r.effects, []);
    // Without it, `Q` waits.
    let rc = "nnoremap Q :ci 1<CR>\nnnoremap QQ :ci 2<CR>\n";
    let r = run_with_rc(rc, Board::new(), "Q");
    assert_eq!(r.engine.board().len(), 0);
    assert_eq!(r.effects, [arm(1000)]);
}

#[test]
fn a_click_drops_keys_waiting_for_a_mapping() {
    let mut r = run_with_rc("inoremap jk <Esc>\n", Board::new(), "ifooj");
    r.click(5.0, 5.0, None);
    assert_eq!(r.engine.pending(), "");
    assert_eq!(text(&r, "t1"), "foo");
    r.feed("k");
    assert_eq!(r.engine.cursor(), &grid(5.0, 4.0));
}

// Keys.

#[test]
fn a_mapping_of_a_shifted_key_wins_over_the_plain_key() {
    // Unmapped, `<S-CR>` on a grid point is `<CR>`: no text there to enter.
    let r = run(Board::new(), "<S-CR>");
    assert_eq!(
        error(&r),
        "no text under the cursor: <CR> enters text and math"
    );
    let rc = "nnoremap <S-CR> l\ncnoremap <S-Esc> x\n";
    let r = run_with_rc(rc, Board::new(), "<S-CR>");
    assert_eq!(r.engine.cursor(), &grid(1.0, 0.0));
    assert_eq!(message(&r), None);
    let r = run_with_rc(rc, Board::new(), ":ci 1<S-Esc><BS><S-CR>");
    assert_eq!(r.engine.board().len(), 1);
    // A mapping of the plain key does not take the shifted one.
    let r = run_with_rc("nnoremap <CR> l\n", Board::new(), "<S-CR>");
    assert_eq!(r.engine.cursor(), &grid(0.0, 0.0));
}

#[test]
fn ctrl_chords_match_exactly() {
    let rc = "nnoremap <C-l> :ci 1<CR>\n";
    for other in ["<C-M-l>", "<M-l>", "<D-l>", "<C-D-l>", "l"] {
        let r = run_with_rc(rc, Board::new(), other);
        assert_eq!(r.engine.board().len(), 0, "{other}");
    }
    // Ctrl folds letters to lowercase, so `<C-L>` is `<C-l>`.
    for same in ["<C-l>", "<C-L>"] {
        let r = run_with_rc(rc, Board::new(), same);
        assert_eq!(r.engine.board().len(), 1, "{same}");
    }
}

#[test]
fn leader_is_replaced_when_the_mapping_is_made() {
    // The default leader is `\`.
    let mut r = run(Board::new(), ":nnoremap <Leader>c :ci 1<lt>CR><CR>\\c");
    assert_eq!(r.engine.board().len(), 1);
    // A new leader applies to new mappings only.
    r.feed(":set leader=,<CR>:nnoremap <Leader>r :re 1 1<lt>CR><CR>,c,r\\c");
    assert_snapshot!(r.snapshot());
    r.feed(":nmap <Leader>r<CR>");
    assert_eq!(message(&r), Some("nnoremap ,r :re 1 1<CR>"));
    r.feed(":nmap \\c<CR>");
    assert_eq!(message(&r), Some("nnoremap <Bslash>c :ci 1<CR>"));
}

// Commands.

#[test]
fn mapping_commands_show_mappings() {
    let mut r = run_with_rc(LAWHRC, Board::new(), ":nmap j<CR>");
    assert_eq!(message(&r), Some("nnoremap j h"));
    assert!(!r.engine.message().unwrap().error);
    r.feed(":map j<CR>");
    assert_eq!(
        message(&r),
        Some("nnoremap j h, xnoremap j h, onoremap j h")
    );
    r.feed(":imap jk<CR>");
    assert_eq!(message(&r), Some("inoremap jk <Esc>"));
    r.feed(":nmap <lt>Leader>s<CR>");
    assert_eq!(message(&r), Some("nnoremap <Space>s :w<CR>"));
    r.feed(":nmap jk<CR>");
    assert_eq!(message(&r), Some("no mapping for `jk`"));
    r.feed(":imap<CR>");
    assert_eq!(
        message(&r),
        Some("1 mapping; `:map {keys}` shows the mappings of some keys")
    );
    r.feed(":map<CR>");
    assert_eq!(
        message(&r),
        Some("27 mappings; `:map {keys}` shows the mappings of some keys")
    );
    r.feed(":nnoremap <nowait> Q <Nop><CR>:nmap Q<CR>");
    assert_eq!(message(&r), Some("nnoremap <nowait> Q <Nop>"));
    // Spaces inside show as typed; at the ends, where a `:map` line drops them, as `<Space>`.
    r.feed(":nnoremap Q <lt>Space>a b<lt>Space><CR>:nmap Q<CR>");
    assert_eq!(message(&r), Some("nnoremap Q <Space>a b<Space>"));
}

#[test]
fn unmap_and_mapclear() {
    let rc = "noremap Q :ci 1<CR>\ninoremap jk <Esc>\ncnoremap <C-a> <Home>\n";
    let mut r = run_with_rc(rc, Board::new(), ":nunmap Q<CR>Q");
    assert_eq!(r.engine.board().len(), 0);
    // Still in Visual and operator-pending mode.
    r.feed(":xmap Q<CR>");
    assert_eq!(message(&r), Some("xnoremap Q :ci 1<CR>"));
    r.feed(":nunmap Q<CR>");
    assert_eq!(error(&r), "no mapping for `Q`");
    r.feed(":unmap Q<CR>:map Q<CR>");
    assert_eq!(message(&r), Some("no mapping for `Q`"));
    // `:mapclear` leaves Insert and command-line mode; `:mapclear!` clears them.
    r.feed(":nnoremap Q :ci 1<lt>CR><CR>:mapclear<CR>:map<CR>");
    assert_eq!(
        message(&r),
        Some("0 mappings; `:map {keys}` shows the mappings of some keys")
    );
    r.feed(":map!<CR>");
    assert_eq!(
        message(&r),
        Some("2 mappings; `:map {keys}` shows the mappings of some keys")
    );
    r.feed(":iunmap jk<CR>:mapclear!<CR>:map!<CR>");
    assert_eq!(
        message(&r),
        Some("0 mappings; `:map {keys}` shows the mappings of some keys")
    );
    r.feed("ijk<Esc>");
    assert_eq!(text(&r, "t1"), "jk");
}

#[test]
fn set_shows_and_changes_options() {
    let mut r = run(Board::new(), ":set timeoutlen?<CR>");
    assert_eq!(message(&r), Some("timeoutlen=1000"));
    r.feed(":set<CR>");
    assert_eq!(
        message(&r),
        Some("escape=board  leader=<Bslash>  timeoutlen=1000")
    );
    r.feed(":se tm=300 leader=<lt>Space> escape=block<CR>:set tm leader? escape<CR>");
    assert_eq!(
        message(&r),
        Some("timeoutlen=300  leader=<Space>  escape=block")
    );
    // `escape=block`: `<Esc>` from Insert mode stops in the block.
    r.feed("ifoo<Esc>");
    assert_eq!(r.engine.mode(), Mode::BlockNormal);
    r.feed("<Esc>:set escape& tm&<CR>");
    assert_eq!(r.engine.settings().escape, Escape::Board);
    assert_eq!(r.engine.settings().timeoutlen, 1000);
    assert_eq!(r.engine.settings().leader, Key::char(' '));
    r.feed("ab<Esc>");
    assert_eq!(r.engine.mode(), Mode::Normal);
    // Mistakes are errors on the status line.
    r.feed(":set foo<CR>");
    assert_eq!(error(&r), "unknown option `foo`");
    r.feed(":set tm=20000<CR>");
    assert_eq!(
        error(&r),
        "bad value `20000` for `timeoutlen`: it takes milliseconds from 0 to 10000"
    );
    assert_eq!(r.engine.settings().timeoutlen, 1000);
}

// Reading a lawhrc.

#[test]
fn a_lawhrc_skips_comments_blank_lines_and_colons() {
    let rc = "\u{feff}\" comment\r\n\r\n   \t\r\n  :nnoremap Q :ci 1<CR>\r\n\t\" indented\r\n::set tm=5\r\n:\"x";
    assert_eq!(warnings(rc), []);
    let r = run_with_rc(rc, Board::new(), "Q");
    assert_eq!(r.engine.board().len(), 1);
    assert_eq!(r.engine.settings().timeoutlen, 5);
}

#[test]
fn a_bad_lawhrc_line_warns_and_the_rest_load() {
    let long = "a".repeat(MAX_MAP_KEYS + 1);
    let rc = format!(
        "\" A lawhrc with mistakes.
nnoremap Q :ci 1<CR>
frobnicate
nnoremap <C-Nope> x
ci 3
nnoremap Q :ci 2<CR>
nnoremap W {long}
inoremap jk <Esc>
set escape=blok
nmap X

set tm=300
nnoremap <Leader>a x
nnoremap \\a y
w
"
    );
    assert_eq!(
        warnings(&rc),
        [
            (3, "unknown command `:frobnicate`".to_owned()),
            (
                4,
                "in `<C-Nope>`: `<C-Nope>` at byte 0 is not a key: modifiers need a key name \
                 and `>`, as in `<C-r>`"
                    .to_owned()
            ),
            (
                5,
                "`:ci` is not allowed in lawhrc, only `:set` and mapping commands are".to_owned()
            ),
            (
                6,
                "`Q` was mapped in Normal mode on line 2 already; this line replaces it".to_owned()
            ),
            (
                7,
                format!("what they map to may be at most {MAX_MAP_KEYS} keys long")
            ),
            (
                9,
                "bad value `blok` for `escape`: it takes `board` or `block`".to_owned()
            ),
            (10, "missing what `X` maps to".to_owned()),
            (
                14,
                "`<Bslash>a` was mapped in Normal mode on line 13 already; this line replaces it"
                    .to_owned()
            ),
            (
                15,
                "`:w` is not allowed in lawhrc, only `:set` and mapping commands are".to_owned()
            ),
        ]
    );
    // The good lines loaded, and the later `Q` won. Nothing ran: no effects, no message.
    let mut engine = Engine::new(Board::new());
    let _ = engine.source(&rc);
    assert_eq!(engine.board().len(), 0);
    assert_eq!(engine.message(), None);
    let r = run_engine(engine, "Qlifoojk");
    assert_snapshot!(r.snapshot());
    assert_eq!(r.engine.settings().timeoutlen, 300);
}

#[test]
fn unmapping_in_a_lawhrc_forgets_the_earlier_line() {
    let rc = "nnoremap Q a\nnunmap Q\nnnoremap Q b\nnoremap W a\nmapclear\nnnoremap W b\n";
    assert_eq!(warnings(rc), []);
    assert_eq!(
        warnings("nunmap Q\n"),
        [(1, "no mapping for `Q`".to_owned())]
    );
}

#[test]
fn source_runs_nothing_else() {
    // The last command line stays the one typed, for `@:`.
    let mut engine = Engine::new(Board::new());
    let mut r = run_engine(engine.clone(), ":ci 1<CR>");
    assert_eq!(r.engine.source("nnoremap Q :ci 2<CR>\nset tm=5\n"), []);
    r.feed("l@:");
    assert_eq!(r.engine.board().len(), 2);
    assert_snapshot!(r.snapshot());
    // A `:set` that shows a value leaves no message.
    assert_eq!(engine.source("set tm?\n"), []);
    assert_eq!(engine.message(), None);
}

#[test]
fn a_lawhrc_larger_than_max_rc_bytes_is_not_read() {
    let line = "nnoremap Q :ci 1<CR>\n\"";
    let just_fits = format!("{line}{}", "x".repeat(MAX_RC_BYTES - line.len()));
    assert_eq!(just_fits.len(), MAX_RC_BYTES);
    assert_eq!(warnings(&just_fits), []);
    let too_big = format!("{just_fits}x");
    let mut engine = Engine::new(Board::new());
    assert_eq!(
        engine.source(&too_big),
        [RcWarning {
            line: 0,
            message: format!("the file is larger than {MAX_RC_BYTES} bytes, so it was not read"),
        }]
    );
    let r = run_engine(engine, "Q");
    assert_eq!(r.engine.board().len(), 0);
}

#[test]
fn a_lawhrc_lists_at_most_max_rc_warnings() {
    let rc = "oops\n".repeat(MAX_RC_WARNINGS + 50);
    let warnings = warnings(&rc);
    assert_eq!(warnings.len(), MAX_RC_WARNINGS + 1);
    assert_eq!(warnings[MAX_RC_WARNINGS - 1].0, MAX_RC_WARNINGS);
    assert_eq!(
        warnings[MAX_RC_WARNINGS],
        (
            MAX_RC_WARNINGS + 1,
            "50 lines from here on have problems too".to_owned()
        )
    );
}

#[test]
fn each_mode_holds_at_most_max_maps() {
    let mut rc = String::new();
    for i in 0..MAX_MAPS {
        writeln!(rc, "nnoremap Z{i:04} x").unwrap();
    }
    assert_eq!(warnings(&rc), []);
    rc.push_str("nnoremap Y :ci 1<CR>\nnnoremap Z0000 :ci 2<CR>\nxnoremap Y d\n");
    let full = format!("Normal mode holds at most {MAX_MAPS} mappings");
    assert_eq!(
        warnings(&rc),
        [
            (MAX_MAPS + 1, full.clone()),
            (
                MAX_MAPS + 2,
                "`Z0000` was mapped in Normal mode on line 1 already; this line replaces it"
                    .to_owned()
            ),
        ]
    );
    // A full mode still takes a mapping of keys it has: `Z0000` was replaced.
    let mut engine = Engine::new(Board::new());
    let _ = engine.source(&rc);
    let mut r = run_engine(engine, "Z0000");
    assert_eq!(r.engine.board().len(), 1);
    // All or nothing: `:noremap` adds `Y` to no mode while Normal mode is full.
    r.feed(":noremap Y x<CR>");
    assert_eq!(error(&r), full);
    r.feed(":omap Y<CR>");
    assert_eq!(message(&r), Some("no mapping for `Y`"));
}
