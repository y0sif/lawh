# lawh compared

How lawh compares to other whiteboards and drawing tools. Every fact about another tool links to
its source in the sections below the table; each source was checked on 2026-10-25.

## Summary

| | lawh | Excalidraw | tldraw | AFFiNE | Obsidian + Excalidraw | qboard | grafli |
|---|---|---|---|---|---|---|---|
| Vim grammar on the canvas (counts, operators, motions) | Yes | No | No | No | No (Vim only inside notes) | No (Vim-style keys for tools) | Modal, Vim-inspired |
| Shapes made and sized from the keyboard | Yes, `:circle r=3` | No: keys pick a tool | No: keys pick a tool | No: keys pick a tool | Via scripts | No: drawing is by mouse | Boxes, in Rect mode |
| Math | Typst syntax | Not built in | SDK example only | LaTeX block | "Insert LaTeX formula" | LaTeX (MathJax) | No |
| Circles and ellipses | Yes | Yes | Yes | Yes | Yes | Yes | No |
| Text and drawing on one surface | Yes | Yes | Yes | Yes, plus a page view | Yes, plus notes | Yes | Notes only |
| File format | Plain-text KDL | JSON | JSON | Yjs (CRDT) documents | Markdown + compressed JSON | JSON | Plain text |
| Collaboration | No | Real-time | Real-time | Real-time | Shared vaults, not live | No (runs in the browser only) | Not stated |
| Runs as | Native app | Web | Web, SDK | Web, desktop | Desktop, mobile | Web | Qt desktop |
| License | MIT | MIT | Source-available, licence key for production | MIT, except the backend | Obsidian closed, plugin AGPL-3.0 | MIT | MIT |

## lawh vs Excalidraw

Excalidraw is the open-source whiteboard most people know. Its keys pick a tool (`R` rectangle,
`O` ellipse, `A` arrow) and `Ctrl+/` opens a command palette
([its shortcut list](https://github.com/excalidraw/excalidraw/blob/master/packages/excalidraw/components/HelpDialog.tsx),
[`shortcuts.ts`](https://github.com/excalidraw/excalidraw/blob/master/packages/excalidraw/actions/shortcuts.ts));
you still drag to set the size and place. Math isn't built in: the request for LaTeX support has
been open since 2022 ([#5265](https://github.com/excalidraw/excalidraw/issues/5265)), and the
request for Vim motions since 2023 ([#6925](https://github.com/excalidraw/excalidraw/issues/6925)).
It saves `.excalidraw` JSON files, has real-time collaboration and a hand-drawn style, and is MIT
licensed ([README](https://github.com/excalidraw/excalidraw)). lawh makes every action a Vim
command, draws math natively, and saves plain text. Excalidraw wins on collaboration, its
hand-drawn style and an established user base.

## lawh vs tldraw

tldraw has the best keyboard navigation of the big canvases: Tab and Ctrl+Arrow move the selection
between shapes
([`SelectTool/childStates/Idle.ts`](https://github.com/tldraw/tldraw/blob/main/packages/tldraw/src/lib/tools/SelectTool/childStates/Idle.ts)),
and since March 2026 AI agents can draw on it through an
[MCP app](https://tldraw.dev/blog/tldraw-mcp-app). Its keys pick tools, with no modal grammar
([shortcut list](https://github.com/tldraw/tldraw/blob/main/packages/tldraw/src/lib/ui/components/KeyboardShortcutsDialog/DefaultKeyboardShortcutsDialogContent.tsx)).
Math is an example in its SDK, not a feature
([`keyboard-math-input`](https://github.com/tldraw/tldraw/tree/main/apps/examples/src/examples/shapes/tools/keyboard-math-input)).
Its ellipse is one of the
[geo shapes](https://github.com/tldraw/tldraw/blob/main/packages/tlschema/src/shapes/TLGeoShape.ts),
its state saves as [JSON snapshots](https://tldraw.dev/docs/persistence), and it has self-hostable
real-time collaboration. The SDK is free in development, but production use needs a licence key
([README](https://github.com/tldraw/tldraw#license),
[LICENSE.md](https://github.com/tldraw/tldraw/blob/main/LICENSE.md)). lawh is MIT and
keyboard-first throughout.

## lawh vs AFFiNE

AFFiNE is the closest to "a text editor that is also a whiteboard": docs and an edgeless
whiteboard are merged, it collaborates in real time on the web and desktop clients
([README](https://github.com/toeverything/AFFiNE)), and it has a
[LaTeX block](https://github.com/toeverything/AFFiNE/tree/canary/blocksuite/affine/blocks/latex)
and an [ellipse shape](https://github.com/toeverything/AFFiNE/blob/canary/blocksuite/affine/model/src/consts/shape.ts).
Its documents are [Yjs](https://github.com/yjs/yjs) CRDT data
([BlockSuite README](https://github.com/toeverything/blocksuite)), and it is MIT licensed outside
its backend ([LICENSE](https://github.com/toeverything/AFFiNE/blob/canary/LICENSE)). On the
whiteboard its keys pick a tool (`S` shape, `T` text, `P` pen,
[`use-shortcuts.ts`](https://github.com/toeverything/AFFiNE/blob/canary/packages/frontend/core/src/components/hooks/affine/use-shortcuts.ts))
and the mouse places it, and its request for Vim support was closed as not planned on 2023-07-14
([#3236](https://github.com/toeverything/AFFiNE/issues/3236)). lawh has no page view but runs on
Vim from top to bottom.

## lawh vs Obsidian with the Excalidraw plugin

Obsidian has a Vim mode for editing notes ("Use Vim key bindings when editing",
[settings](https://obsidian.md/help/settings)) and runs on desktop and mobile
([download](https://obsidian.md/download)). The app is closed source: its
[licence](https://obsidian.md/license) reserves the rights to its code. The
[Excalidraw plugin](https://github.com/zsviczian/obsidian-excalidraw-plugin) (AGPL-3.0) adds a
canvas, an "Insert LaTeX formula" command
([`en.ts`](https://github.com/zsviczian/obsidian-excalidraw-plugin/blob/master/src/lang/locale/en.ts))
and a script engine, and stores a drawing in Markdown with its JSON compressed (the "Compress
Excalidraw JSON in Markdown" setting, same file). Vim stops at the edge of the text: it doesn't
move or create shapes. Obsidian Sync shares a vault but has no live co-editing
([collaborate](https://obsidian.md/help/sync/collaborate)). lawh is a standalone app where Vim
drives the canvas itself.

## lawh vs qboard and grafli

[qboard](https://github.com/cjquines/qboard) (MIT) is a small web whiteboard for lectures. Its
README says "In the spirit of Vim, it's possible to do everything that isn't drawing without
moving your hands", so shapes are still drawn with the mouse, on fixed 16:9 pages. It renders
LaTeX with MathJax, draws ellipses and circles (Shift "makes squares and circles"), saves JSON
files and runs wholly in the browser (same README).

[grafli](https://github.com/MisterGC/grafli) (MIT) is a keyboard-driven diagram tool with modal
editing "in the spirit of vim" (Select, Rect, Text and Connect modes) and line-oriented plain-text
files, built on Qt (PySide6). It has three primitives, boxes, arrows and notes, so no circles and
no math (same README).

lawh takes the keyboard-first idea further, with a full Vim grammar, more shapes and math.

## When to use something else

- You need real-time collaboration: Excalidraw, tldraw or AFFiNE (sources above).
- You draw freehand with a pen: Excalidraw, tldraw, AFFiNE or qboard each has a pen or draw tool
  (the shortcut lists and README above), or [Xournal++](https://xournalpp.github.io), open-source
  note-taking software.
- You want a diagram generated from a description: an AI agent with
  [tldraw's MCP app](https://tldraw.dev/blog/tldraw-mcp-app).
- You want figures for a LaTeX paper: [Ipe](https://ipe.otfried.org), a drawing editor for
  figures in LaTeX documents, or [TikZ](https://tikz.dev).
