# Contributing to lawh

lawh is pre-alpha and the repository is private for now, so outside contributions aren't open yet.
This guide is here for when they are.

## Setup

```bash
git clone https://github.com/y0sif/lawh
cd lawh
cargo build
```

Needs Rust 1.95 or newer. `rust-toolchain.toml` pins the stable channel with rustfmt and clippy.

## Before you open a pull request

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

All three must pass. CI runs the same commands.

## Where code goes

- `lawh-core`: the board model and the `.lawh` reader and writer, then the Vim engine and `:`
  commands. It must not depend on rendering, windowing or Typst. Engine changes should come with a
  test that feeds keys and checks the resulting board.
- `lawh-math`: Typst math, compiled in a worker process. Keep the Typst world sandboxed: no file
  system, no network, no clock, bundled fonts only (see [SECURITY.md](SECURITY.md)).
- `lawh-render`: drawing, the frame budget and export.
- `lawh`: the app shell.

Limits on what a board, a formula or a frame may cost are named constants, documented where they
are defined (most with their reason, and measurements where a cost was measured). If you change one, or a cost in the frame
budget, rerun the calibrations and benches in
[docs/testing.md](docs/testing.md#calibrations-and-benches).

See [docs/architecture.md](docs/architecture.md) for the design and
[docs/tech-stack.md](docs/tech-stack.md) before adding a dependency.

## Code conventions

- Library crates return typed errors (`thiserror`) and never panic, unwrap or print. The app uses `anyhow`.
- Log with `tracing`. Run with `RUST_LOG=debug` to see it.
- To silence a lint, use `#[expect(lint, reason = "...")]`, not a bare `#[allow]`.
- Tests: see [docs/testing.md](docs/testing.md). Engine changes come with a key-script snapshot test.
- New dependencies must pass `cargo deny check` (licences and advisories) and be listed in
  [docs/tech-stack.md](docs/tech-stack.md). Third-party licence notices for the release archives
  are still a to-do before the first public release (see the same page).

## Commits

Imperative mood with a prefix: `feat:`, `fix:`, `refactor:`, `docs:`, `chore:`, `test:`.

```text
feat: add :circle command

Creates an ellipse with equal radii at the grid cursor.
```

## Pull requests

Describe what changed and how you tested it. Keep each pull request to one change.
