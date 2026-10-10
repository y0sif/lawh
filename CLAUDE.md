# lawh

Native Rust whiteboard + text editor driven by a Vim grammar. Linux first, released for Linux,
macOS and Windows (unsigned). Repo is private until the app has been tested.

## Workspace

| Crate | Role | Heavy deps allowed |
|---|---|---|
| `lawh-core` | Board model, Vim engine (modes, counts, operators, motions), `:` commands, undo, `.lawh` format (KDL) | No. Keep it fast to build and test. |
| `lawh-math` | Typst math → vector paths, symbol name search | Yes (typst) |
| `lawh-render` | Board → pixels (vello_cpu), export SVG/PNG/PDF | Yes |
| `lawh` | Binary: window (winit), input, status/command line, config | Yes |

`lawh-core` must never depend on rendering, windowing or Typst.

## Build and check

```bash
nice -n 19 cargo build
nice -n 19 cargo test -p lawh-core      # fast loop for the engine
cargo fmt --all --check
nice -n 19 cargo clippy --all-targets -- -D warnings
```

- The laptop has 14 GB RAM: `~/.cargo/config.toml` caps `jobs = 4`. Keep it, and build under `nice`.
- Never put `CARGO_TARGET_DIR` under `/tmp` (small tmpfs).

## CI and releases

- `ci.yml`: fmt, clippy `-D warnings` and tests on Linux for every push and PR, plus a check on
  Rust 1.95. macOS and Windows only run when triggered by hand
  (`gh workflow run ci.yml -f all_platforms=true`) or from a release, because a private repo has
  2,000 Actions minutes a month. Usage is blocked, not billed, once they run out. **When the repo goes
  public, make all three platforms the default and set `publish = true`.**
- `release.yml`: push a `v*` tag to run CI on all three platforms and build unsigned archives for
  Linux x86_64/arm64, macOS arm64/x86_64 and Windows x86_64, with `SHA256SUMS`. A tag with a
  hyphen becomes a pre-release.
- Lint workflows locally with `uvx --from actionlint-py actionlint`.
- Linux system libraries for winit/softbuffer aren't installed in CI yet. Add an apt step when the
  app gains those dependencies.

## Code quality

- Lints live in the root `Cargo.toml`: clippy `all` + `pedantic` (a few allowed), plus `unwrap_used`,
  `expect_used`, `panic`, `todo`, `dbg_macro`, `print_stdout`, `print_stderr`. `clippy.toml` exempts
  tests. CI runs with `-D warnings`, so a warning fails the build.
- Silence a lint with `#[expect(lint, reason = "...")]`, never a bare `#[allow]`.
- Errors: each library crate has typed `thiserror` enums; the app uses `anyhow` with context.
  Library code never panics, unwraps or prints.
- Logging: `tracing` macros in every crate; only the binary installs the subscriber. `RUST_LOG`
  sets the level (default `warn`), output goes to stderr.
- Dependencies: `deny.toml` allows permissive licences, MPL-2.0 and OFL-1.1 only. A GPL-family
  licence needs a decision, not a config edit. Every new crate goes in `docs/tech-stack.md`.
- Testing: see `docs/testing.md`. Engine tests type Vim key scripts and snapshot the board as `.lawh` text.

## Conventions

- Portable from day one: no hard-coded `/` in paths, platform dirs via `etcetera`, bundled fonts,
  handle the window scale factor, accept CRLF in files. Vim keys match the typed character;
  app shortcuts go through one modifier setting (Ctrl, or Cmd on macOS).
- `.lawh` files are KDL with a `lawh version=1` header. When editing a parsed entry, call
  `KdlEntry::clear_format()` after `set_value`, or the old value is written back.
  The `:` command line has its own parser (KDL can't parse `rect 4x2` or paths, and it mangles
  `math A = pi r^2`). See `docs/research/kdl-spike.md`.
- Commit identity: y0sif <yosifsoli981@gmail.com>.

## Docs

- `docs/architecture.md`: crates, key-press data flow, board model, open design questions (2D cursor, Typst time budget).
- `docs/tech-stack.md`: every planned dependency with version, licence and reason, plus rejected options. Update it when adding a dependency.
- `docs/landscape.md`: competitors and differentiation (feeds README, comparison, FAQ).
- `docs/comparison.md`, `docs/faq.md`: public-facing; keep claims about other tools sourced.
- `SECURITY.md`: the Typst sandbox (no files, no network, bundled fonts) is a hard rule.
- `docs/research/`: layer-vs-own audit, hands-on trial kit, KDL spike.
- `docs/BRAND.md`, `docs/DESIGN.md`: git-ignored maintainer notes. Read them when present.
