# Security policy

## Reporting a vulnerability

Report it privately through a
[GitHub Security Advisory](https://github.com/y0sif/lawh/security/advisories/new), not a public issue.
Expect a first reply within 48 hours.

## Scope

lawh is a local desktop app with no accounts and no server. The risks come from opening files
someone else made.

**In scope:**

- A `.lawh` file that makes lawh read or write files, reach the network, or run code outside the
  Typst sandbox when you open it.
- Math blocks: Typst code inside a board must not escape the sandbox (no file system, no package
  downloads, bundled fonts only).
- Parser bugs that crash lawh or corrupt a board on save.
- A file that hangs or exhausts memory when opened (denial of service), past the limits below.

**Out of scope:**

- Issues in dependencies that lawh doesn't expose. Report those upstream.
- The OS warning on first launch: lawh releases are deliberately unsigned.
- The memory cap on macOS and Windows, which is known not to be in place yet (see below).

## Threat model

Opening a board must be safe whoever wrote it. A board can attack lawh in two ways: through the
file itself, and through the Typst code in its math blocks.

### An untrusted `.lawh` file

- **Size:** a file over 1 MiB is refused before it is read; lawh reads at most one byte past the
  limit, in case the file grows meanwhile.
- **Bounded reader:** lawh reads `.lawh` files with its own parser, in one pass and without
  recursion, so no nesting can overflow the stack. Every limit is checked while reading, so the
  time and memory a file costs follow what a board may hold, not the length of the file. The
  worst hostile file found took 32 ms to read.
- **Caps:** at most 500 objects, 6 entries per node, 64 KiB per string or number in the file,
  5,000 characters and 500 lines per text or formula, 40,000 characters of text on a board, ids of
  64 characters, coordinates and sizes within 1,000,000 grid units, and block comments 16 levels
  deep. The full table is in [docs/architecture.md](docs/architecture.md#limits).
- **First problem stops it:** reading stops at the first problem, and no window opens for a file
  that fails.
- **Drawing:** every frame's work is budgeted, so a board built to be slow to draw can't freeze
  the window: objects over the budget are drawn as outlines.

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

**What the sandbox is not:** it is not an operating-system sandbox. The worker runs as your user
with the same permissions as lawh; what keeps Typst from your files and the network is the Typst
world lawh gives it, which offers no file, network or clock access. Treat a way around that world
as a vulnerability and report it.
