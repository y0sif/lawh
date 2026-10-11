# Architecture

Status: milestone 1 ("see a board") is built. `lawh file.lawh` reads a board, opens a window and
draws text, Typst math, circles, ellipses, rectangles, lines and arrows, with mouse pan and zoom.
The Vim engine, editing, saving and export come in milestone 2 and later. Sections marked
**planned** describe that design; sections marked **open** are decisions still to make. Measured
numbers are in [research/m1-measurements.md](research/m1-measurements.md).

## Crates and dependency rules

```text
lawh (bin) ──┬──► lawh-render ──┬──► lawh-core
             │                  └──► lawh-math
             ├──► lawh-math
             └──► lawh-core

lawh-core and lawh-math depend on no other lawh crate.
```

| Crate | Owns today | Planned | Depends on |
|---|---|---|---|
| `lawh-core` | Board model and its limits, `.lawh` reader (own parser) and writer | Vim engine, `:` command parser, undo, spatial index | kurbo, thiserror. No rendering, windowing or Typst, ever. |
| `lawh-math` | Typst sandbox, math worker process and its protocol, layout to vector paths, caches, recompile limits, bundled fonts | Symbol name search for `:sym` | typst, typst-layout, ttf-parser, rlimit (Unix) |
| `lawh-render` | Board to pixels with `vello_cpu`, text layout with parley, the frame budget, PNG encoding of a frame | SVG and PDF export | lawh-core, lawh-math, vello_cpu, parley |
| `lawh` | Command line, window and event loop (winit), presenting frames (softbuffer), mouse pan and zoom, the hidden `--math-worker` mode | Keys, status line, command line, config, file watching | everything above |

`lawh-core` stays light so `cargo test -p lawh-core` never compiles Typst or the renderer.
[tech-stack.md](tech-stack.md) lists every dependency with its version and licence.

## From file to pixels

```text
lawh file.lawh
 └─► lawh: refuse files over 1 MiB, read the file
      └─► lawh-core from_lawh: parser ─► reader ─► Board          (stops at the first problem)
           └─► lawh: open a window (winit), fit the board to it
                └─► lawh-render Renderer::render(board, viewport), once per frame
                     ├─ text: laid out with parley, cached
                     ├─ math: MathEngine::request, never blocks ─► worker process (Typst)
                     ├─ budget: estimate each object, draw it or its outline
                     └─ vello_cpu ─► RGBA pixels
                          └─► lawh: copy into the softbuffer surface, present
```

Math runs beside this loop. A formula the renderer asks for is queued; the engine's queue thread
sends it to the worker process, caches the result and calls a waker. The app's waker posts an
event to the winit event loop, which draws another frame. Until the user pans or zooms, the app
also fits the view to the board again, since a formula's real size replaces its placeholder's.

A frame that ran out of budget for laying out text asks for the next frame itself
(`FrameStats::incomplete`). Otherwise the window waits for input or the math waker
(`ControlFlow::Wait`), so an idle board draws nothing.

## Coordinates

- Board coordinates are **grid units** (`f64`), with y growing down like a screen. Zoom changes
  pixels per unit, never the stored numbers.
- At zoom 1 and scale factor 1, one unit is 40 pixels (`BASE_PX_PER_UNIT`). Physical pixels per
  unit are `40 x zoom x scale factor`, where the scale factor is the display's (2 on most HiDPI
  screens).
- Text and math have an em (the font size) of 0.5 units (`TEXT_SIZE_UNITS`). Text is laid out at
  20 layout units per em and math at 20 pt (`MATH_FONT_SIZE_PT`), then both are scaled to 0.5
  units, so layouts don't depend on zoom.
- Where each kind sits: text and math at the top-left corner of their box, circles and ellipses
  at their centre, rectangles at their top-left corner, lines and arrows at both ends (the
  arrowhead at the second).
- The mouse wheel zooms from 0.1 to 20, 1.1 times per notch. Fitting a board never zooms in
  past 4. Past 10,000 physical pixels per unit, or a scale factor past 32, only the background is
  drawn.

## Board model

- Objects: `Text`, `Math`, `Ellipse` (a circle is an ellipse with equal radii), `Rect`, `Line`,
  `Arrow`.
- Every object has a stable id (`c1`, `t2`, ...) that also appears in the file: 1 to 64 ASCII
  letters, digits, `_` or `-`, unique on the board.
- Z-order is file order.
- `Board::insert` refuses anything past a limit, and the reader enforces the same limits, so
  bounds, fitting and rendering never meet values that overflow or allocate without bound.

## The `.lawh` format

A board is a flat [KDL v2](https://kdl.dev) document: a `lawh version=1` header, then one object
per line. Files must be UTF-8: the app refuses one that isn't, when it reads it (`main.rs`). A
leading byte order mark (BOM, U+FEFF) is skipped (`format/parse.rs`).

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

### Reader

`from_lawh` reads a file in one pass: the parser (`format/parse.rs`) walks the text once,
**without recursion**, and hands one node at a time to the reader (`format/read.rs`), which checks
it and adds its object to the board. Nested block comments are tracked with a counter, not a call
per level, so no file can overflow the stack. Reading stops at the first problem, and the error
names its line and column.

Every limit is checked **while reading**, not after: the parser stops at the node after the
header and 500 objects, at the seventh entry of a node, and at a string or number over 64 KiB of
the file. A string keeps no more decoded text than a text's character and line caps, and the
reader stops at the content that takes the board past its text cap. Slashdashed nodes and entries
are checked but nothing of them is kept. So the time and memory a file costs follow what a board
holds, not the length of the file. The app also checks a file's size before reading it, and reads
at most one byte past 1 MiB.

Hand-written files may use any flat KDL v2: comments, slashdashes (`/-`, which comments out the
node or entry after it), quoted, raw (`#"..."#`, no escapes) and multi-line strings, and properties
in any order. Children blocks (`{ }`), type annotations such as `(u8)` and a slashdash applied to
another slashdash are rejected.

The parser follows KDL v2 as the `kdl` crate (6.7) reads it, and property tests compare the two on
random documents, values included. It rejects a few exotic forms kdl accepts (`/- /-`, a keyword
running straight into the next token). It accepts **one** text kdl rejects: a slashdashed
multi-line raw string that reads as a lone `"`. KDL v2 only forbids the single-line `#"""#`; kdl
rejects every raw string that reads as `"`. lawh rejects them where the file keeps them, with
kdl's message, and reads past them where they are slashdashed, as its earlier kdl-based reader did.

Why not kdl itself: it parses the whole document, with its formatting, before lawh can check any
of it. A 1 MiB hostile file took it up to 2 s and 459 MB; lawh's parser reads each in under
1 ms. kdl stays as a dev-dependency, the oracle for the differential tests.

### Writer

`to_lawh` writes the header, then one line per object in z-order, with a fixed property order per
kind (as in the block above), so saved files diff cleanly. It doesn't use kdl's writer, which
prints identifier-like strings bare, whole floats as `3.0`, and leaves characters such as U+200F
or U+0000 unescaped, which its own parser then rejects.

- Text and math content is **always** written as a double-quoted string, never bare, raw or
  multi-line. `\\`, `"`, newline, carriage return and tab use short escapes.
- Control characters (C0, DEL, C1), invisible formatting characters, and bidi (bidirectional)
  ones, which set the direction of text that mixes left-to-right and right-to-left writing, are
  written as `\u{X}`, so the file shows them plainly and KDL parsers accept it. These are the soft
  hyphen, zero-width space and joiners, LRM and RLM, bidi embeddings, overrides and isolates, the
  byte order mark and a few others.
- Ids are bare when KDL reads them back as the same string (`c1`), quoted otherwise (`"1"`,
  `"true"`).
- Numbers are written as Rust prints them (`3`, `-0.5`), `-0` as `0`, and in exponent form at
  1e38 and up or below 1e-6, so every `f64` reads back exactly.
- Comments and spacing of a hand-edited file are **not** kept: writing prints the fixed shape.
  The app doesn't save yet.

The largest board the caps allow writes 415,551 bytes, well under the 1 MiB read limit.

### Limits

| Limit | Value | Constant | Defined in |
|---|---|---|---|
| File size | 1 MiB | `MAX_FILE_BYTES` | `lawh-core/src/format/mod.rs` |
| Block comment nesting | 16 levels | `MAX_COMMENT_DEPTH` | same |
| Entries (properties and arguments) per node | 6 | `MAX_ENTRIES` | same |
| One string, bare word or number, in file bytes | 64 KiB | `MAX_STRING_BYTES` | same |
| Objects on a board | 500 | `MAX_OBJECTS` | `lawh-core/src/board.rs` |
| Any coordinate | -1,000,000 to 1,000,000 units | `MAX_COORDINATE` | same |
| Radius, width or height | above 0, at most 1,000,000 units | `MAX_SIZE` | same |
| Characters in one text or math source | 5,000 | `MAX_TEXT_CHARS` | same |
| Lines in one text or math source | 500 | `MAX_TEXT_LINES` | same |
| Characters of text and math on a board | 40,000 | `MAX_BOARD_TEXT_CHARS` | same |
| Characters in an id | 64 | `MAX_ID_CHARS` | same |

Each constant's doc comment gives its reason, and measurements where a cost was measured.

## Math

### The sandbox

A board can come from anyone, and Typst math can hold any Typst code (`#` expressions). The Typst
world lawh compiles in (`lawh-math/src/world.rs`) sees exactly one file: the formula, wrapped in a
generated document. Every other file read fails with "access denied", which also stops package
imports (`@preview/...`). Fonts are the two bundled ones, and there is no clock (`today` returns
nothing). These standard library names are removed, from the global scope and from `std`:

| Removed | Why |
|---|---|
| `plugin` | Runs WebAssembly. |
| `cite`, `bibliography` | Parse CSL styles with quick-xml 0.38, which has open denial-of-service advisories. |
| `image` | Decodes images from inline bytes: a small PNG can claim 40000x40000 pixels. |
| `read`, `json`, `yaml`, `toml`, `cbor`, `csv`, `xml` | Read files (denied anyway) or parse inline bytes: deep XML overflows the stack, a CBOR header can claim a huge array. |
| `raw` | Parses themes and syntaxes from inline bytes. It never draws in lawh (no monospace font). |

Removing `raw` narrows the way in but doesn't close it: the element is still reachable from raw
content. Those parsers then run in the worker, under its timeout (and on Linux its memory cap),
like the rest of Typst.

### The worker process

Typst can't be cancelled, and a thread can't contain a stack overflow or be capped in memory. So
every formula from a board compiles in a separate **worker process**: the app starts itself with
the hidden `--math-worker` flag, which `main` checks before anything else (the worker's stdout
carries only its protocol, so nothing may be printed first). Tests start the `lawh-math-worker`
binary instead.

- The worker starts with the first formula, with one Typst thread (`RAYON_NUM_THREADS=1`), 64 MiB
  stacks and one malloc arena (`MALLOC_ARENA_MAX=1`; glibc's allocator otherwise reserves a
  64 MiB region of address space, an arena, per thread), so its address space tracks the memory
  it uses.
- On start, on Unix, it caps its address space at **768 MiB** (`RLIMIT_AS`) and turns off core
  dumps, lowering both soft and hard limits so nothing in it can raise them again. The memory cap
  holds on Linux. macOS accepts `RLIMIT_AS` but doesn't enforce it, and Windows has no cap yet (it
  needs a Job Object, the Windows feature that sets limits on a group of processes); there only
  the timeout bounds a runaway compile.
- A compile that takes more than **2 s** (`DEFAULT_TIMEOUT`) gets the worker killed and the
  formula marked `Timeout`. A worker that crashes (stack overflow, memory cap) or sends anything
  malformed is killed too, and the formula is marked `Crashed`.
- The next formula starts a fresh worker. A request to a worker that died while idle is retried
  once on a new one. A new worker must announce itself within 5 s; after 3 failed starts in a row
  a formula gets `Unavailable`, and each later formula tries once more.
- If the app goes away mid-compile, the worker exits on its own 1 s past the timeout.
- **Known limitation:** the worker's logs are dropped. `main` hands over to the worker before it
  sets up logging, so the worker's panic hook and its `warn!` lines go nowhere, even though the
  worker's stderr is the app's. Planned fix: set up logging in worker mode (on stderr; stdout
  stays the protocol's).

The engine also has an in-process mode (`MathEngine::new`, `Renderer::new_trusted`) for trusted
input only: tests, goldens and formulas lawh writes itself.

### Protocol (version 1)

The worker first writes one line, `lawh-math-worker 1`. The parent refuses a worker that announces
anything else. After that both sides exchange frames: a little-endian `u32` length, then that many
bytes, at most 16 MiB. The parent sends one request (a timeout and the formula) and waits for its
response (a layout, a compile error, "panicked" or "too large") before sending the next.

The parent treats everything the worker sends as untrusted: counts must fit in the bytes left,
numbers must be finite, fonts must be bundled, glyph ids must exist, paths must start with a move,
and nothing may follow the payload. Counts are added up as they are read, so a layout past the
complexity caps is refused before anything past them is allocated.

A layout reaching past 100,000 pt (`MAX_LAYOUT_EXTENT_PT`), or holding more than 25,000 glyphs
(`MAX_LAYOUT_GLYPHS`) or 10,000 path elements (`MAX_LAYOUT_PATH_ELEMENTS`), is `TooLarge`, from the
worker and in process alike.

### The async API

`MathEngine::request(src)` never blocks. It answers with a `MathStatus`:

| Status | Meaning | Drawn as |
|---|---|---|
| `Ready(layout)` | Laid out and cached | The formula |
| `Failed(error)` | Won't lay out; the error is cached, so asking again is free | Its source, in red |
| `Pending` | Queued (once) or compiling; the waker is called when it finishes | Its source, in grey |
| `Evicted(summary)` | Laid out before, but pushed out of the cache; its size is still known | A faint outline of its bounds |

`MathEngine::layout_blocking` waits instead, for exports and tests. The renderer asks only for
formulas on screen, nearest the centre of the view first, so a slow formula elsewhere can't hold
up the ones in view.

Every outcome is cached by source text, errors and timeouts included, so a hostile formula costs
one compile, not one per frame. Layouts are kept within 64 MiB (estimated), failures within 4,096
entries and 16 MiB, both least recently used first out. Compile errors are cut to 1,024 bytes.

### Eviction and recompiling

Formulas that don't fit in the layout cache together push each other out. Before the limits below,
every formula pushed out was asked for again on the next frame and compiled again, pushing out
another: 40 formulas at the complexity caps made 2,099 compiles in 55 s and never settled.

Now `request` doesn't queue a formula whose layout was pushed out. It answers `Evicted` with a
summary of the layout (size, ink bounds, glyph and path counts), so the formula can still be
placed, skipped when off screen and drawn as an outline, and marks it **wanted**. The engine's
queue thread compiles a wanted formula again only when all of these hold:

- **Cooldown:** at least 5 s (`RECOMPILE_COOLDOWN`) have passed since it was pushed out, so
  formulas pushing each other out don't make the window redraw continuously.
- **Still wanted:** it was asked for within the last 30 s (`WANTED_FOR`). A formula on screen is
  asked for on every frame, and every compile calls the waker, which draws a frame, so it stays
  wanted. One scrolled off screen stops being compiled.
- **Rate limit:** up to 8 at once (`MAX_RECOMPILE_BURST`), then one per 500 ms
  (`RECOMPILE_INTERVAL`), the formula pushed out longest ago first. New formulas go first.

So in any `t` seconds the engine compiles at most **8 + 2t** layouts again (rounded up), however
many formulas want it. Measured: 82 recompiles in 60 s against a bound of 128. Real formulas take
a few kilobytes, so real boards never get here.

## Rendering

`vello_cpu` draws each frame into an RGBA image on the CPU: paper, grid dots, then objects in
board order. The app copies it into the softbuffer surface. Glyphs are drawn as unhinted outlines
from the bundled fonts, so they stay crisp at any zoom. Objects off screen or smaller than half a
pixel are skipped. Move to `vello_gpu` only if profiling shows the CPU path is too slow: a frame
of `examples/area.lawh` at 1920x1080 takes about 1.1 ms in release.

### The frame budget

Without a limit, a hostile board could take seconds a frame: 500 screen-filling objects, or 25,000
glyphs stacked on one spot. So each frame's work is budgeted.

- **Costs** are estimated nanoseconds of `vello_cpu` work, modelled on what it does with a path:
  it flattens curves into lines, walks every line across the 4x4-pixel tiles it touches, and paints
  the area between. A path costs per line, per pixel of outline on screen and per pixel of area
  covered; a glyph the same, from statistics of its outline in the font.
- **Counted before drawing:** each object's cost is estimated before it is drawn, and the estimate
  stops counting once it passes what is left. An object that fits is drawn and charged. One that
  doesn't is charged only the work of estimating it and drawn as a faint **outline** of its
  bounds, from a separate 3 ms outline budget; past that it is left out. Board order never changes.
- **Budgets:** 18 ms of estimated work per window frame, 8 ms for the frame right after a resize
  (resizing costs `vello_cpu` about 10 ms of its own at 2880x1800), and 2 s per export. With a
  frame's fixed cost (1 ms at 1920x1080, 3 ms at 2880x1800) this keeps a frame under 33 ms, two
  refreshes at 60 Hz.
- **Calibrated:** the `calibrate_*` tests check that nothing takes more than 1.2 times its estimate
  in release, and `frame_budget_worst_cases` that the worst boards stay under 33 ms a frame and
  100 ms to open. Estimates lean high, so a frame that spends its whole budget takes less.
- `Renderer::last_frame` returns `FrameStats`: how many objects were drawn, downgraded to outlines,
  left out, too small, waiting for math or text, and the frame's estimated work.

### Text

Text is laid out by parley with the bundled Inter only. parley's `system` feature is off, so no
system font is ever loaded and a board looks the same everywhere.

A text is laid out once and cached by its hash (about 30 MB at most, `MAX_TEXT_CACHE_BYTES`,
least recently used first out). With the layout, the cache keeps the facts the budget needs
(characters, lines, run changes) and a plan of where its glyphs are and what they cost to draw, so
a frame estimates a text without walking its layout. Laying out is charged to the frame that does
it. A text a frame can't afford is drawn as an outline and laid out by a later frame; each frame
lays out at least one, so this ends.

Laying out costs per character (ASCII and other characters priced apart), per line, and per
**run change**. parley shapes text in runs and starts a new one where the script changes (Latin to
Arabic, say) or the bidi level does: the Unicode bidi algorithm gives each character of mixed
left-to-right and right-to-left text a level that sets its direction. The renderer counts run
changes without laying the text out, from each character's script and bidi class in parley's own
tables (`parley_data`) and a short model of the bidi algorithm. A test checks the count against
the runs parley lays out. Two cases the model doesn't follow are charged at their worst:

- **After an explicit bidi code** (embeddings, overrides and isolates, U+202A to U+202E and
  U+2066 to U+2069), every character counts as a run change, the most a character can cost. Real
  text almost never holds these.
- **Brackets in right-to-left text:** the bidi algorithm gives a pair of brackets the direction of
  what is inside or around it (rule N0), which can start runs the model doesn't count. So when a
  text holds any character that can make a right-to-left level (a right-to-left letter, an Arabic
  digit or an explicit code), each paired bracket counts as one more change. The bracket data comes
  from `icu_properties`.

### One glyph per character

OpenType fonts carry optional **features** that change which glyphs a run of characters becomes.
The text shaper turns four of them off: `calt` (contextual alternates), `frac`, `numr` and `dnom`
(automatic fractions). In Inter, `calt` joins `->`, `=>` and the like into one arrow glyph and
raises brackets, `:` and other punctuation next to capitals or between digits; the fraction
features shrink the digits around U+2044 FRACTION SLASH.

The reason is the Vim cursor: it moves over what is on screen, one glyph per character, as typed.
A ligature that turns two characters into one glyph leaves the cursor nowhere to stand. It also
helps speed: with `calt` on, laying out `)`, `]` and `}` took 6.5 to 7.0 µs a character, about
20 times more than with it off. Inter has no `liga`, `clig` or `rlig`, and the shaper leaves `dlig`
and `case` off. `ccmp` stays on: it only joins a letter and its combining marks.

### Math

A formula is drawn from its layout: glyph runs from the bundled fonts, and paths (fraction bars,
radical lines, and glyphs Typst scales or rotates, drawn as outlines), filled or stroked with round
joins and caps.
Typst's default black is drawn in lawh's ink colour. Glyphs with an em over 100,000 pixels and
strokes wider than that are skipped.

## Data flow for one key press (planned, milestone 2)

```text
winit KeyEvent
  └─► lawh: map to a logical key (typed character + modifiers)
        └─► lawh-core Engine::feed(key)
              ├─ pending: wait for more keys (count, operator, motion)
              └─ complete: apply Ops to the Board, push one undo step
                    └─► lawh-render: redraw
                          └─► lawh-math: compile changed math blocks (cached by source)
```

The engine is a pure state machine: `(state, board, key) → (state, board changes, effects)`.
Effects are things only the app can do (write a file, quit, export, open the system clipboard).
That keeps the engine testable without a window.

## The canvas cursor (open)

Vim motions are defined on a stream of text. A canvas is 2D, so the core design question is what
the cursor is. The current plan has two layers:

1. **Grid cursor**: a point on the grid, moved with `hjkl` and counts. New objects appear here.
2. **Object motions**: jump to the nearest object in a direction (like tldraw's
   `getNearestAdjacentShape`), or to any object by a short jump label (like flash.nvim).

Still open: what `w`, `b`, `e`, text objects (`ic` for "inside circle"?) and operators like
`d{motion}` mean on objects, and how "place below" picks a gap.

## Text editing inside blocks (planned)

Insert mode inside a text or math block reuses Vim text editing on that block's string.
parley's `PlainEditor` handles layout, cursor and selection geometry, and the engine drives it.
One glyph per character (above) keeps the cursor on what is typed. Right-to-left text is out of
scope for v1, and the bundled Inter has no Arabic glyphs.

## Portability rules

Linux first, with releases for macOS and Windows from the same code:

- Vim keys match the typed character. App shortcuts go through one modifier setting (Ctrl, or Cmd
  on macOS). (Planned with the Vim engine.)
- Platform config and cache directories via `etcetera`. No hard-coded `/` in paths. (Planned
  with config.)
- The window scale factor is handled from the start (Wayland fractional scaling needs it on Linux).
- `.lawh` files may use CRLF line endings; errors point at the same place as with LF.
- The math worker's memory cap holds on Linux only so far (see [../SECURITY.md](../SECURITY.md)).
