//! `lawh export`, run as the built binary: every format, overwriting, the options, the exit
//! codes, and that a failed run writes nothing. Files go in a directory under `target/` (Cargo's
//! `CARGO_TARGET_TMPDIR`), not the system temp dir.
//!
//! No window opens: display variables are removed from the child's environment, so a run that
//! got as far as opening one would fail with a different message instead of showing it.

// Lets the helpers below unwrap like test functions (see clippy.toml).
#![cfg(test)]

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const AREA: &str = include_str!("../../../examples/area.lawh");

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

fn area_board() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("area.lawh")
}

fn lawh_in(dir: &Path, args: &[&OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lawh"))
        .args(args)
        .current_dir(dir)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("RUST_LOG")
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A directory for one test under `target/`, emptied first and removed when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("lawh-export-cli-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, contents).unwrap();
        path
    }

    /// The names in the directory, sorted.
    fn listing(&self) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// `lawh` with `args`, run in this directory.
    fn lawh(&self, args: &[&str]) -> Output {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        lawh_in(&self.0, &args)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A PNG's width, height and colour type from its header.
fn png_header(bytes: &[u8]) -> (u32, u32, u8) {
    assert!(bytes.starts_with(PNG_SIGNATURE));
    assert_eq!(&bytes[12..16], b"IHDR");
    let word = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    (word(16), word(20), bytes[25])
}

fn assert_written(out: &Output) {
    assert_eq!(out.status.code(), Some(0), "{}", stderr(out));
    assert_eq!(stdout(out), "", "nothing on stdout");
    assert_eq!(stderr(out), "", "nothing on stderr for a clean export");
}

#[test]
fn the_area_board_exports_to_every_format_and_a_second_run_replaces_the_file() {
    let dir = Dir::new("formats");
    let board = area_board();
    let board = board.to_str().unwrap();
    let mut first = Vec::new();
    for (name, signature) in [
        ("area.svg", &b"<?xml"[..]),
        ("area.png", PNG_SIGNATURE),
        ("area.pdf", b"%PDF-"),
        ("upper.PDF", b"%PDF-"),
    ] {
        assert_written(&dir.lawh(&["export", board, name]));
        let bytes = fs::read(dir.path(name)).unwrap();
        assert!(bytes.starts_with(signature), "{name}");
        first.push(bytes);
    }
    assert_eq!(first[2], first[3], "a PDF is the same bytes every time");
    // The board is the same on every run, so the files are too; the old file goes.
    for (name, bytes) in ["area.svg", "area.png", "area.pdf"].iter().zip(&first) {
        fs::write(dir.path(name), "old").unwrap();
        assert_written(&dir.lawh(&["export", board, name]));
        assert_eq!(&fs::read(dir.path(name)).unwrap(), bytes, "{name}");
    }
    // Nothing else is left behind (no temporary files, no board).
    assert_eq!(
        dir.listing(),
        ["area.pdf", "area.png", "area.svg", "upper.PDF"]
    );
    assert_eq!(fs::read_to_string(area_board()).unwrap(), AREA);
}

#[test]
fn scale_and_transparent_change_the_png() {
    let dir = Dir::new("options");
    let board = dir.write("area.lawh", AREA);
    let board = board.to_str().unwrap();
    assert_written(&dir.lawh(&["export", board, "default.png"]));
    assert_written(&dir.lawh(&["export", "--scale", "1", board, "one.png"]));
    assert_written(&dir.lawh(&["export", board, "--scale=4", "--transparent", "four.png"]));
    let header = |name: &str| png_header(&fs::read(dir.path(name)).unwrap());
    let (w2, h2, opaque) = header("default.png");
    let (w1, h1, _) = header("one.png");
    let (w4, h4, clear) = header("four.png");
    // 80 px a unit by default; 40 at scale 1, 160 at 4 (to a pixel of rounding).
    assert!(
        w1.abs_diff(w2 / 2) <= 1 && h1.abs_diff(h2 / 2) <= 1,
        "{w1}x{h1}, {w2}x{h2}"
    );
    assert!(
        w4.abs_diff(w2 * 2) <= 2 && h4.abs_diff(h2 * 2) <= 2,
        "{w4}x{h4}, {w2}x{h2}"
    );
    // RGB on paper, RGBA when transparent.
    assert_eq!((opaque, clear), (2, 6));
    // Transparent SVG and PDF too.
    assert_written(&dir.lawh(&["export", "--transparent", board, "clear.svg"]));
    assert_written(&dir.lawh(&["export", "--transparent", board, "clear.pdf"]));
}

#[test]
fn a_failed_formula_is_one_line_on_stderr_and_the_file_is_written() {
    let dir = Dir::new("failed-formula");
    let board = dir.write(
        "failing.lawh",
        "lawh version=1\n\
         math id=m1 x=0 y=0 \"A = pi r^2\"\n\
         math id=m2 x=0 y=2 \"frac(1,\"\n\
         math id=m3 x=0 y=4 \"frac(2,\"\n",
    );
    let out = dir.lawh(&["export", board.to_str().unwrap(), "out.png"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    assert_eq!(
        stderr(&out),
        "lawh: \"out.png\" exported, 2 formulas failed\n"
    );
    assert!(
        fs::read(dir.path("out.png"))
            .unwrap()
            .starts_with(PNG_SIGNATURE)
    );
}

#[test]
fn a_missing_board_exits_1_and_creates_nothing() {
    let dir = Dir::new("missing");
    let out = dir.lawh(&["export", "missing.lawh", "out.svg"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    let err = stderr(&out);
    assert!(err.starts_with("lawh: missing.lawh: "), "{err}");
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(dir.listing().is_empty(), "{:?}", dir.listing());
}

#[test]
fn exporting_onto_the_board_itself_exits_1_and_leaves_it() {
    let dir = Dir::new("own-file");
    let board = dir.write("board.svg", AREA);
    for out in ["board.svg", "./board.svg"] {
        let run = dir.lawh(&["export", "board.svg", out]);
        assert_eq!(run.status.code(), Some(1), "{out}");
        assert_eq!(
            stderr(&run),
            format!("lawh: cannot export \"{out}\": that is the board's own file\n")
        );
        assert_eq!(fs::read_to_string(&board).unwrap(), AREA);
    }
    #[cfg(unix)]
    {
        let real = dir.write("real.lawh", AREA);
        let link = dir.path("link.png");
        std::os::unix::fs::symlink("real.lawh", &link).unwrap();
        let run = dir.lawh(&["export", "real.lawh", "link.png"]);
        assert_eq!(run.status.code(), Some(1));
        assert_eq!(fs::read_to_string(&real).unwrap(), AREA);
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    }
    // `export a.lawh a.lawh` is a usage error: no .svg, .png or .pdf.
    let run = dir.lawh(&["export", "board.svg", "board.lawh"]);
    assert_eq!(run.status.code(), Some(2));
}

#[test]
fn failures_exit_1_and_write_nothing() {
    let dir = Dir::new("failures");
    // A board that doesn't parse: its path, line and column.
    dir.write("bad.lawh", "lawh version=1\n\nstar id=s1 x=0 y=0\n");
    let out = dir.lawh(&["export", "bad.lawh", "bad.png"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).starts_with("lawh: bad.lawh:3:1: unknown object kind `star`"),
        "{}",
        stderr(&out)
    );
    // A board over the file size cap, refused before it is read.
    let huge = dir.path("huge.lawh");
    fs::File::create(&huge)
        .unwrap()
        .set_len(1024 * 1024 + 1)
        .unwrap();
    let out = dir.lawh(&["export", "huge.lawh", "huge.png"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("lawh reads files of up to"),
        "{}",
        stderr(&out)
    );
    // A directory is no board.
    fs::create_dir(dir.path("sub")).unwrap();
    let out = dir.lawh(&["export", "sub", "sub.png"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).starts_with("lawh: sub: "), "{}", stderr(&out));
    // An empty board: nothing to export.
    dir.write("empty.lawh", "lawh version=1\n");
    let out = dir.lawh(&["export", "empty.lawh", "empty.pdf"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "lawh: cannot export \"empty.pdf\": nothing to export\n"
    );
    // A file that can't be written.
    dir.write("area.lawh", AREA);
    let out = dir.lawh(&["export", "area.lawh", "no-such-dir/area.svg"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).starts_with("lawh: cannot export \"no-such-dir/area.svg\": "),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        dir.listing(),
        ["area.lawh", "bad.lawh", "empty.lawh", "huge.lawh", "sub"]
    );
}

#[test]
fn usage_errors_exit_2_and_write_nothing() {
    let dir = Dir::new("usage");
    dir.write("area.lawh", AREA);
    let cases: &[(&[&str], &str)] = &[
        (&["export"], "export needs a board and a file to write"),
        (&["export", "area.lawh"], "export needs a file to write"),
        (
            &["export", "area.lawh", "a.png", "b.png"],
            "unexpected argument 'b.png'",
        ),
        (
            &["export", "--zoom", "area.lawh", "a.png"],
            "unknown option '--zoom'",
        ),
        (
            &["export", "area.lawh", "a.jpg"],
            "export needs .svg, .png or .pdf, not 'a.jpg'",
        ),
        (
            &["export", "area.lawh", "a"],
            "export needs .svg, .png or .pdf, not 'a'",
        ),
        (
            &["export", "--scale", "2", "area.lawh", "a.svg"],
            "--scale is for .png exports, not .svg",
        ),
        (
            &["export", "area.lawh", "a.pdf", "--scale=1"],
            "--scale is for .png exports, not .pdf",
        ),
        (
            &["export", "--scale", "9", "area.lawh", "a.png"],
            "bad value '9' for --scale",
        ),
        (
            &["export", "--scale", "x", "area.lawh", "a.png"],
            "bad value 'x' for --scale",
        ),
        (
            &["export", "area.lawh", "a.png", "--scale"],
            "--scale needs",
        ),
    ];
    for (args, message) in cases {
        let out = dir.lawh(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(stdout(&out), "", "{args:?}");
        let err = stderr(&out);
        assert!(
            err.starts_with(&format!("lawh: {message}")),
            "{args:?}: {err}"
        );
        assert!(err.contains("Usage: lawh [file.lawh]"), "{args:?}: {err}");
    }
    // `lawh export` alone says how to open a board named `export`.
    let err = stderr(&dir.lawh(&["export"]));
    assert!(
        err.starts_with(
            "lawh: export needs a board and a file to write; to open a board named export, \
             type lawh ./export or lawh -- export\n"
        ),
        "{err}"
    );
    assert_eq!(dir.listing(), ["area.lawh"]);
}

#[test]
fn a_board_named_export_opens_with_a_path_or_after_the_double_dash() {
    // A bad board named `export`: opening it fails with its path, line and column, before any
    // window, which shows the window's open read it.
    let dir = Dir::new("named-export");
    dir.write("export", "lawh version=1\n\nstar id=s1 x=0 y=0\n");
    for (args, path) in [
        (&["./export"][..], "./export"),
        (&["--", "export"], "export"),
    ] {
        let out = dir.lawh(args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        let err = stderr(&out);
        assert!(
            err.starts_with(&format!("lawh: {path}:3:1: unknown object kind `star`")),
            "{args:?}: {err}"
        );
        assert_eq!(err.lines().count(), 1, "{err}");
    }
}

#[test]
fn help_lists_the_export_form() {
    let dir = Dir::new("help");
    let out = dir.lawh(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    let help = stdout(&out);
    assert!(
        help.contains("lawh export [--scale N] [--transparent] <board.lawh> <out.svg|png|pdf>"),
        "{help}"
    );
    assert!(help.contains("lawh ./export or lawh -- export"), "{help}");
    assert!(help.contains("--transparent"), "{help}");
    assert_eq!(stdout(&dir.lawh(&["export", "--help"])), help);
}
