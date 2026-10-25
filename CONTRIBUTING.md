# Contributing to lawh

Contributions are welcome: bug reports, fixes, docs and features. For anything larger than a
small fix, open an issue first so we can agree on how it should work before you write it.

## Setup

```bash
git clone https://github.com/y0sif/lawh
cd lawh
cargo build
```

Needs Rust 1.95 or newer and a C compiler (for `psm`, through Typst). Nothing else to install on
Linux: the X11 and Wayland libraries are loaded when lawh starts. `rust-toolchain.toml` pins the
stable channel with rustfmt and clippy.

## Before you open a pull request

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

All three must pass. For every pull request CI runs clippy and the tests on Linux (x86_64 and
arm64), macOS and Windows, and the format check on Linux, together with `cargo deny` (licences
and advisories), a check on Rust 1.95, and a job that generates the third-party licence file the
release archives carry (see [Licence notices](#licence-notices)).

## Where code goes

- `lawh-core`: the board model, the `.lawh` reader and writer, and the Vim engine (`:` commands,
  undo, key maps, lawhrc parsing). It must not depend on rendering, windowing or Typst. Engine
  changes should come with a test that feeds keys and checks the resulting board
  (`crates/lawh-core/tests/common/mod.rs`).
- `lawh-math`: Typst math, compiled in a worker process. Keep the Typst world sandboxed: no file
  system, no network, no clock, bundled fonts only (see
  [the sandbox](docs/architecture.md#the-sandbox)).
- `lawh-render`: drawing, the frame budget, the editing overlay and export.
- `lawh`: the app shell: window, keys, writing files, reading the lawhrc.

Limits on what a board, a formula or a frame may cost are named constants, documented where they
are defined (most with their reason, and measurements where a cost was measured). If you change
one, a cost in the frame budget or a work weight in `crates/lawh-core/src/engine/work.rs`, rerun
the calibrations and benches in [docs/testing.md](docs/testing.md#calibrations-and-benches).

See [docs/architecture.md](docs/architecture.md) for the design and
[docs/tech-stack.md](docs/tech-stack.md) before adding a dependency.

## Code conventions

- Library crates return typed errors (`thiserror`) and never panic, unwrap or print. The app uses `anyhow`.
- Log with `tracing`. Run with `RUST_LOG=debug` to see it.
- To silence a lint, use `#[expect(lint, reason = "...")]`, not a bare `#[allow]`.
- Tests: see [docs/testing.md](docs/testing.md). Engine changes come with a key-script snapshot test.
- New dependencies must pass `cargo deny check` (licences and advisories) and be listed in
  [docs/tech-stack.md](docs/tech-stack.md).

## Licence notices

Each release archive carries `THIRD-PARTY-LICENSES.txt`. The release workflow generates it with
[cargo-about](https://github.com/EmbarkStudios/cargo-about) for each target
(`cargo about generate --locked --fail --target <target> about.hbs`, settings in `about.toml`)
and appends `assets/licenses/EMBEDDED-NOTICES.txt`: notices for data compiled into the binary
that no Rust licence tool reports.

- When Typst, krilla, hayro or subsetter change version, refresh `EMBEDDED-NOTICES.txt` from
  their new releases (its header says how).
- If cargo-about prints "failed to validate" for a crate, a clarification in `about.toml` no
  longer matches that crate's licence file: update its checksum. CI fails on it.

To try it locally: `cargo about generate --locked --fail about.hbs -o target/THIRD-PARTY.txt`
(cargo-about 0.9.2, as CI uses).

## Commits

Imperative mood with a prefix: `feat`, `fix`, `docs`, `refactor`, `test` or `ci`, with an optional
scope naming the crate: `core`, `math`, `render` or `app` (`feat(core):`, `fix(app,render):`).

```text
feat(core): add :circle command

Creates an ellipse with equal radii at the grid cursor.
```

## Pull requests

Describe what changed and how you tested it. Keep each pull request to one change.
