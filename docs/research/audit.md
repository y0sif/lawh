# Layer feasibility audit (2026-10-10)

Technical half of the layer-vs-own decision. V = verified in source or official docs, I = inferred.
Spot-checked by hand: tldraw LICENSE.md production clause, Obsidian Excalidraw AGPL (LICENSE added
2026-05-13, package.json still says MIT), Excalidraw issue #6925 open since 2023-08-23.

## Matrix

| | Excalidraw pkg | tldraw SDK | Obsidian Excalidraw | AFFiNE / BlockSuite | Build own |
|---|---|---|---|---|---|
| Licence | MIT (V) | Source-available, production needs a key (V) | AGPL-3.0 since 2026-05 (V) | MIT outside backend (V) | Ours |
| Key intercept | No disable/remap prop; window capture listener (I) | `areKeyboardShortcutsEnabled` + capture (V/I) | Script can intercept (V) | Hardcoded `bindHotKey`; capture (I) | Full |
| Shape API | Rich, no nearest-in-direction (V) | Richest, has `getNearestAdjacentShape` (V) | Rich helpers via ExcalidrawAutomate (V) | `addElement`, low-level (V) | Write it |
| Math | Image only; issue #5265 open since 2022 (V) | Custom shape; official KaTeX example (V) | `addLaTex` → MathJax image (V) | Native KaTeX block (V) | KaTeX → SVG |
| Doc + board | No | No | Via Obsidian markdown (I) | Native page + edgeless (V) | Our choice |
| Format | JSON (V) | JSON snapshot (I) | `.excalidraw.md`, compressed JSON (V) | Yjs CRDT binary (I) | Plain text possible |
| Churn | Slow releases, master 432 commits ahead, API renames (V) | Yearly majors, breaking (V) | Very active (V) | npm frozen at 0.22.4 since 2025-07 (V) | None |
| Vim attempts | Issue #6925 open, nothing built | `biliboss/canvas` hjkl + `o` only, 0 stars | 1 tiny non-modal script | AFFiNE #3236 closed "not planned" | n/a |

## Blockers

- **tldraw:** v4+ needs a licence key in production. Without one it stops rendering after 5 s. The hobby key is
  discretionary, watermarked and tied to allowed hosts. That rules out an MIT app that people self-host.
- **BlockSuite:** no maintained standalone package; would mean vendoring AFFiNE's monorepo.
- **Obsidian Excalidraw:** AGPL, a plugin on a plugin, on closed-source Obsidian and a forked Excalidraw.
- **Excalidraw:** no hard blocker. Fixed element types (math stays an image + `customData`), no
  shortcut-disable API, text editor would need driving or replacing.

## Audit agent's recommendation

Build our own canvas, with the vim grammar (modes, counts, operators, motions over a spatial index,
ex-commands) as a pure core behind an abstract scene interface, so the choice stays reversible.
Reasons: the vim grammar is the hard novel part either way. Much of what a base provides (mouse tools,
toolbars, shortcuts) is exactly what vim mode would switch off. The base with the right extension model
(tldraw) is blocked by its licence, and the cleanly licensed one (Excalidraw) can't take a first-class math element.

If layering anyway: Excalidraw (MIT), window capture-phase key listener, math as SVG + TeX in
`customData`, pinned version. Fastest throwaway prototype: Obsidian Excalidraw (`addLaTex` exists).
Own-canvas likely stack (inferred): SVG DOM rendering, KaTeX, optionally roughjs + perfect-freehand.

## Prior art worth reading

- tldraw `apps/examples/.../keyboard-math-input/MathShapeUtil.tsx`: KaTeX shape that edits like text
- tldraw `Editor.getNearestAdjacentShape` / `selectAdjacentShape`: a model for directional motions
- `3xpyth0n/ideon` (634 stars, xyflow canvas): CodeMirror vim only inside text blocks
