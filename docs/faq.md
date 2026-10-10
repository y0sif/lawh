# FAQ

lawh is pre-alpha. Answers describe the v1 plan unless they say otherwise.

### What is lawh?

A native, open-source whiteboard and text editor you drive with Vim motions and commands. You draw
shapes, write math and move text around without a mouse or a drawing tablet. The name is Arabic
(لوح) for a board or slate.

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

Not in v1. Math uses Typst syntax, which is shorter and reads closer to plain names (`pi`, `sum`,
`integral`). Converting LaTeX would add a translation step that can break on edge cases.

### Can I use a mouse?

To pan, zoom and click to select, yes. Creating, sizing, moving and editing are all done from the
keyboard.

### How is lawh different from Excalidraw?

Excalidraw is built for the mouse: shortcuts pick a tool, then you drag. lawh makes every action a
Vim command, renders math natively, and saves boards as plain text. Excalidraw has collaboration
and a hand-drawn style that lawh doesn't have in v1. See [comparison.md](comparison.md).

### How is lawh different from tldraw?

tldraw is a web canvas and SDK with good keyboard navigation between shapes, but no modal grammar,
and its SDK needs a licence key for production. lawh is a native, MIT-licensed, Vim-first app.

### Why not build it as a plugin for Excalidraw or Obsidian?

We looked: tldraw's licence blocks a free open-source release, the Obsidian Excalidraw plugin is
AGPL and tied to Obsidian, AFFiNE's editor isn't packaged on its own, and Excalidraw can't hold a
real math element. Most of what an existing whiteboard gives you is mouse tooling that a Vim mode
would switch off anyway. See [research/audit.md](research/audit.md).

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

That's the intent. Math is compiled with Typst in a sandbox with no file access, no network and
only bundled fonts. See [../SECURITY.md](../SECURITY.md).

### Can several people edit the same board?

Not in v1. Boards are local files. Share them by sending the file or exporting SVG, PNG or PDF.

### Which platforms does it run on?

Linux first. macOS and Windows builds come from the same code. They aren't code-signed, so your
OS will warn you the first time you open lawh.

### Does it support Arabic or other right-to-left text?

Not in v1.

### Is it free?

Yes. MIT licensed.
