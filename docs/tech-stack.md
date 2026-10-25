# Tech stack

Why each dependency, and what it beat. Versions are the ones in `Cargo.lock` on 2026-10-25, and
licences and minimum Rust versions come from each crate's own `Cargo.toml`. Every new crate goes
here and must pass `cargo deny check` (see [deny.toml](../deny.toml)).

## In use

| Job | Crate | Version | Licence | Used by | Why |
|---|---|---|---|---|---|
| Geometry | `kurbo` | 0.13.1 | Apache-2.0 OR MIT | core, math, render | Points, rectangles, Bézier paths and transforms. Must stay the version `vello_cpu` re-exports. |
| Library errors | `thiserror` | 2.0.21 | MIT OR Apache-2.0 | core, math, render | Typed error enums callers can match on. |
| Graphemes | `unicode-segmentation` | 1.13.3 | MIT OR Apache-2.0 | core | Splits a block's text into graphemes (what a reader sees as one character), so the cursor moves and `x` deletes `é` with a combining accent, or a family emoji, as one. Its `GraphemeCursor` finds the boundary next to a byte without splitting the whole text. Already in the tree through Typst and winit. |
| Symbol search | `nucleo-matcher` | 0.3.1 (pinned `=`) | MPL-2.0 | core | Fuzzy matching (a name matches when it holds the query's letters in order) of symbol names for `:sym`: Helix's matcher, with fzf's query syntax and smart case. Only the matcher, not `nucleo` 0.5, which brings threads and rayon. It adds only `memchr` to lawh-core (it uses `unicode-segmentation` too, already there). Pinned exactly: the ranking `:sym` shows, and its tests, follow its scores. Matching over Typst's 1,142 `sym` names took 25 to 75 µs a query in release (measured while choosing it). |
| Logging | `tracing` | 0.1.44 | MIT | math, render, app | Structured logs, the Rust standard. |
| Log output | `tracing-subscriber` (`env-filter`) | 0.3.23 | MIT | app | Prints logs to stderr, filtered by `RUST_LOG`. |
| App errors | `anyhow` | 1.0.104 | MIT OR Apache-2.0 | app | Context chaining at the top level. |
| Math | `typst` | 0.15.1 (pinned `=`) | Apache-2.0 | math | Real math layout from readable syntax (`pi r^2`), pure Rust. Pinned exactly: lawh-math walks its frame types, which change between releases. |
| Math output | `typst-layout` | 0.15.1 (pinned `=`) | Apache-2.0 | math | `PagedDocument`, the compiled page lawh converts to paths. |
| Symbol names | `codex` (no default features) | 0.3.0 (pinned `=`) | Apache-2.0 | math | Typst's own table of symbol names (`sym`, with `gender` and `control`), read for `:sym`'s list without running Typst. Pinned to the version Typst 0.15.1 takes them from, so every name in the list is one the worker compiles. No new crates: Typst already brings it. |
| Glyph outlines | `ttf-parser` (no default features) | 0.25.1 | MIT OR Apache-2.0 | math, render | Typst's own font type. Reads glyph outlines and bounds for math, and outline statistics for the frame budget. |
| Worker memory cap | `rlimit` | 0.11.0 | MIT | math (Unix only) | Caps the math worker's address space (`RLIMIT_AS`) and turns off its core dumps, without `unsafe` code of our own (the workspace denies it). |
| Drawing | `vello_cpu` | 0.3.0 | Apache-2.0 OR MIT | render | Vello's most mature renderer by its own README; draws on the CPU, headless. |
| Text layout | `parley` (`std`, `bytemuck`; no default features) | 0.12.0 | Apache-2.0 OR MIT | render | Shaping, bidi, line breaking. The `system` feature is off: lawh never loads system fonts, and it would need fontconfig. |
| PDF export | `krilla` (no default features) | 0.8.2 (pinned `=`) | MIT OR Apache-2.0 | render | The PDF library Typst's own PDF export uses: pages, paths, form XObjects, and fonts embedded as subsets with a `ToUnicode` map, so board text in a PDF can be searched and copied. Its default features (raster images, simple text shaping) are off: lawh draws no images, and its glyphs come shaped from parley. It brings 4 more crates, all MIT OR Apache-2.0: `pdf-writer` 0.15.0, `subsetter` 0.2.6, `write-fonts` 0.48.1 and `xmp-writer` 0.3.3. It embeds its own compact sRGB ICC profile (from Compact-ICC-Profiles; the profile's copyright tag says CC0), which lawh asks for so a PDF's colours are sRGB, as the screen's are. Minimum Rust 1.92. The five crates took 63 s of compile time in the dev profile, 33.5 s of wall time rebuilt alone ([research/m4-measurements.md](research/m4-measurements.md)). |
| PNG export | `png` | 0.18.1 | MIT OR Apache-2.0 | render | The encoder `vello_cpu`'s own PNG support uses, taken directly so a file can carry an sRGB chunk and be streamed through the export's byte cap. No new crate: it was in the tree through `vello_cpu`. |
| Script and bidi data | `parley_data` | 0.12.0 | Apache-2.0 OR MIT | render | Each character's script and bidi class from the tables parley lays text out with, so the frame budget counts where parley starts a new run. Already in the tree through parley. |
| Paired brackets | `icu_properties` (`compiled_data`; no default features) | 2.3.0 | Unicode-3.0 | render | Which characters are paired brackets, for the bidi bracket charge in the frame budget. Already in the tree through parley. |
| Byte casts | `bytemuck` | 1.25.2 | Zlib OR Apache-2.0 OR MIT | render | Casts parley's font variation coordinates to the `i16` slice `vello_cpu` takes. |
| Window, input | `winit` (default features) | 0.30.13 | Apache-2.0 | app | The standard Rust windowing crate: X11, Wayland, macOS and Windows. X11 and Wayland libraries are loaded at run time, so building needs no system packages. |
| Show CPU-drawn frames | `softbuffer` | 0.4.8 | MIT OR Apache-2.0 | app | Puts a pixel buffer in a winit window on every platform, no GPU needed. |
| Config directory | `etcetera` | 0.11.0 | MIT OR Apache-2.0 | app | Where the lawhrc lives: `choose_app_strategy` gives XDG on Linux and macOS (`~/.config/lawh`, `XDG_CONFIG_HOME` honoured) and the roaming AppData folder on Windows. Small: its only dependencies are `cfg-if` and `windows-sys`, both already in the tree. |

`peniko` (brushes, font data) and `fontique` (font collections) are used through the re-exports in
`vello_cpu` and `parley`, not as direct dependencies.

## Tests only (dev-dependencies)

| Job | Crate | Version | Licence | Used by | Why |
|---|---|---|---|---|---|
| Snapshot tests | `insta` | 1.49.0 | Apache-2.0 | core, math, render | Error messages, written files, math layouts and a small board's SVG export as reviewed text. |
| Property tests | `proptest` | 1.11.0 | MIT OR Apache-2.0 | core | Random boards for write and read round-trips, random documents for the parser. |
| Reading PDFs back | `hayro` (no default features) | 0.7.1 (pinned `=`) | Apache-2.0 OR MIT | render | Reads exported PDFs (pages, fonts, the text of their glyphs) and renders them to compare with the PNG export. Pinned to the hayro Typst already compiles (through `typst-svg`), with the same features, so nothing new is built. |
| Drawing SVGs | `resvg` (no default features) | 0.47.0 (pinned `=`) | Apache-2.0 OR MIT | render | An independent rasterizer: draws exported SVGs to compare with the PNG export. Pinned to the `usvg` 0.47 Typst already compiles, so usvg isn't built twice; without its default features it draws no text and loads no fonts. It brings `tiny-skia` 0.12.0 (BSD-3-Clause) and `rgb` 0.8.53 (MIT). |
| Checking SVGs | `roxmltree` | 0.21.1 (pinned `=`) | MIT OR Apache-2.0 | render | Parses exported SVGs to check their structure: only the elements lawh writes, every reference inside the file. The version usvg already uses: no new crate. |
| Test oracle | `kdl` (`span`; no default features) | 6.7.1 | Apache-2.0 | core | The reference KDL v2 parser. Differential tests check that lawh's own parser reads flat KDL as kdl does, and that the writer's numbers and escapes read back in kdl. Not in the app: as a reader it took up to 2 s and 459 MB on a hostile 1 MiB file ([research/m1-measurements.md](research/m1-measurements.md)). |

kdl 6.7 needs Rust 1.95, which sets the workspace's `rust-version`. The highest among the
dependencies the app builds is 1.92, for Typst 0.15 and krilla 0.8.

## Tools

| Tool | Where | Why |
|---|---|---|
| `cargo-deny` | CI (`EmbarkStudios/cargo-deny-action@v2`), or `cargo deny check` | Blocks GPL-family licences, unknown sources and known advisories. |
| `cargo-insta` | Local | `cargo insta review` accepts or rejects changed snapshots. |
| `actionlint` | Local, `uvx --from actionlint-py actionlint` | Lints the GitHub workflows. |

## Bundled fonts

lawh never uses system fonts. Both fonts are embedded in the binary with `include_bytes!`
(`crates/lawh-math/src/fonts.rs`), so text and math look the same on every platform and a board
can't probe what is installed. Sources and checksums are in
[assets/fonts/README.md](../assets/fonts/README.md).

| Font | File | Version | Licence | Used for |
|---|---|---|---|---|
| Inter Regular (static) | `Inter-Regular.ttf` | 4.1 | OFL-1.1 (`Inter-OFL.txt`) | Text, the status bar and jump labels (the short labels `s` puts on objects to jump to), and text inside math |
| Libertinus Math Regular | `LibertinusMath-Regular.otf` | 7.051 | OFL-1.1 (`Libertinus-OFL.txt`) | Math (it has the OpenType `MATH` table Typst needs), and the fallback for characters Inter lacks in text, the status bar and jump labels (`:sym` symbols such as `⊞`) |

Of the 1,189 symbol names Typst math knows (codex 0.3.0), the two fonts draw 732; `:sym` lists
only those. A bundled font with more symbols is a decision for the visual design pass.

OFL-1.1 asks that the licence travel with the font. Since the binary embeds both, the release
archives ship `Inter-OFL.txt` and `Libertinus-OFL.txt` next to `LICENSE`.

## Licences and advisories

`deny.toml` allows permissive licences, MPL-2.0 (file-level copyleft) and OFL-1.1. GPL, LGPL and
AGPL are absent on purpose: adding one needs a decision, not a config edit. Unmaintained crates
fail the check only when lawh depends on them directly; the rest arrive through Typst's tree
(bincode, paste, rustybuzz, yaml-rust).

Advisories ignored in `deny.toml`, and why:

| Advisory | Crate | What | Why it is ignored |
|---|---|---|---|
| RUSTSEC-2026-0194 | quick-xml 0.38 | Denial of service: quadratic time on duplicate attribute names | Arrives only through hayagriva and citationberg in typst-library, to parse CSL styles. That is reachable only through `cite` and `bibliography`, which lawh-math removes from Typst's library (`crates/lawh-math/src/world.rs`). |
| RUSTSEC-2026-0195 | quick-xml 0.38 | Denial of service: unbounded memory for namespace declarations | Same path, same reason. |
| RUSTSEC-2026-0192 | ttf-parser | Unmaintained | Typst's own font type: `Font::ttf()` returns a `ttf_parser::Face`, so reading glyph outlines needs it directly. Revisit when Typst moves off it. |

**To do before the first public release:** ship third-party licence notices for the Rust crates
compiled into the binary (for example generated with `cargo-about`) in each release archive, and
note krilla's sRGB profile (CC0), which every PDF export embeds. Today the archives carry lawh's
`LICENSE`, the README and the two font licences only.

## Planned

Not in `Cargo.toml` yet; each is added with the code that needs it. Versions and licences were
checked on crates.io on 2026-10-10.

| Job | Crate | Version | Licence | Why |
|---|---|---|---|---|
| Spatial index for motions | `rstar` | 0.13.0 | MIT OR Apache-2.0 | Nearest-object queries for `H J K L`, if scanning the board's 500 objects (how motions work today, bounded by `MAX_KEY_WORK`) gets too slow. |
| Clipboard | `arboard` | 3.6.1 | MIT OR Apache-2.0 | Cross-platform, Wayland support behind a feature. |
| Live reload | `notify` | 8.2.0 | CC0-1.0 | Reload a board when it changes on disk (hand edits, agents). |
| Accessibility | `accesskit` | 0.25.1 | MIT OR Apache-2.0 | Screen-reader tree for winit apps. |

`vello_gpu` (0.3.0) is the upgrade path if the CPU renderer gets too slow; a release frame of
`examples/area.lawh` takes about 1.1 ms today.

`nucleo-matcher` is MPL-2.0, a file-level copyleft: fine as an unmodified dependency of an MIT
app, but changes to its own files would have to stay MPL.

## Rejected

| Option | Why not |
|---|---|
| Tauri | A webview in a desktop window, not native rendering. That's the web stack, which we chose against. |
| egui | Bidirectional text support is still an open issue, and an Arabic cursor bug is open (Sep 2026). Immediate mode fights a retained board model. |
| iced | A full framework that owns the event loop and app structure. A Vim app's chrome is a status line and a command line, which we draw ourselves. |
| GPUI (Zed) | Pre-1.0, its README warns of frequent breaking changes, and the crates.io release lags Zed's. |
| xilem / masonry | Early (0.4), and we don't need a widget toolkit. |
| `vello` (compute) | Now labelled experimental by its own README. |
| `kdl` as the `.lawh` reader | Parses the whole document before anything can be checked, so a hostile 1 MiB file took up to 2 s and 459 MB. lawh's own parser checks every cap while reading. kdl stays as the test oracle. |
| `kdl`'s writer | Prints identifier-like strings bare, whole floats as `3.0`, and leaves characters such as U+200F or U+0000 unescaped, which its own parser then rejects. |
| SVG text as `<text>` with an embedded font | Math places glyphs no character maps to (size variants, stretched delimiters), so it needs paths anyway. The font subsets krilla makes (with `subsetter`) are for PDF only, their `cmap` removed, so every SVG would carry both whole fonts: 549 KB and 785 KB as base64 (derived from their sizes). Viewers would shape the text again, with the ligatures lawh turns off, and librsvg (behind GNOME's image viewers and ImageMagick) ignored an embedded font when it was tried during M4's research. lawh writes every glyph as a path; PDF is the format to search and copy. |
| `lopdf`, `pdf-extract` as the tests' PDF reader | Each brings many new crates (`lopdf` its own crypto, date and parser crates), and `pdf-extract` only extracts text. `hayro` reads, interprets and renders a PDF, and is in the tree already. |
| `typst-svg` | Math is converted to paths by lawh-math itself, from Typst's frames, and an SVG export writes the whole board, not one formula. lawh writes its SVG files itself, with no dependency. |
| System fonts (parley's `system` feature) | Needs fontconfig, makes boards look different per machine, and lets a board probe what is installed. |
| LaTeX via `mitex` | Needs a LaTeX → Typst conversion step, and its crates.io copy is from 2024. Typst syntax directly is simpler. |
| KaTeX / MathJax | JavaScript. Would need an embedded JS engine in a native app. |
| Building on Excalidraw, tldraw, AFFiNE or Obsidian | See [research/audit.md](research/audit.md): licence, packaging or math blockers. |
