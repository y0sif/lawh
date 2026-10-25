# FAQ

### What is lawh?

A native, open-source whiteboard and text editor you drive with Vim motions and commands. You draw
shapes, write math and move text around without a mouse or a drawing tablet. It is written in
Rust and renders math with Typst. The name is Arabic (لوح) for a board or slate.

### Who is it for?

People who explain things on a whiteboard (students, tutors, lecturers, engineers in design
reviews) who already use Vim and don't want to draw with a mouse.

### Do I need to know Vim?

To get the most out of it, yes. lawh uses Vim's modes (Normal, Insert, Visual, Command), counts,
operators and motions. If you know `hjkl`, `:w` and `dd`, you'll feel at home.

### How do I draw a circle?

Move the cursor where you want it and type `:circle r=3`. The circle appears at the cursor with
radius 3 grid units.

### How do I write math?

In a math block, using [Typst](https://typst.app) math syntax: `A = pi r^2`, `sum_(i=1)^n i`,
`integral_0^1 f(x) dif x`. To find a symbol by name, use `:sym` with a word like `integral`.

### Does it support LaTeX?

No. Math uses Typst syntax, which is shorter and reads closer to plain names (`pi`, `sum`,
`integral`). Converting LaTeX would add a translation step that can break on edge cases.

### Can I use a mouse?

To pan, zoom and click to select, yes. Creating, sizing, moving and editing are all done from the
keyboard.

### How do I move an object?

As in Vim: take it with `x` (or `dd`), then put it with `p` below another object or `P` above it,
clear of the rest. On a grid point, `p` puts it at the cursor. `u` undoes either step.

### How is lawh different from Excalidraw?

Excalidraw is built for the mouse: shortcuts pick a tool, then you drag
([its shortcut list](https://github.com/excalidraw/excalidraw/blob/master/packages/excalidraw/components/HelpDialog.tsx),
checked 2026-10-25). lawh makes every action a Vim command, renders math natively, and saves
boards as plain text. Excalidraw has real-time collaboration and a hand-drawn style
([README](https://github.com/excalidraw/excalidraw), checked 2026-10-25) that lawh doesn't have.
See [comparison.md](comparison.md).

### How is lawh different from tldraw?

tldraw is a web canvas and SDK with good keyboard navigation between shapes (Tab and Ctrl+Arrow)
but no modal grammar, and its SDK needs a licence key for production
([README](https://github.com/tldraw/tldraw#license), checked 2026-10-25). lawh is a native,
MIT-licensed, Vim-first app. Sources for each point: [comparison.md](comparison.md#lawh-vs-tldraw).

### Why not build it as a plugin for Excalidraw or Obsidian?

We looked, on 2026-10-10, and checked again on 2026-10-25. tldraw needs a licence key in
production ([LICENSE.md](https://github.com/tldraw/tldraw/blob/main/LICENSE.md)), which an
MIT app that anyone can build and run can't carry. The
[Obsidian Excalidraw plugin](https://github.com/zsviczian/obsidian-excalidraw-plugin) is AGPL-3.0
and runs only inside Obsidian. AFFiNE's editor, BlockSuite, has had no npm release since
2025-07-01 (`@blocksuite/store` 0.22.4, [npm](https://www.npmjs.com/package/@blocksuite/store)).
Excalidraw has no math element: the request has been open since 2022
([#5265](https://github.com/excalidraw/excalidraw/issues/5265)). Most of what an existing
whiteboard gives you is mouse tooling that a Vim mode would switch off anyway.

### Why native instead of a web app?

Speed and ownership: lawh opens local files directly, works offline, and doesn't depend on a
browser engine. The trade-off is building text editing ourselves instead of getting it from a browser.

### What does a board file look like?

Plain-text [KDL](https://kdl.dev), one object per line, so boards diff cleanly in git and AI agents
can write them:

```kdl
lawh version=1
circle id=c1 x=0 y=0 r=3
math   id=m1 x=5 y=0 "A = pi r^2"
```

### Is opening someone else's board safe?

It is meant to be, and a way around it is a vulnerability ([SECURITY.md](../SECURITY.md) says how
to report one). Files are read by a parser that checks every limit as it goes. Math is compiled
with Typst in a separate process, with no file access, no network, no clock and only bundled
fonts, and it is killed if a formula takes more than 2 seconds. On Linux that process also has a
memory cap; on macOS and Windows it doesn't yet. The threat model is in
[architecture.md](architecture.md#threat-model).

### Can several people edit the same board?

No. Boards are local files. Share them by sending the file or exporting SVG, PNG or PDF.

### Which platforms does it run on?

Linux, macOS and Windows, from the same code. The release builds aren't code-signed, so your OS
will warn you the first time you open lawh; the [README](../README.md#install) says how to get
past the warning.

### Does it support Arabic or other right-to-left text?

Not yet: right-to-left text is out of scope for now, and the bundled text font, Inter, has no
Arabic glyphs.

### Is it free?

Yes. MIT licensed.
