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
