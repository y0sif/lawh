# Testing

How lawh is tested, layer by layer. Test dependencies (`insta`, `proptest`) are added with the
first tests that use them.

## The idea: keys in, board out

The Vim engine in `lawh-core` is a pure state machine, so most of lawh is tested without a window.
A test types a key script in Vim notation and snapshots the resulting board **as a `.lawh` file**:

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

Reviewing a snapshot means reading a board file, the same format users see. `run` accepts Vim key
notation (`<CR>`, `<Esc>`, `<C-r>`, `<Space>`), and the same parser will later read key mappings
from config.

## Layers

| Layer | What | How |
|---|---|---|
| Engine (`lawh-core`) | Modes, counts, operators, motions, undo, `.` repeat | Key script → `insta` snapshot of the board, mode and cursor |
| `:` commands (`lawh-core`) | Parsing and errors | Table tests: input → command or error message |
| File format (`lawh-core`) | Read, write, comments kept, errors | Round-trip tests, `proptest` random boards, hand-written golden files, snapshots of error output |
| Acceptance (`lawh-core/tests/`) | The "area of a circle" benchmark built from keys only | One key script must produce `examples/area.lawh`. Its key count is recorded, since "fewer actions than a mouse" is the product's promise. |
| Math (`lawh-math`) | Typst snippets compile, sandbox holds | Size and path-count snapshots. `#read(...)`, `#import "@preview/..."` and `#image(...)` must fail. |
| Rendering (`lawh-render`) | Pixels and exports | Golden PNGs from `vello_cpu` with bundled fonts, compared with a small per-pixel tolerance (CPU float maths can differ between x86 and ARM). SVG export as text snapshots. |
| App (`lawh`) | Window, input, presenting frames | No automated GUI tests in v1. A manual checklist on Linux (Wayland and X11) before any release. |

## Later

- **Fuzzing:** `cargo-fuzz` targets for the `.lawh` loader and the `:` command parser, since boards
  come from other people (see `SECURITY.md`).
- **Performance:** timed benchmarks for loading a file, handling a key and rendering a frame.
  Every recorded number states the machine, the date and whether it was measured or estimated.

## What not to test

- winit, Vello, parley or Typst internals.
- Pixel-identical text across operating systems beyond the tolerance above.

## Commands

```bash
cargo test -p lawh-core        # the fast loop: no Typst, no renderer
cargo test --workspace         # everything, as CI runs it
cargo insta review             # accept or reject changed snapshots (cargo install --locked cargo-insta)
```

In CI, `insta` fails on any new or changed snapshot instead of writing it, so snapshots are
always reviewed locally before they're committed.
