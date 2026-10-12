# Architecture

Status: milestone 1 ("see a board") is built: `lawh file.lawh` reads a board, opens a window and
draws text, Typst math, circles, ellipses, rectangles, lines and arrows, with mouse pan and zoom.
Milestone 2 (the Vim engine) is built and merged: keys move the cursor, make and edit objects,
select, undo and repeat, `:w` saves, and key maps and settings load from a lawhrc. Milestone 3 is
built on `feat/m3-sizes-jumps` and waiting for the maintainer's test: sizes for text and math,
jump labels (`s` puts a short label on every object in view; typing one jumps there), and `:sym`
to put in a symbol by name. Export and the rest come later. Sections marked **planned** describe
design not built yet; **open** marks decisions still to make. Measured numbers are in
[research/m1-measurements.md](research/m1-measurements.md),
[research/m2-measurements.md](research/m2-measurements.md) and
[research/m3-measurements.md](research/m3-measurements.md). The keys themselves are listed in
[keys.md](keys.md).

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
| `lawh-core` | Board model and its limits (text and math sizes included), `.lawh` reader (own parser) and writer, Vim key notation, the Vim engine: modes, board grammar, sizes, jump labels, text editing in blocks, `:` command parser, `:sym` matching, undo, `.`, key maps, settings, lawhrc parsing | Spatial index | kurbo, nucleo-matcher, thiserror, unicode-segmentation. No rendering, windowing or Typst, ever. |
| `lawh-math` | Typst sandbox, math worker process and its protocol, layout to vector paths, caches, recompile limits, bundled fonts, the list of Typst's symbol names for `:sym` (`symbols()`, with whether the bundled fonts draw each) | | typst, typst-layout, codex, ttf-parser, rlimit (Unix) |
| `lawh-render` | Board to pixels with `vello_cpu`, text layout with parley (Libertinus Math for characters Inter lacks), text and math sizes, the frame budget, PNG encoding of a frame, the editing overlay (cursor, Visual selection, caret, jump labels, `:sym` rows, status and command line), caret geometry, hit tests, and measuring text and math for the engine (`impl Measure for Renderer`), with what laying text out cost | SVG and PDF export | lawh-core, lawh-math, vello_cpu, parley |
| `lawh` | Command line, window and event loop (winit), presenting frames (softbuffer), winit keys to engine keys, the view for jump labels (`Engine::set_view`), the symbol list into the engine at start, the engine's effects (atomic writes and the same-file check after them, quitting, the mapping timer), the status line's text, the lawhrc from the config directory, mouse pan, zoom and click to select, the hidden `--math-worker` mode | App shortcuts, file watching, clipboard | everything above, etcetera |

`lawh-core` stays light so `cargo test -p lawh-core` never compiles Typst or the renderer.
[tech-stack.md](tech-stack.md) lists every dependency with its version and licence.

## From file to pixels

```text
lawh file.lawh
 └─► lawh: refuse files over 1 MiB, read the file (a missing file opens as an empty board)
      └─► lawh-core from_lawh: parser ─► reader ─► Board          (stops at the first problem)
           └─► lawh-core Engine::new(board); lawh adds the :sym list and reads the lawhrc
                └─► lawh: open a window (winit), cursor on the first object, fit the board
                     └─► lawh-render Renderer::render_with(board, viewport, overlay), per frame
                          ├─ text: laid out with parley, cached
                          ├─ math: MathEngine::request, never blocks ─► worker process (Typst)
                          ├─ budget: estimate each object, draw it or its outline
                          ├─ overlay: cursor, selection, caret, jump labels, :sym rows,
                          │           status bar (outside the budget)
                          └─ vello_cpu ─► RGBA pixels
                               └─► lawh: copy into the softbuffer surface, present
```

At start, before the lawhrc, the app builds the list of symbols `:sym` picks from
(`lawh_math::symbols`, kept to the 732 the bundled fonts draw) and hands it to the engine
(`Engine::set_symbols`): 1.2 to 1.8 ms in release, 6.2 ms in debug (measured).

Math runs beside this loop. A formula the renderer asks for is queued; the math engine's queue
thread sends it to the worker process, caches the result and calls a waker. The app's waker posts
an event to the winit event loop, which draws another frame. Until the user pans, zooms, clicks or
types a key, the app also fits the view to the board again, since a formula's real size replaces its placeholder's.
After a key, the view pans as little as it can to keep the cursor, the current object or the
caret `FOLLOW_MARGIN_PX` (48 logical pixels) inside the window.

A frame that ran out of budget for laying out text asks for the next frame itself
(`FrameStats::incomplete`). Otherwise the window waits for input or the math waker
(`ControlFlow::Wait`), so an idle board draws nothing.

## Coordinates

- Board coordinates are **grid units** (`f64`), with y growing down like a screen. Zoom changes
  pixels per unit, never the stored numbers.
- At zoom 1 and scale factor 1, one unit is 40 pixels (`BASE_PX_PER_UNIT`). Physical pixels per
  unit are `40 x zoom x scale factor`, where the scale factor is the display's (2 on most HiDPI
  screens).
- Text and math of size 1 have an em (the font size) of 0.5 units (`TEXT_SIZE_UNITS`); a size
  multiplies it (see [Text and math sizes](#text-and-math-sizes)). Text is laid out at 20 layout
  units per em and math at 20 pt (`MATH_FONT_SIZE_PT`), then both are scaled to 0.5 units times
  the size, math `MATH_SCALE` (1.2725) times more, so layouts don't depend on zoom or size.
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
- Text and math have a `size`: a multiplier on the text height, `DEFAULT_TEXT_SIZE` (1) unless
  the file or the user sets another, from `MIN_TEXT_SIZE` (0.25) to `MAX_TEXT_SIZE` (16). The
  board refuses a size out of range or not finite, as it refuses other bad geometry.
- `Board::insert` refuses anything past a limit, and the reader enforces the same limits, so
  bounds, fitting and rendering never meet values that overflow or allocate without bound.

## The `.lawh` format

A board is a flat [KDL v2](https://kdl.dev) document: a `lawh version=1` header, then one object
per line. Files must be UTF-8: the app refuses one that isn't, when it reads it (`read_board` in
`crates/lawh/src/files.rs`). A leading byte order mark (BOM, U+FEFF) is skipped
(`format/parse.rs`).

```kdl
lawh version=1
text id=t1 x=0 y=-5 "Area of a circle"
text id=t2 x=0 y=-8 size=2 "A title"
math id=m1 x=5 y=0 "A = pi r^2"
circle id=c1 x=0 y=0 r=3
ellipse id=e1 x=0 y=0 rx=3 ry=2
rect id=r1 x=0 y=0 w=4 h=2
line id=l1 x1=0 y1=0 x2=3 y2=0
arrow id=a1 x1=0 y1=0 x2=3 y2=0
```

**Sizes.** `text` and `math` take an optional `size=` after `x y`: a number from 0.25 to 16, the
multiplier on the text height. The writer gives it only when the size is not exactly 1, so a
board without sizes writes the same bytes it did in milestone 2, and the header stays
`lawh version=1`. A size out of range stops the read
(``2:20: property `size` must be between 0.25 and 16``), as does a size on a shape or one given
twice. A build from milestone 2 refuses a file that gives a size, since its reader rejects
properties it doesn't know.

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
- `size=` follows `y` on `text` and `math`, in the same number style, and only when the size is
  not 1.
- Comments and spacing of a hand-edited file are **not** kept: writing prints the fixed shape.
  `:w` writes it atomically (see [Data flow for one key press](#data-flow-for-one-key-press)).

The largest board the caps allow writes 415,751 bytes (`largest_board` in
`tests/format_errors.rs`: 8 texts at a size written with 19 characters, the rest arrows), well
under the 1 MiB read limit.

## Limits

Every cap on what a file, a board, a key or a frame may cost is a named constant, documented where
it is defined with its reason, and measurements where a cost was measured. A cap refuses with a
message; it never panics. `CLAUDE.md` has the short list by file; the math caps are under
[Math](#math), the frame budget's under [The frame budget](#the-frame-budget).

### File and board

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
| Size of a text or math object | 0.25 to 16 (a multiplier on the text height; 1 by default) | `MIN_TEXT_SIZE`, `MAX_TEXT_SIZE` (`DEFAULT_TEXT_SIZE`) | same |

Below 0.25 text is a smudge at any zoom one works at (5 logical pixels an em at zoom 1). At 16 a
formula at the closest zoom (20) on a scale factor 4 screen has an em of about 32,600 pixels
(derived), inside the 65,536 pixels an em the frame budget is calibrated for.

### Engine, overlay and app

Paths are under `crates/`. "Measured" numbers are release builds unless they say debug (see
[research/m2-measurements.md](research/m2-measurements.md) and
[research/m3-measurements.md](research/m3-measurements.md)); "derived" ones are worked out from
the caps.

| Limit | Value | Constant, file | Why |
|---|---|---|---|
| Count before a command | 10,000 | `MAX_COUNT`, `lawh-core/src/engine/mod.rs` | No motion, undo or put needs more (a board holds 500 objects); saturating keeps `999999999p` cheap to refuse. |
| Undo steps kept | 1,000 | `MAX_UNDO_STEPS`, `engine/undo.rs` | Vim's default `undolevels`. |
| Undo memory | 16 MiB, estimated | `MAX_UNDO_BYTES`, same | A step holds whole objects: 1,000 edits of a 5,000-character text would hold 40 MB (derived). Keeps 39 steps that change a whole board (about 0.42 MB each, with a 136-byte edit, derived). |
| Keys one change keeps for `.` | 10,000 | `MAX_REPEAT_KEYS`, `engine/repeat.rs` | The longest text worth typing in one go (5,000 characters), and as much again for moving and correcting. |
| Characters on the command line | 5,064 (`MAX_TEXT_CHARS` + 64) | `MAX_COMMAND_LINE_CHARS`, `engine/cmdline.rs` | `:text` with a full text, plus room for the name. |
| Mappings per mode | 1,000 | `MAX_MAPS`, `engine/keymap.rs` | A lawhrc holds tens. Bounds what mappings hold (about 6 MB a mode, derived). The worst lawhrc it allows loads in 676 ms (measured). |
| Keys on each side of a mapping | 256 | `MAX_MAP_KEYS`, same | Vim takes 50 bytes; a long `:te` or `:ma` line still fits on the right side. |
| Mapping expansions per typed key | 1,000 | `MAX_MAP_DEPTH`, same | Vim's `maxmapdepth`. Stops `:nmap a b` with `:nmap b a`, and `:map x yx`, which runs commands as it loops. |
| Keys one typed key sets off | 30,000 | `MAX_EXPANDED_KEYS`, same | Bounds the lookups behind one key: before it, one key took 82 s (measured). Leaves room for `.` of the longest change with a count. |
| Keys waiting in the typeahead | 10,000 | `MAX_PENDING_KEYS`, same | Stops a mapping that feeds itself more keys than it takes, within 40 expansions. |
| `timeoutlen` | at most 10,000 ms | `MAX_TIMEOUTLEN`, `engine/settings.rs` | Nobody pauses 10 s inside a mapping; a typo would leave keys stuck on the status line. |
| lawhrc size | 1 MiB | `MAX_RC_BYTES`, `engine/rc.rs` | A real lawhrc is a few kilobytes; a wrong file (a log, a binary) is refused cheaply. |
| lawhrc warnings listed | 100, then one that counts the rest | `MAX_RC_WARNINGS`, same | A wrong file can fail on every line; nobody reads past the first few. |
| Work per typed key | 100,000,000 units (about 1 ns each in debug) | `MAX_KEY_WORK`, `engine/work.rs` | A key that runs out takes about 0.1 s in debug, 10 ms in release (derived from the measured cost of a unit; 30 ms in release when matching `:sym` names); the costliest single command fits. See [What one key may cost](#what-one-key-may-cost). |
| Work per estimated nanosecond of laying out text | 1 unit | `KEY_WORK_PER_LAYOUT_NS`, `lawh-render/src/budget.rs` | Text the renderer lays out to measure it for a key counts against that key. A key that runs out this way stopped in 46 to 99 ms in release (measured), where it ran for 1.9 s to 6.4 s before. |
| Characters in a jump label | 2 | `MAX_LABEL_KEYS`, `engine/jump.rs` | Two of the default 26 characters make 676 labels, more than a board holds objects (500). A third key would slow every label, for small alphabets only. |
| Characters in `jumplabels` | 2 to 52 | `MIN_LABEL_CHARS`, `MAX_LABEL_CHARS`, same | One character makes one label. 52 is the ASCII letters in both cases, past what a hand finds without looking; the fixed array keeps the option `Copy` (208 bytes). |
| Symbols `:sym` keeps | 4,096 | `MAX_SYMBOLS`, `engine/sym.rs` | Typst's 1,189 names (732 drawable), with room for a later Typst and more fonts. Every key of the costliest 64-character query over 4,096 Typst-like names fits in a third of `MAX_KEY_WORK`: 4.3 ms in debug (measured). |
| Characters in a `:sym` query | 64 | `MAX_SYM_QUERY_CHARS`, same | The longest Typst name has 28. Each query word is a pass over every name: a query of 1,000 characters and 500 words took 7.3 ms in release (measured); 64 characters hold at most 32 words. |
| Bytes of a `:sym` query read | 256 (4 a character) | `MAX_SYM_QUERY_BYTES`, same | A longer query is refused before the line is copied or counted (derived from UTF-8's 4 bytes a character). A mapping that typed on a 20 KB `:sym` line 30,000 times took 110 ms in release before, 6.5 ms after (measured). |
| `:sym` rows shown | 8 | `SYM_ROWS`, same | As many as a completion menu shows in Helix or Neovim without scrolling; they fit above the status bar in any window lawh is usable in. |
| Writes per typed key | 4 | `MAX_WRITES_PER_KEY`, `engine/exec.rs` | One write costs the app 2.76 ms (measured on an SSD, build not recorded); 4 take about 11 ms and leave room for a mapping that writes the board and a copy. |
| Writes waiting for their report | 4 (= `MAX_WRITES_PER_KEY`) | `MAX_PENDING_WRITES`, same | The app reports right after the key, so only one key's writes wait together. |
| Visual highlights per frame, less what jump labels cost | 3 ms of estimated work | `HIGHLIGHT_BUDGET_NS`, `lawh-render/src/budget.rs` | A full board selected, every object crossing the screen, would take tens of milliseconds; past it the rectangle still shows the selection. Jump labels are always drawn in full (one not drawn can't be typed), and their estimated cost comes out of this first. |
| Characters of a status string drawn | 256 | `STATUS_WINDOW_CHARS`, `lawh-render/src/status.rs` | About 1,800 logical pixels, wider than most windows; lays out in at most about 0.5 ms. |
| Status strings kept laid out | 40 | `STATUS_CACHE_ENTRIES`, same | Left, right, the command line, and the name and value of each of 8 `:sym` rows (19 in a frame), with room for those shown a moment ago. A compile-time check keeps it at least `2 x SYM_ROWS + 3`. |
| Status bar height | 24 logical pixels | `STATUS_BAR_PX`, `lawh-render/src/overlay.rs` | One line of 13-pixel text with room above and below. |
| Pointer travel that is still a click | 4 logical pixels | `CLICK_SLOP_PX`, `lawh/src/app.rs` | A hand never holds a mouse perfectly still; past it the press pans. |
| Distance at which a click still hits | 4 logical pixels | `HIT_TOLERANCE_PX`, same | Thin lines stay easy to click. |
| Room the view keeps around the cursor | 48 logical pixels | `FOLLOW_MARGIN_PX`, `lawh/src/view.rs` | A little more than a grid unit at zoom 1, so what is next to the cursor stays in view. |
| Temporary file names tried per write | 100 | `TEMP_NAME_TRIES`, `lawh/src/files.rs` | A crash may leave a temporary file behind, or another lawh may write the same file. |
| Symbolic links followed per write | 40 | `MAX_LINK_HOPS`, same | Linux follows at most 40 (`MAXSYMLINKS`); longer chains loop. |

Design constants shape the grammar and the drawing without capping anything: `ROW_TOLERANCE`
(0.5 units, `engine/motion.rs`, see [Reading order](#reading-order)), `PLACE_GAP` (0.5 units,
`engine/place.rs`, see [Put and the place rule](#put-and-the-place-rule)), `SIZE_STEPS`
(`engine/size.rs`, see [Sizes](#sizes)), `DEFAULT_JUMP_LABELS` (`engine/jump.rs`), `MATH_SCALE`
(1.2725, `lawh-render/src/renderer.rs`, see [Text and math sizes](#text-and-math-sizes)), the
jump labels' sizes (`LABEL_BOX_PX` 14, `LABEL_FONT_PX` 11 and `LABEL_PAD_PX` 3 logical pixels, and
`LABEL_FONTS`, in `renderer/labels.rs`) and the `:sym` rows' (`ROW_PX` 20 and `VALUE_GAP_PX` 16, in
`renderer/completions.rs`).


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
- **Logs:** in worker mode `main` sets up logging on stderr before it hands over to the worker, so
  the worker's panic hook and its `warn!` lines reach the app's stderr; stdout stays the
  protocol's (`math_worker_logs_go_to_stderr_only` in `lawh/tests/cli.rs`).

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
  100 ms to open. A row over its limit is measured again, up to 3 times in all, and keeps its
  least time: load only adds time, while an estimate that is too low fails every time (see
  [testing.md](testing.md#calibrations-and-benches)). Estimates lean high, so a frame that spends
  its whole budget takes less.
- `Renderer::last_frame` returns `FrameStats`: how many objects were drawn, downgraded to outlines,
  left out, too small, waiting for math or text, and the frame's estimated work.

### Text

Text is laid out by parley in the bundled Inter, and a character Inter lacks in Libertinus Math,
the other bundled font. This is **font fallback**: a character goes to the next font that has
it, so the symbols `:sym` puts into a text block (`∧`, `⊞`) show. A character neither font has is
Inter's missing-glyph box. Which characters fall back is read once from the two fonts' character
maps (`Fallback` in `text.rs`); the status bar and the `:sym` rows are laid out the same way.
parley's `system` feature is off, so no system font is ever loaded and a board looks the same
everywhere.

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
The text shaper turns five of them off: `calt` (contextual alternates), `frac`, `numr` and `dnom`
(automatic fractions), and `liga` (standard ligatures). In Inter, `calt` joins `->`, `=>` and the
like into one arrow glyph and raises brackets, `:` and other punctuation next to capitals or
between digits; the fraction features shrink the digits around U+2044 FRACTION SLASH. Libertinus
Math's `liga` would join `fi`, `ffl`, and `ch` or `ct` with a zero-width joiner between, in
letters that fall back to it (as letters do when a joiner or a mark only it has goes with them).

The reason is the Vim cursor: it moves over what is on screen, one glyph per character, as typed.
A ligature that turns two characters into one glyph leaves the cursor nowhere to stand. It also
helps speed: with `calt` on, laying out `)`, `]` and `}` took 6.5 to 7.0 µs a character, about
20 times more than with it off. Neither font has `clig` or `rlig`, and the shaper leaves `dlig`
and `case` off. `ccmp` stays on: it only joins a letter and its combining marks.

### Math

A formula is drawn from its layout: glyph runs from the bundled fonts, and paths (fraction bars,
radical lines, and glyphs Typst scales or rotates, drawn as outlines), filled or stroked with round
joins and caps.
Typst's default black is drawn in lawh's ink colour. Glyphs with an em over 100,000 pixels and
strokes wider than that are skipped (`MAX_GLYPH_EM_PX`, `MAX_STROKE_PX`); a size-16 formula gets
there only at views closer than the app ever shows (derived).

### Text and math sizes

A text of size `s` is laid out once, at the same em whatever its size, and drawn `s` times as
large, so a size change lays nothing out again. Math is drawn `MATH_SCALE` (1.2725) times larger
than text of the same size, so the two have the same **x-height** (the height of a lowercase
`x`): Libertinus Math's x-height is smaller than Inter's, 0.429 em (429 of 1,000 units) against
0.5459 em (1,118 of 2,048), read from the fonts' OS/2 tables, and at one em math's `x` would be
21% shorter than the text's beside it. A unit test holds `MATH_SCALE` to the fonts' ratio within
0.1%.

The formula is then moved so its first **baseline** (the line its letters sit on) is where a
text's first baseline is when the two share a `y` (`MathLayout::baseline`): a short formula beside
a label reads as one line. Measured on the drawn pixels of an `x` in each
(`math_and_text_share_x_height_and_baseline` in `lawh-render/tests/render.rs`, which asserts
within 0.05 units and 3%): at size 1 the baselines are 0.0075 units apart and math's `x` is 2.30%
taller, the same ratio at sizes 0.5, 2 and 4. The 2.3% is Libertinus Math's italic `x`, 0.439 em
tall where its OS/2 x-height says 0.429 (derived from the two). A two-line formula's first line
sits where a one-line formula does.

A size outside the range (only from a board built without the board's checks) is drawn clamped
to it. A glyph's drawing cost grows with its size on screen, and the frame budget prices it so:
`calibrate_text_drawing` has rows at size 16, and the largest size stays inside the 65,536
pixels an em that `calibrate_glyphs` covers (see [Limits](#file-and-board)).

### The overlay

`Renderer::render_with` draws the board as `render` does, then the overlay over it: the cursor (a
cross on a grid point, an outline around the current object), the Visual rectangle and an
outline around each selected object, the caret (a bar in Insert mode and on the command line, a
block in block-Normal mode), jump labels, `:sym` rows and the status bar along the bottom. Its
sizes are in logical pixels times the scale factor, so it stays readable at any zoom.

- **Jump labels** (`Overlay::labels`, from `Engine::jump_labels`): a 14-pixel box of the cursor's
  blue centred on each object's anchor, kept inside the window above the status bar, with the
  label in 11-pixel letters of the paper colour; the characters typed so far are dimmer, and only
  the labels that start with them are drawn. Nearer labels go over farther ones, and a farther
  label's letter under a nearer box is left out. The boxes are pixel-aligned fills and the
  letters one glyph run per font and colour, however many labels there are.
- **`:sym` rows** (`Overlay::completions`, from `Engine::completions`): a panel along the top of
  the status bar, best match at the top, each row the name and then the symbol in a column, the
  selected row highlighted. Rows are laid out with the status strings and kept with them
  (`STATUS_CACHE_ENTRIES`), so a frame lays out only rows that changed.

The overlay isn't charged to the frame budget: its cost is bounded by the image and lawh-core's
caps, not the board. Selection outlines have their own 3 ms budget (`HIGHLIGHT_BUDGET_NS`). Jump
labels are always drawn in full, since a label that isn't drawn can't be typed, and their
estimated cost comes out of that budget first: with labels on hundreds of objects, fewer selected
objects are outlined (the Visual rectangle still shows the selection). 500 two-letter labels add
about 2.7 ms to a frame at 1920x1080 and 4.1 ms at 2880x1800 (release, measured). Only 256
characters of each status string are laid out, and only when the string changes.

Measured in release (`overlay_frame_time` in `lawh-render/tests/bench.rs`, 2026-10-12): an
everyday overlay adds up to 0.3 ms to a frame. The worst in Visual mode (a full board selected, a
Visual rectangle over the whole view, a jump label on every object) adds 4.3 to 4.7 ms at
1920x1080 and 6.5 to 7.2 ms at 2880x1800; the worst on a `:sym` line (a full board selected, the
longest command line and eight rows of the longest names, changing every frame) 5.0 to 5.6 ms and
4.7 to 5.4 ms. Both are under the 8 ms target the bench asserts (`OVERLAY_TARGET`).

## Data flow for one key press

```text
winit KeyEvent (presses and repeats; releases and synthetic presses are dropped)
  └─► lawh keys::translate: the typed character with Ctrl or Cmd, or a named key ─► Vec<Key>
        │  (on macOS, Cmd+Q is taken here as a close request)
        └─► lawh: drop the armed mapping timer, Engine::set_view(view), then for each key:
              └─► lawh-core Engine::feed(key, &mut dyn Measure) -> Vec<Effect>
              │     ├─ key maps: the key joins the typeahead, which expands or waits
              │     ├─ the mode's dispatcher runs whole commands from its key table
              │     └─ a command that changes the Board makes one undo step
              └─► lawh: do the effects, in order
                    ├─ Write { path, quit_after }: one files::WriteBatch, then Engine::write_done
                    ├─ Quit: leave the event loop
                    └─ ArmTimer { ms }: call Engine::timeout after ms, unless a key comes first
  └─► lawh: window title, the view follows the cursor (FOLLOW_MARGIN_PX), redraw
        └─► lawh-render Renderer::render_with(board, viewport, overlay)
              └─► lawh-math: compile changed math blocks (cached by source)
```

The engine is a state machine with no I/O: `(state, board, key) → (state, board changes,
effects)`. Effects are what only the app can do (write a file, quit, run a timer). The engine
never blocks, prints or logs, so tests type the same keys and read the board without a window.

**Logical keys.** The app takes winit's logical key: the character the keyboard layout makes,
with Shift, AltGr and Option already applied, so `:` is `:` on every layout. Alt with a
character is that character (Linux never reports AltGr as Alt, Windows reports it as Ctrl+Alt,
and macOS Option types characters). Only Ctrl and Cmd make chords (`<C-r>`, `<D-s>`). A dead
key composes in Insert mode and on the command line, and is a key of its own elsewhere
(`crates/lawh/src/keys.rs`).

**Measuring.** The app passes its `Renderer` as the engine's `Measure`, so object motions,
Visual mode, jump labels and the place rule see text and math at the size they are drawn, and
the renderer reports what laying text out cost (`Measure::take_work`, see
[What one key may cost](#what-one-key-may-cost)). Tests use `FixedMeasure`: 0.3 units a
character and 0.6 a line, times the object's size.

**The view.** Jump labels go on the objects in view, which only the app knows. Right before each
key and each mapping timeout, the app tells the engine the part of the board the window shows
above the status bar, in grid units (`Engine::set_view`, from `view::board_area`), so `s` sees
the view the user sees, whatever panned, zoomed, resized or followed the cursor since. Before the
first frame, and after a formula's real size makes the app fit the board again, there is no view
(`None`) and every object counts as in view. Labels already showing stay on their objects when
the view moves. Inside one mapping that moves first (`10ls`), `s` labels the view from before the
move, since the view follows the cursor only after the key.

**The mapping timer.** When typed keys only start a longer mapping, `feed` returns
`ArmTimer { ms: timeoutlen }`. The app drops its timer on every key press that makes engine
keys, before `feed`, and arms the new one. When it fires, the app calls `Engine::timeout`,
which runs the waiting keys and never returns `ArmTimer`. A click drops the waiting keys and the
timer.

**Writing.** A key asks for at most `MAX_WRITES_PER_KEY` (4) writes, and the board doesn't
change while the app does them. So the app runs a key's effects through one
`files::WriteBatch`: the board is turned into text once (`to_lawh`), each file is written once,
and every `Write` effect still gets its own `write_done` report, in order. The engine matches
each report to the oldest write it asked for with that path.

- `write_done` marks the board saved when the write went to the board's own file and the board
  hasn't changed since the write was asked for. The app decides "own file" by the files'
  identity, after a good write (`files::same_file`): it follows symbolic links as the atomic
  write does, then compares device and inode on Unix, and canonical paths (`fs::canonicalize`)
  elsewhere, since the standard library has no stable file id on Windows. So `~/a.lawh`, an
  absolute path and a link to the board's file all count, and a hard link the write replaced with
  a new file doesn't. The engine also counts two names that are the same path once `.` and `..`
  are worked out.
- For `:wq` and `:x` it then returns `Quit` when the board has no unwritten changes left, or when
  forced (`:wq!`, `:x!`). Neither the save mark nor the quit happens when the board changed after
  the write was asked for (later keys of the same mapping).
- A write to another file is a copy: the board keeps its name and its unwritten changes. So
  `:wq other` on a changed board writes the copy and doesn't quit unless forced, while on an
  unchanged board it writes the copy and quits.
- `:w name` on an unnamed board names it when `write_done` reports the write went well, so a
  failed write (a missing directory) leaves the board unnamed. Until the report, `:w` alone in
  the same key writes to that name (`:w a.lawh<CR>:wq<CR>` in a mapping). Of several writes that
  would name it, the first to go well does.
- `:x` on a board with no unwritten changes quits at once, without writing.
- A relative path is taken from the directory lawh started in, and a leading `~` (alone, `~/`,
  and `~\` on Windows) from the home directory (`files::write_target`, etcetera's `home_dir`).
  `~user` stays as typed, as does the path of the file lawh opened (the shell expanded any `~`
  in it). The engine and the status line keep the name as typed (`"~/a.lawh" written`).

**Atomic writes** (`files::write_atomic`). An atomic write leaves the file either the old board
or the new one, never half written:

- The board goes to a new file in the same directory (`.area.lawh.<pid>-<n>.tmp`, up to
  `TEMP_NAME_TRIES` names, in case a crash left one behind), is flushed to disk, then renamed
  over the target. On Unix the directory is flushed too, so the rename survives a power cut.
- On Unix the temporary file is created with the old file's permissions, so the board is never
  more readable than it was.
- A symbolic link is written through: up to `MAX_LINK_HOPS` (40, as Linux) links at the end of
  the path are followed, the file they point at is replaced, and the link stays.
- On an error the temporary file is removed and the target is untouched. The status line says
  `cannot write "<path>": <error>`.

**Closing.** The window's close button, the desktop's close shortcut and Cmd+Q on macOS close at
once when the board has no unwritten changes. Otherwise the first request shows
`unsaved changes: :w to save, :q! to discard, or close again`, and a second one closes while that
warning shows. A key, a click on the board or a mapping timeout drops the warning, and with it the
armed close, so the next close warns again.

## The engine

`lawh-core/src/engine/` holds one `Engine` per board. Its module docs are the detailed
reference; this is the map.

### Modes

| Mode | What | Keys from |
|---|---|---|
| `Normal` | The board: motions, operators, puts, undo | `normal.rs` (`MOTIONS`, `OPERATORS`, `NORMAL`) |
| `Visual` | Selecting objects with a rectangle | `visual.rs` (`VISUAL`, plus `MOTIONS`) |
| `CommandLine` | Typing a `:` command | `cmdline.rs` (editing), `command.rs` (`COMMANDS`), `sym.rs` (`:sym` completions) |
| `Insert` | Typing into a text block | `insert.rs` |
| `BlockNormal` | Vim's Normal mode inside a text block (status line `-- TEXT --`) | `block.rs` (`BLOCK`) |

Jump labels are not a mode. They are state on top of Normal mode, Visual mode or a pending
operator (`labels: Option<Labels>`), and while they show, `dispatch` gives every key to them
before the mode sees it (see [Jump labels](#jump-labels)).

In Normal and Visual mode, keys gather in a pending buffer (a count, an operator and its count,
then the command's keys) until they make a whole command or none can. Counts stop at
`MAX_COUNT` (10,000). An operator's count and its motion's multiply, as in Vim (`2d3w` is
`d6w`). The status line shows the pending keys (`Engine::pending`), and while jump labels show,
the keys that showed them and a first label character typed (`ds`, `dsk`).

### The key queue and remaps

Keys from `feed` go into a **typeahead** before the grammar sees them: the queue of keys not yet
run, as in Vim. A **remap** (a key mapping) replaces keys typed in one mode with others, such as
`:nnoremap U <C-r>`. Each of Vim's map modes has its own mappings: `n` (Normal, on the board and
in block-Normal mode), `x` (Visual; `v` means the same in lawh), `o` (operator-pending: `d` or
`y` waiting for a motion), `i` (Insert) and `c` (command line). A key is looked up in the mode
the engine is in when its turn comes, so `inoremap jk <Esc>` works right after `i`.

- When the typeahead's first keys are a mapping's keys, they are replaced by what they map to.
  Keys from `:noremap` and its family aren't mapped again. Keys from `:map` are, except the
  first when the right side starts with the left (Vim's rule: `:nmap l lw` moves right, then
  onto the next object).
- Keys that only start a longer mapping wait for the next key, or for the **mapping timeout**:
  `timeoutlen` milliseconds (1,000 by default, at most `MAX_TIMEOUTLEN`, 10 s). On a timeout
  they take the longest mapping they make, or run as typed. A mapping made with `<nowait>` runs
  at once, without waiting for longer ones.
- `<Leader>` in a mapping is replaced by the `leader` setting at the time the mapping is made
  (`\` by default).
- Each mode's mappings are sorted by their keys, and a lookup narrows them one typed key at a
  time with binary searches: its cost grows with the keys looked at, not with the number of
  mappings. Before that, one crafted key took 82 s in release (measured; see
  [research/m2-measurements.md](research/m2-measurements.md)).
- As in Vim, a `0` that goes on a count and the key after `@` aren't mapped, nor are keys typed
  while jump labels show (as the key after Vim's `f`, `t` or `r`), and `.` replays keys after
  mapping, without mapping them again.

### lawhrc

A lawhrc is a file of `:` commands, one a line, the `:` optional, read once at start by
`Engine::source`, as Vim reads its vimrc. Only `:set` and the mapping commands are allowed. Blank
lines and lines starting with `"` are comments. A line that fails is skipped with an
`RcWarning { line, message }` and the rest load; keys mapped again in a mode warn, and the later
line wins. `source` never emits effects or sets the engine's message: the app shows the first
warning on the status line and logs all of them. Where the file lives and what it may hold:
[keys.md](keys.md#lawhrc).

### Undo

Undo is built from the edits each command makes, not from snapshots of the board:

- A `Step` holds the command's edits in order (`Insert` or `Remove` of a whole object at a
  z-order position, `Replace` of an object's shape) and the cursor before and after. A step made
  inside a text block also keeps the text cursor's byte, so undo puts it back.
- Undo applies a step's edits backwards as one batch (`Board::apply`: all or nothing, the
  board's id index rebuilt once), redo forwards. Undo restores the cursor from before the
  change, redo the one after.
- One step per Normal command, per Insert session (from entering Insert mode to leaving it) and
  per `:` command. A new block still empty when Insert ends leaves no step.
- The history is a straight line, not Vim's undo tree. It keeps `MAX_UNDO_STEPS` (1,000, Vim's
  `undolevels`) and about `MAX_UNDO_BYTES` (16 MiB, estimated from the objects held); past
  either the oldest steps go, never the newest.
- The board is modified when the undo position differs from the saved one, so undoing back to
  the last write makes it unmodified again.

### `.` and `@:`

`.` repeats the last change by replaying the keys that made it: board `d`, `x`, `p`, `P`, `o`,
`O`, `<C-a>`, `<C-x>` and `i a I A` with what was typed, block-Normal `x` and inserts in a block.
`ds{label}` replays `ds` alone, so the labels show again for a new label. A count typed before
`.` replaces the change's own. Board changes repeat only on the board, block `x` only in a block,
and inserts from `i a I A` in either. A `:sym` pick made with `<C-k>` inside an insert is
recorded as the characters it put in. A change keeps at most `MAX_REPEAT_KEYS` (10,000) keys; a
longer insert session can't be repeated. Visual changes aren't repeatable, the size keys in
Visual mode included. `@:` runs the last `:` line again; `:` commands don't go through `.`.

### Sizes

`engine/size.rs`. `<C-a>` and `<C-x>` step the current text or math block along `SIZE_STEPS`
(`0.25 0.5 0.75 1 1.25 1.5 2 2.5 3 4 5 6 8 10 12 16`: from 0.75 up each step is 1.2 to 1.33 times
the one before, so every press looks about as large a change, and 15 presses go from one end to
the other). From a size between steps the first press goes to the next step that way; at an end
the size stays and the message says so. In Visual mode they resize every text and math block of
the selection, each from its own size, as one undo step, and Visual mode stays. `:si N`, `:si +N`
and `:si -N` set, add or take, on the current object or on a selection kept from Visual mode
(below), refused whole when any result is out of range. A resize is an `Edit::Replace` of the
object's shape, so undo needs nothing new.

- **New blocks** take the `size` option, kept in hundredths (`u32`): a float would stop
  `OptionValue` from being `Eq`, and `:set size=` reads its digits without float rounding, at
  most two decimals. `o` and `O` from a text or math block take that block's size.
- **`++size=N`** on `:te` and `:ma` (`command.rs`): only `++name=value` words right after the
  name are options, as Vim's `++opt`, and an unknown one is an error, so `:te size=2` stays text.
- **The selection** is a list of objects, not the rectangle: what is highlighted is what `d`,
  `x`, `y` and `:si` act on. The Visual size keys keep it the same objects, those that grew out of
  the rectangle too; the next motion selects from the rectangle again. `:` from Visual mode keeps
  it while the command line is open (`visual_command`), as Vim's `:'<,'>`, for `:si`.

### Jump labels

`engine/jump.rs`. **Jump labels**, as flash.nvim and hop have them in Neovim: `s` puts a label of
one or two characters on every object in view, and typing a label acts on its object. `s` works
in Normal mode (the cursor moves there), in Visual mode (the cursor moves, stretching the
rectangle) and after `d` or `y` (the operator acts on that object alone and the cursor stays, as
flash's remote `r`; undo puts the object and the cursor back). Counts are ignored.

- **In view:** objects whose bounds overlap the view the app set (`Engine::set_view`, see
  [The view](#data-flow-for-one-key-press)), edges included, except the current object. With no
  view (tests, before the first frame), every object.
- **Order:** distance from the cursor point to the object's anchor, nearest first, ties by rank
  in reading order (which breaks its own ties by z-order).
- **Labels** come from the `jumplabels` option in its order (`LabelChars`, a fixed array of up to
  `MAX_LABEL_CHARS`, so the option stays `Copy` and `Eq`). They are **prefix-free**: no label is
  the start of another, so a label acts as soon as it is typed. With `m` characters, as many of
  the nearest objects as possible get one character while the characters left over, as first
  characters, still give every other object a label of two (`split`); never more than
  `MAX_LABEL_KEYS` (2), so at most `m x m` labels, and the message counts the objects left out.
  500 objects and the default 26 letters: 7 single labels and 19 first letters.
- **State:** `labels: Option<Labels>` holds the labels, the first character typed of a
  two-character label, what typing one does (`Jump::Move`, `Stretch` or `Remote(operator)`) and
  the keys that showed them (`s`, `ds`, for `Engine::pending`). `Engine::jump_labels` gives the
  labels and the typed prefix to the overlay. `<Esc>` ends them, and a pending operator too, as
  `df<Esc>` ends `d` in Vim; a key that starts no label ends them the same way and is dropped
  (`no label "x"`); `<BS>` takes back a typed first character; a click ends them. Keys typed
  while they show skip key mappings and never wait for one.
- **`.`:** `ds{label}` finishes the change with `ds` alone, so `.` shows the labels again and
  never deletes an object the user didn't pick.

### `:sym`

`engine/sym.rs`. `:sy[m] query` puts in a symbol by its Typst name: in a math block the name
(`alpha`, spaced from a name it would touch, since Typst reads letters and digits that touch as
one name), in a text block the character (`α`), on the board a new math block of the name.

- **The list is passed in.** lawh-core never depends on Typst or codex. The app builds the list
  once at start with `lawh_math::symbols()`, which reads codex's static tables (codex is the crate
  Typst takes its `sym` names from, pinned to Typst 0.15.1's version) and checks each value
  against the bundled fonts' character maps, in the app process, without running Typst. It keeps
  the 732 the fonts draw, of 1,189 names, and passes them with
  `Engine::set_symbols(Arc<[Symbol]>)`. The engine keeps up to `MAX_SYMBOLS` usable ones (a name of
  1 to 64 characters with no space or control character, a value with no control character, the
  first of a name given twice), and takes a list already in its order as it is, without a copy.
- **Matching** is **fuzzy** (a name matches when it holds the query's letters in order, not
  necessarily side by side) by `nucleo-matcher`, Helix's matcher, with fzf's syntax (`^`, `$`,
  `'`, `!`), smart case and a bonus for matches near the start of a name. Best first: the exact
  name, then the score, then dictionary order (ignoring case, a small letter before its capital).
  The matcher's scratch memory (about 135 KB) is made the first time it matches.
- **Completions:** while the command line holds a `:sym` line, the engine keeps the best
  `SYM_ROWS` (8) matches and the row selected (`Engine::completions`, for the overlay). Matching
  runs again only when the query changes. `<Tab>` and `<C-n>`, `<S-Tab>` and `<C-p>` move through
  the rows and put the name on the line; past either end the query comes back. `<CR>` takes the
  exact name if typed, else the best match.
- **`<C-k>` in Insert mode** opens a line with `sym ` typed that goes back to the Insert session:
  only `:sym` runs from it, and the pick is typed into the session, so it is part of its undo step
  and `.` types the same characters again.

### What one key may cost

### What one key may cost

Mappings, `.` and counts let one typed key run a great many commands. Four caps bound it, each
counted from the typed key (or timeout, or click) and reset at the next:

- **`MAX_MAP_DEPTH`** (1,000 mapping expansions, Vim's `maxmapdepth`): past it the typeahead is
  dropped and the message says "recursive mapping". It counts until the next typed key, not until
  a command runs, because `:map x yx` runs a command and maps `x` again forever.
- **`MAX_EXPANDED_KEYS`** (30,000 keys set off by mappings, `.` and counts on inserts) and
  `MAX_PENDING_KEYS` (10,000 keys in the typeahead).
- **`MAX_KEY_WORK`** (100,000,000 weighted units, in `work.rs`): commands count their work as
  they go, each kind of work at its own weight (the `*_WORK` constants): objects measured, put
  in reading order, looked at by `H J K L` and the place rule, taken, put, resized
  (`SIZE_WORK`), labelled for a jump (`LABEL_WORK`) and edited by undo, jump labels looked
  through (`PICK_WORK`), symbol names matched for `:sym` (`SYM_WORK`, `SYM_CHAR_WORK`,
  `SYM_CELL_WORK`), fresh ids looked for, bytes of `:` lines run, and bytes of a block's text
  copied, scanned, split into graphemes or walked. A unit is about a nanosecond of work in a debug
  build: each weight was measured with `work_unit_costs` in `tests/engine_cost.rs`. A key that
  runs out takes about 0.1 s in debug and 10 ms in release (50 ms if it was typing, 15 to 22 ms
  if it was resizing or showing jump labels; derived from the measured cost of a unit). Matching
  `:sym` names is the exception: nucleo is a dependency, built optimised in debug too, so a key
  that runs out matching takes 30 ms in release over 4,096 names as long as Typst's, 66 ms over
  names of 64 characters (measured). The costliest single commands fit whole: `10000J` down a
  column of 500 objects is 62.5 million units, `s` over a full board of text 1.3 million, and each
  key of a 64-character `:sym` query over 4,096 names as long as Typst's 32.8 million at most.
- **`MAX_WRITES_PER_KEY`** (4): writes are counted apart, since the app does them and one costs
  it 2.76 ms (measured, build not recorded, mostly the flush to disk).

**Text the renderer lays out.** The engine counts `MEASURE_WORK` for each object and character
it measures: about what the renderer costs for text it has laid out already. Laying out text it
hasn't (one just edited) costs far more, so the measure reports it: `Measure::take_work` returns
the work done since it was last asked, beyond the engine's own count (0 by default, as for
`FixedMeasure`). The renderer charges `KEY_WORK_PER_LAYOUT_NS` (1) unit per estimated nanosecond
of laying out. The engine asks after every measuring (the board's layout, the Visual selection,
placing `o` and `O`, puts) and counts the answer against the key; it asks too when a key, a
timeout or a click starts, and drops that answer, since measuring between keys (for drawing) is
no key's work. Measuring counts once done, so a command may pass the budget by what it measured;
it finishes, and the key stops after it. `Engine::go_first`, on a board just opened, doesn't count
laying the board out: a hostile board's text at the caps is estimated at about 125 million units
(about 70 ms in release). Before laying out counted, a mapping that typed into a text of 3,800
characters and then showed jump labels, or moved `w` and `b`, ran 1,000 rounds in 1.9 s and 6.4 s
in release; now it stops in 46 to 99 ms in release and 163 to 290 ms in debug (measured,
`key_costs` in `lawh-render/tests/measure_cost.rs`).

When a key runs out of work or asks for a fifth write, the command under way stops where the
board is consistent: a motion stops on the last object it reached, an operator or a put does
nothing, an undo with a count stops between steps, and an Insert session that `.` was typing ends
as `<Esc>` would (one undo step). Nothing more runs for that key: the typeahead is dropped and the
message says `that key did too much work; the rest of it was dropped` or `that key asked for more
than 4 writes; the rest of it was dropped`. A `:wq` or `:x` refused this way doesn't quit.

Counts never multiply a command that makes no progress: motions stop where they can't go on,
undo stops at the ends of the history, and a put that doesn't fit is refused whole before the
register is copied.

### Messages

Messages are plain English on the status line, never `E123:` numbers. A message lasts until the
next typed key or click; a timeout runs the waiting keys and keeps what they left. Within one
key, information never replaces an error, while a later error replaces any message, so the
status line says why a key was cut short.

## The canvas cursor

The cursor is either a grid point or an object, never both: `Cursor::Grid(Point)` or
`Cursor::Object(id)`. The object is the **current object**.

- **Anchor:** the point where the cursor stands on an object, where motions start from it and
  where new objects go next to it. Text, math and rectangles: the top-left corner. Ellipses: the
  centre. Lines and arrows: the first end (`x1,y1`).
- **Grid motions** `h j k l` move a grid point 1 unit a step, with counts. From an object they
  start at its anchor, and the cursor becomes a grid point. Every coordinate the engine makes is
  rounded to 0.001 unit (`round3`), so files never show float noise (`0.30000000000000004`), and
  kept within `MAX_COORDINATE`.
- **Object motions** land on an object: `w` and `b` the next and previous in reading order, `gg`
  and `G` the first and last (with a count, the count-th), `H J K L` the nearest object to the
  left, below, above and to the right. From a grid point, the point takes part as an object of
  no size. A motion that finds nothing leaves the cursor where it is.
- **New objects** (the shape commands, `:text`, `:math`, `:sym` on the board, `i a I A` on a grid
  point) appear at the cursor point and become the current object.
- **`e`, `W`, `B`, `E` and text objects** mean nothing on the board yet, so `W B E` are free to
  map. Inside a text block `w`, `b` and `e` are Vim's word motions, and `W`, `B` and `E` its WORD
  motions (runs of characters that aren't blanks).
- **Jump labels** (`s`) reach any object in view in two or three keys (see
  [Jump labels](#jump-labels)).

### Reading order

`w`, `b`, `gg`, `G` and the operators walk objects in reading order: rows from top to bottom,
left to right within a row. Objects are taken by the y of the centre of their bounds. One more
than `ROW_TOLERANCE` (0.5 units) below the first object of the current row starts the next row.
Things drawn side by side are rarely aligned exactly: half a unit keeps a label beside a circle
in its row, while two lines of text 0.6 units apart still read as two rows. Within a row objects
go by their left edge; ties go to the one earlier in the file.

### The nearest object in a direction

`H J K L` consider only objects whose centre lies strictly further in the direction than the
cursor's. First come those whose bounds overlap the cursor's across the direction (sharing more
than an edge), nearest by the gap between facing edges, then by the distance between centres. If
there are none, an object must lie in a cone, its centre at most twice as far across the direction
as along it, scored by the distance along plus twice the distance across. Each step lands
strictly further on, so a count never comes back.

### Operators on objects

`d` (delete) and `y` (yank, copy) need an object cursor; on a grid point the message is "no
object under the cursor".

| Keys | Acts on |
|---|---|
| `dd`, `yy`, `x` | This object and the next count - 1 in reading order |
| `dw`, `d3w` | Exclusive, as in Vim: `dw` is this object, `d3w` this one and the next two |
| `db`, `d3b` | The previous object(s), not this one |
| `dH`, `dJ`, `dK`, `dL` | Inclusive: this object and each object the motion lands on |
| `dgg`, `dG` | This object, the target, and everything between them in reading order |

Grid motions after an operator cancel it. Deleted and yanked objects go to the unnamed register,
in reading order. After a delete the cursor is the grid point at the anchor of the first object
deleted, so `dd` then `p` puts it back where it was. After a yank it is the first object yanked.

### Put and the place rule

`p` and `P` put the register's objects (a **group**). With an object cursor the group goes below
(`p`) or above (`P`) the current object by the place rule. With a grid cursor it is moved so its
first object's anchor is at the cursor. `{count}p` puts count copies, each below the one before.
A put object keeps its id when that id is free, else gets a fresh one for its kind (`t` text,
`m` math, `c` circle, `e` other ellipse, `r` rectangle, `l` line, `a` arrow). A put is all or
nothing: copies past `MAX_OBJECTS` refuse the whole put. The cursor ends on the first object put.
Text deleted inside a block with `x` puts on the board as a new text block.

The **place rule** (`p`, `P`, `o`, `O`) finds where a group of size S goes next to a reference R,
the current object's bounds (or a box of no size at a grid point):

1. Below: `x = R.x0`, `y = R.y1 + PLACE_GAP` (0.5 units). Above: `y = R.y0 - PLACE_GAP - S.height`.
2. While the candidate box overlaps other objects, move it past them: below, to `PLACE_GAP`
   under the lowest bottom among them; above, the mirror image. Then try again.
3. Each try rounds y outward, away from R, to a multiple of 0.1, so files show `y=4.3` and the box
   never moves back into what it just cleared; x is rounded with `round3`. The box tested is the
   box placed.

Boxes that only touch don't overlap, unless one is flat (a level or upright line), so a group
never ends up lying on a line. Half a unit reads as "next to" at any zoom where the objects are
legible, and keeps their bounds apart so `w`, `J` and `K` still tell them apart. The rule ends
within one try per object; past `MAX_COORDINATE` it fails with "no room".

### Visual mode

`v` starts Visual mode. Its anchor is the cursor's region when `v` was pressed (a point, or an
object's bounds), and the rectangle is the bounding box of the anchor and the cursor's current
region. Selected are the objects whose bounds lie fully inside it, edges included. Motions,
counts and jump labels move the cursor; `d`, `x` and `y` act on the selection and return to
Normal mode; `<C-a>` and `<C-x>` resize its text and math and stay in Visual mode; `v` or `<Esc>`
leave; `:` leaves Visual mode and opens the command line, keeping the selection for `:si`. The
selection is what the window highlights and what `d`, `x`, `y` and `:si` act on (see
[Sizes](#sizes)).

### Clicks

A click puts the cursor on the object under the pointer, or on the nearest whole grid point. An
object is hit where it is drawn, within `HIT_TOLERANCE_PX` (4 logical pixels): a line near its
segment, an ellipse inside its shape, a text in its box. A text or formula wins over a shape, then
the smallest area, then the topmost. A click ends jump labels, an Insert session (one undo step),
block-Normal mode, Visual mode and the command line, and drops pending keys. A press that moves
more than `CLICK_SLOP_PX` (4 logical pixels) pans instead.

Still open: text objects on the board (`ic` for "inside circle"?), and drawing shapes from a
Visual rectangle.

## Text editing inside blocks

Text and math objects are text blocks. The engine edits their text itself (`engine/text.rs`,
`insert.rs`, `block.rs`, `text_motion.rs`). The renderer only places the caret
(`Renderer::caret_rect`, from parley's cursor geometry); parley's `PlainEditor` isn't used.

- **Entering:** on a grid point, `i a I A` start a new text block there in Insert mode, and `o`
  and `O` start one below or above the cursor by the place rule. On a text or math object, `i`
  and `I` insert at the start, `a` and `A` at the end, and `<CR>` enters block-Normal mode at the
  start; `o` and `O` start a new block below or above it. Other kinds have no text: "only text and
  math have text to edit".
- **Size of a new block:** the `size` option, except that `o` and `O` from a text or math block
  take that block's size, as do the further blocks a count on them makes.
- **Live board:** a new block is on the board, empty, from the moment Insert mode starts, and every
  edit replaces the block on the board, which checks the caps on text. So the board always shows
  the live text. A new block still empty when Insert ends is taken off again with no undo step.
- **Graphemes:** a step is one grapheme (an extended grapheme cluster, what a reader sees as one
  character), from `unicode-segmentation`. `é` typed as `e` and a combining accent, or a family
  emoji joined with zero-width joiners, moves and deletes as one. Offsets stay on grapheme
  boundaries, and `j` and `k` keep the column counted in graphemes. Motions look only around the
  cursor; those that step back more than once split the line into graphemes once.
- **Insert mode:** typed characters (Alt with a character types it), `<CR>` a line break, `<Tab>`
  a tab, `<BS>` and `<Del>`, the arrows (left and right stay in the line), `<Home>` and `<End>`.
  `<C-k>` opens a `:sym` line (see [`:sym`](#sym)); other Ctrl and Cmd chords type nothing.
- **Block-Normal mode:** `h l j k 0 $ w b e W B E` with counts, `x` (into the unnamed register),
  `i a I A`, `u`, `<C-r>`, `.` and `:`. `<Esc>` goes back to the board with the block as the
  current object. The command line comes back to the block unless its command moved the cursor off
  it.
- **Where `<Esc>` goes** from Insert mode is the `escape` setting: `board` (the default) goes
  back to where Insert mode was entered from, the board with the block as the current object or
  block-Normal mode; `block` always goes to block-Normal mode, and `<Esc>` there goes on to the
  board.
- A math block being edited is drawn as its source. Once the caret leaves, it compiles again,
  drawn as a grey placeholder until it is ready.
- One glyph per character (above) keeps the cursor on what is typed. Right-to-left text is out of
  scope for v1, and the bundled Inter has no Arabic glyphs.

## Portability rules

Linux first, with releases for macOS and Windows from the same code:

- Vim keys match the typed character (see [logical keys](#data-flow-for-one-key-press)). App
  shortcuts will go through one modifier setting (Ctrl, or Cmd on macOS); the only one today is
  Cmd+Q on macOS, a close request. (Planned.)
- The config directory comes from `etcetera` (`choose_app_strategy`): the lawhrc is
  `~/.config/lawh/lawhrc` on Linux and macOS (XDG, `XDG_CONFIG_HOME` honoured) and
  `%APPDATA%\y0sif\lawh\config\lawhrc` on Windows. No hard-coded `/` in paths.
- The window scale factor is handled from the start (Wayland fractional scaling needs it on Linux).
- `.lawh` files may use CRLF line endings; errors point at the same place as with LF.
- The math worker's memory cap holds on Linux only so far (see [../SECURITY.md](../SECURITY.md)).
