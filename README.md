<div align="center">

# lawh

**A keyboard-driven whiteboard for people who think in Vim.**

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 1.95+](https://img.shields.io/badge/rust-1.95%2B-orange.svg)](https://www.rust-lang.org)
[![Status: pre-alpha](https://img.shields.io/badge/status-pre--alpha-lightgrey.svg)](#status)

</div>

lawh (لوح, "board" or "slate") is a native, open-source whiteboard and text editor for the
keyboard. You draw shapes, write math and arrange text with Vim motions and commands instead of a
mouse or a pen. Type `:circle r=3` and the circle appears at your cursor. Type `:math A = pi r^2`
and it renders as math. Jump between objects with `w` and `HJKL`, or to any object in view with
`s` and a short label, take one with `dd` and put it under another with `p`. No drawing tablet, no
hunting for toolbar buttons, no dragging to get the size right.

Whiteboards like Excalidraw and tldraw are built for the mouse: keyboard shortcuts pick a tool, then
you drag. lawh drives the whole canvas with a Vim grammar (modes, counts, operators, motions and
`:` commands). Its boards are plain-text files you keep in git.

## Status

**Pre-alpha.** Milestone 1, "see a board", is done: `lawh file.lawh` opens a window and draws the
board's text, math (rendered with Typst), circles, ellipses, rectangles, lines and arrows.

Milestone 2, the Vim engine, is done: the keyboard drives the board, and `:w` saves it. Building
the "area of a circle" example from an empty board takes 62 keys (see the
[quick start](#quick-start)).

Milestone 3 is done: sizes for text and math, jump labels to reach any object in view in two or
three keys, and `:sym` to put in a math symbol by name.

Milestone 4 is built and being tested: export to SVG, PNG and PDF, from the window with `:export`
or without one with `lawh export`.

## What works

- [x] **Modes:** Normal, Insert (inside a text or math block), block-Normal (Vim's Normal mode inside a block), Visual (select objects) and Command (`:`)
- [x] **Canvas cursor** moved with `hjkl` and counts (`5l`), or onto objects with `w`, `b` and `HJKL`. New objects appear at the cursor.
- [x] **Objects:** text block, math block, circle or ellipse, rectangle, line, arrow
- [x] **Commands:** `:circle r=3`, `:rect 4 2`, `:arrow 3 0`, `:text`, `:math`
- [x] **Math** in [Typst](https://typst.app) syntax: `A = pi r^2`, `sum_(i=1)^n i`
- [x] **Editing:** delete, yank and put, undo and redo, `.` to repeat, counts everywhere, Vim editing inside text and math blocks
- [x] **Sizes:** text and math from 0.25 to 16 times the default size, with `<C-a>` and `<C-x>`, `:size 2`, or `size=2` in the file; math is drawn to match the height of the text beside it
- [x] **Jump labels:** `s` puts a one- or two-character label on every other object in view; type one to jump there, or `ds` and a label to delete a far object without moving
- [x] **Symbol search:** `:sym integral` finds a math symbol by its Typst name, with the best matches shown as you type (fuzzy matching: a name matches when it holds the query's letters in order, not necessarily side by side)
- [x] **Placing:** put objects or a new block below or above another, clear of the rest (`p`, `P`, `o`, `O`, and `:text` or `:math` with the cursor on an object)
- [x] **Files:** plain-text `.lawh` boards that diff cleanly in git, saved atomically with `:w`
- [x] **Export:** `:export board.pdf` draws the board into an SVG, PNG or PDF file, or only a Visual selection; `lawh export board.lawh board.png` does it without a window. Board text in a PDF can be searched and copied
- [x] **Config:** key maps (`:nnoremap` and family) and settings (`:set`) in a `lawhrc`
- [x] **Mouse:** pan, zoom and click to select. Everything else from the keyboard.

## Planned for v1

- [ ] **Moving things:** move a selection with `hjkl`, place one object beside another (next). Moving already works by taking an object with `x` and putting it with `p`
- [ ] **Visual design pass:** the look of the canvas, the status line and the labels, before the repository goes public

Not in v1: freehand drawing, collaboration, a document view, right-to-left text, images, plugins.

## The file format

A board is a plain-text [KDL](https://kdl.dev) file. This is `examples/area.lawh`:

```kdl
// The "area of a circle" benchmark board, in grid units.
lawh version=1

text   id=t1 x=0 y=-5 "Area of a circle"
circle id=c1 x=0 y=0 r=3
line   id=l1 x1=0 y1=0 x2=3 y2=0
text   id=t2 x=1 y=-1 "r"
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
at their top-left corner, and arrows point from `x1,y1` to `x2,y2`. Text and math may take a
`size` after `y` (`text id=t3 x=0 y=-8 size=2 "Title"`): a multiplier on the text height, from
0.25 to 16, written only when it isn't 1. Hand-written files may use comments, any property order
and other KDL string forms; lawh writes this fixed shape. A board holds at most 500 objects and
the file at most 1 MiB. Files must be UTF-8; a leading byte order mark (BOM) is skipped.

The `:` commands use the same names, so `:circle r=3` reads like the line it creates.

## How it works

```text
 file, keys, mouse ──► lawh (window, input, saving, exporting; lawh export)
                         │
                         ▼
                    lawh-core ── board model, .lawh reader and writer,
                         │       Vim engine (modes, : commands, undo, key maps,
                         │       sizes, jump labels, :sym)
                         ▼
                    lawh-render ── draws the board (vello_cpu) with a per-frame work budget,
                         │         and the cursor, selection, jump labels, :sym rows and
                         │         status line over it; exports SVG, PNG and PDF from
                         │         the same draw list
                         │
                         └──► lawh-math ── Typst math → vector paths, compiled in a
                                           sandboxed worker process; Typst's symbol
                                           names for :sym
```

`lawh-core` has no rendering or windowing code, so the Vim engine is tested as plain functions:
keys in, board out. See [docs/architecture.md](docs/architecture.md).

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

## Quick start

Open a new board (the file is created when you write it):

```bash
cargo run -p lawh -- circle.lawh
```

Then type these 62 keys, each `<...>` being one key, with no lawhrc installed (the sample one
moves with `jkl;`):

```text
5kiArea of a circle<Esc>5j:ci 3<CR>:li 3 0<CR>lkir<Esc>4lj:ma A = pi r^2<CR>:w<CR>
```

| Keys | Does |
|---|---|
| `5k` | Move the cursor 5 grid units up |
| `iArea of a circle<Esc>` | Type the title in a new text block |
| `5j` | Back down to the origin |
| `:ci 3<CR>` | A circle of radius 3 around the cursor |
| `:li 3 0<CR>` | The radius: a line from the circle's centre, 3 units right |
| `lk` | One unit right and one up |
| `ir<Esc>` | The label `r` |
| `4lj` | Four units right, one down |
| `:ma A = pi r^2<CR>` | The formula, rendered with Typst |
| `:w<CR>` | Write `circle.lawh` |

`circle.lawh` now holds the same board as `examples/area.lawh`. `:q` quits.

To share it as a picture or a document, type `:exp circle.png<CR>` (or `.svg`, `.pdf`) on the
board, or export it from a shell without opening a window:

```bash
cargo run -p lawh -- export circle.lawh circle.pdf
```

### Key maps and settings

lawh reads `lawhrc` from its config directory at start: `:set` and mapping commands, one a line.
To try the sample, which moves with `jkl;` instead of `hjkl`:

```bash
mkdir -p ~/.config/lawh
cp examples/lawhrc ~/.config/lawh/lawhrc
```

That path is for Linux and macOS; Windows and the file's format are in
[docs/keys.md](docs/keys.md#lawhrc).

## Controls

The essentials. Every key and command: [docs/keys.md](docs/keys.md).

| Keys | Does |
|---|---|
| `h` `j` `k` `l` | Move the cursor one grid unit; with a count, `5l` |
| `w` `b`, `H` `J` `K` `L` | Jump to the next or previous object, or the nearest one left, down, up or right |
| `s` then a label | Jump to any object in view by the label shown on it |
| `:circle r=3`, `:rect 4 2`, `:line 3 0`, `:arrow 3 0` | Draw at the cursor |
| `:text Hello`, `:math A = pi r^2` | Text or math at the cursor |
| `i` `a` `o` | Type a new text block, or into the current text or math block |
| `<CR>` | Edit the current text or math block in Vim's Normal mode |
| `<C-a>` `<C-x>` | Make the current text or math block larger or smaller |
| `:sym integral` | Put in a math symbol by name; `<C-k>` while typing in a block |
| `dd` `yy` `p` `P` | Delete, yank, put below or above |
| `v` | Select objects with a rectangle |
| `u` `<C-r>` `.` | Undo, redo, repeat the last change |
| `:w` `:q` `:wq` `:q!` | Write, quit |
| `:export board.pdf` | Export the board (or, from Visual mode, the selection) to SVG, PNG or PDF |
| Mouse | Drag (left or middle button) to pan, scroll to zoom, click to select |

`RUST_LOG=info` prints how long the first frame took.

## Project structure

```text
lawh/
├── crates/
│   ├── lawh-core/     # board model, file format and the Vim engine
│   ├── lawh-math/     # Typst math in a sandboxed worker process
│   ├── lawh-render/   # drawing, the editing overlay, and export to SVG, PNG and PDF
│   └── lawh/          # the app: window, keys, mouse, saving, lawh export
├── examples/          # a sample board and a sample lawhrc
├── assets/fonts/      # bundled fonts
├── contrib/           # (empty for now)
└── docs/              # architecture, keys, testing, tech stack, comparison, FAQ
```

## How it compares

| | lawh | Excalidraw | tldraw | AFFiNE | qboard |
|---|---|---|---|---|---|
| Vim grammar on the canvas | Yes | No | No | No | Partial: keys for tools |
| Make and size shapes without a drag | Yes | No | No | No | No |
| Math | Typst | Add-on only | SDK example only | Yes | LaTeX |
| Plain-text files | Yes | JSON | JSON | No | JSON |
| Runs as | Native app | Web | Web / SDK | Web / desktop | Web |
| License | MIT | MIT | Source-available | Open source | MIT |

More in [docs/comparison.md](docs/comparison.md) and [docs/faq.md](docs/faq.md).

## Development

```bash
cargo test -p lawh-core                        # fast loop: board model, file format, Vim engine
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
```

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
