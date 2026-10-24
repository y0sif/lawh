# lawh

Native Rust whiteboard + text editor driven by a Vim grammar. Linux first, released for Linux,
macOS and Windows (unsigned). Repo is private until the app has been tested.

Milestone 1 ("see a board") is done: `lawh file.lawh` draws a board, with mouse pan and zoom.
Milestone 2 (the Vim engine) is done and merged: keys, editing, selection, saving, key maps and
`lawhrc`. Milestone 3 is done and merged: text and math sizes (`<C-a>`/`<C-x>`, `:si`, `size=` in
files), jump labels on `s`, `:sym` symbol search (`<C-k>` in Insert mode) and `W B E` in blocks.
Milestone 4 (export to SVG, PNG and PDF) is next. Keys are listed in `docs/keys.md`.

## Workspace

| Crate | Role today (planned) | Heavy deps allowed |
|---|---|---|
| `lawh-core` | Board model and limits (text and math sizes), `.lawh` reader (own parser) and writer, Vim key notation, the Vim engine (modes, board grammar, sizes, jump labels, text in blocks, `:` commands, `:sym` matching with nucleo-matcher, undo, `.`, key maps, `:set`, lawhrc parsing). | No. Keep it fast to build and test. |
| `lawh-math` | Typst math → vector paths in a sandboxed worker process, caches, bundled fonts, the `:sym` symbol list (`symbols()`: codex's names, each flagged drawable or not from the bundled fonts). | Yes (typst) |
| `lawh-render` | Board → pixels (vello_cpu, parley, Libertinus Math as the fallback for characters Inter lacks), text and math sizes (`MATH_SCALE`, baseline alignment), frame budget, PNG of a frame, the editing overlay (cursor, selection, caret, jump labels, `:sym` rows, status and command line), hit tests and text measuring for the engine, with what laying text out cost. (SVG/PDF export.) | Yes |
| `lawh` | Binary: window (winit, softbuffer), keys to the engine, the view for jump labels (`Engine::set_view`), the drawable symbols into the engine at start, atomic `:w` and the same-file check after it, lawhrc from the config dir (etcetera), mouse pan, zoom and click to select, hidden `--math-worker` mode. (App shortcuts, live reload.) | Yes |

`lawh-core` must never depend on rendering, windowing or Typst (nor codex: the app builds the
symbol list in lawh-math and passes it in with `Engine::set_symbols`).

## Build and check

```bash
nice -n 19 cargo build
nice -n 19 cargo run -p lawh -- examples/area.lawh
nice -n 19 cargo test -p lawh-core      # fast loop: no Typst, no renderer
cargo fmt --all --check
nice -n 19 cargo clippy --all-targets -- -D warnings
```

- The laptop has 14 GB RAM: `~/.cargo/config.toml` caps `jobs = 4`. Keep it, and build under `nice`.
- Never put `CARGO_TARGET_DIR` under `/tmp` (small tmpfs).
- Dev profile (root `Cargo.toml`): `opt-level = 2` for every dependency and for the `lawh` crate,
  so panning is smooth in debug. Measured: a debug frame went from 116 ms to 1.2 ms, and a cold
  build from 151 s to 479 s. Incremental builds of our crates are unchanged; lawh-render stays at
  0 so it is easy to step through.
- Timings and calibrations are `#[ignore]` tests: run them in release with `--test-threads=1` on a
  quiet machine (see `docs/testing.md`).

## CI and releases

- `ci.yml`: fmt, clippy `-D warnings` and tests on Linux for pushes to `main` and for pull
  requests, `cargo deny`, plus a check of all targets on Rust 1.95. macOS and Windows only run
  when triggered by hand (`gh workflow run ci.yml -f all_platforms=true`) or from a release,
  because a private repo has 2,000 Actions minutes a month. Usage is blocked, not billed, once they
  run out. **When the repo goes public, make all three platforms the default and set
  `publish = true`.**
- `bench.yml`: calibrations and benches in release, the key-cost measurements (`engine_cost` in
  release, `work_unit_costs` in debug, `measure_cost`) and the symbol list's build time, by hand
  only (`gh workflow run bench.yml`).
- `release.yml`: push a `v*` tag to run CI on all three platforms and build unsigned archives for
  Linux x86_64/arm64, macOS arm64/x86_64 and Windows x86_64, with `SHA256SUMS`. Each archive holds
  the binary, `README.md`, `LICENSE` and the two font licences (OFL-1.1 requires them, since the
  binary embeds the fonts). A tag with a hyphen becomes a pre-release.
- Lint workflows locally with `uvx --from actionlint-py actionlint`.
- CI needs no apt step: winit loads the X11 and Wayland libraries at run time.
- Before the first public release: third-party licence notices for the Rust crates (see
  `docs/tech-stack.md`).

## Code quality

- Lints live in the root `Cargo.toml`: clippy `all` + `pedantic` (a few allowed), plus `unwrap_used`,
  `expect_used`, `panic`, `todo`, `dbg_macro`, `print_stdout`, `print_stderr`. `unsafe_code` is
  denied. `clippy.toml` exempts tests. CI runs with `-D warnings`, so a warning fails the build.
- Silence a lint with `#[expect(lint, reason = "...")]`, never a bare `#[allow]`.
- Errors: each library crate has typed `thiserror` enums; the app uses `anyhow` with context.
  Library code never panics, unwraps or prints.
- Logging: `tracing` macros in lawh-math, lawh-render and the app (lawh-core doesn't log); only the
  binary installs the subscriber. `RUST_LOG` sets the level (default `warn`), output goes to
  stderr. The math worker's stdout carries only its protocol: nothing else may print there.
- Dependencies: `deny.toml` allows permissive licences, MPL-2.0 and OFL-1.1 only. A GPL-family
  licence needs a decision, not a config edit. Every new crate goes in `docs/tech-stack.md`.
- Testing: see `docs/testing.md`. Engine tests type Vim key scripts (`tests/common/mod.rs`) and
  snapshot the board as `.lawh` text.

## Limits

Every cap on what a board, a formula, a key or a frame may cost is a named constant, documented
where it is defined (most with their reason, and measurements where a cost was measured). Change
the constant, not a copy:

- File and parser: `crates/lawh-core/src/format/mod.rs` (`MAX_FILE_BYTES`, `MAX_ENTRIES`, ...).
- Board: `crates/lawh-core/src/board.rs` (`MAX_OBJECTS`, `MAX_TEXT_CHARS`, ...); text and math
  sizes `MIN_TEXT_SIZE` = 0.25, `MAX_TEXT_SIZE` = 16, `DEFAULT_TEXT_SIZE` = 1, same file.
- Math: `crates/lawh-math/src/lib.rs` (layout caps), `engine.rs` (timeout, caches, recompile
  limits), `worker.rs` (memory cap).
- Worker protocol: `crates/lawh-math/src/protocol.rs` (`MAX_FRAME_BYTES`).
- Worker starts: `crates/lawh-math/src/process.rs` (`HANDSHAKE_TIMEOUT`, `MAX_SPAWN_FAILURES`).
- Frame budget: `crates/lawh-render/src/budget.rs`; text cache: `text.rs`. Laying out text to
  measure it for a key: `KEY_WORK_PER_LAYOUT_NS` = 1 (work units per estimated ns), `budget.rs`.
- Huge glyphs and strokes: `crates/lawh-render/src/renderer.rs` (`MAX_GLYPH_EM_PX`,
  `MAX_STROKE_PX`).
- Drawable views: `crates/lawh-render/src/viewport.rs` (`MAX_PX_PER_UNIT`, `MAX_SCALE_FACTOR`,
  `FIT_MAX_ZOOM`).
- Wheel zoom: `crates/lawh/src/view.rs` (`MIN_ZOOM`, `MAX_ZOOM`); following the cursor:
  `FOLLOW_MARGIN_PX`, same file.
- Counts: `crates/lawh-core/src/engine/mod.rs` (`MAX_COUNT`).
- Undo: `crates/lawh-core/src/engine/undo.rs` (`MAX_UNDO_STEPS`, `MAX_UNDO_BYTES`).
- `.` repeat: `crates/lawh-core/src/engine/repeat.rs` (`MAX_REPEAT_KEYS`).
- Command line: `crates/lawh-core/src/engine/cmdline.rs` (`MAX_COMMAND_LINE_CHARS`).
- Key maps: `crates/lawh-core/src/engine/keymap.rs` (`MAX_MAPS`, `MAX_MAP_KEYS`, `MAX_MAP_DEPTH`,
  `MAX_EXPANDED_KEYS`, `MAX_PENDING_KEYS`); `settings.rs` (`MAX_TIMEOUTLEN`).
- lawhrc: `crates/lawh-core/src/engine/rc.rs` (`MAX_RC_BYTES`, `MAX_RC_WARNINGS`).
- Work per key: `crates/lawh-core/src/engine/work.rs` (`MAX_KEY_WORK` and its `*_WORK` weights,
  `SIZE_WORK`, `LABEL_WORK`, `PICK_WORK`, `SYM_WORK`, `SYM_CHAR_WORK` and `SYM_CELL_WORK` among
  them). The measure adds text layout through `Measure::take_work` (`engine/measure.rs`).
- Jump labels: `crates/lawh-core/src/engine/jump.rs` (`MAX_LABEL_KEYS` = 2, `MIN_LABEL_CHARS` = 2,
  `MAX_LABEL_CHARS` = 52).
- `:sym`: `crates/lawh-core/src/engine/sym.rs` (`MAX_SYMBOLS` = 4096, `MAX_SYM_QUERY_CHARS` = 64,
  `MAX_SYM_QUERY_BYTES` = 4 x 64, `SYM_ROWS` = 8).
- Writes per key: `crates/lawh-core/src/engine/exec.rs` (`MAX_WRITES_PER_KEY` = 4,
  `MAX_PENDING_WRITES` = `MAX_WRITES_PER_KEY`).
- Overlay: `crates/lawh-render/src/budget.rs` (`HIGHLIGHT_BUDGET_NS`, which jump labels' cost comes
  out of first), `status.rs` (`STATUS_WINDOW_CHARS`, `STATUS_CACHE_ENTRIES` = 40), `overlay.rs`
  (`STATUS_BAR_PX`); bench target `OVERLAY_TARGET` in `tests/bench.rs`.
- Mouse: `crates/lawh/src/app.rs` (`CLICK_SLOP_PX`, `HIT_TOLERANCE_PX`).
- Atomic writes: `crates/lawh/src/files.rs` (`TEMP_NAME_TRIES`, `MAX_LINK_HOPS`).

Design constants, not caps: `ROW_TOLERANCE` (`engine/motion.rs`), `PLACE_GAP`
(`engine/place.rs`), `SIZE_STEPS` (`engine/size.rs`), `DEFAULT_JUMP_LABELS` (`engine/jump.rs`),
`MATH_SCALE` = 1.2725 (`crates/lawh-render/src/renderer.rs`), the jump labels' `LABEL_BOX_PX`,
`LABEL_FONT_PX`, `LABEL_PAD_PX` and `LABEL_FONTS` (`renderer/labels.rs`), and the `:sym` rows'
`ROW_PX` and `VALUE_GAP_PX` (`renderer/completions.rs`).

Overview table: `docs/architecture.md#limits`. Measured numbers: `docs/research/m1-measurements.md`,
`docs/research/m2-measurements.md` and `docs/research/m3-measurements.md`.

## Conventions

- Portable from day one: no hard-coded `/` in paths, platform dirs via `etcetera`, bundled fonts
  only, handle the window scale factor, accept CRLF in files. Vim keys match the typed character
  (`crates/lawh/src/keys.rs`); app shortcuts, when they come, go through one modifier setting
  (Ctrl, or Cmd on macOS). The only one today is Cmd+Q on macOS, taken as a close request.
- Config: `lawhrc` in etcetera's app config dir (`choose_app_strategy`, `crates/lawh/src/rc.rs`):
  `~/.config/lawh/lawhrc` on Linux and macOS (XDG, `XDG_CONFIG_HOME` honoured),
  `%APPDATA%\y0sif\lawh\config\lawhrc` on Windows. One `:` command a line, only `:set` and
  mapping commands; `"` starts a comment line. Sample: `examples/lawhrc`.
- `.lawh` files are flat KDL v2 with a `lawh version=1` header. lawh reads them with its own parser
  (`format/parse.rs`, one pass, no recursion, caps checked while reading) and writes them with its
  own writer (`format/write.rs`, one fixed shape per object, comments not kept). The `kdl` crate is
  a dev-dependency only: the oracle for the differential tests (`LAWH_PROPTEST_CASES` for long
  runs, `tests/format_proptest.rs` included). Text and math take an optional `size=`, written
  only when not 1, so the header stays `version=1`. The `:` command line has its own parser
  (`engine/command.rs`): KDL can't parse paths and mangles `math A = pi r^2` (see
  `docs/research/kdl-spike.md`).
- Math from a board always compiles in the worker process (`MathEngine::with_worker`). The
  in-process engine and `Renderer::new_trusted` are for tests and goldens only.
- Commit identity: y0sif <yosifsoli981@gmail.com>.

## Gotchas

- **Link error:** if linking `lawh` fails with
  `rust-lld: error: undefined hidden symbol ... .llvm.<number>`, run `cargo clean -p lawh`. It is
  a stale incremental cache with the crate's `opt-level = 2`. Seen on 2026-10-11; the rebuild took
  10 s.

## Docs

- `docs/architecture.md`: crates, file-to-pixels and key-press data flow, the engine (modes,
  key maps, undo, sizes, jump labels, `:sym`, work budget), the canvas cursor and place rule,
  `.lawh` reader and writer, limits, math worker and sandbox, frame budget, text shaping and
  fallback, text and math sizes, the overlay.
- `docs/keys.md`: user-facing key reference: modes, keys, `:` commands, `:set`, mappings, lawhrc.
- `docs/tech-stack.md`: every dependency in use or planned, with version, licence and reason; fonts;
  advisory exceptions; rejected options. Update it when adding a dependency.
- `docs/testing.md`: test kinds, differential tests, goldens, calibrations and benches.
- `docs/landscape.md`: competitors and differentiation (feeds README, comparison, FAQ).
- `docs/comparison.md`, `docs/faq.md`: public-facing; keep claims about other tools sourced.
- `SECURITY.md`: threat model for untrusted boards and formulas. The Typst sandbox (no files, no
  network, no clock, bundled fonts) is a hard rule. The worker's memory cap is Linux only so far.
- `docs/research/`: layer-vs-own audit, hands-on trial kit, KDL spike, M1, M2 and M3
  measurements.
- `docs/BRAND.md`, `docs/DESIGN.md`: git-ignored maintainer notes. Read them when present.
