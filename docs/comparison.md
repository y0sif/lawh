# lawh compared

How lawh compares to other whiteboards and drawing tools. Facts about other tools were checked
on 2026-10-10 from their sites, docs and source (details and links in [landscape.md](landscape.md)).
lawh is pre-alpha, so its column describes the v1 plan.

## Summary

| | lawh | Excalidraw | tldraw | AFFiNE | Obsidian + Excalidraw | qboard | grafli |
|---|---|---|---|---|---|---|---|
| Vim grammar on the canvas (counts, operators, motions) | Yes | No | No | No | No (Vim only inside notes) | No (Vim-style keys for tools) | Modal, Vim-inspired |
| Create and size shapes without dragging | Yes, `:circle r=3` | No | No | No | Via scripts | No | Boxes, in Rect mode |
| Math | Typst syntax | Third-party add-on | SDK example only | LaTeX block | "Insert LaTeX formula" | LaTeX (MathJax) | No |
| Circles and ellipses | Yes | Yes | Yes | Yes | Yes | Yes | No |
| Text and drawing on one surface | Yes | Yes | Yes | Yes, plus a page view | Yes, plus notes | Yes | Notes only |
| File format | Plain-text KDL | JSON | JSON | Binary (Yjs) | Markdown + compressed JSON | JSON | Plain text |
| Collaboration | No (v1) | Yes | Yes | Yes | Not real-time | No | Not stated |
| Runs as | Native app | Web | Web, SDK | Web, desktop | Desktop, mobile | Web | Qt desktop |
| License | MIT | MIT | Source-available, licence key for production | Open source | Obsidian closed, plugin AGPL | MIT | MIT |

## lawh vs Excalidraw

Excalidraw is the open-source whiteboard most people know, with single-key tools (`R`, `O`, `A`),
arrow-key nudging and a command palette. You still pick a tool and drag to set the size and place.
Math needs a third-party extension (native math support has been an open request since 2022), and
a request for Vim motions has been open since 2023. lawh builds every action as a Vim command,
draws math natively, and saves plain text. Excalidraw wins on collaboration, its hand-drawn style
and an established user base.

## lawh vs tldraw

tldraw has the best keyboard navigation of the big canvases: Tab and Ctrl+Arrow move the selection
between shapes, and AI agents can draw on it through an MCP app. It has no modal grammar, and the
SDK needs a licence key for production use. lawh is MIT and keyboard-first throughout.

## lawh vs AFFiNE

AFFiNE is the closest to "a text editor that is also a whiteboard": every doc has a page view and
an edgeless whiteboard view, and it has LaTeX blocks. It is mouse-driven, and its request for Vim
support was closed as not planned. lawh has no page view in v1 but runs on Vim from top to bottom.

## lawh vs Obsidian with the Excalidraw plugin

Obsidian has Vim mode for notes, and the Excalidraw plugin adds a canvas, a LaTeX command and
scriptable macros. Vim stops at the edge of the text: it doesn't move or create shapes, and some
users turn it off on the canvas because it gets in the way. lawh is a standalone app where Vim
drives the canvas itself.

## lawh vs qboard and grafli

qboard is a small web whiteboard for lectures. It's built so everything except drawing is a key
press, and it renders LaTeX, but shapes are still drawn with the mouse on fixed 16:9 pages.
grafli is a new keyboard-driven diagram tool with Vim-inspired modes and a plain-text format,
aimed at architecture sketches: boxes, arrows and notes, no circles and no math. lawh takes the
keyboard-first idea further, with a full Vim grammar, more shapes and math.

## When to use something else

- You need real-time collaboration: Excalidraw, tldraw or AFFiNE.
- You draw freehand with a pen: any of the above, or Xournal++.
- You want a diagram generated from a description: an AI agent with the tldraw or Excalidraw MCP.
- You want publication-quality figures in a LaTeX paper: Ipe or TikZ.
