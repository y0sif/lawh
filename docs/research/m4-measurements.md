# Milestone 4 measurements (2026-10-25)

The numbers behind the limits and design choices of milestone 4 of the lawh whiteboard: export to
SVG, PNG and PDF (`:export` in the window, `lawh export` without one), from a draw list the window
shares. Each figure is marked:

- **measured**: timed or counted on the machine below.
- **derived**: computed from the code's constants or other figures, not timed.
- **target**: a limit a test asserts.

Every measured figure here was measured again for this document, on 2026-10-25, unless it names
another source: the doc comment of a constant, or a test that prints it. The milestone's lane
logs and research notes are not in the repo; where a number comes from them through a doc
comment, the comment is named.

## Machine and conditions

| | |
|---|---|
| CPU | Intel Core Ultra 9 185H (22 threads) |
| RAM | 14 GB |
| OS | Arch Linux, Hyprland (Wayland), Linux 7.2.9 |
| Rust | 1.99.0 (stable) |
| Date | 2026-10-25, 08:28 to 08:53 |
| Build | release unless a row says debug (the dev profile, `opt-level = 2` for dependencies) |
| Load | the one-minute load average, given with each section; other sessions were running |

As in [m1-measurements.md](m1-measurements.md), timings move with machine load. Peak memory is
the test process's `VmHWM` (its peak resident memory) after a reset, fonts included, or for
`lawh export` the main process's, sampled while it runs. "Mpx" is millions of pixels.

## Exports: time, file and memory

The boards: `examples/area.lawh` (text, a circle, a line and a formula); `limits_board` (the
tests' board at lawh-core's caps: shapes a million units across at coordinates of plus and minus
1,000,000, a text of 5,000 characters on one line, one of 500 lines, and a formula at the far
edge);
"hostile 500" (250 texts of 160 characters spread over the whole coordinate range and 250 circles
a million units across); 40,000 characters (250 texts of 160, the board's text cap); 500 formulas
of 22,500 glyphs each (under lawh-math's 25,000); and 500 formulas that fail, compiled as part of
the export (in process: the worker would add its round trips). Math is compiled and text laid
out before the clock starts, except on the failing formulas' row. Times are three runs.

### PNG

`measure_exports` in `crates/lawh-render/tests/export.rs`, `--test-threads=1`, 08:28:06 to
08:28:25, load average 1.7 to 1.9.

| Board, `pngscale` | Image | File | Time | Peak | Kind |
|---|---|---|---|---|---|
| area, 2 | 970 x 802 | 32,012 B | 9.9 to 41 ms (41 ms the first run) | 27 to 28 MB | measured |
| area, 8 | 3,879 x 3,208 | 322,241 B | 87 to 94 ms | 75 to 76 MB | measured |
| `limits_board`, 2 or 8, shrunk to the pixel cap | 8,763 x 7,303 (64.0 Mpx) | 945,953 B | 394 to 410 ms | 282 to 283 MB | measured |
| hostile 500, 2 or 8, shrunk | 11,301 x 5,662 (64.0 Mpx) | 1,095,847 B | 683 to 724 ms | 320 to 324 MB | measured |
| 500 failing formulas, cold, 2 | 11,857 x 3,249 (38.5 Mpx) | 2,936,726 B | 331 to 345 ms | 184 to 185 MB | measured |

At the pixel cap the peak is the 256 MB image (4 bytes a pixel, derived) plus about 26 to 68 MB:
the image is moved into the encoder, never copied. Images past 2 Mpx are compressed with fdeflate
(see [PNG compression](#png-compression)), which is why the area board at `pngscale` 8 is
322 KB, not zlib's 163 KB.

### SVG

`measure_svg_exports` in `crates/lawh-render/tests/export_svg.rs`, 08:28:25 to 08:28:29, load
average 1.9. Each run times three exports; the time column covers all nine.

| Board | Time | File | Glyphs drawn / defined | Peak | Kind |
|---|---|---|---|---|---|
| area | 0.1 ms | 7,413 B | 19 / 14 | 23 to 24 MB | measured |
| 40,000 characters | 4.5 to 6.4 ms | 1,219,437 B | 34,500 / 16 | 28 MB | measured |
| `limits_board` | 1.3 to 1.5 ms | 337,635 B | 8,080 / 17 | 27 MB | measured |
| hostile 500 | 6.7 to 7.2 ms | 1,979,077 B | 34,500 / 16 | 29 MB | measured |
| 500 formulas of 22,500 glyphs | 101 to 102 ms, then `export stopped: the file is over 64 MiB` | | | 98 MB | measured |
| 500 failing formulas, cold | 47 to 51 ms | 194,247 B | 4,390 / 16 | 32 MB | measured |

About 35 bytes a glyph drawn (1,219,437 B for 34,500 glyphs, derived). Writing up to the byte cap
peaks at the 64 MiB file plus about 30 MB.

### PDF

`measure_pdf_exports` in `crates/lawh-render/tests/export_pdf.rs`, 08:28:29 to 08:28:36, load
average 1.9. Three runs of three exports each.

| Board | Time | File | Shrunk to | Peak | Kind |
|---|---|---|---|---|---|
| area | 0.8 to 1.0 ms | 9,930 B | | 26 MB | measured |
| 40,000 characters | 14.2 to 16.4 ms | 12,460 B | 0.96 | 27 MB | measured |
| `limits_board` | 4.8 to 6.9 ms | 14,514 B | 0.00016 | 27 MB | measured |
| hostile 500 | 18.0 to 19.6 ms | 66,416 B | 0.00012 | 27 MB | measured |
| 500 formulas of 22,500 glyphs | 1,094 to 1,122 ms, then `export stopped: the file is over 64 MiB` | | | 154 to 155 MB | measured |
| 500 failing formulas, cold | 57 to 61 ms | 10,853 B | | 32 MB | measured |

A PDF counts its uncompressed content against the byte cap while it draws, since krilla builds
the file in memory, so the 500 formulas stop at 64 MiB of content though their file would be far
smaller: 20 such formulas are 28.5 MB of content and a 1.55 MB file (`calibrate_pdf`, below).

### Why `MAX_PDF_PAGE_PT` is 14,400

From the doc comment of `MAX_PDF_PAGE_PT` (`crates/lawh-render/src/export/mod.rs`), measured on
scratch PDFs during M4's research (not in the repo): 14,400 pt is PDF 1.x's implementation limit
for a page side (ISO 32000-1, Annex C, which Acrobat keeps). Reader warns that a larger page is
out of range, Ghostscript fails on a page of 1.2e8 pt, and PDF/A-2 refuses one (veraPDF rule
6.1.13-11), though krilla writes any size. A board up to 480 units across, margin included, keeps
30 pt a unit (derived: 14,400 / 30).

## PNG compression

`calibrate_png_encoding` in `crates/lawh-render/src/export/png.rs`, 08:29:16 to 08:30:54, load
average 1.8 to 2.1. Nanoseconds a byte of image data (3 bytes a pixel, 4 with transparency, one
a row), the image's copy taken off; medians of 1 to 5 runs.

| Content | zlib (`Compression::Balanced`) | fdeflate (`Compression::Fast`) | Kind |
|---|---|---|---|
| Uniform noise, 0.25 to 64 Mpx, RGB and RGBA | 28.6 to 32.8 ns | 3.8 to 6.1 ns | measured |
| Random pixels of 3 values a channel, 2,000 x 2,000 (zlib's worst found) | 79.4 ns | 4.2 ns | measured |
| 2 random bits a channel; 1 random bit | 63.3 ns; 56.2 ns | 4.2 ns; 5.3 ns | measured |
| 4 colours | 43.6 ns | 5.2 ns | measured |
| Six scribbles of 2,000 lines across an image at the pixel cap (8,023 x 7,976) | 23.1 ns (4.42 s) | 4.7 ns (0.90 s) | measured |
| Noise at 8,000 x 8,000 | 5.58 s RGB, 7.37 s RGBA | 0.92 s RGB, 1.17 s RGBA | measured |
| The area board at `pngscale` 2 and 8 | 2.0 to 2.9 ns | 1.2 to 1.3 ns | measured |
| Weight charged (`PNG_NS_PER_BYTE_BALANCED`, `PNG_NS_PER_BYTE_FAST`) | 100 ns | 6.5 ns | target |
| Most any image took of its estimate | 0.79 | 0.94 | measured; 1.2 plus 0.5 ms is the target |

What it means for the caps:

| What | Figure | Kind |
|---|---|---|
| zlib's worst case at the pixel cap | about 15 s (192 MB of image data at 79 ns a byte) | derived |
| fdeflate's charge at the pixel cap | 1.25 s, 1.66 s with transparency | derived (`PNG_NS_PER_BYTE_FAST` x the image data) |
| zlib's charge at `PNG_BALANCED_MAX_PIXELS` (2,000,000) | 0.6 s, 0.8 s with transparency | derived |
| The area board at `pngscale` 2 (0.78 Mpx), zlib and fdeflate | 32,012 B and 47,187 B; 6.8 ms and 2.8 ms | measured |
| The area board at `pngscale` 8 (12.4 Mpx) | 162,758 B and 322,241 B; 77 ms and 48 ms | measured |
| The six scribbles | 61.1 MB and 78.2 MB | measured |

So zlib stays only where its worst case fits the budget with room to draw: up to 2 Mpx, which
holds every everyday board (the area board at `pngscale` 2 is 0.78 Mpx). Past it files are
larger: 1.3 to 2 times as large as zlib's in the rows above.

`measure_png_peak` (same file, 08:28:36): noise at the pixel cap, compressed with fdeflate through
the byte cap, which stops it.

| Image | Time | Peak | Image alone | Stopped at | Kind |
|---|---|---|---|---|---|
| 8,000 x 8,000 RGB | 360 ms | 316 MB | 252 MB | 67,111,978 B | measured |
| 8,000 x 8,000 RGBA | 328 ms | 316 MB | 252 MB | 67,111,978 B | measured |

The encoder holds `PNG_CHUNK_BYTES` (256 KiB) of compressed data before it writes it, so it
stops within a chunk of the cap (67,108,864 B) and never holds a second copy of the file.

## SVG and PDF work weights

Each writer charges what it writes to the export's budget before it writes it, with weights fitted
by a calibration that times the pieces of a file and then whole boards. Run 08:28:47 to 08:29:16,
load average 2.0 to 2.1.

### `calibrate_svg` (`crates/lawh-render/src/export/svg.rs`)

| Piece | Time | Kind |
|---|---|---|
| A cubic segment of 12-digit coordinates | 119 ns | measured |
| An arrowhead (an element of 4 segments) | 175 ns | measured |
| A glyph used (5,000 in one run) | 98 ns | measured |
| A glyph defined (all 7,351 glyphs with outlines of both fonts, 28.6 segments on average) | 3.6 µs (26.5 ms in all) | measured |

| Board | Estimate | Took | Ratio | File | Kind |
|---|---|---|---|---|---|
| area | 0.119 ms | 0.042 ms | 0.35 | 7,413 B | measured |
| 250 texts of 160 characters | 6.70 ms | 4.55 ms | 0.68 | 1,219,437 B | measured |
| Distinct glyphs | 9.49 ms | 3.54 ms | 0.37 | 523,490 B | measured |
| 500 circles a million units across | 6.40 ms | 4.08 ms | 0.64 | 1,479,851 B | measured |
| 500 arrows | 2.05 ms | 0.20 ms | 0.10 | 92,873 B | measured |
| 500 formulas | 6.37 ms | 4.76 ms | 0.75 | 1,288,100 B | measured |
| 500 failing formulas | 1.93 ms | 1.03 ms | 0.53 | 193,897 B | measured |
| Allowed | | | 1.2 plus 0.2 ms | | target |

### `calibrate_pdf` (`crates/lawh-render/src/export/pdf.rs`)

| Piece | Drawn | Finished (at the end of the file) | Content | Kind |
|---|---|---|---|---|
| A file with one stroke | 27.2 µs | 10.2 µs | 89 B | measured |
| A cubic segment, filled or stroked | 1.45 to 1.55 µs | 5 ns | 61 B | measured |
| A line segment, stroked | 0.52 µs | 2 ns | 22 B | measured |
| An arrowhead (4 segments) | 0.99 µs | 5 ns | 102 B | measured |
| A text glyph (5,000 in one run) | 0.37 µs | 0.10 µs | 8.5 B | measured |
| A text run of 50 characters | 17.6 µs | 1.0 µs | 516 B | measured |
| A math glyph used (22,500 of one formula) | 0.86 µs | 3 ns | 44 B | measured |
| A math glyph defined and used (every glyph of Libertinus Math; of Inter) | 40.7 µs; 43.0 µs | 1.6 µs; 1.4 µs | 627 B; 858 B | measured |
| A font subset of one glyph (Inter; Libertinus Math) | 29.5 µs; 28.2 µs | 216 µs; 141 µs | 124 B | measured |
| A text glyph subset (2,053 distinct glyphs) | 0.74 µs | 5.0 µs | 7.9 B | measured |

| Board | Estimate | Took | Ratio | Content estimate / written | File | Kind |
|---|---|---|---|---|---|---|
| area | 0.91 ms | 0.73 ms | 0.79 | 1.72 | 9,930 B | measured |
| 250 texts of 160 characters | 26.1 ms | 14.4 ms | 0.55 | 1.94 | 12,460 B | measured |
| Distinct glyphs | 8.66 ms | 7.85 ms | 0.91 | 1.29 | 101,373 B | measured |
| 500 circles a million units across | 12.9 ms | 14.6 ms | 1.13 | 1.07 | 81,743 B | measured |
| 500 arrows | 7.47 ms | 2.26 ms | 0.30 | 1.19 | 13,890 B | measured |
| 500 formulas | 25.1 ms | 12.4 ms | 0.49 | 1.26 | 60,091 B | measured |
| 500 formulas in 500 colours | 89.9 ms | 56.7 ms | 0.63 | 1.83 | 784,384 B | measured |
| 80,000 dots in their own colours | 2,597 ms | 2,404 ms | 0.93 | 1.83 | 31,137,195 B | measured |
| 500 failing formulas | 7.13 ms | 4.06 ms | 0.57 | 1.37 | 13,060 B | measured |
| 20 formulas of 22,500 glyphs | 809 ms | 496 ms | 0.61 | 1.14 | 1,553,594 B | measured |
| Allowed | | | 1.2 plus 0.5 ms | at least 1 | | target |

Numbers that repeat each other's digits compress slowest, so the circles come closest to their
estimate. Each math glyph is a form XObject (a drawing stored once in the file and drawn by
reference); writing each glyph's outline where it is used instead took 47 µs and 1,644 bytes a
glyph, a 7.2 MB file for one formula of 22,500 glyphs, against 73 KB (the module doc of
`export/pdf.rs`, measured during M4).

## `lawh export` from the command line

`target/release/lawh export`, built 2026-10-25 08:26 from HEAD `11c3716`, run 15 times after one
warm-up, wall time from the shell (`date +%s%N` around each run): process start, reading the
board, starting the math worker, compiling the formula, drawing, writing and exiting. 08:30:54,
load average 1.8.

| Board | SVG | PNG | PDF | Kind |
|---|---|---|---|---|
| `examples/area.lawh` | median 20.2 ms (18.8 to 24.5), 7,413 B | median 33.2 ms (30.7 to 53.3), 32,012 B | median 21.5 ms (20.0 to 26.7), 9,930 B | measured |
| The same without its formula (no worker starts) | 10.8 ms (9.9 to 11.7), 4,610 B | 19.6 ms (18.1 to 21.6), 27,063 B | 12.5 ms (11.1 to 15.0), 7,319 B | measured |
| `lawh --version` | 5.0 to 5.7 ms | | | measured |

So starting the worker and compiling the formula take about 9 to 14 ms of each export (derived
from the medians).

### Hostile boards

Boards built for M4's cost review (generated, not in the repo), each exported twice with the same
binary through a script that samples the main process's peak memory; 08:30:56 to 08:31:48, load
average 1.5 to 1.8, and 08:48, load 1.0, for the two scribbles.

| Board | Exit | Wall | Peak | Result | Kind |
|---|---|---|---|---|---|
| One scribble of 2,000 lines at size 16, PNG | 0 | 1.37 to 1.43 s | 344 MB | written, shrunk to 0.48x to fit, 55.4 MB | measured |
| Two scribbles, PNG | 0 | 1.85 s | 394 to 395 MB | written, shrunk to 0.48x, 64.8 MB | measured |
| Six scribbles, PNG | 1 | 0.87 s | 377 MB | `export stopped: drawing it costs too much` | measured |
| Eight scribbles, PNG | 1 | 0.85 to 0.90 s | 376 to 377 MB | the same | measured |
| Eight scribbles at `--scale 0.5` | 0 | 1.67 s | 212 MB | written, 30.7 MB | measured |
| Six scribbles and five formulas of a long loop, PNG | 1 | 8.59 to 8.62 s | 376 MB | `export stopped: drawing it costs too much`, after waiting for the formulas | measured |
| Eight formulas of a long loop, PNG | 1 | 10.03 to 10.04 s | 14 MB | `export stopped: math took over 10 s to compile` | measured |
| Four formulas of 20,000 dots, each dot its own colour, PDF | 1 | 2.74 to 2.79 s | 157 MB | `export stopped: drawing it costs too much` | measured |
| Two circles at x = -1,000,000 and 1,000,000, SVG | 0 | 14 to 41 ms | 14 MB | written: `width="16384" height="1" viewBox="0 0 80000162 162"`, shrunk to 0.0002x | measured |

`rsvg-convert` and ImageMagick (`magick`) both draw that SVG, 16,384 x 1 pixels. The same file
with its `width` and `height` set back to the `viewBox` (80,000,162 x 162), as SVG exports were
before the side cap applied to them, `rsvg-convert` refuses: "The resulting image would be larger
than 32767 pixels on either dimension" (measured at 08:53; ImageMagick not tried on it).

The math wait's stop is on time: stderr closes with the exit (the worker is gone), and an export
with a wait of 200 ms and a formula of about a second stopped after 200.1 ms
(`the_math_wait_ends_on_time` in `crates/lawh-render/tests/export.rs`; the doc comment of
`EXPORT_MATH_WAIT`). Dropping a math engine while its worker compiles took 1.7 to 2.4 ms over 4
runs, the worker killed and reaped (`dropping_the_engine_mid_compile_kills_and_reaps_the_worker_at_once`
in `crates/lawh-math/tests/worker_process.rs`, release, 08:38 to 08:48).

## The window on the shared draw list

Milestone 4 moved the window onto the draw list exports use (`crates/lawh-render/src/scene.rs`).

| What | Figure | Kind |
|---|---|---|
| A window frame at zoom 1 against a PNG export at `pngscale` 1, 662 x 445 | 0 of 292,754 pixels differ (1,836 around grid dots left out, 8,902 not paper) | measured (`tests/scene.rs`, debug; pixels don't depend on the build) |
| resvg's drawing of an SVG export against the PNG export, 227,652 pixels | 0 apart inside shapes, glyphs and paper; at most 56 a channel on 9,332 edge pixels | measured (`tests/export_svg.rs`, debug); allowed 2 and 64 (target) |
| hayro's drawing of a PDF export against the PNG export | 0 inside; at most 54 on edges, 54 pixels past 32 | measured (`tests/export_pdf.rs`, debug); allowed 2 and 64 (target) |
| The window's goldens | unchanged since milestone 3 | measured (`git diff d9ee554 HEAD` touches only the two new `export_*` goldens) |

**Speed.** `crates/lawh-render/tests/bench.rs` (unchanged by M4) built from the tree before M4
(`d9ee554`, in a target directory of its own) and from HEAD, run three times each, interleaved,
the old tree first; 08:38:45 to 08:42:25, load average 1.7 to 3.2.

| Bench | Before M4 | HEAD | Kind |
|---|---|---|---|
| All 589 timings the bench prints, summed | 3,937 to 4,036 ms | 3,953 to 4,023 ms | measured |
| The 484 timings of 1 ms or more, least of three runs, HEAD over before | median 1.000, geometric mean 0.999 | | measured |
| `area_1080p_frame_time`, steady median | 0.98 to 1.23 ms | 0.90 to 1.14 ms | measured |
| `frame_budget_worst_cases`, worst frame (target 33 ms) | 22.9 to 23.5 ms | 21.4 to 23.6 ms | measured |
| The same, worst open (target 100 ms) | 64.0 to 74.2 ms | 63.8 to 66.0 ms | measured |
| `board_objects_frame_time`, 500 objects fitted, medians: circles of radius 1 | 18.4 to 20.2 ms | 18.4 to 20.1 ms | measured |
| The same: arrows; rectangles; text lines | 17.3 to 19.2; 14.8 to 16.3; 12.0 to 13.6 ms | 17.3 to 19.0; 14.8 to 16.4; 11.8 to 13.1 ms | measured |
| `limits_frame_time`, 7 views at zoom 20, medians summed | 11.0 to 37.1 ms (37.1 one run) | 11.3 to 11.5 ms | measured |
| `overlay_frame_time`, 500 labels: 1920x1080; 2880x1800 | 3.95 to 4.32; 5.77 to 6.91 ms | 3.94 to 4.00; 5.76 to 5.79 ms | measured |

**Calibrations at HEAD** (the frame budget's, `renderer::calibrate`, 08:43:31 to 08:46:42, load
average 2.1 to 2.7): all 8 pass. The worst ratio of measured time to estimate among rows
estimated at 1 ms or more, each allowed 1.2 (plus 0.2 ms):

| Calibration | Worst ratio | Rows | Kind |
|---|---|---|---|
| `calibrate_estimating` | 0.73 | 1 | measured |
| `calibrate_formulas` | 1.06 | 66 | measured |
| `calibrate_glyphs` | 0.88 | 126 | measured |
| `calibrate_paths` | 0.94 | 95 | measured |
| `calibrate_runs` | 0.61 | 16 | measured |
| `calibrate_text_drawing` | 0.91 | 1,163 | measured |
| `calibrate_text_layout` | 0.79 | 118 | measured |

## The engine's side

`work_unit_costs` and `key_costs` in `crates/lawh-core/tests/engine_cost.rs`, 08:32:21 to
08:32:44, load average 1.2 to 1.3. `work_unit_costs` times each command once.

| What | Release | Debug | Units | Kind |
|---|---|---|---|---|
| `<CR>` on `:exp out.png` over 500 texts | 2.5 µs | 17.0 µs | 5,044 | measured |
| The same with nothing selected (`nothing to export`) | 1.3 µs | 8.8 µs | 44 | measured |
| `<CR>` on `:exp out.png` from Visual mode with 500 texts selected | 57.5 µs | 210 µs (1.2 ns a unit) | 180,044 | measured |
| `<CR>` on `:te x` below the top of a column of 499 texts | 0.76 ms | 5.1 ms (0.8 ns a unit) | 6,087,856 | measured |
| `EXPORT_WORK`; `EXPORT_OBJECT_WORK` | | | 5,000; 350 an object | target (the weights) |
| A mapping of `:exp a.png` that runs itself, 500 texts | 92 µs, stopped at the second export | | | measured (`key_costs`) |
| A mapping of `:te x` below the current text that runs itself, 250 texts | 10.6 ms, out of work | | | measured (`key_costs`) |

The weights were fitted in debug, averaged over 2,000 keys (the doc comments of `EXPORT_WORK` and
`EXPORT_OBJECT_WORK` in `crates/lawh-core/src/engine/work.rs`): `<CR>` on `:exp out.png` over 500
texts 4.2 to 4.6 µs a key, the line's reading included, of which a refused `<CR>` is 4.1 to
4.4 µs; with 500 texts selected 338 ns an object.

## Build cost

The five crates krilla brings, rebuilt alone in the dev profile (`cargo clean -p` of the five, then
`cargo build -p lawh-render --timings`, which also rebuilt lawh-math and lawh-render), 08:33:04 to
08:33:38, load average 1.3 to 2.5, 4 jobs:

| Crate | Compile time | Kind |
|---|---|---|
| `write-fonts` 0.48.1 | 33.3 s | measured |
| `krilla` 0.8.2 | 15.1 s | measured |
| `subsetter` 0.2.6 | 9.1 s | measured |
| `pdf-writer` 0.15.0 | 4.5 s | measured |
| `xmp-writer` 0.3.3 | 1.4 s | measured |
| All five | 63.4 s of compile time, 33.5 s of wall time for the whole rebuild | measured |

How much they add to a cold build of the whole workspace, where they compile beside Typst, was
not measured.

## The acceptance test

| What | Figure | Kind |
|---|---|---|
| Keys to type `examples/area.lawh` from an empty board and write it | 62, unchanged | target, asserted by `the_area_example_in_62_keys` in `crates/lawh-core/tests/acceptance.rs` |
