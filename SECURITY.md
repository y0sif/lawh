# Security policy

## Reporting a vulnerability

Report it privately through a
[GitHub Security Advisory](https://github.com/y0sif/lawh/security/advisories/new), not a public issue.
Expect a first reply within 48 hours.

## Scope

lawh is a local desktop app with no accounts and no server. The risks come from opening files
someone else made.

**In scope:**

- A `.lawh` file that makes lawh read or write files, reach the network, run code outside the
  Typst sandbox, or run keys or `:` commands when you open it.
- Math blocks: Typst code inside a board must not escape the sandbox (no file system, no package
  downloads, bundled fonts only).
- Parser bugs that crash lawh or corrupt a board on save.
- A file that hangs or exhausts memory when opened (denial of service), past the limits below.
- A key, mapping or lawhrc that freezes lawh past the per-key caps below.
- A `:w` that leaves a board half written, or writes somewhere other than the path asked for.

**Out of scope:**

- Issues in dependencies that lawh doesn't expose. Report those upstream.
- The OS warning on first launch: lawh releases are deliberately unsigned.
- The memory cap on macOS and Windows, which is known not to be in place yet (see below).
- What your own lawhrc mappings do when you type them: a mapping may run `:w` like any typed
  command.

## Threat model

Opening a board must be safe whoever wrote it. A board can attack lawh in two ways: through the
file itself, and through the Typst code in its math blocks. Keys and the lawhrc come from the user,
so they are trusted, but what one key may cost is still capped, so a mistake can't freeze the
window.

### An untrusted `.lawh` file

- **Size:** a file over 1 MiB is refused before it is read; lawh reads at most one byte past the
  limit, in case the file grows meanwhile.
- **Bounded reader:** lawh reads `.lawh` files with its own parser, in one pass and without
  recursion, so no nesting can overflow the stack. Every limit is checked while reading, so the
  time and memory a file costs follow what a board may hold, not the length of the file. The
  worst hostile file found took 32 ms to read.
- **Caps:** at most 500 objects, 6 entries per node, 64 KiB per string or number in the file,
  5,000 characters and 500 lines per text or formula, 40,000 characters of text on a board, ids of
  64 characters, coordinates and sizes within 1,000,000 grid units, text and math sizes from 0.25
  to 16, and block comments 16 levels deep. The full table is in
  [docs/architecture.md](docs/architecture.md#limits).
- **First problem stops it:** reading stops at the first problem, and no window opens for a file
  that fails.
- **Drawing:** every frame's work is budgeted, so a board built to be slow to draw can't freeze
  the window: objects over the budget are drawn as outlines. The largest text size is inside what
  the budget's glyph costs were measured for.

### An untrusted formula

Typst math can hold any Typst code (`#` expressions), and Typst can loop, can't be cancelled and
can recurse deeply. So:

- **A separate process:** every formula from a board compiles in a worker process, the lawh
  binary started again with a hidden flag. A formula that crashes, hangs or runs out of memory
  takes down only the worker, which lawh kills and replaces. A worker that sends anything malformed
  is killed too, and its output is checked as untrusted (frames of at most 16 MiB, layouts within
  fixed size and complexity caps).
- **Timeout:** a compile that takes more than 2 s gets the worker killed. The failure is cached,
  so the same formula isn't compiled again on every frame.
- **No file, package or clock access:** the Typst world sees only the formula. Every file read
  is denied, which also stops package imports, and there are no downloads. Fonts are the two
  bundled with lawh, never system fonts. There is no clock. `plugin` (WebAssembly), `image`,
  `read` and the data loaders (`json`, `yaml`, `toml`, `cbor`, `csv`, `xml`), `raw`, `cite` and
  `bibliography` are removed from the library.
- **Memory cap, Linux only:** on Linux the worker caps its own address space at 768 MiB
  (`RLIMIT_AS`), so an allocation past it ends the worker.
- **No core dumps, on Unix:** on Linux and macOS the worker turns core dumps off (`RLIMIT_CORE`),
  so a crash doesn't write the worker's memory to disk.

**The memory cap is not in place on macOS or Windows yet.** macOS accepts the same limit but
doesn't enforce it, and Windows needs a Job Object (the Windows feature that sets limits on a
group of processes), which lawh doesn't create yet. On both, a hostile formula can allocate as
much memory as it can in the 2 s before the worker is killed.

The worker runs with one Typst thread, so a runaway compile keeps at most one core busy until it is
killed.

**CPU per board (derived, not measured):** the worker compiles one formula at a time, and each
compile ends at the 2 s timeout. A board holds at most 500 formulas, and every outcome is cached,
failures and timeouts included, so a hostile board costs at most about 17 minutes of one core
(500 x 2 s) of first compiles. Formulas whose layouts don't fit in the cache together are compiled
again, but only while they are on screen (or were in the last 30 s), and at most 8 + 2t times in
t seconds. lawh stays responsive meanwhile:

- Math is async: the window never waits for a compile. `MathEngine::request` answers at once,
  and the engine's own thread talks to the worker.
- Only formulas on screen are asked for, nearest the centre of the view first.
- A formula that failed or timed out isn't compiled again: its failure is cached.

**The `:sym` list compiles nothing.** The symbols `:sym` offers come from codex's static tables
(Typst's own list of names, pinned to the version lawh's Typst takes them from) and the bundled
fonts' character maps, read in the app process at start. Nothing from a board or a keystroke goes
into making it, and no formula is compiled to make it. A pick puts a name into a math block,
which then compiles in the worker like any other formula.

**What the sandbox is not:** it is not an operating-system sandbox. The worker runs as your user
with the same permissions as lawh; what keeps Typst from your files and the network is the Typst
world lawh gives it, which offers no file, network or clock access. Treat a way around that world
as a vulnerability and report it.

### Keys, commands and the lawhrc

- **A board can't run commands.** A `.lawh` file is only ever read as a board: it has no
  modelines (Vim's settings embedded in a file), and nothing in it reaches the engine as keys or
  `:` commands. The app feeds the engine only keys typed in its window, and the lawhrc.
- **The lawhrc is trusted, and capped.** It is your own file in your config directory, read once at
  start, like a vimrc. It may only `:set` options and make mappings: shape, write and quit
  commands are refused there, so it can't change a board or a file by itself. Caps: 1 MiB
  (`MAX_RC_BYTES`; a larger file is refused from its size, unread), 1,000 mappings a mode
  (`MAX_MAPS`), 256 keys on each side of a mapping (`MAX_MAP_KEYS`). The slowest lawhrc the caps
  allow, 1 MiB of mappings made and dropped at the front of 999 others, loads in 676 ms in release
  and 1.7 s in debug (measured).
- **One key can't cause unbounded work.** Mappings, `.` and counts let one key run many commands,
  so each typed key, mapping timeout or click has a budget, reset at the next:
  - at most 1,000 mapping expansions (`MAX_MAP_DEPTH`) and 30,000 keys set off
    (`MAX_EXPANDED_KEYS`);
  - at most 100 million units of work (`MAX_KEY_WORK`), each kind of work weighted by what it
    costs: spending it all takes about 0.1 s in debug and 10 ms in release (derived from the
    measured cost of a unit; 30 ms in release when it was matching `:sym` names, 66 ms over the
    longest names, measured). Text the renderer lays out to measure it for a key counts too
    (`Measure::take_work`): before it did, a mapping that typed into a long text and then showed
    jump labels (the short labels `s` puts on objects to jump to) or moved froze the window for
    1.9 to 6.4 s in release; now it stops within 46 to 99 ms (measured);
  - at most 4 writes (`MAX_WRITES_PER_KEY`): about 11 ms, derived from 2.76 ms a write
    (measured, build not recorded).

  Past any of them the key stops with a message, its remaining keys are dropped, and the board is
  left consistent, as after any command. Before these caps, crafted mappings froze the window for
  1.8 s to 858 s in release (measured). In a fuzz of 4,000 random key scripts during review, the
  slowest key took 35 ms in release (measured).

  Milestone 3's commands have caps of their own: jump labels of at most 2 characters
  (`MAX_LABEL_KEYS`) from an alphabet of 2 to 52 (`MIN_LABEL_CHARS`, `MAX_LABEL_CHARS`); at most
  4,096 symbols for `:sym` (`MAX_SYMBOLS`); and a `:sym` query of at most 64 characters
  (`MAX_SYM_QUERY_CHARS`), one past 256 bytes (`MAX_SYM_QUERY_BYTES`) refused before it is copied
  or counted.
- **Atomic writes.** `:w` writes a new file next to the board, flushes it to disk and renames it
  over the board; on Unix it flushes the directory too. A crash or power cut leaves the old board
  or the new one, never half of each. On Unix the new file has the old file's permissions from
  the moment it is created, so a write never makes a board more readable.
- **Links.** A symbolic link is written through: the file it points at is replaced and the link
  stays. At most 40 links in a row are followed (`MAX_LINK_HOPS`); past that the write fails.
- **Same file.** After a good write, the app asks whether the file it wrote is the board's own
  under another name (`~/a.lawh`, an absolute path, a link), to mark the board saved. It follows
  links as the atomic write does, then compares device and inode on Unix, or canonical paths
  elsewhere. It only reads file metadata, after the write; a path it can't look up (a loop of
  links) counts as another file, so the board stays marked unsaved.
- **Where writes go.** Only to the path typed after `:w` (or the board's own), a relative one taken
  from the directory lawh started in. `:w !cmd` and `:w >>file` are refused: no shell runs.
