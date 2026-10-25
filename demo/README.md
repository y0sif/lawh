# README demo

`assets/demo.gif` is lawh's own frames: a key script (`area.demo`) is typed into the app with no
window, each frame is drawn by the code that draws the window (board, cursor, labels, `:sym`
rows, status line), and ffmpeg frames the result with a title bar and, under the window, a
magnifier on the status bar's left end, where the command line, the mode and messages show.

To render it, from anywhere in the repository:

```sh
demo/render.sh
```

Needs Rust (the toolchain in `rust-toolchain.toml`), bash and ffmpeg 7 or later (tested with
9.0.2). It writes `assets/demo.gif` and `demo/out/lawh-demo.mp4`; `demo/out/` is git-ignored.
`SKIP_FRAMES=1 demo/render.sh` frames the last frames again without typing the script, and
`DEMO_SCRIPT=other.demo demo/render.sh` renders another script.

## What it makes

`render.sh` runs the driver, an ignored test in the app crate (`crates/lawh/src/app/demo.rs`):

```sh
LAWH_DEMO_SCRIPT=demo/area.demo LAWH_DEMO_OUT=demo/out \
  cargo test -p lawh --bin lawh --release -- --ignored --exact app::demo::render_demo
```

It types the script into a new board named by the script and writes, in `demo/out/`:

- `frames/frame-00001.png` and on: one PNG for each frame that differs from the one before
  (frames change only on keys; the caret doesn't blink), in physical pixels.
- `timeline.ffconcat`: the frames and how long each shows, for ffmpeg's concat demuxer.
- `demo.json`: the same timing for a video editor (below), with what the status bar's left
  end shows on each frame, for the magnifier.
- whatever the script's `:w` and `:exp` write (`area.pdf` for `area.demo`).

Two runs give byte-identical frames, timeline and JSON: time is the script's, not the clock's;
math compiles in the test's process and every formula is compiled before each frame; the view
is fixed; no lawhrc is read. `LAWH_DEMO_TRACE=1` prints each key's status line, jump labels and
`:sym` rows, which helps when writing a script.

Then `render.sh` frames the frames: a dark backdrop, a soft shadow, a window with rounded
corners, three dots and the title (Inter, from `assets/fonts`), and the magnifier below it.

The magnifier is the status bar's left 144 logical pixels, below the line along its top, cut
from the full-resolution frames and shown at twice the window's size (at scale 2 that is one
frame pixel a GIF pixel, so it is as sharp as the frames), under the window's left edge, with
rounded corners, a 1 px edge and a small soft shadow. 144 holds `area.demo`'s longest left
text, `:sym arrow.r.dashed` and its caret, which ends 134.5 pixels in (measured on its frames),
with as much room after it as before it; `LENS_SRC` in `render.sh` sets it for a script with
longer commands. It shows only on frames whose `status_left` (`demo.json`) isn't empty: a
command line (`:ci 3`), a mode (`-- INSERT --`) or a message (`"area.pdf" exported`). It fades
in over the first 150 ms of a run of such frames and out over its last 150 ms, and stays up
through a gap shorter than 500 ms rather than blink. Keys that show nowhere on the left (`5j`,
`s`, `<C-a>`) don't bring it up.

The mp4 is 30 fps, libx264 CRF 18, yuv420p. The GIF is 15 fps with a 128-colour palette
(`palettegen stats_mode=diff`, `paletteuse dither=none diff_mode=rectangle`), as
keybr-tui's demo is.

## Script format

Plain text, one directive a line. Blank lines and lines starting with `#` are skipped.

| Line | Does |
|---|---|
| `window <width> <height> <scale>` | The window in logical pixels and a whole scale factor from 1 to 4. Frames are `width x scale` by `height x scale` pixels; `render.sh` shows them at the logical size |
| `view <x> <y> <zoom>` | The fixed view: its centre in grid units and its zoom (1 is 40 logical pixels a grid unit). It must hold everything the cursor reaches: the driver fails if the window would follow the cursor |
| `file <name>` | The board's file name, on the status line. The board starts empty and unwritten |
| `scene <caption>` | A captioned section starts here (`scenes` in `demo.json`) |
| `pace <ms>` | Time from one key to the next from here on (80 ms until the first `pace`) |
| `wait <ms>` | Hold the frame showing |
| `keys <keys>` | Type keys in Vim notation, one every `pace`: everything after `keys ` counts, spaces too, and `<Esc>`, `<CR>`, `<Tab>`, `<Space>`, `<C-k>` are one key each (see [Key notation](../docs/keys.md#key-notation)). Write a trailing space as `<Space>` |

`window`, `view` and `file` are required; the script must end with a `wait`, so the last frame
shows. An export (`:exp`) shows its `exporting` frame for one `pace`, then its result.

## demo.json

Times are milliseconds from the start; `file` paths are relative to `demo/out/`.

```json
{
  "version": 1,
  "file": "area.lawh",
  "width": 2000, "height": 1132,
  "logical_width": 1000, "logical_height": 566, "scale": 2,
  "duration_ms": 33150,
  "frames": [{"file": "frames/frame-00001.png", "start_ms": 0, "duration_ms": 1000,
              "status_left": ""}],
  "keys": [{"at_ms": 1000, "key": "5", "display": "5", "named": false}],
  "scenes": [{"caption": "Type text where the cursor is", "start_ms": 1000, "end_ms": 4600}]
}
```

- `file`: the board's file name, as in `file`.
- `width`, `height`: the frames' size in pixels; `logical_*` and `scale` as in `window`.
- `frames`: every frame, in order; each shows from `start_ms` for `duration_ms`, until the next.
  `status_left` is what the status bar's left end shows on it: the command line while it is
  open (`:ci 3`), else the mode or a message (`-- INSERT --`, `"area.pdf" exported`); empty
  when it shows nothing.
- `keys`: every key typed: `key` in Vim notation (`<CR>`), `display` as a key display would
  show it (`Enter`; a character as itself), `named` when it has no character of its own.
- `scenes`: each `scene` line, until the next one or the end.
