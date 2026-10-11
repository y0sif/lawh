<div align="center">

# lawh

**A keyboard-driven whiteboard for people who think in Vim.**

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 1.95+](https://img.shields.io/badge/rust-1.95%2B-orange.svg)](https://www.rust-lang.org)
[![Status: pre-alpha](https://img.shields.io/badge/status-pre--alpha-lightgrey.svg)](#status)

</div>

lawh (لوح, "board" or "slate") is a native, open-source whiteboard and text editor where you draw
shapes, write math and arrange text with Vim motions and commands instead of a mouse or a pen.
Type `:circle r=3` and the circle appears at your cursor. Write `A = pi r^2` and it renders as
math. Move the label under the circle with a motion. No drawing tablet, no hunting for toolbar
buttons, no dragging to get the size right.

Whiteboards like Excalidraw and tldraw are built for the mouse: keyboard shortcuts pick a tool, then
you drag. lawh drives the whole canvas with a Vim grammar (modes, counts, operators, motions and
`:` commands), and it works on plain-text files you keep in git.

## Status

**Pre-alpha.** Milestone 1, "see a board", is done: `lawh file.lawh` opens a window and draws the
board's text, math (rendered with Typst), circles, ellipses, rectangles, lines and arrows. You can
pan and zoom with the mouse.

Not there yet: the keyboard, editing, selection and saving. They start with milestone 2, the Vim
engine. The feature list below is the plan for v1.

## Planned for v1

- [ ] **Modes:** Normal, Insert (inside a text or math block), Visual (select objects) and Command (`:`)
- [ ] **Canvas cursor** moved with `hjkl` and counts (`5l`). New objects appear at the cursor.
- [ ] **Objects:** text block, math block, circle or ellipse, rectangle, line, arrow
- [ ] **Commands:** `:circle r=3`, `:rect 4x2`, `:arrow`, `:math`, `:sym integral` (find a symbol by name)
- [ ] **Math** in [Typst](https://typst.app) syntax: `A = pi r^2`, `sum_(i=1)^n i`
- [ ] **Moving things:** move a selection with `hjkl`, jump to any object by a short label, place one object above, below or beside another
- [ ] **Editing:** delete, yank and put, undo and redo, `.` to repeat, counts everywhere
- [ ] **Files:** plain-text `.lawh` boards that diff cleanly in git
- [ ] **Export:** SVG, PNG and PDF
- [ ] **Mouse:** pan, zoom and click to select. Everything else from the keyboard.

Not in v1: freehand drawing, collaboration, a document view, right-to-left text, images, plugins.

## The file format

A board is a plain-text [KDL](https://kdl.dev) file. This is `examples/area.lawh`:

```kdl
// The "area of a circle" benchmark board, in grid units.
lawh version=1

text   id=t1 x=0 y=-5 "Area of a circle"
circle id=c1 x=0 y=0 r=3
line   id=l1 x1=0 y1=0 x2=3 y2=0
text   id=t2 x=1.5 y=-0.6 "r"
math   id=m1 x=5 y=0 "A = pi r^2"
```

Each kind of object, one per line, as lawh writes it (coordinates in grid units, y down):

```kdl
lawh version=1
text id=t1 x=0 y=-5 "Area of a circle"
math id=m1 x=5 y=0 "A = pi r^2"
circle id=c1 x=0 y=0 r=3
ellipse id=e1 x=0 y=0 rx=3 ry=2
rect id=r1 x=0 y=0 w=4 h=2
line id=l1 x1=0 y1=0 x2=3 y2=0
arrow id=a1 x1=0 y1=0 x2=3 y2=0
```

Text and math sit at the top-left of their box, circles and ellipses at their centre, rectangles
at their top-left corner, and arrows point from `x1,y1` to `x2,y2`. Hand-written files may use
comments, any property order and other KDL string forms; lawh writes this fixed shape. A board
holds at most 500 objects and the file at most 1 MiB.

The `:` commands will use the same names, so `:circle r=3` reads like the line it creates.

## How it works

```text
 file, mouse ──► lawh (window, input)
                   │
                   ▼
              lawh-core ── board model, .lawh reader and writer
                   │       (next: Vim engine, : commands, undo)
                   ▼
              lawh-render ── draws the board (vello_cpu) with a per-frame work budget
                   │         (later: export SVG / PNG / PDF)
                   │
                   └──► lawh-math ── Typst math → vector paths, compiled in a
                                     sandboxed worker process
```

`lawh-core` has no rendering or windowing code, so the Vim engine will be tested as plain
functions: keys in, board changes out. See [docs/architecture.md](docs/architecture.md).

## Platforms

Linux first. Builds for macOS and Windows ship from the same code. They aren't code-signed, so
macOS Gatekeeper and Windows SmartScreen will warn the first time you open lawh.

## Build from source

```bash
git clone https://github.com/y0sif/lawh
cd lawh
cargo run -p lawh -- examples/area.lawh
```

Needs Rust 1.95 or newer. No system packages are needed to build on Linux: the X11 and Wayland
libraries are loaded when lawh starts.

## Controls

| Action | How |
|---|---|
| Pan | Drag with the left or middle mouse button |
| Zoom | Mouse wheel or touchpad scroll, at the cursor |
| Quit | Close the window |

There are no keyboard controls yet: they come with the Vim engine. `RUST_LOG=info` prints how long
the first frame took.

## Project structure

```text
lawh/
├── crates/
│   ├── lawh-core/     # board model and file format (Vim engine next)
│   ├── lawh-math/     # Typst math in a sandboxed worker process
│   ├── lawh-render/   # drawing (export later)
│   └── lawh/          # the app: window and input
├── examples/          # sample boards
├── assets/fonts/      # bundled fonts
├── contrib/           # desktop entry, packaging
└── docs/              # architecture, tech stack, comparison, FAQ
```

## How it compares

| | lawh | Excalidraw | tldraw | AFFiNE | qboard |
|---|---|---|---|---|---|
| Vim grammar on the canvas | Yes (planned) | No | No | No | Partial: keys for tools |
| Make and size shapes without a drag | Yes (planned) | No | No | No | No |
| Math | Typst | Add-on only | SDK example only | Yes | LaTeX |
| Plain-text files | Yes | JSON | JSON | No | JSON |
| Runs as | Native app | Web | Web / SDK | Web / desktop | Web |
| License | MIT | MIT | Source-available | Open source | MIT |

More in [docs/comparison.md](docs/comparison.md) and [docs/faq.md](docs/faq.md).

## Development

```bash
cargo test -p lawh-core                        # fast loop for the engine
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
```

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
