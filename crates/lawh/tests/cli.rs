//! The `lawh` command line: version, usage errors and board loading errors.
//!
//! None of these open a window. Display variables are removed from the child's environment, so a
//! run that got as far as opening one would fail with a different message instead of showing it.

// Lets the helpers below unwrap like test functions (see clippy.toml).
#![cfg(test)]

use std::path::PathBuf;
use std::process::{Command, Output};

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
        let path =
            std::env::temp_dir().join(format!("lawh-cli-test-{}-{name}.lawh", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        Self(path)
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
    assert_eq!(stderr(&out), "");
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
