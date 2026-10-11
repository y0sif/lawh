# Milestone 1 measurements (2026-10-11)

The numbers behind the limits and design choices of milestone 1 ("see a board"). Each figure is
marked:

- **measured**: timed or counted on the machine below.
- **derived**: computed from the code's constants, not timed.
- **target**: a limit a test asserts.

Numbers recorded in code comments are copied here with their source, so the comment stays the
place to update them.

## Machine and conditions

| | |
|---|---|
| CPU | Intel Core Ultra 9 185H |
| RAM | 14 GB |
| OS | Arch Linux, Hyprland (Wayland) |
| Date | 2026-10-11 unless a row says otherwise |
| Build | release unless a row says dev or debug |
| Load | the laptop had other work running; the load average is given where it was noted |

Timings move with machine load, the calibrations most of all (see [testing.md](../testing.md)).
Treat a range as the spread across runs, not as an error bar.

## Frames

| What | Figure | Kind | Conditions |
|---|---|---|---|
| A frame of `examples/area.lawh` at 1920x1080 | about 1.1 ms | measured | release, during M1 |
| First frame, from process start to presented | median 17.7 to 19.0 ms (sets of 5 runs) | measured | dev profile, load 2.4; the previous commit `bbc3595` measured the same |
| Of that, rendering the first frame | 3.4 to 5.1 ms | measured | as above |
| `frame_budget_worst_cases`: worst frame | 23.1 to 26.5 ms | measured | runs on 2026-10-10 and 2026-10-11, load 4 to 7 |
| `frame_budget_worst_cases`: worst open (bounds of a new board) | 67.7 to 73.5 ms | measured | as above |
| Frame target | 33 ms | target | `FRAME_TARGET` in `crates/lawh-render/tests/bench.rs` |
| Open target | 100 ms | target | `OPEN_TARGET`, same file |

The app logs both first-frame numbers at `RUST_LOG=info` (the `first frame presented` line:
`since_start_ms` and `render_ms`).

### Boards at the object cap

From the doc comment of `MAX_OBJECTS` in `crates/lawh-core/src/board.rs`:
`board_objects_frame_time`, release, fitted at 1920x1080, 2026-10-10, before the frame budget.
Median frames, all measured:

| Objects | Full-screen rects | Screen-filling circles | `r=1` circles, stacked |
|---|---|---|---|
| 400 | 54 ms | 56 ms | 22 ms |
| 500 | 65 ms | 79 ms | 28 ms |
| 1,000 (before the cap) | 130 ms | 141 ms | 49 ms |

### Boards at the text caps

From the doc comment of `MAX_BOARD_TEXT_CHARS` in `crates/lawh-core/src/board.rs`:
`text_caps_worst_cases`, release, 2026-10-11, all text stacked on one spot. All measured:

| Board | Laying out on open | Extra memory | Slowest frame |
|---|---|---|---|
| 8 texts of 5,000 characters, the direction changing at every one, 455 lines each | 69 ms | +10 MiB | 13 ms |
| 8 texts of 5,000 characters, the script changing at every one | 68 ms | +14 MiB | 9 ms |
| 80 texts of 499 line breaks | 22 ms | +9 MiB | 8 ms |
| 500 texts of 80 characters of words | 26 ms | +4 MiB | 19 ms |
| 80 failing formulas of 499 line breaks, drawn as their source | 25 ms | +9 MiB | 9 ms |
| Before these caps: 83 texts of 100,000 line breaks (an 8.3 MB file) | 4,203 ms | +1,475 MiB | 167 ms |

## Reading `.lawh` files

### Hostile 1 MiB files, before and after lawh's own parser

Release. Before: the reader lawh had on top of the `kdl` crate. All measured.

| File | Before (kdl) | Peak memory before |
|---|---|---|
| `lawh version=1` then 524K arguments | 2.04 s | 459 MB |
| 499 circles with about 1 KB of arguments each | 1.8 s | 369 MB |
| `a=1` repeated on the header | 1.3 s | |
| A string of 349K lines | 1.04 s | |
| One string of 1 MiB | 0.41 s | |

| After (lawh's parser) | Figure | Kind |
|---|---|---|
| Each file above | under 1 ms | measured |
| The worst of an adversarial search over about 110 file shapes | 32.1 ms (one slashdashed node with 524K arguments) | measured |
| Extra heap while reading, worst case | at most 225 KB | measured |
| The largest board the caps allow | 2.7 ms (66 ms before) | measured |
| Size of that board as written | 415,551 bytes | measured |

### Read time at the size limit

From the doc comment of `MAX_FILE_BYTES` in `crates/lawh-core/src/format/mod.rs`:
`read_time_at_the_size_limit` in `crates/lawh-core/tests/format_errors.rs`, Linux `x86_64`,
release, other work running, 2026-10-11, files of up to 1 MiB. All measured. This is a separate
run from the table above, so the kdl figures differ a little.

| File | Read | Through kdl |
|---|---|---|
| Spaces, comments or line breaks | 1.4 to 5.7 ms | 8 to 31 ms |
| Slashdashed nodes or properties, or a mix of filler | 15 to 22 ms | 25 to 57 ms |
| The largest board the caps allow (416 KB) | 2.7 ms | 66 ms |
| 500 circles, then more up to the limit | 0.6 ms | 14.5 ms |
| `lawh version=1` then 524K ` 1` arguments | 0.001 ms | 2.04 s, 416 MB |
| 499 circles with 2 KB of arguments each | 0.002 ms | 1.85 s, 347 MB |
| `a=1` repeated on the header | 0.002 ms | 1.34 s, 225 MB |
| A multi-line text of 349K lines | 0.5 ms | 1.06 s, 30 MB |
| One string of 1 MiB | 0.1 ms | 0.41 s |
| A multi-line string indented 2,000 spaces | 0.03 ms | 0.39 s |

The last six peaked at 0.2 MB of extra memory or less with lawh's parser (the measurement's
floor); the sizes in the last column are kdl's peaks.

## Differential testing

Counts of generated cases, all measured. "Differential" means the same input goes through two
implementations and the test fails on any difference. `LAWH_PROPTEST_CASES` sets how many
documents the parser's property tests generate.

| Check | Cases | Result |
|---|---|---|
| lawh's parser against kdl, random documents | 2.0M, plus 1.2M after the raw-string fix | 0 disagreements |
| lawh's parser against the old kdl-based reader | 8.0M | 0 disagreements, apart from the documented caps |
| Values (strings, numbers, keywords) against kdl | 3.0M | checked |
| Random boards written by the writer and read back | 20K | all round-tripped; the largest wrote 415,551 bytes |

## Math

| What | Figure | Kind | Conditions |
|---|---|---|---|
| Worker's peak address space, everyday formulas | 177 MiB of the 768 MiB cap | measured | during M1 |
| Compiling one formula, cold | about 7 ms | measured | release |
| The same formula from the engine's cache | about 1 µs | measured | release |
| Recompiles, 40 formulas that don't fit in the cache together | 82 in 60 s | measured | |
| Bound for 60 s | 128 (8 + 2t) | derived | `MAX_RECOMPILE_BURST`, `RECOMPILE_INTERVAL` |
| Recompiles, 90 such formulas | 97 in 45 s | measured | |
| Bound for 45 s | 98 | derived | as above |
| CPU used by the engine's queue thread | 40 ms over 60 s of churn, 0 ms over 10 s idle | measured | |

From code comments, all measured:

- `WORKER_MEMORY_LIMIT_BYTES` (`crates/lawh-math/src/worker.rs`): 173 MiB peak address space and
  21 MiB peak resident over 40 everyday formulas, debug test worker, Linux `x86_64`, 2026-10-10.
  The cap is about four times that peak.
- `MAX_RECOMPILE_BURST` (`crates/lawh-math/src/engine.rs`): before the recompile limit, 40
  formulas at the complexity caps, all on screen, made 2,099 compiles in 55 s and never settled.
- `MAX_LAYOUT_GLYPHS` (`crates/lawh-math/src/lib.rs`, release, 2026-10-10, before the frame
  budget): the largest real formula measured, a 30x30 matrix of subscripted entries, has 4,932
  glyphs. A formula of under 60 bytes that lays out 490,000 glyphs took 1.4 s a frame. At the cap, glyphs
  stacked on one spot took 1.2 s a frame at zoom 20.
- `MAX_LAYOUT_PATH_ELEMENTS` (same file and conditions): a 95-byte `#polygon` of 200,002 path
  elements took 5.3 s a frame. A hostile layout at the cap took 155 to 305 ms a frame fitted to
  1920x1080.

## Text shaping

Laying out text with parley and Inter, per character, release, `calibrate_text_layout`. All
measured except the estimate.

| Characters | With `calt` on | With `calt` off |
|---|---|---|
| `)` `]` `}` | 6.5 to 7.0 µs | about 0.35 µs |
| `(` `[` `{` `-` `:` `=` `~` `+` `@` `<` `>` `*` | 4.0 to 4.9 µs | 0.4 to 0.64 µs |

After `calt`, `frac`, `numr` and `dnom` were turned off, every printable ASCII character measured
0.24 to 0.75 µs, against the frame budget's estimate of 0.95 µs per ASCII character (derived:
`NS_PER_LAYOUT_ASCII_CHAR` in `crates/lawh-render/src/budget.rs`).

## Dev profile

`opt-level = 2` for every dependency and for the `lawh` crate in debug builds (root
`Cargo.toml`).

| What | Before | After | Kind |
|---|---|---|---|
| A debug frame (`area.lawh`, 1920x1080) | 116 ms | about 1.2 ms | measured |
| A cold `cargo build` | 151 s | 479 s | measured |

From the comment above the profile in the root `Cargo.toml` (debug, 1920x1080, `area.lawh`), all
measured:

- All dependencies at opt-level 2: lawh-render's bench went from 116 ms to 1.3 ms a frame.
- lawh-render at opt-level 1 on top: no measurable gain, so it stays at 0.
- `lawh` at opt-level 2: copying a frame to the window went from 35 ms (opt 0) and 3.2 ms
  (opt 1) to 1 ms, for about a second more per build.
