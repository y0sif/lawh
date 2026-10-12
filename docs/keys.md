# lawh whiteboard keys

Every key and `:` command the lawh whiteboard takes today, from the code on
`feat/m3-sizes-jumps`. How they work inside is in [architecture.md](architecture.md#the-engine).

Notation: `{count}` is a number typed first (`3w`). Keys are in Vim notation: `<CR>` is Enter,
`<Esc>` Escape, `<BS>` Backspace, `<C-r>` Ctrl+R, `<S-Tab>` Shift+Tab. Keys match the character
typed, whatever the keyboard layout.

## Modes

| Mode | What it is for | Status line | Enter with | Leave with |
|---|---|---|---|---|
| Normal | Moving on the board, making and changing objects | (empty) | `<Esc>` from the others | |
| Visual | Selecting objects with a rectangle | `-- VISUAL --` | `v` | `v`, `<Esc>`, `d`, `x`, `y`, `:` |
| Insert | Typing into a text or math block | `-- INSERT --` | `i a I A o O` | `<Esc>` |
| Block-Normal | Vim's Normal mode inside a text or math block | `-- TEXT --` | `<CR>` on a text or math object | `<Esc>` |
| Command line | Typing a `:` command | the line itself | `:`; `<C-k>` in Insert mode opens a `:sym` line | `<CR>` runs, `<Esc>` cancels |

Jump labels (`s`, see [Jump labels](#jump-labels)) are not a mode: they show over Normal mode,
Visual mode or a pending `d` or `y` until a label is typed or they end.

## The cursor

The cursor is either a **grid point** or an **object** (the current object), never both. The
status line shows its point as `x,y`, then the object's id when it is on one (`3,-2 c1`).

An object's **anchor** is where the cursor stands on it and where new objects go: the top-left
corner of text, math and rectangles, the centre of a circle or ellipse, the first end of a line or
arrow. New objects appear at the cursor point and become the current object.

## Normal mode

### Counts

Type a number before a command to repeat it: `5j`, `3w`, `2dd`. Counts stop at 10,000. An
operator (`d` or `y`: a key that acts on the objects reached by the motion typed after it, see
[Changing objects](#changing-objects)) and its motion may each take a count, and the two
multiply: `2d3w` is `d6w`.

### Moving

| Keys | Moves to |
|---|---|
| `h` `j` `k` `l` | One grid unit left, down, up, right. From an object, start at its anchor; the cursor becomes a grid point |
| `w` | The next object in reading order (rows top to bottom, left to right within a row) |
| `b` | The previous object in reading order |
| `H` `J` `K` `L` | The nearest object to the left, below, above, to the right |
| `gg` | The first object in reading order; `{count}gg` the count-th |
| `G` | The last object in reading order; `{count}G` the count-th |
| `s{label}` | The object of the jump label typed (see [Jump labels](#jump-labels)) |

An object motion that finds nothing leaves the cursor where it is. From a grid point, `w` and `b`
start from that point.

### Changing objects

The operators `d` (delete) and `y` (yank, which copies) take objects into the unnamed register:
the one place lawh keeps what was deleted or yanked, which `p` and `P` put from. They need the
cursor on an object; on a grid point the status line says "no object under the cursor". Only
`ds{label}` and `ys{label}` work from a grid point too.

| Keys | Acts on |
|---|---|
| `dd`, `yy` | This object; `{count}dd` this one and the next count - 1 in reading order |
| `x` | As `dd` |
| `dw`, `yw` | This object; `d3w` this one and the next two |
| `db`, `yb` | The previous object; `d3b` the three before this one, not this one |
| `dH` `dJ` `dK` `dL` | This object and the nearest one in that direction; with a count, each one reached |
| `dgg`, `dG` | This object, the first (or last) object, and everything between in reading order |
| `ds{label}`, `ys{label}` | The object of the jump label typed, alone; the cursor stays where it was |
| `p` | Put the register below the current object, clear of others; on a grid point, with its first object's anchor at the cursor. `{count}p` puts count copies, each below the last |
| `P` | Put the register above the current object; `{count}P` stacks copies upward |

A grid motion after `d` or `y` cancels it. After a delete the cursor is the grid point where the
first deleted object was, so `dd` then `p` puts it back. After a put the cursor is on the first
object put. Text deleted inside a block with `x` puts as a new text block, at the `size` option's
size.

**Where `p`, `P`, `o` and `O` put things** (the place rule): half a unit below (or above) the
current object, left edges lined up, moved further down (or up) past any object in the way, with
y rounded to a tenth of a unit.

### Other Normal keys

| Keys | Does |
|---|---|
| `i` `a` `I` `A` | On a grid point: a new text block there, in Insert mode, at the `size` option's size. On a text or math object: Insert mode at the start (`i`, `I`) or the end (`a`, `A`) of its text |
| `o` `O` | A new text block below or above the current object (or grid point), in Insert mode. From a text or math block it takes that block's size, else the `size` option's. `{count}o` makes count blocks |
| `<CR>` | On a text or math object: block-Normal mode at the start of its text |
| `<C-a>` `<C-x>` | The current text or math block one size step larger or smaller; `{count}<C-a>` steps count times (see [Sizes](#sizes)) |
| `s` | Jump labels on every object in view (see [Jump labels](#jump-labels)) |
| `v` | Visual mode |
| `:` | The command line |
| `u` | Undo; `{count}u` undoes count changes |
| `<C-r>` | Redo; `{count}<C-r>` |
| `.` | Repeat the last change; `{count}.` with a new count |
| `@:` | Run the last `:` command again |

`.` repeats `d`, `x`, `p`, `P`, `o`, `O` and `i a I A` with what was typed, `<C-a>` and `<C-x>`,
and `ds{label}`, which shows the labels again for a new label (`.` never deletes an object you
didn't pick). Visual changes and `:` commands aren't repeated by `.`; use `@:` for commands. A
count on an insert types it again: `3ia<Esc>` types `aaa`.

Undo takes back one Normal command, one Insert session (from entering Insert mode to leaving it)
or one `:` command at a time, and puts the cursor back where it was. The history keeps up to
1,000 changes, fewer when they are large: past about 16 MiB (estimated) the oldest are dropped.

## Visual mode

`v` starts a rectangle at the cursor (a point, or the current object's bounds). Moving the cursor
stretches it to the cursor's point or object. Objects fully inside the rectangle are selected and
highlighted.

| Keys | Does |
|---|---|
| `h j k l`, `w b`, `H J K L`, `gg G`, with counts | Move the cursor |
| `s{label}` | Move the cursor to the object of the jump label typed, stretching the rectangle |
| `d`, `x` | Delete the selection, back to Normal mode |
| `y` | Yank the selection, back to Normal mode |
| `<C-a>` `<C-x>` | Every text and math block selected one size step larger or smaller, each from its own size, as one undo step; shapes are skipped. Visual mode stays. `{count}<C-a>` steps count times |
| `v`, `<Esc>` | Back to Normal mode |
| `:` | Leave Visual mode and open the command line. The selection stays highlighted while the line is open, for `:si` |

What is highlighted is what `d`, `x`, `y` and `:si` act on. After `<C-a>` or `<C-x>` the selection
stays the same objects, those that grew out of the rectangle too, and takes in none that the
rectangle now holds only because a block grew; the next motion selects from the rectangle again.
`.` doesn't repeat Visual changes, so `<C-a>` and `<C-x>` here forget the last change.

## Sizes

Text and math blocks have a size: a multiplier on the height lawh draws text at. Size 1 is the
default; size 2 is twice as tall and twice as wide. Sizes go from 0.25 to 16.

Math is drawn to match the text beside it. At the same size a formula's lowercase letters are as
tall as the text's (the same **x-height**, the height of a lowercase `x`), and a one-line formula
at the same `y` as a text stands on the same **baseline** (the line letters sit on).

| Keys or command | Does |
|---|---|
| `<C-a>` | The current text or math block one step larger; `{count}<C-a>` count steps |
| `<C-x>` | One step smaller; `{count}<C-x>` |
| `:si[ze] N` | Set the size to N |
| `:si +N`, `:si -N` | Add N to the size, or take N from it |
| `:set size=N` | The size of new blocks |
| `:te ++size=N <text>`, `:ma ++size=N <formula>` | A new block of size N |

**Steps.** `<C-a>` and `<C-x>` step through `0.25 0.5 0.75 1 1.25 1.5 2 2.5 3 4 5 6 8 10 12 16`.
From a size between two steps (`:si 1.1`), the first press goes to the next step that way. At an
end the size stays and the status line says `16 is the largest size` or `0.25 is the smallest
size`. On a shape they change nothing and say `only text and math have a size`; on a grid point,
`no object under the cursor`. In block-Normal and Insert mode they do nothing.

**`:si`.** It acts on the current object, or, when `:` was typed in Visual mode, on every text
and math block selected. A size out of range is refused whole (`size 20 is out of range: sizes go
from 0.25 to 16`), so a selection changes all or nothing. `+N` and `-N` results are rounded to
0.001, as coordinates are.

**New blocks.** The `size` option (at most two decimals) sizes the blocks made by `i a I A` on a
grid point, `:te`, `:ma`, `:sym` on the board, `o` and `O` from a grid point or a shape, and `p`
of text deleted in a block. `o` and `O` from a text or math block take that block's size.

**`++size`.** Only `++name=value` words right after the command's name are options, as Vim's
`++opt`, and `++size=N` is the only one: `:te ++size=2 Title`. Another `++` option is an error.
`:te size=2 Title` makes the text `size=2 Title`.

Each size change is one undo step, with the cursor where it was.

## Jump labels

**Jump labels** (as in flash.nvim or hop for Neovim) reach any object in view in two or three
keys: `s` puts a short label of one or two characters on every object in view, and typing a label
acts on its object.

| Keys | Does |
|---|---|
| `s{label}` | Normal mode: the cursor goes onto the object |
| `s{label}` in Visual mode | The cursor goes onto the object, stretching the rectangle; Visual mode stays |
| `ds{label}`, `ys{label}` | Delete or yank that object alone. The cursor stays where it was, a grid point too |
| `<Esc>` | End the labels. In Normal and Visual mode only the labels end; after `d` or `y` the operator ends too |
| `<BS>` | Take back the first character typed; with none typed, end the labels |

- **Which objects:** every object whose bounds overlap the window above the status bar, except
  the current object. The view is the one on screen when the key was typed: inside a mapping
  that moves first (`10ls`), the labels go on what was in view before the move.
- **Which labels:** the objects nearest the cursor (measured to their anchors) get one
  character, the rest two, never three. Ties go by reading order, then by order in the file.
  Labels are **prefix-free**: no label is the start of another, so a label acts as soon as it is
  typed. With the default 26 letters every object of a full board gets a label (26 x 26 = 676
  two-character labels). With fewer characters, objects past the square of their number get none
  and the status line says `N objects in view have no label`.
- **Keys:** a key that starts no label ends the labels, as `<Esc>` does, and is dropped:
  `no label "x"`. Nothing jumps until a whole label is typed, even with one object labelled.
  Label keys are never mapped, as the key after Vim's `f` isn't, so a label `;` works under
  `noremap ; l`, and they never wait for a mapping. A count before `s` is ignored.
- **Mouse:** a click ends the labels. Panning and zooming keep them on their objects.
- **Nothing to label:** `no objects in view`.
- `.` after `ds{label}` shows the labels again, for a new label. Undo of `ds{label}` puts the
  object back and the cursor where it was.
- The status line shows the keys so far: `s`, `ds`, `dsk`.
- The characters are the `jumplabels` option, the first for the nearest object.

## Text and math blocks

Text and math objects are blocks of text you can edit. The cursor moves and deletes by
grapheme (what you see as one character), so an accented letter or an emoji is one step.

### Insert mode

| Keys | Does |
|---|---|
| A character | Types it (also with Alt held, for layouts that type characters with AltGr or Option) |
| `<CR>` | A line break |
| `<Tab>` | A tab |
| `<BS>`, `<Del>` | Delete the character before or after the cursor |
| `<Left>` `<Right>` | One character, within the line |
| `<Up>` `<Down>` | One line, keeping the column |
| `<Home>` `<End>` | The start or end of the line |
| `<C-k>` | A `:sym` line, to put in a symbol by name, then back to Insert mode (see [Symbols](#symbols-sym)) |
| `<Esc>` | Leave Insert mode (where to: the `escape` setting) |

Other Ctrl and Cmd chords type nothing. A new block still empty when you leave Insert mode is
removed.

### Block-Normal mode

| Keys | Does |
|---|---|
| `h` `l` | One character left or right, within the line |
| `j` `k` | One line down or up, keeping the column |
| `0` `$` | The start or end of the line |
| `w` `b` `e` | Vim's word motions, across lines |
| `W` `B` `E` | Vim's WORD motions: runs of characters that aren't blanks (spaces, tabs, line breaks), across lines |
| `x` | Delete the character under the cursor (into the register); `{count}x` |
| `i` `a` | Insert mode before or after the character under the cursor |
| `I` `A` | Insert mode at the first non-blank character, or the end, of the line |
| `u` `<C-r>` `.` | Undo, redo, repeat (`.` repeats `x` and inserts) |
| `:` | The command line; it comes back to the block unless the command moved the cursor off it |
| `<Esc>` | Back to the board, the block as the current object |

Motions, `x`, the inserts, `u`, `<C-r>` and `.` take counts. Other keys do nothing, `<C-a>` and
`<C-x>` included. On the board `W`, `B` and `E` do nothing, so they are free to map.

### Where `<Esc>` goes

| `escape` | `<Esc>` in Insert mode goes to |
|---|---|
| `board` (default) | Where Insert mode was entered from: the board (the block as the current object) or block-Normal mode |
| `block` | Block-Normal mode; `<Esc>` there goes on to the board |

A math block is drawn as its source while you edit it, and as math again once you leave.

## The command line

| Keys | Does |
|---|---|
| Characters | Type at the cursor (at most 5,064 characters) |
| `<BS>` | Delete the character before the cursor; on an empty line, cancel |
| `<Left>` `<Right>` `<Home>` `<End>` | Move the cursor |
| `<Tab>`, `<C-n>` | On a `:sym` line: select the next row and put its name on the line |
| `<S-Tab>`, `<C-p>` | On a `:sym` line: the previous row |
| `<CR>` | Run the command |
| `<Esc>` | Cancel |

## `:` commands

A command may be cut short down to the letters in brackets: `:ci`, `:cir` ... `:circle`. Only
`:w`, `:q`, `:x` and `:u` take one letter. Errors are plain English on the status line.

### Shapes, text and math

Shapes go at the cursor point (the grid point, or the current object's anchor) unless `x=` `y=`
place them (`x1=` `y1=` for a line or an arrow). Positional numbers are sizes for `:ci`, `:el`
and `:re`, and offsets from the start for `:li` and `:ar`. `key=value` uses the file's names, is
absolute, and wins over a positional.

| Command | Makes |
|---|---|
| `:ci[rcle] R` | A circle of radius R centred on the cursor. Keys: `x=` `y=` `r=` |
| `:el[lipse] RX RY` | An ellipse centred on the cursor. Keys: `x=` `y=` `rx=` `ry=` |
| `:re[ct] W H` | A rectangle with its top-left corner at the cursor. Keys: `x=` `y=` `w=` `h=` |
| `:li[ne] DX DY` | A line from its start to the start + (DX, DY). The start is the cursor point, or `x1=` `y1=`. Keys: `x1=` `y1=` `x2=` `y2=` |
| `:ar[row] DX DY` | An arrow, the same way, its head at the end |
| `:te[xt] [++size=N] <text>` | A text block of everything after the first space and the options, as typed |
| `:ma[th] [++size=N] <formula>` | A math block in [Typst](https://typst.app) math syntax: `:ma A = pi r^2` |

Examples: `:ci 3`, `:circle r=3 x=0 y=0`, `:rect 4 2`, `:li 3 0`, `:ar 0 -2`, `:te ++size=2 Title`.

Numbers: an optional sign, digits and an optional fraction (`-2.5`, `3`, `.5`, `2.`); no
exponent, `inf` or `nan`. Radii, widths and heights must be above 0 and at most 1,000,000;
coordinates within -1,000,000 and 1,000,000. A board holds at most 500 objects.

### Sizes: `:si`

| Command | Does |
|---|---|
| `:si[ze] N` | Set the size of the current text or math block, from 0.25 to 16 |
| `:si +N`, `:si -N` | Add N to it, or take N from it |

From Visual mode they act on every text and math block selected. See [Sizes](#sizes).

### Symbols: `:sym`

`:sy[m] <query>` finds a math symbol by its [Typst](https://typst.app) name and puts it in:
`:sym integral` finds `integral` (∫). As you type, the best 8 matches show above the status line,
best first, each its name and the symbol.

Matching is **fuzzy**: a name matches when it holds the query's letters in order, not
necessarily side by side, so `:sym arrl` finds `arrow.l`. Matches nearer the start of a name rank
higher. The query takes fzf's syntax: each word separated by spaces must match; `^word` matches
the start of a name, `word$` its end, `'word` a run of characters exactly as typed, and `!word`
leaves out the names it matches. A word with a capital letter matches case as typed; other words
ignore case.

| Keys | Does |
|---|---|
| `<Tab>`, `<C-n>` | Select the next row and put its name on the line; past the last, the query comes back as typed |
| `<S-Tab>`, `<C-p>` | The previous row |
| `<CR>` | Pick the symbol named exactly as the line says, else the best match |
| `<Esc>` | Cancel |

What a pick puts in depends on where the command line was opened:

| Opened from | Puts in |
|---|---|
| A math block (block-Normal mode, or Insert mode with `<C-k>`) | The Typst name, `alpha`, with a space before it where the text before ends in a name and after it where a letter or digit follows, so Typst reads it as a name of its own (`xalpha` and `alpha2` would be other names; `2alpha` is 2 times alpha) |
| A text block | The symbol itself, `α`. A symbol the text font (Inter) lacks is drawn from the math font: **font fallback**, drawing a character in the next bundled font that has it |
| The board | A new math block of the name at the cursor, as `:ma alpha` makes it, at the `size` option's size |

- Only symbols the bundled fonts can draw are listed: 732 of the 1,189 names Typst math knows
  (counted from Typst's symbol tables, codex 0.3.0).
- A pick in block-Normal mode goes in before the character under the cursor, as `i` would type
  it, and the cursor ends on its last character. A pick there or on the board is one undo step.
  `.` doesn't repeat it (`:` commands never go through `.`); on the board, `@:` picks again.
- **`<C-k>` in Insert mode** opens the command line with `sym ` typed. `<CR>` picks and goes back
  to Insert mode with the caret after what went in; `<Esc>`, or `<BS>` on an empty line, goes
  back with nothing put in. Only `:sym` runs from there: another command says
  ``from Insert mode the command line runs only `:sym` ``. The pick is part of the Insert session
  and its one undo step, and `.` (or a count on the key that started Insert mode) types the same
  characters again, not the picker's keys.
- Messages: `no symbol matches "xyz"`; a query longer than 64 characters,
  ``a `:sym` query holds at most 64 characters``.

### Files and quitting

| Command | Does |
|---|---|
| `:w[rite]` | Write the board to its file |
| `:w <file>` | Write to `<file>`. An unnamed board takes the name once the write succeeds; a named board writes a copy and keeps its name, unless `<file>` is the board's own file under another name |
| `:w! [file]` | As `:w` |
| `:q[uit]` | Quit, unless the board has unwritten changes |
| `:q!` | Quit, dropping unwritten changes |
| `:wq [file]` | Write, then quit. Doesn't quit while the board has unwritten changes: a copy to another file leaves them unwritten |
| `:wq! [file]` | Write, then quit anyway |
| `:x[it] [file]` | With unwritten changes, as `:wq`; without, quit without writing |
| `:x! [file]` | With unwritten changes, as `:wq!`; without, quit without writing |

Writes are atomic: the file is always either the old board or the new one. A relative path is
taken from the directory lawh was started in, and a leading `~` (`~` alone, `~/`, and `~\` on
Windows) is your home directory; `~user` is not expanded. `:w !cmd` and `:w >>file` are refused.

A write to the board's own file marks the board saved however the path was typed: an absolute
path, `./`, `~/` or a link to it all count, since lawh compares the files themselves after the
write, not their names.

### Undo and settings

| Command | Does |
|---|---|
| `:u[ndo]` | Undo one change |
| `:red[o]` | Redo one change |
| `:se[t]` | Show every option |
| `:set {option}={value}` | Set an option (`:` works as `=`) |
| `:set {option}?`, `:set {option}` | Show an option |
| `:set {option}&` | Put an option back to its default |

Several options may follow one `:set`. A word starting with `"` ends the line.

| Option | Values | Default | Does |
|---|---|---|---|
| `escape` | `board`, `block` | `board` | Where `<Esc>` goes from Insert mode (see above) |
| `jumplabels` | 2 to 52 different characters, none of them a space or a control character | `asdfghjklqwertyuiopzxcvbnm` | The characters jump labels are made of, the first for the nearest object |
| `leader` | One key in Vim notation, such as `<Space>` or `,` | `\` | What `<Leader>` stands for in mappings made after it |
| `size` | 0.25 to 16, at most two decimals | `1` | The size of new text and math blocks (see [Sizes](#sizes)) |
| `timeoutlen` (`tm`) | 0 to 10,000 | 1000 | Milliseconds keys that start a longer mapping wait for the rest |

The default `jumplabels` is flash.nvim's: the home row, then the top row, then the bottom row,
each from the left.

## Key mappings

A mapping makes keys typed in one mode run other keys, as in Vim: `:nnoremap U <C-r>` makes `U`
redo on the board. `noremap` mappings run their keys as built in; `map` mappings run them through
other mappings too.

| Commands | Short forms | Modes |
|---|---|---|
| `:map` `:noremap` `:unmap` `:mapclear` | `:map` `:no` `:unm` `:mapc` | Normal, Visual, operator-pending |
| `:map!` `:noremap!` `:unmap!` `:mapclear!` | `:map!` `:no!` `:unm!` `:mapc!` | Insert, command line |
| `:nmap` `:nnoremap` `:nunmap` `:nmapclear` | `:nm` `:nn` `:nun` `:nmapc` | Normal (the board and block-Normal mode) |
| `:xmap` `:xnoremap` `:xunmap` `:xmapclear` | `:xm` `:xn` `:xu` `:xmapc` | Visual |
| `:vmap` `:vnoremap` `:vunmap` `:vmapclear` | `:vm` `:vn` `:vu` `:vmapc` | Visual (the same as `x` in lawh) |
| `:omap` `:onoremap` `:ounmap` `:omapclear` | `:om` `:ono` `:ou` `:omapc` | Operator-pending: after `d` or `y` |
| `:imap` `:inoremap` `:iunmap` `:imapclear` | `:im` `:ino` `:iu` `:imapc` | Insert |
| `:cmap` `:cnoremap` `:cunmap` `:cmapclear` | `:cm` `:cno` `:cu` `:cmapc` | Command line |

| Form | Does |
|---|---|
| `:nnoremap {keys} {to}` | Map `{keys}` (up to the first space) to `{to}` (the rest of the line, spaces at its ends dropped) |
| `:nmap {keys}` | Show the mappings of `{keys}` |
| `:map` | Count the mappings |
| `:nunmap {keys}` | Remove a mapping |
| `:nmapclear` | Remove every mapping of the mode |

- Both sides are in Vim key notation (below), at most 256 keys each. `|` is a key, not a new
  command.
- `<Leader>` is the `leader` setting at the time the mapping is made. `<Nop>` maps to nothing.
- `<nowait>` before the keys runs the mapping at once, without waiting to see whether a longer
  mapping is being typed. `<silent>` and `<special>` are accepted and change nothing.
- Not supported: `<buffer>`, `<expr>`, `<script>`, `<unique>`, `<Cmd>`, `<Plug>`, `<SID>`,
  `<SNR>`, `<ScriptCmd>`, `<LocalLeader>`.
- Keys that start a longer mapping wait `timeoutlen` milliseconds for the rest, shown on the
  status line, then run the longest mapping they make, or run as typed.
- Keys typed while jump labels show are never mapped. The completion keys of a `:sym` line
  (`<Tab>`, `<C-n>`, `<S-Tab>`, `<C-p>`) go through `cmap` mappings first.
- At most 1,000 mappings a mode. One typed key may expand at most 1,000 mappings ("recursive
  mapping" past that) and set off at most 30,000 keys.
- Everything one typed key sets off also shares two caps. It may ask for at most 4 writes
  (`MAX_WRITES_PER_KEY`); past that: "that key asked for more than 4 writes; the rest of it was
  dropped". And it has a work budget (`MAX_KEY_WORK`): each command counts the work it does
  (objects looked at, bytes of text walked, text laid out to measure it, blocks resized, jump
  labels made, symbol names matched), and past the budget: "that key did too much work; the rest
  of it was dropped". The key stops where it got to, and the board is left as after any command.

### Key notation

| Write | For |
|---|---|
| `a`, `A`, `:` | The character itself |
| `<Esc>` `<CR>` (or `<Enter>`, `<Return>`) `<Tab>` `<BS>` `<Del>` `<Space>` | Named keys |
| `<lt>` `<Bar>` `<Bslash>` | `<`, `\|` and `\` |
| `<Up>` `<Down>` `<Left>` `<Right>` `<Home>` `<End>` `<PageUp>` `<PageDown>` `<Insert>` | Named keys |
| `<F1>` to `<F24>` | Function keys |
| `<C-r>`, `<M-x>` (or `<A-x>`), `<S-Tab>`, `<D-s>` | Ctrl, Alt, Shift, Cmd; they stack: `<C-S-Tab>` |

Names ignore case. `<C-R>` is `<C-r>`, `<C-[>` is `<Esc>`, `<S-a>` is `A`. A `<` that starts no
name is a plain `<`.

## lawhrc

lawh reads settings and mappings at start from `lawhrc` in its config directory:

| System | Path |
|---|---|
| Linux, macOS | `~/.config/lawh/lawhrc` (or `$XDG_CONFIG_HOME/lawh/lawhrc`) |
| Windows | `%APPDATA%\y0sif\lawh\config\lawhrc` |

- One `:` command a line; the `:` is optional. Only `:set` and the mapping commands are allowed.
- Blank lines, and lines starting with `"`, are comments.
- A line that fails is skipped and the rest still load. A line that maps keys mapped earlier in
  the file replaces the earlier mapping, with a warning.
- The first warning shows on the status line (`lawhrc line 5: ... (2 more warnings in the log)`);
  all of them go to the log on stderr.
- At most 1 MiB, UTF-8. A missing file is fine.

To start from the sample, which moves with `jkl;` instead of `hjkl`:

```bash
mkdir -p ~/.config/lawh
cp examples/lawhrc ~/.config/lawh/lawhrc
```

## Mouse and window

| Action | How |
|---|---|
| Pan | Drag with the left or middle mouse button. Jump labels stay on their objects |
| Zoom | Mouse wheel or touchpad scroll, at the pointer, from 0.1 to 20. Jump labels stay on their objects |
| Select | Click an object to make it the current object, or empty space to put the cursor on the nearest grid point. A click ends jump labels, Insert, block-Normal and Visual mode and the command line, and drops keys waiting for a mapping |
| Close | The window's close button (Cmd+Q on macOS). With unwritten changes the first close only warns; close again while the warning shows to quit (a key, a click on the board or a mapping timeout drops it; panning and zooming don't) |

The view follows the cursor as you type.

## Starting lawh

```bash
lawh                 # an empty board with no name: :w name.lawh names it
lawh board.lawh      # open board.lawh; if it doesn't exist, an empty board that :w creates
```

The cursor starts on the board's first object in reading order, as after `gg`. On an empty board
it starts on the grid point 0,0.

Logs go to stderr. lawhrc warnings show at the default level; `RUST_LOG=info lawh board.lawh`
logs more.
