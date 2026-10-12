# Testing

How lawh is tested, layer by layer. Test-only dependencies are `insta` (snapshots), `proptest`
(property tests) and `kdl` (the reference parser the differential tests compare against). See
[tech-stack.md](tech-stack.md).

## Kinds of test

| Kind | What | Where |
|---|---|---|
| Unit | Small checks next to the code: limits, escapes, protocol encoding, budget maths, pan and zoom maths | `#[cfg(test)]` modules in each crate |
| Snapshots (`insta`) | Every way a `.lawh` file can be rejected, with its message and `line:column`; files as the writer prints them; math layouts (box size, baseline, what is drawn) | `lawh-core/tests/format_errors.rs`, `format_files.rs`; `lawh-math/tests/layout.rs` |
| Engine key scripts (`insta`) | Keys in, board out: see [The Vim engine](#the-vim-engine-keys-in-board-out) | `lawh-core/tests/engine_*.rs`, `keymap.rs`, `acceptance.rs` |
| Property tests (`proptest`) | Random boards survive a write and a read exactly, bit for bit; random edits keep the board's index and text count right; a batch of changes does what they do one by one, or nothing; the fast text count agrees with counting one character at a time; printed keys read back as the same keys; random key scripts undo and redo exactly | `lawh-core/tests/format_proptest.rs`, `lawh-core/src/board.rs` (tests module), `key_notation.rs`, `engine_undo.rs` |
| Differential, against `kdl` | lawh's parser reads random flat KDL documents and random "soup" (pieces of KDL syntax, mostly invalid) as kdl does, values included, and accepts what kdl accepts; written numbers and escapes read back the same in kdl and in lawh's parser | `lawh-core/src/format/parse.rs`, `write.rs` (tests modules) |
| Golden PNGs | `examples/area.lawh` fitted to 800x600 and at zoom 3 around its formula; the overlay (`overlay_*.png`): the cursor on an object, a Visual selection, Insert in a word, a math block being edited, the command line, an error message | `lawh-render/tests/golden.rs`, `tests/golden/*.png` |
| Renderer behaviour | Viewport maths, culling, placeholders, the frame budget's outlines; caret geometry, hit tests, the bounds the engine measures with, and the overlay leaving the board as `render` draws it | `lawh-render/tests/render.rs`, `overlay.rs` |
| Math sandbox | File reads, package imports, plugins, images, data loaders, `raw`, CSL styles, the clock and system fonts all fail | `lawh-math/tests/sandbox.rs` |
| Math worker and timeouts | Hostile formulas fail in time and the worker recovers; the worker's layouts match in-process ones exactly; a missing or wrong worker is `Unavailable`; an endless loop times out once and is cached | `lawh-math/tests/worker.rs`, `timeout.rs` |
| Worker process (Linux only) | The memory cap holds, a timed-out worker is killed, dropping the engine ends the worker | `lawh-math/tests/worker_process.rs`, `lawh/tests/worker_process.rs` |
| App | The command line and load errors (no window is opened); the app binary as its own math worker, its logs on stderr only; key translation, the status line and title, atomic writes and write batches, the lawhrc reader, the mapping timer, pan, zoom and following the cursor | `lawh/tests/cli.rs`, `math_worker.rs`; unit tests in `lawh/src/*.rs` |

### Differential tests

A differential test feeds the same input to two implementations and fails on any difference. The
parser's property tests run 4,096 cases each by default; set `LAWH_PROPTEST_CASES` for a long run.
The same variable sets the cases of `key_notation.rs` (1,024 by default), `engine_undo.rs` (256)
and the three board property tests in `board.rs` (256 each):
`edits_keep_the_index_and_text_count`, `a_batch_is_its_changes_one_by_one_or_nothing` and
`check_text_counts_as_one_character_at_a_time_does`. `format_proptest.rs` ignores it and always
runs 512:

```bash
LAWH_PROPTEST_CASES=200000 nice -n 19 cargo test --release -p lawh-core --lib parser_
```

The known differences are written down in the test (`agree_with_kdl`) and in
[architecture.md](architecture.md#reader): lawh rejects slashdash chains and keywords glued to the
next token, and reads past a slashdashed raw string that reads as a lone `"`. Counts from M1 runs
are in [research/m1-measurements.md](research/m1-measurements.md#differential-testing).

### Golden images

The overlay goldens draw the area board with the cursor, a selection, a caret or the status line
over it, through `Renderer::render_with`.

At most 0.01 % of pixels (100 in a million) may differ from the golden by more than 3 in any
channel. The tolerance absorbs last-bit float differences between machines (x86 and ARM) along
anti-aliased edges; a wrong glyph changes whole strokes, and the `a_wrong_glyph_fails_*` tests prove
the comparison catches one. To rewrite the images, then look at every one before committing:

```bash
LAWH_UPDATE_GOLDEN=1 cargo test -p lawh-render --test golden
```

A failing comparison writes the actual image to Cargo's test temp directory and names it.

## Calibrations and benches

Timings are `#[ignore]` tests: they don't run with `cargo test`. Run them in release, one thread
at a time, so they don't share the CPU:

```bash
# The frame budget's cost estimates: each asserts measured <= 1.2 x estimate in release
nice -n 19 cargo test --release -p lawh-render --lib -- --ignored --nocapture --test-threads=1 calibrate

# Worst-case boards (frames < 33 ms, opening < 100 ms) and the other benches,
# overlay_frame_time included (asserts the worst overlay adds at most 8 ms, OVERLAY_TARGET)
nice -n 19 cargo test --release -p lawh-render --test bench -- --ignored --nocapture --test-threads=1

# What one key may cost: key_costs times the worst cases, work_unit_costs the cost of a work unit
nice -n 19 cargo test --release -p lawh-core --test engine_cost -- --ignored --nocapture --test-threads=1

# The work weights are debug timings, so run work_unit_costs in debug too
nice -n 19 cargo test -p lawh-core --test engine_cost -- --ignored --nocapture --test-threads=1 work_unit_costs
```

`work_unit_costs` prints, for single commands at their costliest on a full board, the time taken,
the work units counted and the nanoseconds a unit: a unit should stay about a nanosecond in debug (see `MAX_KEY_WORK` in
`lawh-core/src/engine/work.rs`). `key_costs` prints how long each worst case took and the message
it ended with. Neither asserts a time: read the numbers.

| Variable | Picks |
|---|---|
| `LAWH_CAL_KINDS` | Calibration kinds (and boards) by comma-separated name prefix, for example `LAWH_CAL_KINDS=words,bidi` |
| `LAWH_CAL_COUNT` | How deep `calibrate_glyphs` stacks glyphs |
| `LAWH_BENCH_KINDS` | Bench cases or boards by name prefix |
| `LAWH_BENCH_SIZES` | Sizes for `math_complexity_frame_time` and `board_objects_frame_time` |
| `LAWH_CAP_CHARS`, `LAWH_CAP_LINES`, `LAWH_CAP_TOTAL` | Other text caps for `text_caps_worst_cases` |
| `LAWH_READ_BYTES` | File size for `read_time_of_filler_files` |
| `LAWH_COST_KINDS` | `key_costs` cases by comma-separated name prefix |
| `LAWH_PROPTEST_CASES` | Cases for the parser, board, key notation and undo property tests |

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

Put `nice -n 19` in front of each on the laptop.

**Calibrations are sensitive to machine load.** They compare measured time with an estimate, so a
busy machine makes them fail where a quiet one passes. Run them on a quiet machine, and read the
printed numbers whether they pass or fail. Every recorded number states the machine, the date and
whether it was measured or derived ([research/m1-measurements.md](research/m1-measurements.md),
[research/m2-measurements.md](research/m2-measurements.md)).

### In CI

`bench.yml` runs the calibrations and benches in release on GitHub's Linux runner. It runs only
when triggered by hand (`gh workflow run bench.yml`, `workflow_dispatch`), because the repo is
private and Actions minutes are limited. `calibrate_text_memory` has its own step, so it doesn't
run after the other calibrations in one process. Every step runs even when an earlier one fails.
The limits were measured on the maintainer's laptop, so a slower or noisier runner can fail them.

`bench.yml` doesn't run the `engine_cost` measurements: run those by hand.

`ci.yml` runs `cargo test --workspace`, which skips everything above. It does run the regression
tests in `engine_cost.rs`, in debug: each worst case must stop within a generous bound (5 s in
debug) and say why.

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
| `run(board, script)` | Types `script` into an engine for `board`, measuring with `FixedMeasure` (0.3 units a character, 0.6 a line) |
| `run_with_rc(rc, board, script)` | Reads `rc` as a lawhrc first (`Engine::source`); fails on any warning |
| `run_engine(engine, script)` | Types into an engine built by the test (a file name, a lawhrc with warnings) |
| `Run::feed`, `Run::click`, `Run::write_done` | More keys into the same engine, a click, the app's report of a write |
| `Run::snapshot`, `Run::header` | The header line then the board; the header alone |
| `trace(board, script)` | One header line per key, to see where a script goes wrong |
| `board(objects)` | A board from `.lawh` lines after the header |

The header gives the mode (`normal`, `visual`, `insert`, `block-normal`, `command-line`), the
cursor (`grid(x,y)` or `object(id)`) and the keys typed, each key in Vim notation counting one
(`<CR>` is one). Then, when there are any: ` selection=c1,t1` in Visual mode, ` byte=3` (the text
cursor in Insert and block-Normal mode) and ` message="..."`.

`<Timeout>` in a script stands for the mapping timeout: the harness calls `Engine::timeout`
there. It is not a key and counts as none.

A failing snapshot writes `*.snap.new` next to the old one. Read every new snapshot, then accept
it with `cargo insta review` or `INSTA_UPDATE=always`.

### Engine test files

All in `lawh-core/tests/`:

| File | Covers |
|---|---|
| `key_notation.rs` | Every key name and modifier, the canonical forms, literal `<`, errors; a property test that printing then reading any keys gives them back |
| `engine_basic.rs` | Grid motions with counts, the command line, `:circle`, undo and redo, and their caps |
| `engine_grammar.rs` | Object motions, operators and the register, puts and the place rule, Visual mode, `.` and `@:`, the `:` commands, writing and quitting, clicks |
| `command_parse.rs` | The `:` parser alone: every command and short form, numbers, mapping commands, `:set`, errors |
| `engine_undo.rs` | Property test: random scripts of real commands on random small boards. Undoing everything gives back the start exactly, redoing everything the board after the last change, and no script panics |
| `engine_text.rs` | Text in blocks: new blocks, Insert and block-Normal mode, graphemes, the caps on text, the `escape` setting, undo, `.`, clicks |
| `acceptance.rs` | The "area of a circle" example typed from an empty board, then written |
| `keymap.rs` | Every map mode, `map` against `noremap`, waiting and the timeout, `<nowait>`, `<Leader>`, the caps on mappings, `:set`, reading a lawhrc, `examples/lawhrc` |
| `engine_cost.rs` | Keys that mappings, `.` and counts multiply into a great deal of work stop within `MAX_KEY_WORK`, and the costliest single commands fit; two ignored measurements, `key_costs` and `work_unit_costs` (see [Calibrations and benches](#calibrations-and-benches)) |

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
| Rendering (`lawh-render`) | Pixels, the frame budget, the overlay | Golden PNGs, behaviour tests, calibrations and benches | Drawing done (milestone 1); the overlay built (milestone 2); SVG and PDF export to come |
| App (`lawh`) | Command line, worker mode, keys, writes, lawhrc | Process tests without a window, unit tests | Command line and worker mode done (milestone 1); keys, writes and lawhrc built (milestone 2) |
| Engine (`lawh-core`) | Modes, counts, operators, motions, text in blocks, undo, `.` repeat, key maps, the cost of a key | Key script → `insta` snapshot of the board, mode and cursor; undo property test; cost regressions | Built (milestone 2) |
| `:` commands (`lawh-core`) | Parsing and errors | Table tests: input → command or error message | Built (milestone 2) |
| Acceptance (`lawh-core/tests/`) | The "area of a circle" benchmark built from keys only | One key script produces `examples/area.lawh` in 62 keys, asserted | Built (milestone 2) |
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
