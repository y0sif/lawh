//! Command-line arguments and the text the CLI prints.

use std::ffi::OsString;
use std::fmt;
use std::io::Write as _;
use std::path::PathBuf;

const USAGE: &str = "\
Usage: lawh <file.lawh>

Opens a lawh board in a window. Drag with the left or middle mouse button to
pan, and scroll to zoom.

Options:
  -h, --help     Print this help
  -V, --version  Print the version";

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Open this board in a window.
    Open(PathBuf),
    /// Print the version.
    Version,
    /// Print the help.
    Help,
}

/// Arguments that don't make a valid command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageError {
    /// No board file was given.
    MissingFile,
    /// A second file, or anything else after the file.
    UnexpectedArgument(String),
    /// An option lawh doesn't have.
    UnknownOption(String),
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFile => f.write_str("no board file given"),
            Self::UnexpectedArgument(arg) => write!(f, "unexpected argument '{arg}'"),
            Self::UnknownOption(arg) => write!(f, "unknown option '{arg}'"),
        }
    }
}

/// Parses the arguments after the program name.
///
/// `--help` and `--version` win wherever they appear before `--`. Everything after `--` is a
/// file, so `lawh -- -odd-name.lawh` opens a file whose name starts with `-`.
pub fn parse_args<I>(args: I) -> Result<Command, UsageError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut file = None;
    let mut only_files = false;
    for arg in args {
        if !only_files {
            match arg.to_str() {
                Some("-h" | "--help") => return Ok(Command::Help),
                Some("-V" | "--version") => return Ok(Command::Version),
                Some("--") => {
                    only_files = true;
                    continue;
                }
                _ => {}
            }
            let text = arg.to_string_lossy();
            if text.len() > 1 && text.starts_with('-') {
                return Err(UsageError::UnknownOption(text.into_owned()));
            }
        }
        if file.is_some() {
            return Err(UsageError::UnexpectedArgument(
                arg.to_string_lossy().into_owned(),
            ));
        }
        file = Some(PathBuf::from(arg));
    }
    file.map(Command::Open).ok_or(UsageError::MissingFile)
}

// The CLI writes with `writeln!` and ignores write errors: `println!` panics when stdout is a
// closed pipe (`lawh --version | head -c0`), and there is nowhere left to report the failure.

/// `lawh --version`, on stdout.
pub fn print_version() {
    let _ = writeln!(
        std::io::stdout().lock(),
        "lawh {}",
        env!("CARGO_PKG_VERSION")
    );
}

/// `lawh --help`, on stdout.
pub fn print_help() {
    let _ = writeln!(std::io::stdout().lock(), "{USAGE}");
}

/// A usage error and the usage, on stderr.
pub fn print_usage_error(err: &UsageError) {
    let _ = writeln!(std::io::stderr().lock(), "lawh: {err}\n\n{USAGE}");
}

/// A fatal error with its causes, on stderr.
pub fn print_error(err: &anyhow::Error) {
    let _ = writeln!(std::io::stderr().lock(), "lawh: {err:#}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, UsageError> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn a_file_opens_it() {
        assert_eq!(
            parse(&["board.lawh"]),
            Ok(Command::Open(PathBuf::from("board.lawh")))
        );
    }

    #[test]
    fn version_and_help_flags() {
        assert_eq!(parse(&["--version"]), Ok(Command::Version));
        assert_eq!(parse(&["-V"]), Ok(Command::Version));
        assert_eq!(parse(&["--help"]), Ok(Command::Help));
        assert_eq!(parse(&["-h"]), Ok(Command::Help));
    }

    #[test]
    fn flags_win_over_a_file() {
        assert_eq!(parse(&["board.lawh", "--help"]), Ok(Command::Help));
        assert_eq!(parse(&["-V", "board.lawh"]), Ok(Command::Version));
    }

    #[test]
    fn no_arguments_is_an_error() {
        assert_eq!(parse(&[]), Err(UsageError::MissingFile));
        assert_eq!(parse(&["--"]), Err(UsageError::MissingFile));
    }

    #[test]
    fn a_second_file_is_an_error() {
        assert_eq!(
            parse(&["a.lawh", "b.lawh"]),
            Err(UsageError::UnexpectedArgument("b.lawh".into()))
        );
    }

    #[test]
    fn unknown_options_are_errors() {
        assert_eq!(
            parse(&["--zoom", "a.lawh"]),
            Err(UsageError::UnknownOption("--zoom".into()))
        );
        assert_eq!(
            parse(&["a.lawh", "-x"]),
            Err(UsageError::UnknownOption("-x".into()))
        );
    }

    #[test]
    fn double_dash_ends_options() {
        assert_eq!(
            parse(&["--", "-odd.lawh"]),
            Ok(Command::Open(PathBuf::from("-odd.lawh")))
        );
        assert_eq!(
            parse(&["--", "--help"]),
            Ok(Command::Open(PathBuf::from("--help")))
        );
    }

    #[test]
    fn a_lone_dash_is_a_file_name() {
        assert_eq!(parse(&["-"]), Ok(Command::Open(PathBuf::from("-"))));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_file_names_are_kept_exactly() {
        use std::os::unix::ffi::OsStringExt as _;
        let name = OsString::from_vec(vec![b'b', 0xFF, b'.', b'l']);
        assert_eq!(
            parse_args([name.clone()]),
            Ok(Command::Open(PathBuf::from(name)))
        );
    }
}
