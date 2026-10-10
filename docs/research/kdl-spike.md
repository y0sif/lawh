# KDL file format spike (2026-10-10)

Question: can `.lawh` files use KDL (`kdl` crate 6.7.1, Apache-2.0, MSRV 1.95) without
problems, or do we need our own format? Measured with a throwaway release build
(`~/.cache/lawh-kdl-spike`), on y0sif's laptop.

## Results

| Check | Result |
|---|---|
| Typst math with `"`, `\`, `#`, `$`, `//` round-trips exactly | Pass |
| Multi-line text with quotes, tabs and backslashes round-trips exactly | Pass (written as one escaped line, `\n`) |
| Floats (0.1, 0.1+0.2, -2.5, 1e-7, 123456.789, 3.0) round-trip exactly | Pass |
| Edit one value in a hand-written file, keep comments and spacing | Pass, **but** only after `KdlEntry::clear_format()`. `set_value` alone keeps printing the old value. |
| Error message for a broken file | Good: line, column, a pointer and a hint (miette) |
| Parse speed | 1k nodes 38 ms, 10k nodes 383 ms, 100k nodes 3.9 s (about 1.2 MB/s). Turning off the `span` feature made no difference. |
| Serialize speed | 1k nodes 2 ms, 10k nodes 23 ms |

## Ex-command lines are not KDL

| Line | As KDL |
|---|---|
| `circle r=3`, `rect w=4 h=2`, `sym integral`, `w`, `export svg` | Parses |
| `rect 4x2`, `e ~/notes/area.lawh`, `s/foo/bar/g` | Parse error |
| `math A = pi r^2` | Parses, but **wrongly**: becomes property `A=pi` plus argument `r^2` |

## Decision

- Use KDL for `.lawh` files, behind a `format` module in `lawh-core`, with a `lawh version=1`
  header so the format can change later.
- Write files ourselves in a fixed shape (one node per line, stable property order) for clean diffs.
- The `:` command line gets its own small Vim-style parser. Commands that create objects use the
  same `key=value` names as the file, so the two read alike, but they are not one parser.
- Revisit if real boards pass a few thousand objects: a 10k-object file takes about 0.4 s to load,
  which would also slow live reload.
