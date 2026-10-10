# Tech stack

Why each dependency, and what it beat. Versions, licences and minimum Rust versions were checked
on crates.io on 2026-10-10. None of these are in `Cargo.toml` yet; they get added as the code
that needs them lands.

## Planned dependencies

| Job | Crate | Version | Licence | Why |
|---|---|---|---|---|
| Window, keyboard, input methods | `winit` | 0.30.13 | Apache-2.0 | The standard Rust windowing crate, works on X11, Wayland, macOS and Windows. |
| Show CPU-drawn frames | `softbuffer` | 0.4.8 | MIT OR Apache-2.0 | Puts a pixel buffer in a winit window on every platform, no GPU needed. |
| Drawing | `vello_cpu` | 0.3.0 | Apache-2.0 OR MIT | Vello's most mature renderer by its own README. `vello_gpu` (0.3.0) is the later upgrade path. |
| Geometry, colours | `kurbo`, `peniko` | 0.13.1, 0.6.1 | Apache-2.0 OR MIT | Circles, Béziers, hit tests and brushes, shared with the Vello family. |
| Text layout and editing | `parley`, `fontique` | 0.12.0 | Apache-2.0 OR MIT | Shaping, line breaking, font fallback and a `PlainEditor` for cursor and selection. |
| Math | `typst` (+ `typst-svg` or own path conversion) | 0.15.1 | Apache-2.0 | Real math layout from readable syntax (`pi r^2`), pure Rust. |
| Spatial index for motions | `rstar` | 0.13.0 | MIT OR Apache-2.0 | Nearest-object queries for "jump to the shape on the right". |
| Symbol search | `nucleo` | 0.5.0 | MPL-2.0 | Helix's fuzzy matcher for `:sym integral`. |
| File format | `kdl` | 6.7.1 | Apache-2.0 | Spec'd plain-text format with good error messages. See `research/kdl-spike.md`. |
| PDF export | `krilla` | 0.8.2 | MIT OR Apache-2.0 | The PDF library Typst itself uses. |
| Clipboard | `arboard` | 3.6.1 | MIT OR Apache-2.0 | Cross-platform, Wayland support behind a feature. |
| Live reload | `notify` | 8.2.0 | CC0-1.0 | Reload a board when it changes on disk (hand edits, agents). |
| Config and cache dirs | `etcetera` | 0.11.0 | MIT OR Apache-2.0 | XDG on Linux, native folders on macOS and Windows. |
| Accessibility | `accesskit` | 0.25.1 | MIT OR Apache-2.0 | Screen-reader tree for winit apps. |

All licences above were read from crates.io. `nucleo` is MPL-2.0, a file-level copyleft: fine as an
unmodified dependency of an MIT app, but changes to its own files would have to stay MPL.

## Rejected

| Option | Why not |
|---|---|
| Tauri | A webview in a desktop window, not native rendering. That's the web stack, which we chose against. |
| egui | Bidirectional text support is still an open issue, and an Arabic cursor bug is open (Sep 2026). Immediate mode fights a retained board model. |
| iced | A full framework that owns the event loop and app structure. A Vim app's chrome is a status line and a command line, which we draw ourselves. |
| GPUI (Zed) | Pre-1.0, its README warns of frequent breaking changes, and the crates.io release lags Zed's. |
| xilem / masonry | Early (0.4), and we don't need a widget toolkit. |
| `vello` (compute) | Now labelled experimental by its own README. |
| LaTeX via `mitex` | Needs a LaTeX → Typst conversion step, and its crates.io copy is from 2024. Typst syntax directly is simpler. |
| KaTeX / MathJax | JavaScript. Would need an embedded JS engine in a native app. |
| Building on Excalidraw, tldraw, AFFiNE or Obsidian | See `research/audit.md`: licence, packaging or math blockers. |
