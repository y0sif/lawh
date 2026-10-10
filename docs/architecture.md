# Architecture

Status: design. Nothing below is implemented yet beyond the empty crates. Sections marked
**open** are decisions still to make.

## Crates and dependency rules

```text
lawh (bin) ──► lawh-render ──► lawh-math
   │               │
   └──────────────►└──► lawh-core ◄── (no rendering, windowing or Typst)
```

| Crate | Owns | May depend on |
|---|---|---|
| `lawh-core` | Board model, object ids, geometry (kurbo), spatial index (rstar), Vim engine, `:` command parser, undo history, `.lawh` read/write (kdl) | Small, pure crates only |
| `lawh-math` | Typst world, math compile and cache, glyph outlines → paths, symbol name table and fuzzy search | typst, nucleo |
| `lawh-render` | Board → display list → pixels (vello_cpu); SVG, PNG and PDF export | lawh-core, lawh-math, vello_cpu, parley, krilla |
| `lawh` | Window and event loop (winit), presenting frames (softbuffer), key mapping, status line, command line, config, file watching | everything above |

`lawh-core` stays light so `cargo test -p lawh-core` never compiles Typst or the renderer.

## Data flow for one key press

```text
winit KeyEvent
  └─► lawh: map to a logical key (typed character + modifiers)
        └─► lawh-core Engine::feed(key)
              ├─ pending: wait for more keys (count, operator, motion)
              └─ complete: apply Ops to the Board, push one undo step
                    └─► lawh-render: redraw dirty region
                          └─► lawh-math: compile changed math blocks (cached by source)
```

The engine is a pure state machine: `(state, board, key) → (state, board changes, effects)`.
Effects are things only the app can do (write a file, quit, export, open the system clipboard).
That keeps the engine testable without a window.

## Board model

- Objects: `Text`, `Math`, `Ellipse` (a circle is an ellipse with equal radii), `Rect`, `Line`, `Arrow`.
- Every object has a stable id (`c1`, `t2`, ...) that also appears in the file.
- Coordinates are in **grid units** (floats). One `l` moves the cursor one grid cell. Zoom
  changes pixels per unit, never the stored numbers.
- Z-order is file order.

## The canvas cursor (open)

Vim motions are defined on a stream of text. A canvas is 2D, so the core design question is what
the cursor is. The current plan has two layers:

1. **Grid cursor**: a point on the grid, moved with `hjkl` and counts. New objects appear here.
2. **Object motions**: jump to the nearest object in a direction (like tldraw's
   `getNearestAdjacentShape`), or to any object by a short jump label (like flash.nvim).

Still open: what `w`, `b`, `e`, text objects (`ic` for "inside circle"?) and operators like
`d{motion}` mean on objects, and how "place below" picks a gap.

## Text editing inside blocks

Insert mode inside a text or math block reuses Vim text editing on that block's string.
parley's `PlainEditor` handles layout, cursor and selection geometry, and the engine drives it.
Right-to-left text is out of scope for v1.

## Math

- A math block holds Typst math source, for example `A = pi r^2`.
- `lawh-math` compiles it with an embedded Typst world, lays it out, and converts glyph outlines
  into vector paths that the renderer draws at any zoom. Results are cached by source text.
- **Sandbox (decided):** the Typst world has no file system access, no package downloads and only
  bundled fonts. A `.lawh` file from someone else can contain any Typst code (math mode allows `#`
  expressions), so opening a file must never read your disk or reach the network.
- **Time budget (open):** Typst code can loop. Compile on a worker thread and show an error badge
  if a block takes too long. Typst has no cancellation, so a runaway compile keeps its thread
  until it finishes. Needs a measured answer.

## File format

KDL with a `lawh version=1` header, one object per line, fixed property order. Comments in a
hand-edited file survive a save. Measured trade-offs and the gotcha with editing parsed entries are
in [research/kdl-spike.md](research/kdl-spike.md). The `:` command line has its own parser.

## Rendering

`vello_cpu` first: its README calls it the most mature Vello renderer. Frames go to the window
through `softbuffer`. Move to `vello_gpu` if profiling shows the CPU path is too slow. Fonts are
bundled so text looks the same on every platform and snapshot tests match everywhere.

## Portability rules

Linux first, with releases for macOS and Windows from the same code:

- Vim keys match the typed character. App shortcuts go through one modifier setting (Ctrl, or Cmd on macOS).
- Platform config and cache directories via `etcetera`. No hard-coded `/` in paths.
- Handle the window scale factor from the start (Wayland fractional scaling needs it on Linux).
- Accept CRLF line endings in `.lawh` files.
