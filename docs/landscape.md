# Competitive landscape

Phase 0 research, 2026-10-10. Live web searches, plus source pages and the GitHub API.
Star counts and push dates were read that day. Parity figures are estimates.
This feeds `README.md`, `docs/comparison.md` and `docs/faq.md`.

## Differentiation statement

Given that Excalidraw and tldraw own the infinite-canvas whiteboard, AFFiNE already merges
documents and whiteboards, and qboard and grafli have shown keyboard-first drawing in small
tools, lawh still earns its existence because (1) it is the only editor where the whole canvas,
shapes, text blocks and math alike, is driven by a real Vim grammar (counts, operators,
motions, ex-commands) instead of single-key tool pickers that still need a mouse drag;
(2) text and drawing live on one surface, so a block of text moves under a circle with a
motion; and (3) it is open source, native and local-first, with a plain-text file format that
diffs in git and that agents can write.

## Matrix

| Name | Type | Status (2026-10-10) | Does | Doesn't |
|---|---|---|---|---|
| [qboard](https://github.com/cjquines/qboard) | OSS (MIT) web | 89★, pushed 2026-09-04, one maintainer | "In the spirit of Vim, everything that isn't drawing" by keys. E/R ellipse/rect, LaTeX via MathJax, 16:9 pages, PDF export. | Shapes still mouse-drawn. No motions, operators or commands. Fixed pages. ~30% parity. |
| [grafli](https://github.com/MisterGC/grafli) | OSS (MIT) Qt desktop | 3★, created 2026-03 | Modal Select/Rect/Text/Connect, jump labels, plain-text format, agent skill. | Boxes, arrows and notes only. No circles or math. |
| [ascdraw](https://github.com/exlee/ascdraw) | OSS (GPL-3) Rust | 97★, changelog 2026-10-04 | hjkl, modes, infinite UTF-8 canvas. | Character grid, rectangles only, no math. |
| [Excalidraw](https://excalidraw.com) | OSS (MIT) + paid Plus | 133.6k★ | Single-key tools, arrow nudge, Ctrl-/ palette, AI text-to-diagram (Plus). | No modal grammar. No native math (issue #5265 open since 2022). Vim request #6925 open since 2023. |
| [Obsidian](https://obsidian.md) + [Excalidraw plugin](https://github.com/zsviczian/obsidian-excalidraw-plugin) | Proprietary app + AGPL plugin | Plugin 7.7k★ | Vim in notes. Plugin: "Insert LaTeX formula", hotkeyable script macros. | Vim only inside text. Canvas users install plugins to turn vim off. |
| [tldraw](https://tldraw.dev) | Source-available SDK | 50.8k★ | Tab and Ctrl+Arrow move selection between shapes. MCP App lets agents draw (Mar 2026). | No modal grammar. Production use needs a licence key. |
| [AFFiNE](https://affine.pro) | OSS, freemium | 73k★ | Page mode and Edgeless (whiteboard) mode per doc, single-key tools. | No vim (issue #3236 closed "not planned"). Mouse placement. |
| [Mathcha](https://www.mathcha.io) | Proprietary web + desktop | Live | Symbol suggestion box, diagrams exported to TikZ. | Mouse-driven drawing. |
| [Ipe](https://ipe.otfried.org), [Castel's Inkscape shortcuts](https://github.com/gillescastel/inkscape-shortcut-manager) | OSS desktop / personal script | Niche | LaTeX labels, snapping, chord shortcuts. | Mouse placement, tied to desktop apps. |

Substitutes: AI agents drawing through MCP (tldraw, Excalidraw, Eraser), diagrams as code
(TikZ, Typst CeTZ, Mermaid, D2) and ink-to-shape (Microsoft Whiteboard Alt+B, Goodnotes).

## Platform threats

- **Obsidian**: has the vim users, a canvas and a scriptable Excalidraw plugin. A plugin author
  could build a vim layer there. Nobody has yet.
- **AI drawing**: a substitute for one-off diagrams, not for live editing while explaining.
  tldraw's founder said at AI Engineer Europe 2026 that agent drawing still feels like handing
  his keyboard to an AI.

## Kill criteria

None hit hard. The soft one is market size: the closest tools are small (qboard has 89 stars after six
years). The main risk is design, not competition: what a motion and a cursor mean on a 2D canvas.

## Decision

Build our own canvas rather than a layer on an existing one. See `research/audit.md`. tldraw's
licence blocks an MIT release, Obsidian Excalidraw is AGPL, and BlockSuite has no maintained
standalone package. Excalidraw is the only clean base, but it can't take a first-class math element.
