# Milestone 3 measurements (2026-10-12)

The numbers behind the limits and design choices of milestone 3 of the lawh whiteboard: sizes for
text and math, jump labels (`s` puts a short label on every object in view; typing one jumps
there) and `:sym` (a symbol by its Typst name). Each figure is marked:

- **measured**: timed or counted on the machine below.
- **derived**: computed from the code's constants or other figures, not timed.
- **target**: a limit a test asserts.

Most numbers are copied from the doc comment of the constant they justify, or from the comment
above the test that measured them, with that source named, so the comment stays the place to
update them. Some come from the milestone's research notes and lane logs, which are not in the
repo; they say so.

## Machine and conditions

| | |
|---|---|
| CPU | Intel Core Ultra 9 185H (22 threads) |
| RAM | 14 GB |
| OS | Arch Linux, Hyprland (Wayland) |
| Date | 2026-10-12 |
| Build | given on every row: release or debug (the dev profile, `opt-level = 2` for dependencies) |
| Load | given where it was recorded (load average) |

As in [m1-measurements.md](m1-measurements.md), timings move with machine load.

## Sizes

### Why math is drawn larger

Text and math share an em of 0.5 grid units at size 1, but the fonts differ in how tall their
lowercase letters are. The **x-height** is the height of a lowercase `x`.

| What | Figure | Kind |
|---|---|---|
| Inter's x-height | 0.5459 em (1,118 of 2,048 units, its OS/2 table) | measured (read from the font) |
| Libertinus Math's x-height | 0.429 em (429 of 1,000 units) | measured (read from the font) |
| Math's `x` beside text's at one em, before M3 | 21% shorter | derived (1 - 0.429 / 0.5459) |
| The same, drawn at an em of 200 pixels | `x` 90 against 109 pixels, `A` 132 against 146 | measured, release (research notes, not in the repo) |
| `MATH_SCALE` | 1.2725, held to the fonts' ratio within 0.1% | derived; `the_math_scale_is_the_ratio_of_the_fonts_x_heights` in `lawh-render/src/renderer.rs` |

Source: the doc comment of `MATH_SCALE` in `crates/lawh-render/src/renderer.rs`.

### Baselines and x-heights as drawn

The **baseline** is the line letters sit on. Before M3 a text box held Inter's ascender space
above its first baseline, while a formula's box was tight to its ink, so math sat higher than
text at the same `y` (research notes, not in the repo). Now the formula is moved onto the text's
baseline.

`math_and_text_share_x_height_and_baseline` in `crates/lawh-render/tests/render.rs` draws an `x`
as text and as math at the same `y`, with an em of 400 pixels at every size, and measures the
drawn pixels. Pixel positions don't depend on the build; this run was a debug build on
2026-10-12.

| Size | Baselines apart | Math's x-height against text's | Kind |
|---|---|---|---|
| 0.5 | 0.0038 units | +2.30% | measured |
| 1 | 0.0075 units | +2.30% | measured |
| 2 | 0.0150 units | +2.30% | measured |
| 4 | 0.0300 units | +2.30% | measured |
| Allowed (acceptance criterion A4) | 0.05 units x size | 3% | target |

The 2.3% is Libertinus Math's italic `x`: 0.439 em tall where its OS/2 x-height says 0.429
(derived from the 2.30% above).

### Range and caps

| What | Figure | Kind |
|---|---|---|
| Smallest size, 0.25: its em at zoom 1 | 5 logical pixels | derived (doc of `MIN_TEXT_SIZE`, `crates/lawh-core/src/board.rs`) |
| Largest size, 16: its em at zoom 1 | 320 logical pixels | derived (doc of `MAX_TEXT_SIZE`) |
| Size-16 text at zoom 20 and scale factor 4 | an em of 25,600 pixels | derived (doc of `MAX_GLYPH_EM_PX`, `renderer.rs`) |
| Size-16 math there (`MATH_SCALE` larger) | an em of about 32,600 pixels | derived |
| Ems the frame budget's glyph costs are calibrated for | up to 65,536 pixels | target (`calibrate_glyphs`) |
| `calibrate_text_drawing`'s size-16 rows | at most 1.07 times their estimate | measured, release (lane log, not in the repo) |
| Largest file the caps allow | 415,751 bytes (8 texts at a size written with 19 characters) | measured (`largest_board` in `crates/lawh-core/tests/format_errors.rs`); 415,551 before sizes |
| One undo edit (`Edit`) | 136 bytes on Linux `x86_64` (120 before sizes) | measured (`an_edit_is_as_small_as_the_cap_assumes`, `engine/undo.rs`) |
| An undo step that changes a whole board | about 0.42 MB, so `MAX_UNDO_BYTES` (16 MiB) keeps 39 | derived (doc of `MAX_UNDO_BYTES`) |

### What resizing costs a key

From the doc comments of `SIZE_WORK` and `MAX_KEY_WORK` in `crates/lawh-core/src/engine/work.rs`
(`work_unit_costs` and `key_costs` in `crates/lawh-core/tests/engine_cost.rs`).

| What | Figure | Kind |
|---|---|---|
| `<C-a>` in Visual mode over 500 texts | 2.2 ms, debug (4.4 µs an object) | measured |
| 30,000 `<C-a><C-x>` on one text | 308 ms, debug (10 µs each, a key's own 1.3 µs included) | measured |
| `SIZE_WORK` | 4,500 units an object, and once more for the undo step | from the two rows above |
| `10000<C-a>` in Visual mode over a full board of text | 2.3 million units, 2.2 ms, debug | measured |
| A mapping that resizes until the key runs out of work | 92 to 115 ms in debug, 12 to 19 ms in release | measured |

## Jump labels

### Making and typing them

From the doc comments of `LABEL_WORK`, `PICK_WORK` and `MAX_KEY_WORK` in
`crates/lawh-core/src/engine/work.rs`.

| What | Figure | Kind |
|---|---|---|
| `s` over 500 rectangles (499 labels) | 0.43 ms, debug, against 0.18 ms for `w` | measured |
| `LABEL_WORK` | 500 units an object labelled | from the row above |
| 127 times `m<BS>` among 499 labels, every label looked at each time | 1.27 ms, debug | measured |
| `PICK_WORK` | 20 units a label looked at | from the row above |
| `s` over a full board of text | 1.3 million units: 1.1 ms in debug, 0.14 ms in release | measured |
| A mapping that shows labels until the key runs out of work | 15 to 22 ms in release, 89 to 95 ms in debug | measured (lane log, not in the repo) |
| 500 objects and the default 26 letters | 7 one-character labels, 19 first letters for the two-character ones | derived (`split` in `engine/jump.rs`) |
| Labels two characters of 26 can make | 676, more than a board holds objects (500) | derived (doc of `MAX_LABEL_KEYS`) |

### Drawing them

From the module doc of `crates/lawh-render/src/renderer/labels.rs` (release, Linux `x86_64`).

| What | 1920x1080 | 2880x1800, scale 2 | Kind |
|---|---|---|---|
| 500 two-letter labels spread over the window | about 2.7 ms | about 4.1 ms | measured, release |
| Visual mode, 500 selected and 500 labels, if labels didn't share `HIGHLIGHT_BUDGET_NS` | | 8.2 ms | measured, release (lane log, not in the repo) |

Two thirds of the labels' cost is their letters. Filling the letters as one path, or the boxes as
one path, was no faster (same source).

## `:sym`

### The symbol list

From the doc comment of `lawh_math::symbols` (`crates/lawh-math/src/symbols.rs`) and
`counts_are_codex_0_3_0_and_the_bundled_fonts` in `crates/lawh-math/tests/symbols.rs`.

| What | Figure | Kind |
|---|---|---|
| Names Typst math knows (codex 0.3.0, not deprecated) | 1,189: 1,142 in `sym`, 47 in `gender` (12) and `control` (35) | measured (counted) |
| Drawn by the bundled fonts | 732: 725 with glyphs, 7 as nothing (`zws`, `zwj`, `zwnj`, `wj`, `lrm`, `rlm`, `hyph.soft`) | measured (counted) |
| Not drawn (Libertinus Math's missing-glyph box) | 457: 419 in `sym`, 38 in the sub-modules | measured (counted) |
| Every name compiled and its glyphs checked against its flag | 0.6 s, debug | measured (`every_name_compiles_and_draws_as_its_flag_says`) |
| Building the list (`symbols()`) | 0.7 ms a call in release (1.0 ms the first, which parses the fonts); 3.7 ms in debug (4.5 to 6.3 ms the first) | measured (`measure_symbol_list`) |
| Its memory | 83 KB: 1,189 entries of 56 bytes, 12.8 KB of names, 3.6 KB of values | measured |
| The app at start: the list built, filtered and handed to the engine (732 kept) | 1.2 to 1.8 ms in release, 6.2 ms in debug | measured (lane log, not in the repo; the app logs it at `RUST_LOG=info`) |
| The longest Typst name | 28 characters | measured (doc of `MAX_SYM_QUERY_CHARS`, `engine/sym.rs`) |

### Matching

From the doc comments of `SYM_WORK`, `SYM_CHAR_WORK`, `SYM_CELL_WORK` and `MAX_KEY_WORK`
(`crates/lawh-core/src/engine/work.rs`), and of `MAX_SYMBOLS`, `MAX_SYM_QUERY_CHARS` and
`MAX_SYM_QUERY_BYTES` (`crates/lawh-core/src/engine/sym.rs`). "Typst-like" names are the tests'
`typst_like_symbols`, 14 characters on average.

| What | Figure | Kind |
|---|---|---|
| `nucleo-matcher` over Typst's 1,142 `sym` names, one query | 25 to 75 µs in release, 40 to 385 µs in debug | measured (research notes, not in the repo) |
| `a` over 4,096 Typst-like names | 0.40 ms, debug; each more word of `a a ... a` 24 µs more | measured |
| The same | 0.20 ms, release | measured (lane log, not in the repo) |
| `aa` and 61 `a`s over 4,096 names of 64 characters | 1.42 and 2.00 µs a name, debug | measured |
| 32 `a`s over 4,096 names of 64 characters | 23.8 ms, debug; two words of 31 `a`s 47 ms | measured |
| Each key of the costliest 64-character query over 4,096 Typst-like names | at most 32.8 million units, 4.3 ms in debug | measured (`work_unit_costs`) |
| A key that runs out of work matching | 30 ms in release over 4,096 Typst-like names, 66 ms over names of 64 characters | measured |
| A query of 1,000 characters and 500 words over the 1,142 `sym` names | 7.3 ms, release | measured (why `MAX_SYM_QUERY_CHARS` is 64) |
| A mapping that types and takes back a character after a 20 KB `:sym` query, 30,000 keys | before `MAX_SYM_QUERY_BYTES`: 110 ms in release, 171 ms in debug, no work counted; after: 6.5 ms and 52 ms | measured (`key_costs`) |

Matching is the one kind of work whose units don't shrink in release: nucleo is a dependency, and
dependencies build with `opt-level = 2` in debug too, so a unit of it takes up to 0.9 ns in
either build (measured).

## Text layout charged to the key

The renderer measures text for the engine (`impl Measure for Renderer`). Before the fix wave, the
engine counted only its own fixed weight for each character measured, so text the renderer had to
lay out afresh cost the key nothing. Now the renderer reports it (`Measure::take_work`) at
`KEY_WORK_PER_LAYOUT_NS`, 1 unit per estimated nanosecond. Source: the doc comment of
`KEY_WORK_PER_LAYOUT_NS` in `crates/lawh-render/src/budget.rs` (`key_costs` in
`crates/lawh-render/tests/measure_cost.rs`, two runs).

A mapping types into a text of 3,800 characters, then shows jump labels (or moves `w` and `b`),
over and over:

| Text | Before: 1,000 rounds (the mapping depth) | After, release | After, debug | Kind |
|---|---|---|---|---|
| Words | 1.9 s in release, 7 s in debug | 27 rounds in 54 to 55 ms (46 to 47 ms with `wb`) | 179 to 187 ms (163 to 170 ms) | measured |
| The script changing at every character | 6.4 s in release, 19.5 s in debug | 14 rounds in 95 to 99 ms (93 to 95 ms) | 277 to 290 ms (264 to 280 ms) | measured |

lawh-render builds without optimisation in debug and lays text out about 3.3 times slower there
(same source).

| What | Figure | Kind |
|---|---|---|
| `w` over 500 texts of 79 characters already laid out | 1.05 million units, 0.63 ms in debug, 40 µs in release | measured (`work_unit_costs` in `measure_cost.rs`; doc of `MEASURE_WORK`) |
| A whole board of text laid out afresh, at the text caps | estimated at up to 125 ms (125 million units); takes about 70 ms in release | estimate derived, time measured (`text_caps_worst_cases`) |
| The worst board to open, after board text gained its font fallback (a character Inter lacks drawn from Libertinus Math) | 70.7 ms, release | measured (lane log, not in the repo; noted in `MAX_BOARD_TEXT_CHARS`'s doc, whose table keeps the 68 and 69 ms of 2026-10-11) |
| Text with characters neither font has, after the font fallback | 0.05 to 0.2 µs a character slower to lay out | measured (lane log, not in the repo; build not recorded) |

`Engine::go_first` on a board just opened doesn't count laying the board out, since that would
pass a key's whole budget on a hostile board.

## The overlay

`overlay_frame_time` in `crates/lawh-render/tests/bench.rs`, release, 2026-10-12, load average 2
to 2.6 on 22 threads: what drawing the overlay adds to a frame (the median of `render_with` minus
that of `render`, timed by turns). Source: the doc comment of `Renderer::render_with`
(`crates/lawh-render/src/renderer/ui.rs`).

| Overlay | 1920x1080 | 2880x1800, scale 2 | Kind |
|---|---|---|---|
| Everyday (the cursor on an object, a caret on every other frame, a short status line, on the area board) | at most 0.3 ms | at most 0.3 ms | measured |
| Worst in Visual mode (500 objects selected, a Visual rectangle over the view, a jump label on every object) | 4.3 to 4.7 ms | 6.5 to 7.2 ms | measured |
| Worst on a `:sym` line (500 selected, the longest command line, eight rows of the longest names, changing every frame) | 5.0 to 5.6 ms | 4.7 to 5.4 ms | measured |
| M2's worst (no labels or rows), on the same day | 1.9 to 4.5 ms over both sizes | | measured (M2 recorded 1.88 and 4.2 ms on 2026-10-11) |
| `OVERLAY_TARGET` | 8 ms | 8 ms | target |

## Calibrations

The frame budget's calibrations after the fix waves (release, load average 1.6 to 2.1): the worst
ratio of measured time to estimate among rows estimated at 1 ms or more. Each must stay at most
1.2 (plus 0.2 ms of noise). From the second fix wave's log, not in the repo.

| Calibration | Worst ratio | Kind |
|---|---|---|
| `calibrate_estimating` | 0.91 | measured |
| `calibrate_formulas` | 1.10 | measured |
| `calibrate_glyphs` | 0.93 | measured |
| `calibrate_paths` | 1.12 | measured |
| `calibrate_runs` | 0.68 | measured |
| `calibrate_text_drawing` | 0.97 | measured |
| `calibrate_text_layout` | 0.84 | measured |

Why a failing row is now measured again (up to 3 times, keeping the least): gate runs of
`calibrate_text_drawing` in release at a load of 1.5 to 7 failed 3 of 3 times, each on about
0.16% of its rows, no row twice, rows of small estimates 0.3 to 4 ms over; `main` failed at the
same rate (measured; the doc comment of `MEASURES` in `crates/lawh-render/src/renderer/calibrate.rs`).

## The acceptance test

| What | Figure | Kind |
|---|---|---|
| Keys to type `examples/area.lawh` from an empty board and write it | 62, unchanged | target, asserted by `the_area_example_in_62_keys` in `crates/lawh-core/tests/acceptance.rs` |
