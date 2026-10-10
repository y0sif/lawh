//! The `lawh` command line: version, usage errors and board loading errors.
//!
//! None of these open a window. Display variables are removed from the child's environment, so a
//! run that got as far as opening one would fail with a different message instead of showing it.

// Lets the helpers below unwrap like test functions (see clippy.toml).
#![cfg(test)]

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn lawh(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lawh"))
        .args(args)
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

/// A board file in the system temp dir, removed when dropped.
struct TempBoard(PathBuf);

impl TempBoard {
    fn new(name: &str, contents: &str) -> Self {
        let path = Self::path(name);
        std::fs::write(&path, contents).unwrap();
        Self(path)
    }

    /// A file of `len` bytes that takes no disk space: the size is set, nothing is written.
    fn sparse(name: &str, len: u64) -> Self {
        let path = Self::path(name);
        std::fs::File::create(&path).unwrap().set_len(len).unwrap();
        Self(path)
    }

    fn path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("lawh-cli-test-{}-{name}.lawh", std::process::id()))
    }
}

impl Drop for TempBoard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn version_prints_name_and_version() {
    for flag in ["--version", "-V"] {
        let out = lawh(&[flag.as_ref()]);
        assert_eq!(out.status.code(), Some(0), "{flag}");
        assert_eq!(
            stdout(&out),
            format!("lawh {}\n", env!("CARGO_PKG_VERSION")),
            "{flag}"
        );
    }
}

#[test]
fn help_prints_usage_on_stdout() {
    let out = lawh(&["--help".as_ref()]);
    assert_eq!(out.status.code(), Some(0));
    assert!(stdout(&out).starts_with("Usage: lawh <file.lawh>"));
    assert!(!stdout(&out).contains("worker"), "{}", stdout(&out));
    assert_eq!(stderr(&out), "");
}

#[test]
fn math_worker_flag_runs_the_worker_until_stdin_closes() {
    let out = Command::new(env!("CARGO_BIN_EXE_lawh"))
        .arg("--math-worker")
        .stdin(Stdio::null())
        .env("RUST_LOG", "debug")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    // Only the handshake: the worker logs and prints nothing else.
    assert_eq!(stdout(&out), "lawh-math-worker 1\n");
    assert_eq!(stderr(&out), "");
}

#[test]
fn math_worker_flag_only_counts_first() {
    let out = lawh(&["a.lawh".as_ref(), "--math-worker".as_ref()]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("unknown option '--math-worker'"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn no_arguments_prints_usage_and_exits_2() {
    let out = lawh(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stdout(&out), "");
    assert!(
        stderr(&out).contains("Usage: lawh <file.lawh>"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn extra_arguments_print_usage_and_exit_2() {
    let out = lawh(&["a.lawh".as_ref(), "b.lawh".as_ref()]);
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.contains("unexpected argument 'b.lawh'"), "{err}");
    assert!(err.contains("Usage: lawh <file.lawh>"), "{err}");
}

#[test]
fn missing_file_names_the_path_and_exits_1() {
    let path = std::env::temp_dir().join(format!(
        "lawh-cli-test-{}-does-not-exist.lawh",
        std::process::id()
    ));
    let out = lawh(&[path.as_os_str()]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.starts_with(&format!("lawh: {}: ", path.display())),
        "{err}"
    );
}

#[test]
fn syntax_error_reports_path_line_and_column() {
    let board = TempBoard::new("syntax", "lawh version=1\ncircle id=c1 x=0 y=0 r=3 {\n");
    let out = lawh(&[board.0.as_os_str()]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.starts_with(&format!("lawh: {}:2:26: ", board.0.display())),
        "{err}"
    );
}

#[test]
fn format_error_reports_path_line_and_column() {
    let board = TempBoard::new("kind", "lawh version=1\n\nstar id=s1 x=0 y=0\n");
    let out = lawh(&[board.0.as_os_str()]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.starts_with(&format!(
            "lawh: {}:3:1: unknown object kind `star`",
            board.0.display()
        )),
        "{err}"
    );
}

#[test]
fn too_many_objects_reports_path_line_and_column() {
    use std::fmt::Write as _;

    let max = lawh_core::board::MAX_OBJECTS;
    let mut src = "lawh version=1\n".to_owned();
    for i in 0..=max {
        writeln!(src, "circle id=c{i} x=0 y=0 r=1").unwrap();
    }
    let board = TempBoard::new("many", &src);
    let out = lawh(&[board.0.as_os_str()]);
    assert_eq!(out.status.code(), Some(1));
    // The first object over the limit is on the line after the header and `max` objects.
    assert_eq!(
        stderr(&out),
        format!(
            "lawh: {}:{}:1: too many objects; a board holds at most {max}\n",
            board.0.display(),
            max + 2
        )
    );
}

#[test]
fn oversized_file_is_refused_before_it_is_read() {
    // 1 GiB: the error must report this size, which only the file's metadata knows, and come
    // back at once instead of after reading a gigabyte.
    let board = TempBoard::sparse("huge", 1 << 30);
    let started = std::time::Instant::now();
    let out = lawh(&[board.0.as_os_str()]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        format!(
            "lawh: {}:1:1: the file is 1073741824 bytes; lawh reads files of up to 8388608 bytes\n",
            board.0.display()
        )
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn file_one_byte_over_the_limit_reports_the_same_error_as_the_reader() {
    let board = TempBoard::sparse("over", 8 * 1024 * 1024 + 1);
    let out = lawh(&[board.0.as_os_str()]);
    assert_eq!(out.status.code(), Some(1));
    let err = lawh_core::FormatError::FileTooLarge {
        bytes: 8 * 1024 * 1024 + 1,
    };
    assert_eq!(stderr(&out), format!("lawh: {}:{err}\n", board.0.display()));
}
