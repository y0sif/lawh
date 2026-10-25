# Testing

How lawh is tested, layer by layer. Test-only dependencies are `insta` (snapshots), `proptest`
(property tests), `kdl` (the reference parser the differential tests compare against), and for
exports `resvg` (draws exported SVGs), `roxmltree` (reads their XML) and `hayro` (reads and draws
exported PDFs). See [tech-stack.md](tech-stack.md).

## Kinds of test

| Kind | What | Where |
|---|---|---|
| Unit | Small checks next to the code: limits, escapes, protocol encoding, budget maths, pan and zoom maths | `#[cfg(test)]` modules in each crate |
| Snapshots (`insta`) | Every way a `.lawh` file can be rejected, with its message and `line:column`; files as the writer prints them; math layouts (box size, baseline, what is drawn); a small board's whole SVG export | `lawh-core/tests/format_errors.rs`, `format_files.rs`; `lawh-math/tests/layout.rs`; `lawh-render/tests/export_svg.rs` |
| Engine key scripts (`insta`) | Keys in, board out: see [The Vim engine](#the-vim-engine-keys-in-board-out) | `lawh-core/tests/engine_*.rs`, `jump_labels.rs`, `sym.rs`, `keymap.rs`, `acceptance.rs` |
| Property tests (`proptest`) | Random boards, text and math sizes included, survive a write and a read exactly, bit for bit, and only sizes other than 1 are written; random edits keep the board's index and text count right; a batch of changes does what they do one by one, or nothing; the fast text count agrees with counting one character at a time; printed keys read back as the same keys; random key scripts undo and redo exactly; jump labels (the short labels `s` puts on objects to jump to) are prefix-free (no label is the start of another) | `lawh-core/tests/format_proptest.rs`, `lawh-core/src/board.rs` (tests module), `key_notation.rs`, `engine_undo.rs`, `jump_labels.rs` |
| Differential, against `kdl` | lawh's parser reads random flat KDL documents and random "soup" (pieces of KDL syntax, mostly invalid) as kdl does, values included, and accepts what kdl accepts; written numbers and escapes read back the same in kdl and in lawh's parser | `lawh-core/src/format/parse.rs`, `write.rs` (tests modules) |
| Golden PNGs | `examples/area.lawh` fitted to 800x600 and at zoom 3 around its formula; text and math at four sizes (`sizes`); symbols in text drawn from the math font (`text_symbols`); the overlay (`overlay_*.png`): the cursor on an object, a Visual selection, Insert in a word, a math block being edited, the command line, an error message, jump labels before and after a first character (`overlay_labels`, `overlay_labels_typed`), `:sym` rows (`overlay_completions`); whole PNG exports (`export_area`, and `export_mixed`: text at sizes 0.5, 1 and 2, a formula on a text's baseline, a formula of size 2, `∧` and `∠` from the math font, an ellipse, a line, an arrow) | `lawh-render/tests/golden.rs`, `tests/export.rs`, `tests/golden/*.png` |
| Renderer behaviour | Viewport maths, culling, placeholders, the frame budget's outlines; text and math bounds scale with their size, math and text share x-height (the height of a lowercase `x`) and baseline (the line letters sit on), measured on drawn pixels, and larger text costs more; caret geometry, hit tests, the bounds the engine measures with (by size and the math scale), symbols from the math font as one caret step, and the overlay leaving the board as `render` draws it | `lawh-render/tests/render.rs`, `overlay.rs` |
| Exports | What is framed (every object, outlines and arrowheads, a one-unit margin, no grid), transparency, selections (objects deleted since are left out), the PNG scale and its range, shrinking to every size cap, failed and timed-out formulas drawn red and counted, the math wait (stops, and ends on time), the budget, the byte cap, limits above the defaults changing nothing, the same bytes every time, the window untouched by an export, the zoom-1 sizes; PNG compression by size and its charge (a smaller scale is never refused where a larger one is written); error messages | `lawh-render/tests/export.rs`, unit tests in `src/export/*.rs` |
| SVG and PDF exports | SVG: resvg's drawing against the PNG export, the XML's structure (roxmltree: only the elements lawh writes, every reference inside the file, ids in order, printable ASCII), no board text in the file (markup, quotes, `]]>`, control characters, as text and as failing formulas), the side cap. PDF: read back with hayro (one page at 30 pt a unit, two fonts embedded as subsets with `ToUnicode`, the board's text recovered, no metadata or date), hayro's drawing against the PNG export, math glyphs defined once, the page cap, hostile text. Both: transparency, selections, the byte cap, the budget, the same bytes every time | `lawh-render/tests/export_svg.rs`, `export_pdf.rs` |
| The window and the exports | A window frame at zoom 1 against a PNG export at `pngscale` 1, pixel by pixel; the objects and items the window's sink and an export's are handed; every colour from the `Palette` (a scan of the sources) | `lawh-render/tests/scene.rs`, `palette_guard.rs` |
| Measuring for the engine | Laying out text the renderer measures for a key counts against the key's work budget: a mapping that edits a long text and shows jump labels or moves stops in time; text laid out already costs only the engine's own count; measuring between keys isn't charged | `lawh-render/tests/measure_cost.rs` |
| Math sandbox | File reads, package imports, plugins, images, data loaders, `raw`, CSL styles, the clock and system fonts all fail | `lawh-math/tests/sandbox.rs` |
| The `:sym` symbol list | Counts for codex 0.3.0 and the bundled fonts (1,189 names, 732 drawable, 457 not); names unique and in the engine's order; every name compiles in Typst, and its drawable flag agrees with the glyphs Typst lays out (0.6 s in debug) | `lawh-math/tests/symbols.rs` |
| Math worker and timeouts | Hostile formulas fail in time and the worker recovers; the worker's layouts match in-process ones exactly; a missing or wrong worker is `Unavailable`; an endless loop times out once and is cached | `lawh-math/tests/worker.rs`, `timeout.rs` |
| Worker process (Linux only) | The memory cap holds, a timed-out worker is killed, dropping the engine ends the worker, also mid-compile (killed and reaped before the drop returns) | `lawh-math/tests/worker_process.rs`, `lawh/tests/worker_process.rs` |
| App | The command line and load errors (no window is opened); the app binary as its own math worker, its logs on stderr only; key translation, the status line and title, atomic writes and write batches, the same-file check after a write (links, hard links a write replaced), the lawhrc reader, the mapping timer, pan, zoom and following the cursor; the view jump labels go on (the window above the status bar, as it is when `s` runs); the engine keeps every drawable symbol; exports from the window (the `exporting` status line, paths, the board's own file refused, effects and keys in the engine's order, a click or a close running waiting exports first) | `lawh/tests/cli.rs`, `math_worker.rs`; unit tests in `lawh/src/*.rs` |
| `lawh export` | The built binary, run in a directory under `target/tmp`: every format, a second run replacing the file, `--scale` and `--transparent`, a failed formula's one stderr line, a missing board, the board itself as the file, failures that write nothing, every usage error with exit 2, a board named `export`, the help; an export stopped by the math wait leaving no worker behind (it takes 10 s) | `lawh/tests/export.rs` |

### Differential tests

A differential test feeds the same input to two implementations and fails on any difference. The
parser's property tests run 4,096 cases each by default; set `LAWH_PROPTEST_CASES` for a long run.
The same variable sets the cases of `key_notation.rs` (1,024 by default), `engine_undo.rs` (256),
`format_proptest.rs` (512), `jump_labels.rs` (256) and the three board property tests in
`board.rs` (256 each): `edits_keep_the_index_and_text_count`,
`a_batch_is_its_changes_one_by_one_or_nothing` and
`check_text_counts_as_one_character_at_a_time_does`:

```bash
LAWH_PROPTEST_CASES=200000 nice -n 19 cargo test --release -p lawh-core --lib parser_
```

The known differences are written down in the test (`agree_with_kdl`) and in
[architecture.md](architecture.md#reader): lawh rejects slashdash chains and keywords glued to the
next token, and reads past a slashdashed raw string that reads as a lone `"`. Counts from M1 runs
are in [research/m1-measurements.md](research/m1-measurements.md#differential-testing).

### Golden images

The overlay goldens draw the area board with the cursor, a selection, a caret, jump labels,
`:sym` rows or the status line over it, through `Renderer::render_with`. Milestone 3 added
`sizes`, `text_symbols`, `overlay_labels`, `overlay_labels_typed` and `overlay_completions`, and
regenerated `area`, `area_zoom3`, `overlay_normal`, `overlay_visual`, `overlay_command_line` and
`overlay_error`, since math is now drawn 1.27 times larger and on the text baseline.

At most 0.01 % of pixels (100 in a million) may differ from the golden by more than 3 in any
channel. The tolerance absorbs last-bit float differences between machines (x86 and ARM) along
anti-aliased edges; a wrong glyph changes whole strokes, and the `a_wrong_glyph_fails_*` tests prove
the comparison catches one. To rewrite the images, then look at every one before committing:

```bash
LAWH_UPDATE_GOLDEN=1 cargo test -p lawh-render --test golden
```

A failing comparison writes the actual image to Cargo's test temp directory and names it.

The `export_*` goldens are whole PNG exports, compared the same way. Milestone 4 changed none of
the window's goldens. To rewrite the export goldens, then look at each:

```bash
LAWH_UPDATE_GOLDEN=1 cargo test -p lawh-render --test export
```

### Exports against the screen and other readers

Each format is held to the PNG export, and the PNG export to the window, by an independent
reader where one exists:

| Test | Compares | Allowed | Measured |
|---|---|---|---|
| `a_window_frame_at_zoom_1_matches_a_png_export_at_100_percent` (`tests/scene.rs`) | A window frame at zoom 1 and scale factor 1, centred on the export's frame, against a PNG export at `pngscale` 1 of a board with text at two sizes, a symbol from the math font, two formulas, a failed formula and every shape | The goldens' tolerance: at most 100 pixels in a million differ by more than 3 a channel. The 3 x 3 pixels around each grid point are left out, since only the window draws grid dots | 0 of 292,754 pixels differ at all (1,836 left out) |
| `svg_draws_as_the_png_export_does` (`tests/export_svg.rs`) | resvg's drawing of the SVG export against the PNG export: text at sizes 1 and 2, a formula on the baseline, a translucent formula, `∧`, every shape | 2 a channel inside shapes, glyphs and paper (pixels whose 3 x 3 neighbourhood in the PNG is one colour), 64 on edges | 0 inside; 56 at most on 9,332 edge pixels |
| `pdf_draws_as_the_png_export_does` (`tests/export_pdf.rs`) | hayro's drawing of the PDF export against the PNG export, the same board | As for SVG | 0 inside; 54 at most on 9,332 edge pixels, all along the horizontal bars of the formula's glyphs |

The edge tolerance is wide on purpose. `vello_cpu` works out exactly how much of a pixel a shape
covers; resvg's tiny-skia samples a pixel's coverage four times along y, so on a horizontal edge
it can be off by an eighth of a pixel, about 30 of the 219 levels between paper and ink, more
where edges meet; hayro draws glyphs from their paths and puts them a fraction of a pixel apart
from the PNG's. A quarter of full coverage (64) allows that and still fails a glyph or a line one
pixel off, or another colour, which differ by up to 219 there. Interior pixels prove the colours,
positions and blending.

The same files also go through checks that don't draw: the SVG's XML (`roxmltree`), the PDF's
pages, fonts and text (`hayro`), and same bytes from the same board, every time and after the
window has drawn. A third test in `tests/scene.rs` checks that the window's sink and an export's
are handed the same objects with the same kinds and counts of items.

**The colour guard** (`tests/palette_guard.rs`) reads every `.rs` file of `lawh-render/src` and
`lawh/src`, except `palette.rs` and test code, and fails on a colour made anywhere else: colour
constructors given a literal, named colours, `.with_alpha(`, hex colours in strings, colour
bytes. It reads files as text, so it can miss a new way of making a colour; to show it isn't
vacuous, it checks that it finds at least 15 colours in `palette.rs` and scans at least 25 files,
and a second test feeds it each kind of violation and each allowed form.

## Calibrations and benches

Timings are `#[ignore]` tests: they don't run with `cargo test`. Run them in release, one thread
at a time, so they don't share the CPU:

```bash
# The frame budget's and the exports' cost estimates: each asserts measured <= 1.2 x estimate in
# release (calibrate_svg, calibrate_pdf and calibrate_png_encoding are the exports')
nice -n 19 cargo test --release -p lawh-render --lib -- --ignored --nocapture --test-threads=1 calibrate

# Worst-case boards (frames < 33 ms, opening < 100 ms) and the other benches,
# overlay_frame_time included (asserts each worst overlay adds at most 8 ms, OVERLAY_TARGET)
nice -n 19 cargo test --release -p lawh-render --test bench -- --ignored --nocapture --test-threads=1

# What one key may cost: key_costs times the worst cases, work_unit_costs the cost of a work unit
nice -n 19 cargo test --release -p lawh-core --test engine_cost -- --ignored --nocapture --test-threads=1

# The work weights are debug timings, so run work_unit_costs in debug too
nice -n 19 cargo test -p lawh-core --test engine_cost -- --ignored --nocapture --test-threads=1 work_unit_costs

# The same with text measured by the real renderer (Measure::take_work)
nice -n 19 cargo test --release -p lawh-render --test measure_cost -- --ignored --nocapture --test-threads=1

# How long building the :sym list takes
nice -n 19 cargo test --release -p lawh-math --test symbols -- --ignored --nocapture --test-threads=1 measure_symbol_list
```

`work_unit_costs` prints, for single commands at their costliest on a full board, the time taken,
the work units counted and the nanoseconds a unit: a unit should stay about a nanosecond in debug (see `MAX_KEY_WORK` in
`lawh-core/src/engine/work.rs`). `key_costs` prints how long each worst case took and the message
it ended with. Neither asserts a time: read the numbers. lawh-render's `measure_cost.rs` has its
own `key_costs` and `work_unit_costs`, with text measured by the real renderer, so they include
laying text out (`Measure::take_work`).

**Re-measuring.** A calibration row over its limit (1.2 times its estimate, plus 0.2 ms of noise,
`SLACK_MS`) is measured again, up to 3 times in all (`MEASURES` in `renderer/calibrate.rs`), and
keeps its least time, each a median of several runs. Load only adds time, so the least of a few
medians is the nearest to what a row costs, while a row whose estimate is too low is over its
limit every time. A drawing row measures its empty frame (the paper and the final render, taken
off each time) once, before its first time (`Rows::add_past`), so measuring again only takes
noise off the time taken, never adds it to what is taken off. `calibrate_text_drawing` has rows
at size 1 and size 16.

**The export calibrations.** `calibrate_svg` and `calibrate_pdf` (in `export/svg.rs` and
`export/pdf.rs`) time the pieces a file is made of (a curve, an arrowhead, a glyph used, a glyph
defined, a font subset) and whole boards (the area board, 40,000 characters of text, many
distinct glyphs, 500 circles a million units across, 500 arrows, 500 formulas, 500 failing
formulas; for PDF also formulas in 500 colours, 80,000 dots in their own colours and 20 formulas
of 22,500 glyphs), and assert each board takes at most 1.2 times its estimate plus 0.2 ms (SVG)
or 0.5 ms (PDF); `calibrate_pdf` also asserts that its content estimate is at least what krilla
writes.
`calibrate_png_encoding` (`export/png.rs`) compresses noise from 500 x 500 to 8,000 x 8,000
pixels, RGB and RGBA, 15 patterns that are hard to compress at 2,000 x 2,000 (all of them at
8,000 x 8,000 too with `LAWH_PNG_ALL=1`, minutes more) and three boards, both ways, and asserts
each within 1.2 times its estimate plus 0.5 ms.

**The overlay bench.** `overlay_frame_time` times what the overlay adds to a frame: the median of
`render_with` minus that of `render`, timed by turns, a frame of each, so a change in the
machine's speed during the run falls on both alike. Its cases, at 1920x1080 and at 2880x1800 with
scale factor 2: an everyday overlay on the area board; the worst in Visual mode (500 objects
selected, a Visual rectangle over the view and a two-letter jump label on each object, spread so
every letter is drawn); the worst on a `:sym` line (the selection kept, the longest command line,
eight rows of the longest names, all changing every frame). The two worst cases are each asserted
under `OVERLAY_TARGET` (8 ms); both at once, which the engine never shows, is printed only.

| Variable | Picks |
|---|---|
| `LAWH_CAL_KINDS` | Calibration kinds (and boards) by comma-separated name prefix, for example `LAWH_CAL_KINDS=words,bidi` |
| `LAWH_CAL_COUNT` | How deep `calibrate_glyphs` stacks glyphs |
| `LAWH_BENCH_KINDS` | Bench cases or boards by name prefix |
| `LAWH_BENCH_SIZES` | Sizes for `math_complexity_frame_time` and `board_objects_frame_time` |
| `LAWH_CAP_CHARS`, `LAWH_CAP_LINES`, `LAWH_CAP_TOTAL` | Other text caps for `text_caps_worst_cases` |
| `LAWH_READ_BYTES` | File size for `read_time_of_filler_files` |
| `LAWH_COST_KINDS` | `key_costs` cases by comma-separated name prefix |
| `LAWH_PROPTEST_CASES` | Cases for the parser, board, file round-trip, key notation, undo and jump label property tests |
| `LAWH_PNG_ALL` | Any value: `calibrate_png_encoding` also compresses every hard pattern at 8,000 x 8,000 |

`calibrate_text_memory` measures resident memory, so it starts itself again in a new process for
each kind of text: after other work, the allocator hands out memory it kept and the numbers read 0.

Other measurements, also `#[ignore]`:

| Test | Measures | Run |
|---|---|---|
| `read_time_at_the_size_limit` | Reading 1 MiB files | `cargo test --release -p lawh-core --test format_errors -- --ignored --nocapture read_time` |
| `measure_heavy_formulas` | Size of heavy and hostile formulas | `cargo test --release -p lawh-math --test layout -- --ignored --nocapture measure` |
| `measure_churn_of_formulas_that_do_not_fit_the_cache` | Recompiles over a minute against the 8 + 2t bound | `cargo test --release -p lawh-math --test worker -- --ignored --nocapture churn` |
| `measure_everyday_footprint` | The worker's peak address space and resident memory | `cargo test --release -p lawh --test worker_process -- --ignored --nocapture` |
| `run_repro_directory` | Every `*.typ` formula in `LAWH_MATH_REPROS` through a worker | `LAWH_MATH_REPROS=dir cargo test -p lawh-math --test worker -- --ignored --nocapture run_repro` |
| `copy_1080p_frame_time` | Copying a frame to the window's pixel format | `cargo test -p lawh --bin lawh -- --ignored --nocapture` |
| `measure_exports` | PNG exports: time, peak memory, image and file size, for the area board and the limits board at `pngscale` 2 and 8, 500 hostile objects, and 500 failing formulas compiled cold | `cargo test --release -p lawh-render --test export -- --ignored --nocapture --test-threads=1 measure_exports` |
| `measure_svg_exports`, `measure_pdf_exports` | The same for SVG and PDF, with 40,000 characters of text and 500 formulas of 22,500 glyphs (stopped by the byte cap) | `cargo test --release -p lawh-render --test export_svg -- --ignored --nocapture --test-threads=1 measure_svg`, and `--test export_pdf ... measure_pdf` |
| `measure_png_peak` | Peak memory of compressing noise at the pixel cap, stopped by the byte cap | `cargo test --release -p lawh-render --lib -- --ignored --nocapture --test-threads=1 measure_png_peak` |
| `math_and_text_share_x_height_and_baseline` (not ignored) | How far math's baseline and x-height are from text's, at four sizes | `cargo test -p lawh-render --test render -- --nocapture math_and_text_share` |

Put `nice -n 19` in front of each on the laptop.

**Calibrations are sensitive to machine load.** They compare measured time with an estimate, so a
busy machine makes them fail where a quiet one passes. Run them on a quiet machine, and read the
printed numbers whether they pass or fail. Every recorded number states the machine, the date and
whether it was measured or derived ([research/m1-measurements.md](research/m1-measurements.md),
[research/m2-measurements.md](research/m2-measurements.md),
[research/m3-measurements.md](research/m3-measurements.md),
[research/m4-measurements.md](research/m4-measurements.md)).

### In CI

`bench.yml` runs the calibrations and benches in release on GitHub's Linux runner. It runs only
when triggered by hand (`gh workflow run bench.yml`, `workflow_dispatch`), because the repo is
private and Actions minutes are limited. `calibrate_text_memory` has its own step, so it doesn't
run after the other calibrations in one process. Every step runs even when an earlier one fails.
The limits were measured on the maintainer's laptop, so a slower or noisier runner can fail them.

Then it runs what one key may cost: `engine_cost` in release (`key_costs` and `work_unit_costs`),
`work_unit_costs` again in debug (the weights are debug timings, so a unit should stay about a
nanosecond there), `measure_cost` in release, and `measure_symbol_list` in release. These assert
no time: read the numbers.

The export calibrations run in the Calibrations step with the frame budget's, since their names
start with `calibrate`. The export measurements (`measure_exports`, `measure_svg_exports`,
`measure_pdf_exports`, `measure_png_peak`) are not in `bench.yml`: run them by hand.

`ci.yml` runs `cargo test --workspace`, which skips everything above. It does run the regression
tests in `engine_cost.rs` and lawh-render's `measure_cost.rs`, in debug: each worst case must
stop within a generous bound (5 s in debug) and say why. It also runs `lawh/tests/export.rs`,
whose math-wait test takes 10 s: six formulas that each run past the 2 s timeout, so the 10 s
wait ends on any machine.

## The Vim engine: keys in, board out

The engine in `lawh-core` is a state machine with no I/O, so most of lawh is tested without a
window. A test types a key script in Vim notation and snapshots a header line, then the board
**as a `.lawh` file**:

```rust
#[test]
fn circle_at_the_cursor() {
    assert_snapshot!(run(Board::new(), "3l2j:ci 2<CR>").snapshot());
}
```

```text
// mode=normal cursor=object(c1) keys=10
lawh version=1
circle id=c1 x=3 y=2 r=2
```

Reviewing a snapshot means reading a board file, the same format users see. Scripts are in the
key notation the engine reads everywhere (`parse_keys`: `<CR>`, `<Esc>`, `<C-r>`, `<Space>`), the
same one mappings and the lawhrc use.

### The harness

`lawh-core/tests/common/mod.rs`, included by each test file as `mod common;`:

| Helper | Does |
|---|---|
| `run(board, script)` | Types `script` into an engine for `board`, measuring with `FixedMeasure` (0.3 units a character, 0.6 a line, times the object's size) |
| `run_with_rc(rc, board, script)` | Reads `rc` as a lawhrc first (`Engine::source`); fails on any warning |
| `run_with_symbols(rc, symbols, board, script)` | Also gives the engine a symbol list for `:sym` (`Engine::set_symbols`), all of which it must keep |
| `symbol(name, value)`, `typst_like_symbols(n)`, `long_symbols(n)` | One symbol; `n` with names like Typst's (`arrow.r.long`, 14 characters on average); `n` with names of 64 characters, the costliest to match |
| `run_engine(engine, script)` | Types into an engine built by the test (a file name, a lawhrc with warnings) |
| `Run::feed`, `Run::click`, `Run::write_done`, `Run::write_done_same_file`, `Run::export_done` | More keys into the same engine, a click, the app's report of a write (to another file, or to the board's own under another name), the app's report of an export |
| `Run::snapshot`, `Run::header` | The header line then the board; the header alone |
| `trace(board, script)` | One header line per key, to see where a script goes wrong |
| `board(objects)` | A board from `.lawh` lines after the header |

The header gives the mode (`normal`, `visual`, `insert`, `block-normal`, `command-line`), the
cursor (`grid(x,y)` or `object(id)`) and the keys typed, each key in Vim notation counting one
(`<CR>` is one). Then, when there are any: ` selection=c1,t1` in Visual mode, ` byte=3` (the text
cursor in Insert and block-Normal mode) and ` message="..."`.

`<Timeout>` in a script stands for the mapping timeout: the harness calls `Engine::timeout`
there. It is not a key and counts as none.

`jump_labels.rs` adds the labels to its snapshots' header, one a line (the label, its object and
where it is drawn, nearest first), and `sym.rs` adds the command line (` line="sym al"`) and the
completion rows, `>` on the row selected.

A failing snapshot writes `*.snap.new` next to the old one. Read every new snapshot, then accept
it with `cargo insta review` or `INSTA_UPDATE=always`.

### Engine test files

All in `lawh-core/tests/`:

| File | Covers |
|---|---|
| `key_notation.rs` | Every key name and modifier, the canonical forms, literal `<`, errors; a property test that printing then reading any keys gives them back |
| `engine_basic.rs` | Grid motions with counts, the command line, `:circle`, undo and redo, and their caps |
| `engine_grammar.rs` | Object motions, operators and the register, puts and the place rule, Visual mode, `.` and `@:`, the `:` commands (`:te` and `:ma` on an object going below it), writing and quitting, clicks |
| `command_parse.rs` | The `:` parser alone: every command and short form, numbers, mapping commands, `:set`, errors |
| `engine_undo.rs` | Property test: random scripts of real commands on random small boards. Undoing everything gives back the start exactly, redoing everything the board after the last change, and no script panics |
| `engine_text.rs` | Text in blocks: new blocks, Insert and block-Normal mode, graphemes, `W B E`, the caps on text, the `escape` setting, undo, `.`, clicks |
| `engine_size.rs` | Sizes: `<C-a>` and `<C-x>` with counts, `.`, undo and Visual mode, `:si` on the current object and on a selection, `:set size` and `++size=` for new blocks, `o` and `O` keeping a block's size |
| `jump_labels.rs` | `s` in Normal mode, Visual mode and after `d` and `y`; the order and the split between one and two characters (25, 26, 27, 500 objects, the alphabet squared); `<Esc>`, `<BS>`, keys that start no label, clicks, mappings, the view, `.`, undo, `:set jumplabels` |
| `sym.rs` | `:sym`: completions and their order, `<Tab>` `<C-n>` `<S-Tab>` `<C-p>`, `<CR>` and `<Esc>`, picks into math (with its spacing), text and the board, undo, `@:`, `<C-k>` from Insert mode and `.` after it, mappings, the query cap, the symbols the engine keeps. A small hand-made list of Typst names stands in for the app's |
| `engine_export.rs` | `:export`: the command and its short forms (`:ex` is not one), formats in any case, every refusal message, an empty board or selection, Visual mode's selection in board order, `@:` and `.`, `pngscale` and `exportbg` (round trip, errors, lawhrc), `:w` `:wq` `:x` refusing an export's name, the per-key cap through mappings, the shared write slot, every `export_done` wording and the order of messages |
| `acceptance.rs` | The "area of a circle" example typed from an empty board, then written |
| `keymap.rs` | Every map mode, `map` against `noremap`, waiting and the timeout, `<nowait>`, `<Leader>`, the caps on mappings, `:set`, reading a lawhrc, `examples/lawhrc` |
| `engine_cost.rs` | Keys that mappings, `.` and counts multiply into a great deal of work stop within `MAX_KEY_WORK`, and the costliest single commands fit: size keys, jump labels on a full board, a 64-character `:sym` query, a long `:sym` line, `:exp` from a full selection, `:te` below a column of texts; a mapping of exports stops at the first; work a measure reports counts against the key, and opening a board doesn't count laying it out. Two ignored measurements, `key_costs` and `work_unit_costs` (see [Calibrations and benches](#calibrations-and-benches)) |

**Acceptance.** "Fewer actions than a mouse" is the product's promise, so `acceptance.rs` asserts
the key count: 62 keys, counting each `<...>` as one, make `examples/area.lawh` and write it:

```text
5kiArea of a circle<Esc>5j:ci 3<CR>:li 3 0<CR>lkir<Esc>4lj:ma A = pi r^2<CR>:w<CR>
```

## Layers

| Layer | What | How | Status |
|---|---|---|---|
| File format (`lawh-core`) | Read, write, errors, caps | Snapshots of every error, round-trip property tests, differential tests against kdl | Done |
| Math (`lawh-math`) | Typst snippets compile, the sandbox and the worker hold | Layout snapshots, sandbox and worker tests | Done |
| Rendering (`lawh-render`) | Pixels, the frame budget, the overlay, exports | Golden PNGs, behaviour tests, comparisons with resvg and hayro, calibrations and benches | Drawing done (milestone 1); the overlay built (milestone 2); sizes, jump labels, `:sym` rows and the text fallback (characters Inter lacks drawn from Libertinus Math) built (milestone 3); the shared draw list, the palette and export to SVG, PNG and PDF built (milestone 4) |
| App (`lawh`) | Command line, worker mode, keys, writes, exports, lawhrc | Process tests without a window, the built binary for `lawh export`, unit tests | Command line and worker mode done (milestone 1); keys, writes and lawhrc done (milestone 2); the view for labels, the symbol list and the same-file check built (milestone 3); exports from the window and `lawh export` built (milestone 4) |
| Engine (`lawh-core`) | Modes, counts, operators, motions, text in blocks, sizes, jump labels, `:sym`, `:export`, undo, `.` repeat, key maps, the cost of a key | Key script → `insta` snapshot of the board, mode and cursor; undo and label property tests; cost regressions | Done (milestone 2); sizes, jump labels and `:sym` built (milestone 3); `:export` and blocks placed below an object built (milestone 4) |
| `:` commands (`lawh-core`) | Parsing and errors | Table tests: input → command or error message | Done (milestone 2); `:si`, `:sym` and `++size=` built (milestone 3); `:exp` and `:w` refusing an export's name built (milestone 4) |
| Acceptance (`lawh-core/tests/`) | The "area of a circle" benchmark built from keys only | One key script produces `examples/area.lawh` in 62 keys, asserted | Done (milestone 2), unchanged in milestones 3 and 4 |
| Window | Input, presenting frames | No automated GUI tests in v1. A manual checklist on Linux (Wayland and X11) before any release, to be written. | Manual |

## Later

- **Fuzzing:** `cargo-fuzz` targets for the `.lawh` reader and the `:` command parser, since boards
  come from other people (see [SECURITY.md](../SECURITY.md)). Milestone 2's review ran a
  4,000-script random key fuzz by hand (no panic; numbers in
  [research/m2-measurements.md](research/m2-measurements.md)); it isn't in the repo.

## What not to test

- winit, Vello, parley or Typst internals.
- Pixel-identical text across operating systems beyond the golden tolerance above.

## Commands

```bash
cargo test -p lawh-core        # the fast loop: no Typst, no renderer, the engine included
cargo test --workspace         # everything, as CI runs it
cargo insta review             # accept or reject changed snapshots (cargo install --locked cargo-insta)
```

In CI, `insta` fails on any new or changed snapshot instead of writing it, so snapshots are always
reviewed locally before they're committed.
