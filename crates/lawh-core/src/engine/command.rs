//! The `:` command line parser. lawh's own, not KDL: KDL cannot read `rect 4x2` and mangles
//! formulas.
//!
//! A command is a name, an optional `!` and arguments after a space. Names may be cut short
//! down to a fixed abbreviation, as in Vim: `:ci[rcle]` takes `:ci`, `:cir` up to `:circle`.
//! Shape commands take numbers: positional ones first in meaning (a radius, a size), and
//! `key=value` pairs named as in the file, which win over positionals. A number is an optional
//! sign, digits and an optional fraction (`-2.5`, `3`, `.5`, `2.`); no exponent, `inf` or `nan`.
//!
//! `:text` and `:math` take everything after the first space as it is typed, with no escapes,
//! comments or `|` between commands.

use crate::board::MAX_SIZE;

/// A parsed `:` command. Shape coordinates the user left out are `None`: the engine fills them
/// from the cursor.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// `:ci[rcle] R`, with `x= y= r=`: a circle of radius `r` around (`x`, `y`).
    Circle {
        x: Option<f64>,
        y: Option<f64>,
        r: f64,
    },
    /// `:el[lipse] RX RY`, with `x= y= rx= ry=`: an ellipse around (`x`, `y`).
    Ellipse {
        x: Option<f64>,
        y: Option<f64>,
        rx: f64,
        ry: f64,
    },
    /// `:re[ct] W H`, with `x= y= w= h=`: a rectangle with its top-left corner at (`x`, `y`).
    Rect {
        x: Option<f64>,
        y: Option<f64>,
        w: f64,
        h: f64,
    },
    /// `:li[ne] DX DY`, with `x1= y1= x2= y2=`.
    Line(Segment),
    /// `:ar[row] DX DY`, with `x1= y1= x2= y2=`.
    Arrow(Segment),
    /// `:te[xt] <text>`: a text block of everything after the first space.
    Text(String),
    /// `:ma[th] <formula>`: a formula of everything after the first space.
    Math(String),
    /// `:w[rite] [file]` and `:w! [file]`.
    Write { file: Option<String> },
    /// `:q[uit]`; `:q!` (`force`) quits even when the board has changes.
    Quit { force: bool },
    /// `:wq [file]` and `:wq! [file]`: write, then quit.
    WriteQuit { file: Option<String> },
    /// `:x[it] [file]` and `:x! [file]`: write only when the board has changes, then quit.
    Exit { file: Option<String> },
    /// `:u[ndo]`.
    Undo,
    /// `:red[o]`.
    Redo,
}

/// The ends of a line or an arrow. The start is (`x1`, `y1`), the cursor point where left out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub x1: Option<f64>,
    pub y1: Option<f64>,
    pub x2: End,
    pub y2: End,
}

/// One coordinate of the end of a line or an arrow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum End {
    /// A positional number: this far from the start.
    Relative(f64),
    /// `x2=` or `y2=`: exactly here.
    Absolute(f64),
}

impl End {
    /// The coordinate, for a start at `start`. A relative end is rounded with
    /// [`round3`](super::round3), as every coordinate the engine works out.
    pub fn resolve(self, start: f64) -> f64 {
        match self {
            Self::Relative(d) => super::round3(start + d),
            Self::Absolute(v) => v,
        }
    }
}

/// Why a `:` command line was refused. Shown on the status line, so the messages are plain
/// English (no `E123:` numbers).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    #[error("type a command after `:`")]
    Empty,
    #[error("unknown command `:{0}`")]
    Unknown(String),
    #[error("`:{0}` takes no `!`")]
    NoBang(&'static str),
    #[error("`:{0}` takes no arguments")]
    NoArguments(&'static str),
    #[error("`{0}` is not a number")]
    NotANumber(String),
    /// A `key=` the command does not take; `usage` shows the ones it does.
    #[error("unknown key `{key}=`: {usage}")]
    UnknownKey { key: String, usage: &'static str },
    #[error("`{0}=` is given twice")]
    RepeatedKey(String),
    #[error("too many numbers: {0}")]
    TooManyNumbers(&'static str),
    #[error("missing a number: {0}")]
    MissingNumber(&'static str),
    /// A radius, width or height of 0 or less (`-0` too).
    #[error("{0} must be above 0")]
    SizeNotPositive(&'static str),
    #[error("{0} must be at most {max}", max = MAX_SIZE)]
    SizeTooLarge(&'static str),
    /// A line or an arrow without a way to find its end; the field names the kind.
    #[error("{0} needs two numbers, or `x2=` and `y2=`")]
    MissingEnd(&'static str),
    /// `:text` or `:math` with nothing (or only spaces) after the name.
    #[error("missing {0}")]
    MissingBody(&'static str),
    /// `:w !cmd` or `:w >>file`, which Vim reads as a shell command or an append.
    #[error("writing to a command or appending to a file is not supported: `:w {0}`")]
    UnsupportedFile(String),
}

/// One command: its full name, the shortest abbreviation accepted, and how to read the rest.
struct Row {
    name: &'static str,
    min: usize,
    parse: fn(Args<'_>) -> Result<Command, CommandError>,
}

/// Every command, by name. An abbreviation must name one row only (a test checks), and only
/// `:w`, `:q`, `:x` and `:u` may be one letter long, keeping Vim's other one-letter commands
/// free.
const COMMANDS: &[Row] = &[
    Row {
        name: "arrow",
        min: 2,
        parse: arrow,
    },
    Row {
        name: "circle",
        min: 2,
        parse: circle,
    },
    Row {
        name: "ellipse",
        min: 2,
        parse: ellipse,
    },
    Row {
        name: "line",
        min: 2,
        parse: line,
    },
    Row {
        name: "math",
        min: 2,
        parse: math,
    },
    Row {
        name: "quit",
        min: 1,
        parse: quit,
    },
    Row {
        name: "rect",
        min: 2,
        parse: rect,
    },
    Row {
        name: "redo",
        min: 3,
        parse: redo,
    },
    Row {
        name: "text",
        min: 2,
        parse: text,
    },
    Row {
        name: "undo",
        min: 1,
        parse: undo,
    },
    Row {
        name: "wq",
        min: 2,
        parse: write_quit,
    },
    Row {
        name: "write",
        min: 1,
        parse: write,
    },
    Row {
        name: "xit",
        min: 1,
        parse: exit,
    },
];

/// What follows a command's name.
#[derive(Clone, Copy)]
struct Args<'a> {
    name: &'static str,
    bang: bool,
    /// Everything after the name, the `!` and one space, verbatim.
    rest: &'a str,
}

impl Args<'_> {
    fn no_bang(&self) -> Result<(), CommandError> {
        if self.bang {
            Err(CommandError::NoBang(self.name))
        } else {
            Ok(())
        }
    }

    fn nothing_else(&self) -> Result<(), CommandError> {
        self.no_bang()?;
        if self.rest.trim().is_empty() {
            Ok(())
        } else {
            Err(CommandError::NoArguments(self.name))
        }
    }
}

/// Reads a `:` command line, the text after the `:`. Leading spaces and extra colons are
/// skipped, as in Vim.
pub fn parse_command(line: &str) -> Result<Command, CommandError> {
    let line = line.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
    if line.is_empty() {
        return Err(CommandError::Empty);
    }
    let name_len = line
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(line.len());
    let (name, after) = line.split_at(name_len);
    let (bang, after) = match after.strip_prefix('!') {
        Some(after) => (true, after),
        None => (false, after),
    };
    let mut chars = after.chars();
    let rest = match chars.next() {
        None => "",
        Some(c) if c.is_whitespace() => chars.as_str(),
        // `:ci3` or `:3`: the word as typed is not a command.
        Some(_) => return Err(unknown(line)),
    };
    let row = COMMANDS
        .iter()
        .find(|row| name.len() >= row.min && row.name.starts_with(name))
        .ok_or_else(|| unknown(line))?;
    (row.parse)(Args {
        name: row.name,
        bang,
        rest,
    })
}

/// The unknown command at the start of `line`: its first word.
fn unknown(line: &str) -> CommandError {
    let word = line.split_whitespace().next().unwrap_or_default();
    CommandError::Unknown(word.to_owned())
}

fn circle(args: Args<'_>) -> Result<Command, CommandError> {
    const USAGE: &str = "`:ci[rcle] R`, with `x=`, `y=` or `r=`";
    args.no_bang()?;
    let Numbers {
        positional: [radius, _],
        named: [x, y, r, _],
    } = numbers(args.rest, 1, &["x", "y", "r"], USAGE)?;
    let r = size(r.or(radius), "radius", USAGE)?;
    Ok(Command::Circle { x, y, r })
}

fn ellipse(args: Args<'_>) -> Result<Command, CommandError> {
    const USAGE: &str = "`:el[lipse] RX RY`, with `x=`, `y=`, `rx=` or `ry=`";
    args.no_bang()?;
    let Numbers {
        positional: [px, py],
        named: [x, y, rx, ry],
    } = numbers(args.rest, 2, &["x", "y", "rx", "ry"], USAGE)?;
    let rx = size(rx.or(px), "each radius", USAGE)?;
    let ry = size(ry.or(py), "each radius", USAGE)?;
    Ok(Command::Ellipse { x, y, rx, ry })
}

fn rect(args: Args<'_>) -> Result<Command, CommandError> {
    const USAGE: &str = "`:re[ct] W H`, with `x=`, `y=`, `w=` or `h=`";
    args.no_bang()?;
    let Numbers {
        positional: [pw, ph],
        named: [x, y, w, h],
    } = numbers(args.rest, 2, &["x", "y", "w", "h"], USAGE)?;
    let w = size(w.or(pw), "width", USAGE)?;
    let h = size(h.or(ph), "height", USAGE)?;
    Ok(Command::Rect { x, y, w, h })
}

fn line(args: Args<'_>) -> Result<Command, CommandError> {
    const USAGE: &str = "`:li[ne] DX DY`, with `x1=`, `y1=`, `x2=` or `y2=`";
    segment(args, "a line", USAGE).map(Command::Line)
}

fn arrow(args: Args<'_>) -> Result<Command, CommandError> {
    const USAGE: &str = "`:ar[row] DX DY`, with `x1=`, `y1=`, `x2=` or `y2=`";
    segment(args, "an arrow", USAGE).map(Command::Arrow)
}

/// The ends of a line or an arrow: positionals are the end's offset from the start, `x2=` and
/// `y2=` place it exactly and win over them.
fn segment(
    args: Args<'_>,
    kind: &'static str,
    usage: &'static str,
) -> Result<Segment, CommandError> {
    args.no_bang()?;
    let Numbers {
        positional: [dx, dy],
        named: [x1, y1, x2, y2],
    } = numbers(args.rest, 2, &["x1", "y1", "x2", "y2"], usage)?;
    let end = |absolute: Option<f64>, relative: Option<f64>| {
        absolute
            .map(End::Absolute)
            .or(relative.map(End::Relative))
            .ok_or(CommandError::MissingEnd(kind))
    };
    Ok(Segment {
        x1,
        y1,
        x2: end(x2, dx)?,
        y2: end(y2, dy)?,
    })
}

/// A radius, width or height: given, above 0 and at most [`MAX_SIZE`].
fn size(v: Option<f64>, what: &'static str, usage: &'static str) -> Result<f64, CommandError> {
    let v = v.ok_or(CommandError::MissingNumber(usage))?;
    if v <= 0.0 {
        Err(CommandError::SizeNotPositive(what))
    } else if v > MAX_SIZE {
        Err(CommandError::SizeTooLarge(what))
    } else {
        Ok(v)
    }
}

fn text(args: Args<'_>) -> Result<Command, CommandError> {
    body(args, "the text: `:te[xt] <text>`").map(Command::Text)
}

fn math(args: Args<'_>) -> Result<Command, CommandError> {
    body(args, "the formula: `:ma[th] <formula>`").map(Command::Math)
}

/// Everything after the name and the first space, verbatim: spaces at either end, `|`, `"`
/// and `\` stay as typed. Nothing but spaces is missing.
fn body(args: Args<'_>, missing: &'static str) -> Result<String, CommandError> {
    args.no_bang()?;
    if args.rest.trim().is_empty() {
        Err(CommandError::MissingBody(missing))
    } else {
        Ok(args.rest.to_owned())
    }
}

fn write(args: Args<'_>) -> Result<Command, CommandError> {
    Ok(Command::Write {
        file: file(args.rest)?,
    })
}

fn write_quit(args: Args<'_>) -> Result<Command, CommandError> {
    Ok(Command::WriteQuit {
        file: file(args.rest)?,
    })
}

fn exit(args: Args<'_>) -> Result<Command, CommandError> {
    Ok(Command::Exit {
        file: file(args.rest)?,
    })
}

fn quit(args: Args<'_>) -> Result<Command, CommandError> {
    if args.rest.trim().is_empty() {
        Ok(Command::Quit { force: args.bang })
    } else {
        Err(CommandError::NoArguments(args.name))
    }
}

/// The file name after `:w`, `:wq` or `:x`: the rest of the line without spaces at its ends,
/// or `None` when there is none.
fn file(rest: &str) -> Result<Option<String>, CommandError> {
    let name = rest.trim();
    if name.is_empty() {
        Ok(None)
    } else if name.starts_with('!') || name.starts_with(">>") {
        Err(CommandError::UnsupportedFile(name.to_owned()))
    } else {
        Ok(Some(name.to_owned()))
    }
}

fn undo(args: Args<'_>) -> Result<Command, CommandError> {
    args.nothing_else()?;
    Ok(Command::Undo)
}

fn redo(args: Args<'_>) -> Result<Command, CommandError> {
    args.nothing_else()?;
    Ok(Command::Redo)
}

/// The numbers of a shape command: positionals in the order typed, and `key=value` values in
/// the order of the command's keys.
#[derive(Debug, Default, PartialEq)]
struct Numbers {
    positional: [Option<f64>; 2],
    named: [Option<f64>; 4],
}

/// Reads up to `positionals` numbers and `key=value` pairs for `keys` (at most two and four),
/// separated by spaces, in any order.
fn numbers(
    rest: &str,
    positionals: usize,
    keys: &[&str],
    usage: &'static str,
) -> Result<Numbers, CommandError> {
    let mut out = Numbers::default();
    let mut typed = 0;
    for word in rest.split_whitespace() {
        if let Some((key, value)) = word.split_once('=') {
            let slot = keys
                .iter()
                .position(|k| *k == key)
                .and_then(|i| out.named.get_mut(i))
                .ok_or_else(|| CommandError::UnknownKey {
                    key: key.to_owned(),
                    usage,
                })?;
            if slot.is_some() {
                return Err(CommandError::RepeatedKey(key.to_owned()));
            }
            *slot = Some(number(value)?);
        } else {
            let slot = out
                .positional
                .get_mut(typed)
                .filter(|_| typed < positionals)
                .ok_or(CommandError::TooManyNumbers(usage))?;
            *slot = Some(number(word)?);
            typed += 1;
        }
    }
    Ok(out)
}

/// A number: optional sign, digits, optional fraction, at least one digit.
fn number(s: &str) -> Result<f64, CommandError> {
    let unsigned = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if (whole.is_empty() && fraction.is_empty()) || !digits(whole) || !digits(fraction) {
        return Err(CommandError::NotANumber(s.to_owned()));
    }
    // Hundreds of digits read as infinity.
    s.parse()
        .ok()
        .filter(|v: &f64| v.is_finite())
        .ok_or_else(|| CommandError::NotANumber(s.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abbreviations_name_one_command_only() {
        for row in COMMANDS {
            for len in row.min..=row.name.len() {
                let short = &row.name[..len];
                let matches: Vec<&str> = COMMANDS
                    .iter()
                    .filter(|r| short.len() >= r.min && r.name.starts_with(short))
                    .map(|r| r.name)
                    .collect();
                assert_eq!(matches, [row.name], "`:{short}`");
            }
            assert!(row.min >= 1 && row.min <= row.name.len(), "{}", row.name);
        }
    }

    #[test]
    fn only_w_q_x_and_u_are_one_letter_long() {
        for row in COMMANDS.iter().filter(|r| r.min == 1) {
            assert!(
                ["write", "quit", "xit", "undo"].contains(&row.name),
                "`:{}` must not take a one-letter form",
                row.name
            );
        }
    }

    #[test]
    fn numbers_have_digits_and_no_exponent() {
        for ok in ["3", "-2.5", ".5", "2.", "+1", "-.25", "007", "1000000.000"] {
            assert!(number(ok).is_ok(), "{ok}");
        }
        for bad in [
            "", ".", "-", "+", "+-1", "1e3", "1E3", "inf", "-inf", "nan", "NaN", "0x10", "1.2.3",
            "1_000", "١", "--1", "1-", "abc",
        ] {
            assert_eq!(
                number(bad),
                Err(CommandError::NotANumber(bad.to_owned())),
                "{bad}"
            );
        }
        // Too many digits for a finite f64.
        let huge = "9".repeat(400);
        assert_eq!(number(&huge), Err(CommandError::NotANumber(huge.clone())));
    }
}
