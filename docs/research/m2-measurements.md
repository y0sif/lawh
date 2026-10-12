# Milestone 2 measurements (2026-10-12)

The numbers behind the limits and design choices of milestone 2 (the Vim engine). Each figure is
marked:

- **measured**: timed or counted on the machine below.
- **derived**: computed from the code's constants or other figures, not timed.
- **target**: a limit a test asserts.

Most numbers are copied from the doc comment of the constant they justify, or from the comment
above the test that measured them, with that source named, so the comment stays the place to
update them. Two come from the milestone's review runs and are not in the repo; they say so.

## Machine and conditions

| | |
|---|---|
| CPU | Intel Core Ultra 9 185H |
| RAM | 14 GB |
| OS | Arch Linux, Hyprland (Wayland) |
| Date | 2026-10-11 and 2026-10-12 |
| Build | given on every row: release or debug (the dev profile, `opt-level = 2` for dependencies) |

As in [m1-measurements.md](m1-measurements.md), timings move with machine load.

## What one key may cost

One typed key can run many commands through mappings, `.` and counts. Before the per-key caps
(`MAX_KEY_WORK`, `MAX_EXPANDED_KEYS`, `MAX_WRITES_PER_KEY`), crafted lawhrc mappings froze the
window. Each case below is now a regression test in `crates/lawh-core/tests/engine_cost.rs` (the
last row: `keymap.rs`) that checks where the key stopped and bounds its time: 1 s in release, 5 s
in debug (10 s for `keymap.rs`), all targets. The `engine_cost.rs` cases take about 0.1 s each in
debug now (measured; the module doc of `engine_cost.rs`). Sources: the comment above each test.

| Case | Before the caps | Kind |
|---|---|---|
| A mapping that runs `10000J10000K` 20 times, then itself, on a column of 500 rectangles | 7.6 s, release | measured |
| `$10000h` 36 times, then itself, on one line of 5,000 flag halves (regional indicators) | 227 s, release (also before `h` walked left through the line only once) | measured |
| `10000u10000<C-r>`, then itself, over 250 steps that each take a column of 500 objects off or put it back | 858 s, release (also before undo applied a step as one batch) | measured |
| `w` 255 times, then itself, over 500 texts of 80 characters | 2.2 s, release | measured |
| `p` 255 times, then itself, on a full board (every put refused for room) | 1.8 s, release; now no work at all, stopped by `MAX_EXPANDED_KEYS` | measured |
| `yG` 127 times, then itself, on 500 texts | 2.7 s, release | measured |
| 30,000 refused `b`s typed into a text at its 5,000-character cap | 3 s, debug | measured |
| `<Esc>` after `10000A<Left><Right>` on 5,000 flag halves | 187 ms, release; now the keys are typed once more, since they changed nothing | measured |
| 255 `l`s and itself, looked up against 999 mappings that start with those `l`s | 82 s, release (lookups scanned every mapping; now sorted, and capped at 30,000 keys) | measured |

Undo of a step that changes a whole board (500 objects): 1.7 ms a step in release before
`Board::apply` batched it, about 1 ms a step in debug after (measured;
`undoing_steps_of_a_whole_board_is_linear` in `engine_cost.rs`).

### The work budget (`MAX_KEY_WORK`)

`MAX_KEY_WORK` is 100,000,000 units. Each kind of work has a weight, in units, from what one item
of it took in a debug build of lawh-core, measured with `work_unit_costs` in `engine_cost.rs`
(Linux `x86_64`, 2026-10-12, `FixedMeasure`). Source: the doc comments in
`crates/lawh-core/src/engine/work.rs`.

| What | Figure | Kind |
|---|---|---|
| One unit, across the `work_unit_costs` cases | 0.1 to 1.1 ns, debug | measured |
| One unit | at most 0.1 ns, release (0.5 ns for typing, which is copying) | measured |
| A key that spends the whole budget | about 0.1 s in debug, 10 ms in release, 50 ms if it was typing | derived from the two rows above |
| `10000J` down a column of 500 objects, the costliest motion | 62.5 million units; 51 ms debug, 1.8 ms release | units derived, times measured |
| `.` of an insert of 5,000 characters | 25 million units | derived |
| `dG`, `yG` or `p` of a full board | at most 3 million units | derived |
| An undo or redo step of a whole board | 1 to 1.5 million units | derived |
| `10000u` over a history of whole-board steps | stops after about 80 steps; 60 ms debug, 7 ms release | measured |
| 30,000 keys that do no work (`l` on the grid), bounded by `MAX_EXPANDED_KEYS` instead | 36 ms debug, 3.5 ms release | measured |

The weights, each measured in debug:

| Weight | Units | Measured on |
|---|---|---|
| `MEASURE_WORK` | 20 | `w` over 500 texts of 80 characters measured 40,500 objects and characters in 0.8 ms |
| `ORDER_WORK` | 500 | `w` over 500 rectangles: 0.18 ms |
| `LOOK_WORK` | 250 | `10000J` down 500 rectangles looked at 250,000 objects in 51 ms |
| `PLACE_WORK` | 20 | `o` beside a column of 500 texts looked at 250,000 objects in about 5 ms |
| `TAKE_WORK` | 2,500 | `dG` of 500 texts took 1.1 ms more than `G` |
| `EDIT_WORK` | 1,000 | An undo step of 500 edits on a board of 500 objects: 1 ms |
| `PUT_WORK` | 7,000 | Putting 250 rectangles: 1.7 ms |
| `ID_WORK` | 600 | `:ci` after `c1` to `c499`, looking for a fresh id: 0.28 ms |
| `PARSE_WORK` | 4 | `@:` of a `:te` of 5,000 characters: 21 µs |
| `COPY_WORK` | 1 | `10000ia<Esc>` copied 12.5 million bytes and characters in 28 ms |
| `SCAN_WORK` | 60 | `$` on a line of 5,000 flag halves (20,000 bytes): 1.1 ms |
| `SPLIT_WORK` | 250 | `j` between two lines of 2,499 characters: 0.85 ms |
| `STEP_WORK` | 1,300 | `10000b` back along 5,000 characters of two-letter words: 6.4 ms |

### Writes (`MAX_WRITES_PER_KEY`)

From the doc comment of `MAX_WRITES_PER_KEY` in `crates/lawh-core/src/engine/exec.rs`:

| What | Figure | Kind |
|---|---|---|
| One write: the board as text, a new file, flushed to disk, renamed | 2.76 ms on an SSD, mostly the flush | measured (build not recorded) |
| Four writes, the cap | about 11 ms | derived |
| `nmap Q :w<CR>Q` before the cap: 1,000 writes, stopped by `MAX_MAP_DEPTH` | 2.8 s of frozen window | derived (1,000 x 2.76 ms) |
| 85 `:w<CR>` then itself, before the cap: 9,945 writes, stopped by `MAX_EXPANDED_KEYS` | 27 s | derived |
| The engine's own part of those 9,945 writes (reading their `:` lines) | 39,780 work units | derived |

That last row is why `MAX_KEY_WORK` can't bound writes: they cost the app, not the engine.

## lawhrc

From the doc comment of `MAX_MAPS` in `crates/lawh-core/src/engine/keymap.rs`: the slowest lawhrc
the caps allow, `MAX_RC_BYTES` (1 MiB) of `map ! x` and `unmap !` after 999 mappings in three
modes (`!` sorts first, so each makes or closes room at the front of all three).

| What | Figure | Kind |
|---|---|---|
| Loading it | 676 ms, release | measured (`a_lawhrc_of_mappings_made_and_dropped_at_the_front_loads_in_time`; the test's own comment says 675 ms) |
| Loading it | 1.7 s, debug | measured |
| Its bound in the test | 5 s release, 20 s debug | target |
| A real lawhrc | tens of lines | |

It stays this slow on purpose: the file is the user's own and read once at start. A map ordered
by keys (`BTreeMap`) would make mappings cheaply but slow every typed key's lookup.

## The overlay

`overlay_frame_time` in `crates/lawh-render/tests/bench.rs`, release, 2026-10-11: what drawing
the overlay adds to a frame (the median of `render_with` minus that of `render`).

| Overlay | 1920x1080 | 2880x1800, scale 2 | Kind |
|---|---|---|---|
| Everyday (the cursor on an object, a caret on every other frame, a short status line, on the area board) | 0.09 ms | 0.2 ms | measured |
| Worst (all 500 objects selected, a Visual rectangle over the view, the longest command line changing every frame) | 1.88 ms | 4.2 ms | measured |
| `OVERLAY_TARGET` | 8 ms | 8 ms | target |

The 1920x1080 figures are from the milestone's render lane run; the doc comment of
`Renderer::render_with` (`crates/lawh-render/src/renderer/ui.rs`) rounds them to 0.1 ms and
1.9 ms and gives the 2880x1800 ones.

## Fuzzing

Milestone 2's third review round typed 4,000 random key scripts into the engine (not in the
repo):

| What | Figure | Kind |
|---|---|---|
| Panics | 0 | measured |
| Slowest single key | 35 ms, release | measured |

## The acceptance test

| What | Figure | Kind |
|---|---|---|
| Keys to type `examples/area.lawh` from an empty board and write it | 62 | target, asserted by `the_area_example_in_62_keys` in `crates/lawh-core/tests/acceptance.rs` |
