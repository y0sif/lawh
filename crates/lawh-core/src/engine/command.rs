//! The `:` command line parser. lawh's own, not KDL: KDL cannot read `rect 4x2` and mangles
//! formulas.
//!
//! A command is a name, an optional `!` and arguments after a space. Names may be cut short
//! down to a fixed abbreviation, as in Vim: `:ci[rcle]` takes `:ci`, `:cir` up to `:circle`.
//! Shape commands take numbers: positional ones first in meaning (a radius, a size), and
//! `key=value` pairs named as in the file, which win over positionals. A number is an optional
//! sign, digits and an optional fraction (`-2.5`, `3`, `.5`, `2.`); no exponent, `inf` or `nan`.

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
    /// `:u[ndo]`.
    Undo,
    /// `:red[o]`.
    Redo,
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
}

/// One command: its full name, the shortest abbreviation accepted, and how to read the rest.
struct Row {
    name: &'static str,
    min: usize,
    parse: fn(Args<'_>) -> Result<Command, CommandError>,
}

/// Every command. An abbreviation must name one row only (a test checks), and only `:w`, `:q`,
/// `:x` and `:u` may be one letter long, keeping Vim's other one-letter commands free.
const COMMANDS: &[Row] = &[
    Row {
        name: "circle",
        min: 2,
        parse: circle,
    },
    Row {
        name: "redo",
        min: 3,
        parse: redo,
    },
    Row {
        name: "undo",
        min: 1,
        parse: undo,
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
    let r = r.or(radius).ok_or(CommandError::MissingNumber(USAGE))?;
    Ok(Command::Circle { x, y, r })
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
    s.parse()
        .map_err(|_| CommandError::NotANumber(s.to_owned()))
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
    fn circle_takes_a_radius_and_coordinates() {
        let circle = |x, y, r| Ok(Command::Circle { x, y, r });
        assert_eq!(parse_command("ci 3"), circle(None, None, 3.0));
        assert_eq!(parse_command("circle 3"), circle(None, None, 3.0));
        assert_eq!(parse_command("cir   2.5  "), circle(None, None, 2.5));
        assert_eq!(parse_command("ci r=2"), circle(None, None, 2.0));
        assert_eq!(
            parse_command("ci y=-1 x=.5 4"),
            circle(Some(0.5), Some(-1.0), 4.0)
        );
        // `r=` wins over the positional radius.
        assert_eq!(parse_command("ci 4 r=1"), circle(None, None, 1.0));
        assert_eq!(parse_command(" ::ci\t+2."), circle(None, None, 2.0));
        assert_eq!(parse_command("ci -0"), circle(None, None, -0.0));
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
    }

    #[test]
    fn undo_and_redo_take_nothing() {
        for ok in ["u", "un", "und", "undo", "undo  "] {
            assert_eq!(parse_command(ok), Ok(Command::Undo), "{ok}");
        }
        for ok in ["red", "redo"] {
            assert_eq!(parse_command(ok), Ok(Command::Redo), "{ok}");
        }
        assert_eq!(parse_command("u 2"), Err(CommandError::NoArguments("undo")));
        assert_eq!(parse_command("redo!"), Err(CommandError::NoBang("redo")));
    }

    #[test]
    fn errors_say_what_is_wrong() {
        let cases = [
            ("", "type a command after `:`"),
            (" : ", "type a command after `:`"),
            ("foo", "unknown command `:foo`"),
            ("foo bar", "unknown command `:foo`"),
            ("c 3", "unknown command `:c`"),
            ("re 3", "unknown command `:re`"),
            ("circles 3", "unknown command `:circles`"),
            ("ci3", "unknown command `:ci3`"),
            ("Ci 3", "unknown command `:Ci`"),
            ("3", "unknown command `:3`"),
            ("!ls", "unknown command `:!ls`"),
            ("ci! 3", "`:circle` takes no `!`"),
            (
                "ci",
                "missing a number: `:ci[rcle] R`, with `x=`, `y=` or `r=`",
            ),
            (
                "ci x=1 y=1",
                "missing a number: `:ci[rcle] R`, with `x=`, `y=` or `r=`",
            ),
            (
                "ci 1 2",
                "too many numbers: `:ci[rcle] R`, with `x=`, `y=` or `r=`",
            ),
            ("ci abc", "`abc` is not a number"),
            ("ci 1e3", "`1e3` is not a number"),
            ("ci r=", "`` is not a number"),
            (
                "ci 2 z=1",
                "unknown key `z=`: `:ci[rcle] R`, with `x=`, `y=` or `r=`",
            ),
            (
                "ci 2 =1",
                "unknown key `=`: `:ci[rcle] R`, with `x=`, `y=` or `r=`",
            ),
            ("ci x=1 x=2 3", "`x=` is given twice"),
            ("ci r=1 r=1", "`r=` is given twice"),
        ];
        for (line, message) in cases {
            let error = parse_command(line).map(|c| format!("{c:?}"));
            assert_eq!(
                error.map_err(|e| e.to_string()),
                Err(message.to_owned()),
                "{line:?}"
            );
        }
    }
}
