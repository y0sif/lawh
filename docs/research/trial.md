# Hands-on trial: vim whiteboard landscape

Goal: feel what each app does today, find what to steal, and decide
**layer on an existing canvas** vs **build our own**.

## The benchmark (same in every app)

Build this "area of a circle" explainer. Try keyboard only first. When you have
to touch the mouse, count it and note why.

1. Title "Area of a circle" at the top.
2. A circle in the middle.
3. A radius line from the centre to the edge, labelled `r`.
4. The formula `A = πr²` under the circle, rendered as math, entered by name or shortcut.
5. Move the formula to the right of the circle.
6. Duplicate the circle and its label twice to the right, evenly spaced.
7. Write two sentences of explanation as normal text or a doc, then put one of
   them on the board next to the circle.

Time the whole run once you know the app (second attempt, not first).

## Apps

| # | App | How to open | Look at especially | Pivot candidate? |
|---|---|---|---|---|
| 1 | qboard | https://cjquines.com/qboard/ | One-hand key layout, LaTeX entry, what still needs the mouse | No (small, MIT, could borrow ideas) |
| 2 | grafli | `uv tool install grafli`, then `grafli test.grafli` (pulls PySide6, a few hundred MB) | Modes, **jump labels (Ctrl+J)**, F1 cheat sheet, plain-text file | No |
| 3 | ascdraw | `gh release download nightly-37295954469 -R exlee/ascdraw -p ascdraw-linux.tar.gz` into an empty dir, untar, run the binary | hjkl + modifier model, mode switching | No |
| 4 | Excalidraw | https://excalidraw.com | Single-key tools, arrow-key nudge, command palette (Ctrl-/), how far keyboard gets you | **Yes** (MIT React package) |
| 5 | tldraw | https://tldraw.com | Tab / Ctrl+Arrow to move selection to the nearest shape, shortcuts dialog | **Yes** (SDK, check licence) |
| 6 | Obsidian + Excalidraw plugin | Obsidian is installed (1.14.4). Settings → Editor → Vim key bindings. Community plugins → Excalidraw. Command palette → "Insert LaTeX formula". Also try Vim inside a Canvas card | **Yes** (plugin route, existing vim-user audience) |
| 7 | AFFiNE | https://app.affine.pro | Page ↔ Edgeless toggle (Ctrl+Alt+E), moving a doc block onto the board | Maybe (BlockSuite editor) |
| 8 | Mathcha | https://www.mathcha.io | Task 4 only: how fast symbol entry is | No |
| 9 | (optional) AI drawing | Ask Claude with an Excalidraw/tldraw MCP to draw the benchmark | Time it, then time one edit ("move the formula right") | Substitute, not a base |

## Scorecard

| App | Time | Mouse touches | Couldn't do from keyboard | Math entry | Steal this | Hated this |
|---|---|---|---|---|---|---|
| qboard | | | | | | |
| grafli | | | | | | |
| ascdraw | | | | | | |
| Excalidraw | | | | | | |
| tldraw | | | | | | |
| Obsidian + Excalidraw | | | | | | |
| AFFiNE | | | | | | |
| Mathcha | | | | | | |
| AI drawing | | | | | | |

## Questions to answer while trying

- Where should "the cursor" live on a canvas? (grid point, selected object,
  jump labels, nearest-shape like tldraw)
- Do the app's own single-key shortcuts fight with hjkl? Could a layer switch them off?
- Is doc + board in one surface (AFFiNE) actually better than a board with
  text blocks on it?
- Does the AI route feel fast enough to make the whole idea pointless?
