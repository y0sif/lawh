//! Command-line arguments and the text the CLI prints.

use std::ffi::OsString;
use std::fmt;
use std::io::Write as _;
use std::path::PathBuf;

use lawh_core::engine::{Settings, parse_png_scale};
use lawh_core::{ExportBackground, ExportFormat, ExportOptions};

const USAGE: &str = "\
Usage: lawh [file.lawh]
       lawh export [--scale N] [--transparent] <board.lawh> <out.svg|png|pdf>

Opens a lawh board in a window, to edit with Vim keys. A file that doesn't exist
yet opens as an empty board, and :w creates it. Without a file the board has no
name until :w name.lawh gives it one.

Drag with the left or middle mouse button to pan, click to select, and scroll to
zoom. Settings and key maps are read from lawhrc in lawh's config directory:
~/.config/lawh/lawhrc on Linux and macOS, %APPDATA%\\y0sif\\lawh\\config\\lawhrc on
Windows.

lawh export draws a board into an SVG, PNG or PDF file, its format taken from
the file's extension, without a window. An existing file is replaced. No lawhrc
is read, so a board exports the same on every machine. To open a board named
export, type lawh ./export or lawh -- export.

Options:
  -h, --help         Print this help
  -V, --version      Print the version

Export options:
      --scale N      A PNG's scale, from 0.25 to 8 (default 2), as :set pngscale
      --transparent  No paper behind the board, as :set exportbg=none";

/// Hidden first argument that turns this process into the math worker (see
/// `lawh_math::worker_main`). lawh starts itself with it; it isn't in the help.
pub const MATH_WORKER_FLAG: &str = "--math-worker";

/// The first argument that makes the command line an export (`lawh export ...`) instead of a
/// board to open. A board file of that name opens as `./export` or after `--`.
const EXPORT_COMMAND: &str = "export";

/// `lawh export`'s option for a PNG's scale (`:set pngscale`).
const SCALE_OPTION: &str = "--scale";

/// `lawh export`'s option for no paper behind the board (`:set exportbg=none`).
const TRANSPARENT_OPTION: &str = "--transparent";

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Open this board file in a window, or an unnamed board.
    Open(Option<PathBuf>),
    /// Export a board file without a window (`lawh export`).
    Export(ExportArgs),
    /// Print the version.
    Version,
    /// Print the help.
    Help,
}

/// What `lawh export` exports, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportArgs {
    /// The board file. It must exist: a missing board is an error, never a new board.
    pub board: PathBuf,
    /// The file to write; its extension gives the format.
    pub out: PathBuf,
    /// The format from `out`, the defaults of `:set pngscale` and `:set exportbg` (no lawhrc is
    /// read), and `--scale` and `--transparent` over them.
    pub options: ExportOptions,
}

/// Arguments that don't make a valid command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageError {
    /// A second file, or anything else after the file (after the two files of an export).
    UnexpectedArgument(String),
    /// An option lawh doesn't have.
    UnknownOption(String),
    /// `lawh export` alone.
    ExportNeedsFiles,
    /// `lawh export` with a board and nothing to write.
    ExportNeedsOutput,
    /// An export's file without `.svg`, `.png` or `.pdf`.
    ExportFormat(String),
    /// `--scale` without a value after it.
    MissingScale,
    /// A `--scale` that isn't a scale `:set pngscale` takes.
    BadScale(String),
    /// `--scale` for an SVG or a PDF.
    ScaleNotPng(ExportFormat),
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedArgument(arg) => write!(f, "unexpected argument '{arg}'"),
            Self::UnknownOption(arg) => write!(f, "unknown option '{arg}'"),
            Self::ExportNeedsFiles => write!(
                f,
                "export needs a board and a file to write; to open a board named export, \
                 type lawh ./export or lawh -- export"
            ),
            Self::ExportNeedsOutput => write!(f, "export needs a file to write after the board"),
            Self::ExportFormat(file) => {
                write!(f, "export needs .svg, .png or .pdf, not '{file}'")
            }
            Self::MissingScale => write!(f, "{SCALE_OPTION} needs a scale from 0.25 to 8"),
            Self::BadScale(value) => write!(
                f,
                "bad value '{value}' for {SCALE_OPTION}: it takes a scale from 0.25 to 8, with \
                 at most two decimals"
            ),
            Self::ScaleNotPng(format) => write!(
                f,
                "{SCALE_OPTION} is for .png exports, not .{}",
                format.extension()
            ),
        }
    }
}

/// Parses the arguments after the program name: at most one file, or `export` first and an
/// export's arguments after it ([`parse_export`]).
///
/// `--help` and `--version` win wherever they appear before `--`. Everything after `--` is a
/// file, so `lawh -- -odd-name.lawh` opens a file whose name starts with `-`, and `lawh --
/// export` a board named `export`.
pub fn parse_args<I>(args: I) -> Result<Command, UsageError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter().peekable();
    if args.next_if(|arg| arg == EXPORT_COMMAND).is_some() {
        return parse_export(args);
    }
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
            if is_option(&arg) {
                return Err(UsageError::UnknownOption(
                    arg.to_string_lossy().into_owned(),
                ));
            }
        }
        if file.is_some() {
            return Err(UsageError::UnexpectedArgument(
                arg.to_string_lossy().into_owned(),
            ));
        }
        file = Some(PathBuf::from(arg));
    }
    Ok(Command::Open(file))
}

/// Parses `lawh export`'s arguments, those after `export`: the board, the file to write, and the
/// options, in any order before `--` (everything after it is a file). `--scale N` (or
/// `--scale=N`) takes what `:set pngscale` takes ([`parse_png_scale`]), and only for a PNG;
/// `--transparent` is `:set exportbg=none`. The format comes from the file's extension, in any
/// case ([`ExportFormat::from_path`]).
///
/// Arguments are read in order, so the first that is wrong is the one reported; the checks that
/// need both files (their count, the extension, `--scale` with an SVG or a PDF) come after.
fn parse_export(args: impl Iterator<Item = OsString>) -> Result<Command, UsageError> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut scale = None;
    let mut transparent = false;
    let mut only_files = false;
    let mut args = args;
    while let Some(arg) = args.next() {
        if !only_files {
            match arg.to_str() {
                Some("-h" | "--help") => return Ok(Command::Help),
                Some("-V" | "--version") => return Ok(Command::Version),
                Some("--") => {
                    only_files = true;
                    continue;
                }
                Some(TRANSPARENT_OPTION) => {
                    transparent = true;
                    continue;
                }
                Some(SCALE_OPTION) => {
                    let value = args.next().ok_or(UsageError::MissingScale)?;
                    scale = Some(scale_value(&value.to_string_lossy())?);
                    continue;
                }
                Some(text) => {
                    if let Some(value) = text
                        .strip_prefix(SCALE_OPTION)
                        .and_then(|rest| rest.strip_prefix('='))
                    {
                        scale = Some(scale_value(value)?);
                        continue;
                    }
                }
                None => {}
            }
            if is_option(&arg) {
                return Err(UsageError::UnknownOption(
                    arg.to_string_lossy().into_owned(),
                ));
            }
        }
        if files.len() == 2 {
            return Err(UsageError::UnexpectedArgument(
                arg.to_string_lossy().into_owned(),
            ));
        }
        files.push(PathBuf::from(arg));
    }
    let given = files.len();
    let mut files = files.into_iter();
    let (Some(board), Some(out)) = (files.next(), files.next()) else {
        return Err(if given == 0 {
            UsageError::ExportNeedsFiles
        } else {
            UsageError::ExportNeedsOutput
        });
    };
    let name = out.to_string_lossy();
    let format = ExportFormat::from_path(&name)
        .ok_or_else(|| UsageError::ExportFormat(name.clone().into_owned()))?;
    if scale.is_some() && format != ExportFormat::Png {
        return Err(UsageError::ScaleNotPng(format));
    }
    let mut options = Settings::default().export_options(format);
    if let Some(scale) = scale {
        options.png_scale_hundredths = scale;
    }
    if transparent {
        options.background = ExportBackground::None;
    }
    Ok(Command::Export(ExportArgs {
        board,
        out,
        options,
    }))
}

/// `--scale`'s value in hundredths, as `:set pngscale` reads it.
fn scale_value(value: &str) -> Result<u32, UsageError> {
    parse_png_scale(value).ok_or_else(|| UsageError::BadScale(value.to_owned()))
}

/// An argument that looks like an option: a `-` and more. A lone `-` is a file name.
fn is_option(arg: &OsString) -> bool {
    let text = arg.to_string_lossy();
    text.len() > 1 && text.starts_with('-')
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

/// Something to know about a command that worked (an export with failed formulas), on stderr:
/// stdout stays empty for scripts.
pub fn print_note(note: &str) {
    let _ = writeln!(std::io::stderr().lock(), "lawh: {note}");
}

#[cfg(test)]
mod tests {
    use lawh_core::export::DEFAULT_PNG_SCALE_HUNDREDTHS;

    use super::*;

    fn parse(args: &[&str]) -> Result<Command, UsageError> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn a_file_opens_it() {
        assert_eq!(
            parse(&["board.lawh"]),
            Ok(Command::Open(Some(PathBuf::from("board.lawh"))))
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
    fn no_file_opens_an_unnamed_board() {
        assert_eq!(parse(&[]), Ok(Command::Open(None)));
        assert_eq!(parse(&["--"]), Ok(Command::Open(None)));
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
        // Export's options are the export's only.
        assert_eq!(
            parse(&["--transparent", "a.lawh"]),
            Err(UsageError::UnknownOption("--transparent".into()))
        );
    }

    #[test]
    fn double_dash_ends_options() {
        assert_eq!(
            parse(&["--", "-odd.lawh"]),
            Ok(Command::Open(Some(PathBuf::from("-odd.lawh"))))
        );
        assert_eq!(
            parse(&["--", "--help"]),
            Ok(Command::Open(Some(PathBuf::from("--help"))))
        );
    }

    #[test]
    fn a_lone_dash_is_a_file_name() {
        assert_eq!(parse(&["-"]), Ok(Command::Open(Some(PathBuf::from("-")))));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_file_names_are_kept_exactly() {
        use std::os::unix::ffi::OsStringExt as _;
        let name = OsString::from_vec(vec![b'b', 0xFF, b'.', b'l']);
        assert_eq!(
            parse_args([name.clone()]),
            Ok(Command::Open(Some(PathBuf::from(name))))
        );
    }

    /// What `lawh export board out` parses to, with `options`.
    fn export(board: &str, out: &str, options: ExportOptions) -> Command {
        Command::Export(ExportArgs {
            board: PathBuf::from(board),
            out: PathBuf::from(out),
            options,
        })
    }

    fn options(format: ExportFormat) -> ExportOptions {
        ExportOptions {
            format,
            background: ExportBackground::Paper,
            png_scale_hundredths: DEFAULT_PNG_SCALE_HUNDREDTHS,
        }
    }

    #[test]
    fn export_takes_a_board_and_a_file_whose_extension_is_the_format() {
        for (out, format) in [
            ("out.svg", ExportFormat::Svg),
            ("out.png", ExportFormat::Png),
            ("out.pdf", ExportFormat::Pdf),
            ("dir.d/OUT.PDF", ExportFormat::Pdf),
            ("Out.Png", ExportFormat::Png),
        ] {
            assert_eq!(
                parse(&["export", "a.lawh", out]),
                Ok(export("a.lawh", out, options(format))),
                "{out}"
            );
        }
        // The defaults are `:set`'s, lawhrc or not.
        assert_eq!(
            options(ExportFormat::Png),
            Settings::default().export_options(ExportFormat::Png)
        );
    }

    #[test]
    fn export_options_go_anywhere_before_the_double_dash() {
        let png = |hundredths, background| ExportOptions {
            format: ExportFormat::Png,
            background,
            png_scale_hundredths: hundredths,
        };
        let want = Ok(export("a.lawh", "b.png", png(400, ExportBackground::None)));
        for args in [
            &["export", "--scale", "4", "--transparent", "a.lawh", "b.png"][..],
            &["export", "a.lawh", "--transparent", "b.png", "--scale=4"],
            &[
                "export",
                "a.lawh",
                "b.png",
                "--scale",
                "+4.00",
                "--transparent",
            ],
            &[
                "export",
                "--transparent",
                "--scale",
                "1",
                "a.lawh",
                "--scale",
                "4",
                "b.png",
            ],
        ] {
            assert_eq!(parse(args), want, "{args:?}");
        }
        for (value, hundredths) in [("0.25", 25), (".5", 50), ("8", 800), ("1.5", 150)] {
            assert_eq!(
                parse(&["export", "--scale", value, "a.lawh", "b.png"]),
                Ok(export(
                    "a.lawh",
                    "b.png",
                    png(hundredths, ExportBackground::Paper)
                )),
                "{value}"
            );
        }
        // Transparent is for every format.
        let mut svg = options(ExportFormat::Svg);
        svg.background = ExportBackground::None;
        assert_eq!(
            parse(&["export", "a.lawh", "b.svg", "--transparent"]),
            Ok(export("a.lawh", "b.svg", svg))
        );
    }

    #[test]
    fn export_files_after_the_double_dash_may_look_like_options() {
        assert_eq!(
            parse(&["export", "--", "-a.lawh", "--scale.png"]),
            Ok(export("-a.lawh", "--scale.png", options(ExportFormat::Png)))
        );
        assert_eq!(
            parse(&["export", "--transparent", "--", "--help", "-.pdf"]).map(|c| match c {
                Command::Export(args) => (args.board, args.options.background),
                other => panic!("{other:?}"),
            }),
            Ok((PathBuf::from("--help"), ExportBackground::None))
        );
        assert_eq!(
            parse(&["export", "-", "out.svg"]),
            Ok(export("-", "out.svg", options(ExportFormat::Svg)))
        );
    }

    #[test]
    fn a_board_named_export_opens_with_a_path_or_after_the_double_dash() {
        assert_eq!(
            parse(&["./export"]),
            Ok(Command::Open(Some(PathBuf::from("./export"))))
        );
        assert_eq!(
            parse(&["--", "export"]),
            Ok(Command::Open(Some(PathBuf::from("export"))))
        );
        // Only first does `export` start an export.
        assert_eq!(
            parse(&["a.lawh", "export"]),
            Err(UsageError::UnexpectedArgument("export".into()))
        );
    }

    #[test]
    fn export_alone_says_how_to_open_a_board_named_export() {
        assert_eq!(parse(&["export"]), Err(UsageError::ExportNeedsFiles));
        let message = UsageError::ExportNeedsFiles.to_string();
        assert!(message.contains("lawh ./export"), "{message}");
        assert!(message.contains("lawh -- export"), "{message}");
        assert_eq!(parse(&["export", "--"]), Err(UsageError::ExportNeedsFiles));
        assert_eq!(
            parse(&["export", "--transparent"]),
            Err(UsageError::ExportNeedsFiles)
        );
        assert_eq!(
            parse(&["export", "--scale", "2"]),
            Err(UsageError::ExportNeedsFiles)
        );
    }

    #[test]
    fn export_usage_errors() {
        let cases: &[(&[&str], UsageError)] = &[
            (&["export", "a.lawh"], UsageError::ExportNeedsOutput),
            (
                &["export", "--scale=2", "a.png"],
                UsageError::ExportNeedsOutput,
            ),
            (
                &["export", "a.lawh", "b.png", "c.png"],
                UsageError::UnexpectedArgument("c.png".into()),
            ),
            (
                &["export", "--zoom", "a.lawh", "b.png"],
                UsageError::UnknownOption("--zoom".into()),
            ),
            (
                &["export", "a.lawh", "b.png", "-x"],
                UsageError::UnknownOption("-x".into()),
            ),
            (
                &["export", "a.lawh", "b.png", "--scale-x=2"],
                UsageError::UnknownOption("--scale-x=2".into()),
            ),
            (
                &["export", "a.lawh", "b.lawh"],
                UsageError::ExportFormat("b.lawh".into()),
            ),
            (
                &["export", "a.lawh", "b"],
                UsageError::ExportFormat("b".into()),
            ),
            (
                &["export", "a.lawh", ".png"],
                UsageError::ExportFormat(".png".into()),
            ),
            (
                &["export", "a.lawh", "b.jpg"],
                UsageError::ExportFormat("b.jpg".into()),
            ),
            (
                &["export", "a.png", "b.png.d/c"],
                UsageError::ExportFormat("b.png.d/c".into()),
            ),
            (
                &["export", "a.lawh", "b.png", "--scale"],
                UsageError::MissingScale,
            ),
            (
                &["export", "--scale", "9", "a.lawh", "b.png"],
                UsageError::BadScale("9".into()),
            ),
            (
                &["export", "--scale=0.2", "a.lawh", "b.png"],
                UsageError::BadScale("0.2".into()),
            ),
            (
                &["export", "--scale=", "a.lawh", "b.png"],
                UsageError::BadScale(String::new()),
            ),
            (
                &["export", "--scale", "1.125", "a.lawh", "b.png"],
                UsageError::BadScale("1.125".into()),
            ),
            (
                &["export", "--scale", "-2", "a.lawh", "b.png"],
                UsageError::BadScale("-2".into()),
            ),
            (
                &["export", "--scale", "2", "a.lawh", "b.svg"],
                UsageError::ScaleNotPng(ExportFormat::Svg),
            ),
            (
                &["export", "a.lawh", "b.PDF", "--scale=1"],
                UsageError::ScaleNotPng(ExportFormat::Pdf),
            ),
        ];
        for (args, err) in cases {
            assert_eq!(parse(args), Err(err.clone()), "{args:?}");
        }
    }

    #[test]
    fn help_and_version_win_in_an_export_too() {
        assert_eq!(parse(&["export", "--help"]), Ok(Command::Help));
        assert_eq!(
            parse(&["export", "a.lawh", "b.png", "-h"]),
            Ok(Command::Help)
        );
        assert_eq!(parse(&["export", "-V", "a.lawh"]), Ok(Command::Version));
    }

    #[test]
    fn usage_errors_read_as_the_help_does() {
        let cases = [
            (
                UsageError::ExportNeedsFiles,
                "export needs a board and a file to write; to open a board named export, type \
                 lawh ./export or lawh -- export",
            ),
            (
                UsageError::ExportNeedsOutput,
                "export needs a file to write after the board",
            ),
            (
                UsageError::ExportFormat("b.jpg".into()),
                "export needs .svg, .png or .pdf, not 'b.jpg'",
            ),
            (
                UsageError::MissingScale,
                "--scale needs a scale from 0.25 to 8",
            ),
            (
                UsageError::BadScale("9".into()),
                "bad value '9' for --scale: it takes a scale from 0.25 to 8, with at most two \
                 decimals",
            ),
            (
                UsageError::ScaleNotPng(ExportFormat::Svg),
                "--scale is for .png exports, not .svg",
            ),
        ];
        for (err, text) in cases {
            assert_eq!(err.to_string(), text);
            assert!(!text.contains('\u{2014}'), "no em dash: {text}");
        }
        assert!(!USAGE.contains('\u{2014}'));
        assert!(USAGE.contains("lawh export [--scale N] [--transparent]"));
        assert!(USAGE.contains("lawh ./export or lawh -- export"));
        assert!(
            USAGE.lines().all(|line| line.chars().count() <= 80),
            "{USAGE}"
        );
    }
}
