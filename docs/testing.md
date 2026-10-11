# Testing

How lawh is tested, layer by layer. Test-only dependencies are `insta` (snapshots), `proptest`
(property tests) and `kdl` (the reference parser the differential tests compare against). See
[tech-stack.md](tech-stack.md).

## Kinds of test

| Kind | What | Where |
|---|---|---|
| Unit | Small checks next to the code: limits, escapes, protocol encoding, budget maths, pan and zoom maths | `#[cfg(test)]` modules in each crate |
| Snapshots (`insta`) | Every way a `.lawh` file can be rejected, with its message and `line:column`; files as the writer prints them; math layouts (box size, baseline, what is drawn) | `lawh-core/tests/format_errors.rs`, `format_files.rs`; `lawh-math/tests/layout.rs` |
| Property tests (`proptest`) | Random boards survive a write and a read exactly, bit for bit | `lawh-core/tests/format_proptest.rs` |
| Differential, against `kdl` | lawh's parser reads random flat KDL documents and random "soup" (pieces of KDL syntax, mostly invalid) as kdl does, values included, and accepts what kdl accepts; written numbers and escapes read back the same in kdl and in lawh's parser | `lawh-core/src/format/parse.rs`, `write.rs` (tests modules) |
| Golden PNGs | `examples/area.lawh` fitted to 800x600 and at zoom 3 around its formula, compared with reviewed images | `lawh-render/tests/golden.rs`, `tests/golden/*.png` |
| Renderer behaviour | Viewport maths, culling, placeholders, the frame budget's outlines | `lawh-render/tests/render.rs` |
| Math sandbox | File reads, package imports, plugins, images, data loaders, `raw`, CSL styles, the clock and system fonts all fail | `lawh-math/tests/sandbox.rs` |
| Math worker and timeouts | Hostile formulas fail in time and the worker recovers; the worker's layouts match in-process ones exactly; a missing or wrong worker is `Unavailable`; an endless loop times out once and is cached | `lawh-math/tests/worker.rs`, `timeout.rs` |
| Worker process (Linux only) | The memory cap holds, a timed-out worker is killed, dropping the engine ends the worker | `lawh-math/tests/worker_process.rs`, `lawh/tests/worker_process.rs` |
| App | The command line and load errors (no window is opened); the app binary as its own math worker | `lawh/tests/cli.rs`, `math_worker.rs` |

### Differential tests

A differential test feeds the same input to two implementations and fails on any difference. The
parser's property tests run 4,096 cases each by default; set `LAWH_PROPTEST_CASES` for a long run:

```bash
LAWH_PROPTEST_CASES=200000 nice -n 19 cargo test --release -p lawh-core --lib parser_
```

The known differences are written down in the test (`agree_with_kdl`) and in
[architecture.md](architecture.md#reader): lawh rejects slashdash chains and keywords glued to the
next token, and reads past a slashdashed raw string that reads as a lone `"`. Counts from M1 runs
are in [research/m1-measurements.md](research/m1-measurements.md#differential-testing).

### Golden images

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

# Worst-case boards (frames < 33 ms, opening < 100 ms) and the other benches
nice -n 19 cargo test --release -p lawh-render --test bench -- --ignored --nocapture --test-threads=1
```

| Variable | Picks |
|---|---|
| `LAWH_CAL_KINDS` | Calibration kinds (and boards) by comma-separated name prefix, for example `LAWH_CAL_KINDS=words,bidi` |
| `LAWH_CAL_COUNT` | How deep `calibrate_glyphs` stacks glyphs |
| `LAWH_BENCH_KINDS` | Bench cases or boards by name prefix |
| `LAWH_BENCH_SIZES` | Sizes for `math_complexity_frame_time` and `board_objects_frame_time` |
| `LAWH_CAP_CHARS`, `LAWH_CAP_LINES`, `LAWH_CAP_TOTAL` | Other text caps for `text_caps_worst_cases` |
| `LAWH_READ_BYTES` | File size for `read_time_of_filler_files` |

`calibrate_text_memory` measures resident memory, so it starts itself again in a new process for
each kind of text: after other work, the allocator hands out memory it kept and the numbers read 0.

Other measurements, also `#[ignore]`:

| Test | Measures | Run |
|---|---|---|
| `read_time_at_the_size_limit` | Reading 1 MiB files | `cargo test --release -p lawh-core --test format_errors -- --ignored --nocapture read_time` |
| `measure_heavy_formulas` | Size of heavy and hostile formulas | `cargo test --release -p lawh-math --test layout -- --ignored --nocapture measure` |
| `measure_churn_of_formulas_that_do_not_fit_the_cache` | Recompiles over a minute against the 8 + 2t bound | `cargo test --release -p lawh-math --test worker -- --ignored --nocapture churn` |
| `measure_everyday_footprint` | The worker's peak address space and resident memory | `cargo test --release -p lawh --test worker_process -- --ignored --nocapture` |
| `run_repro_directory` | Every `*.typ` formula in `LAWH_MATH_REPROS` through a worker | `LAWH_MATH_REPROS=dir cargo test -p lawh-math --test worker -- --ignored --nocapture` |
| `copy_1080p_frame_time` | Copying a frame to the window's pixel format | `cargo test -p lawh --bin lawh -- --ignored --nocapture` |

Put `nice -n 19` in front of each on the laptop.

**Calibrations are sensitive to machine load.** They compare measured time with an estimate, so a
busy machine makes them fail where a quiet one passes. Run them on a quiet machine, and read the
printed numbers whether they pass or fail. Every recorded number states the machine, the date and
whether it was measured or derived ([research/m1-measurements.md](research/m1-measurements.md)).

### In CI

`bench.yml` runs the calibrations and benches in release on GitHub's Linux runner. It runs only
when triggered by hand (`gh workflow run bench.yml`, `workflow_dispatch`), because the repo is
private and Actions minutes are limited. `calibrate_text_memory` has its own step, so it doesn't
run after the other calibrations in one process. Every step runs even when an earlier one fails.
The limits were measured on the maintainer's laptop, so a slower or noisier runner can fail them.

`ci.yml` runs `cargo test --workspace`, which skips everything above.

## The idea for the Vim engine: keys in, board out (milestone 2)

The Vim engine in `lawh-core` will be a pure state machine, so most of lawh can be tested without
a window. A test types a key script in Vim notation and snapshots the resulting board **as a
`.lawh` file**:

```rust
#[test]
fn circle_at_cursor() {
    let out = run(Board::new(), "3l2j:circle r=2<CR>");
    insta::assert_snapshot!(out.to_lawh());
}
```

```kdl
// snapshot: mode=normal cursor=(3,2) keys=17
lawh version=1
circle id=c1 x=3 y=2 r=2
```

Reviewing a snapshot means reading a board file, the same format users see. `run` will accept Vim
key notation (`<CR>`, `<Esc>`, `<C-r>`, `<Space>`), and the same parser will later read key
mappings from config.

## Layers

| Layer | What | How | Status |
|---|---|---|---|
| File format (`lawh-core`) | Read, write, errors, caps | Snapshots of every error, round-trip property tests, differential tests against kdl | Done |
| Math (`lawh-math`) | Typst snippets compile, the sandbox and the worker hold | Layout snapshots, sandbox and worker tests | Done |
| Rendering (`lawh-render`) | Pixels, the frame budget | Golden PNGs, behaviour tests, calibrations and benches | Done (SVG and PDF export to come) |
| App (`lawh`) | Command line, worker mode | Process tests without a window | Done |
| Engine (`lawh-core`) | Modes, counts, operators, motions, undo, `.` repeat | Key script → `insta` snapshot of the board, mode and cursor | Milestone 2 |
| `:` commands (`lawh-core`) | Parsing and errors | Table tests: input → command or error message | Milestone 2 |
| Acceptance (`lawh-core/tests/`) | The "area of a circle" benchmark built from keys only | One key script must produce `examples/area.lawh`. Its key count is recorded, since "fewer actions than a mouse" is the product's promise. | Milestone 2 |
| Window | Input, presenting frames | No automated GUI tests in v1. A manual checklist on Linux (Wayland and X11) before any release. | Manual |

## Later

- **Fuzzing:** `cargo-fuzz` targets for the `.lawh` reader and the `:` command parser, since boards
  come from other people (see [SECURITY.md](../SECURITY.md)).

## What not to test

- winit, Vello, parley or Typst internals.
- Pixel-identical text across operating systems beyond the golden tolerance above.

## Commands

```bash
cargo test -p lawh-core        # the fast loop: no Typst, no renderer
cargo test --workspace         # everything, as CI runs it
cargo insta review             # accept or reject changed snapshots (cargo install --locked cargo-insta)
```

In CI, `insta` fails on any new or changed snapshot instead of writing it, so snapshots are always
reviewed locally before they're committed.
