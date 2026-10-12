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
//! comments or `|` between commands. Options may come first, as Vim's `++opt` words: words that
//! start with `++` and a letter right after the space, each followed by one space. `++size=N`
//! is the only one (`:te ++size=2 Title`); any other is an error. A word such as `size=2`, or one
//! after the text has started, is text.
//!
//! `:si[ze]` takes one number: `N` sets the size of text and math, `+N` and `-N` add to it or
//! take from it.
//!
//! The mapping commands take Vim's: `:map`, `:noremap`, `:unmap` and `:mapclear` with a mode
//! letter in front (`:nnoremap`, `:iunmap`) or `!` after (`:map!` for Insert and command-line
//! mode). The map arguments `<silent>` (no effect) and `<nowait>` may come first; then the keys
//! to map, up to the first space, and what they map to, the rest of the line without spaces at
//! its ends. Both are in Vim key notation, with `<Leader>` and `<Nop>` (no keys). As with
//! `:text`, `|` is a key, not a new command, so `<Bar>` and `|` are the same. `:se[t]` takes
//! options (see `settings.rs`).

use super::keymap::{IC, MAX_MAP_KEYS, MapKey, MapMode, NVO};
use super::settings::{SetArg, parse_set_arg};
use crate::board::{MAX_SIZE, MAX_TEXT_SIZE, MIN_TEXT_SIZE, is_valid_text_size};
use crate::key::{KeyParseError, parse_keys};

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
    /// `:te[xt] <text>`: a text block of everything after the first space and any options.
    /// `size` is from `++size=N`; without it the block takes `:set size`.
    Text { text: String, size: Option<f64> },
    /// `:ma[th] <formula>`: a formula of everything after the first space and any options, as
    /// [`Command::Text`].
    Math { src: String, size: Option<f64> },
    /// `:si[ze] N`, `:si +N`, `:si -N`: the size of the current text or math object, or of the
    /// text and math in the selection when `:` was typed in Visual mode.
    Size(SizeChange),
    /// `:w[rite] [file]` and `:w! [file]`.
    Write { file: Option<String> },
    /// `:q[uit]`; `:q!` (`force`) quits even when the board has changes.
    Quit { force: bool },
    /// `:wq [file]` and `:wq! [file]`: write, then quit. A write to another file than the
    /// board's own leaves the board's changes unsaved, so it does not quit then, unless
    /// `force` (`!`).
    WriteQuit { file: Option<String>, force: bool },
    /// `:x[it] [file]` and `:x! [file]`: write only when the board has changes, then quit, as
    /// [`Command::WriteQuit`].
    Exit { file: Option<String>, force: bool },
    /// `:u[ndo]`.
    Undo,
    /// `:red[o]`.
    Redo,
    /// `:map {lhs} {rhs}` and its family (`:nnoremap`, `:imap`, `:noremap!` ...): `lhs` typed in
    /// one of `modes` runs `rhs` instead. `noremap`: `rhs` is not mapped again.
    Map {
        modes: &'static [MapMode],
        noremap: bool,
        nowait: bool,
        lhs: Vec<MapKey>,
        rhs: Vec<MapKey>,
    },
    /// A mapping command with keys alone (`:nmap j`) shows their mappings; with none (`:map`),
    /// how many mappings there are.
    ShowMaps {
        modes: &'static [MapMode],
        lhs: Vec<MapKey>,
    },
    /// `:unm[ap] {lhs}` and its family.
    Unmap {
        modes: &'static [MapMode],
        lhs: Vec<MapKey>,
    },
    /// `:mapc[lear]` and its family.
    MapClear { modes: &'static [MapMode] },
    /// `:se[t]` and its arguments; none shows every option.
    Set(Vec<SetArg>),
}

/// What `:si[ze]` does to a size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SizeChange {
    /// `:si N`: make it `N`, from [`MIN_TEXT_SIZE`] to [`MAX_TEXT_SIZE`].
    To(f64),
    /// `:si +N`, `:si -N`: add `N` (taken away when negative). The sizes it makes must be in
    /// range, which only the engine can tell.
    By(f64),
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
    /// `:text` or `:math` with nothing (or only spaces) after the name and its options.
    #[error("missing {0}")]
    MissingBody(&'static str),
    /// A size of text or math (`:si N`, `++size=N`) outside [`MIN_TEXT_SIZE`] to
    /// [`MAX_TEXT_SIZE`]; the field is the size as typed.
    #[error(
        "size {0} is out of range: sizes go from {min} to {max}",
        min = MIN_TEXT_SIZE,
        max = MAX_TEXT_SIZE
    )]
    TextSizeOutOfRange(String),
    /// A `++` option of `:text` or `:math` other than `++size=N`; the field is the word as typed.
    #[error("unknown option `{0}`: `:text` and `:math` take `++size=N`")]
    UnknownTextOption(String),
    /// `:w !cmd` or `:w >>file`, which Vim reads as a shell command or an append; `command` is
    /// the command as typed, with its `!` (`w`, `x`, `wq!`).
    #[error("writing to a command or appending to a file is not supported: `:{command} {file}`")]
    UnsupportedFile { command: String, file: String },
    /// Keys of a mapping that are not Vim key notation.
    #[error("in `{keys}`: {error}")]
    BadKeys { keys: String, error: KeyParseError },
    /// More than [`MAX_MAP_KEYS`] keys on one side of a mapping; the field says which side.
    #[error("{0} may be at most {MAX_MAP_KEYS} keys long")]
    TooManyKeys(&'static str),
    /// A map argument or key name of Vim's that lawh does not have (`<expr>`, `<Cmd>`).
    #[error("`{0}` is not supported in lawh mappings")]
    NotSupported(String),
    /// A mapping command that needs keys and has none.
    #[error("missing the keys for `:{0}`")]
    MissingKeys(&'static str),
    /// `:unmap` with more than the keys to unmap.
    #[error("`:{0}` takes only the keys to unmap")]
    OnlyKeys(&'static str),
    #[error("unknown option `{0}`")]
    UnknownOption(String),
    /// A `:set` word other than `option`, `option?`, `option&` or `option=value`.
    #[error("`{0}`: use `option=value`, `option?` or `option&`")]
    BadSetArgument(String),
    /// A value the option does not take; `usage` says what it takes.
    #[error("bad value `{value}` for `{option}`: it takes {usage}")]
    BadValue {
        option: &'static str,
        value: String,
        usage: String,
    },
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
        name: "cmap",
        min: 2,
        parse: map,
    },
    Row {
        name: "cmapclear",
        min: 5,
        parse: map_clear,
    },
    Row {
        name: "cnoremap",
        min: 3,
        parse: map,
    },
    Row {
        name: "cunmap",
        min: 2,
        parse: unmap,
    },
    Row {
        name: "ellipse",
        min: 2,
        parse: ellipse,
    },
    Row {
        name: "imap",
        min: 2,
        parse: map,
    },
    Row {
        name: "imapclear",
        min: 5,
        parse: map_clear,
    },
    Row {
        name: "inoremap",
        min: 3,
        parse: map,
    },
    Row {
        name: "iunmap",
        min: 2,
        parse: unmap,
    },
    Row {
        name: "line",
        min: 2,
        parse: line,
    },
    Row {
        name: "map",
        min: 3,
        parse: map,
    },
    Row {
        name: "mapclear",
        min: 4,
        parse: map_clear,
    },
    Row {
        name: "math",
        min: 2,
        parse: math,
    },
    Row {
        name: "nmap",
        min: 2,
        parse: map,
    },
    Row {
        name: "nmapclear",
        min: 5,
        parse: map_clear,
    },
    Row {
        name: "nnoremap",
        min: 2,
        parse: map,
    },
    Row {
        name: "noremap",
        min: 2,
        parse: map,
    },
    Row {
        name: "nunmap",
        min: 3,
        parse: unmap,
    },
    Row {
        name: "omap",
        min: 2,
        parse: map,
    },
    Row {
        name: "omapclear",
        min: 5,
        parse: map_clear,
    },
    Row {
        name: "onoremap",
        min: 3,
        parse: map,
    },
    Row {
        name: "ounmap",
        min: 2,
        parse: unmap,
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
        name: "set",
        min: 2,
        parse: set,
    },
    Row {
        name: "size",
        min: 2,
        parse: size_command,
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
        name: "unmap",
        min: 3,
        parse: unmap,
    },
    Row {
        name: "vmap",
        min: 2,
        parse: map,
    },
    Row {
        name: "vmapclear",
        min: 5,
        parse: map_clear,
    },
    Row {
        name: "vnoremap",
        min: 2,
        parse: map,
    },
    Row {
        name: "vunmap",
        min: 2,
        parse: unmap,
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
    Row {
        name: "xmap",
        min: 2,
        parse: map,
    },
    Row {
        name: "xmapclear",
        min: 5,
        parse: map_clear,
    },
    Row {
        name: "xnoremap",
        min: 2,
        parse: map,
    },
    Row {
        name: "xunmap",
        min: 2,
        parse: unmap,
    },
];

/// What follows a command's name.
#[derive(Clone, Copy)]
struct Args<'a> {
    name: &'static str,
    /// The name as typed: `name` or an abbreviation of it.
    typed: &'a str,
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
        typed: name,
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
    let (size, text) = body(args, "the text: `:te[xt] <text>`")?;
    Ok(Command::Text { text, size })
}

fn math(args: Args<'_>) -> Result<Command, CommandError> {
    let (size, src) = body(args, "the formula: `:ma[th] <formula>`")?;
    Ok(Command::Math { src, size })
}

/// The options at the start (`++size=N`, see the module doc), then everything after them,
/// verbatim: spaces at either end, `|`, `"` and `\` stay as typed. Nothing but spaces is
/// missing.
fn body(args: Args<'_>, missing: &'static str) -> Result<(Option<f64>, String), CommandError> {
    args.no_bang()?;
    let mut rest = args.rest;
    let mut size = None;
    while let Some(option) = rest
        .strip_prefix("++")
        .filter(|after| after.starts_with(|c: char| c.is_ascii_alphabetic()))
    {
        let (word, after) = option
            .split_once(char::is_whitespace)
            .unwrap_or((option, ""));
        match word.split_once('=') {
            Some(("size", value)) if size.is_none() => {
                size = Some(text_size(value)?);
            }
            Some(("size", _)) => return Err(CommandError::RepeatedKey("++size".to_owned())),
            None if word == "size" => {
                return Err(CommandError::MissingNumber("`++size=N`"));
            }
            _ => return Err(CommandError::UnknownTextOption(format!("++{word}"))),
        }
        rest = after;
    }
    if rest.trim().is_empty() {
        Err(CommandError::MissingBody(missing))
    } else {
        Ok((size, rest.to_owned()))
    }
}

/// A size of text or math, from [`MIN_TEXT_SIZE`] to [`MAX_TEXT_SIZE`].
fn text_size(word: &str) -> Result<f64, CommandError> {
    let size = number(word)?;
    if is_valid_text_size(size) {
        Ok(size)
    } else {
        Err(CommandError::TextSizeOutOfRange(word.to_owned()))
    }
}

/// `:si[ze] N`, `:si +N`, `:si -N`: one number, a sign making it a change.
fn size_command(args: Args<'_>) -> Result<Command, CommandError> {
    const USAGE: &str = "`:si[ze] N`, `:si +N` or `:si -N`";
    args.no_bang()?;
    let mut words = args.rest.split_whitespace();
    let word = words.next().ok_or(CommandError::MissingNumber(USAGE))?;
    if words.next().is_some() {
        return Err(CommandError::TooManyNumbers(USAGE));
    }
    if word.starts_with(['+', '-']) {
        Ok(Command::Size(SizeChange::By(number(word)?)))
    } else {
        Ok(Command::Size(SizeChange::To(text_size(word)?)))
    }
}

fn write(args: Args<'_>) -> Result<Command, CommandError> {
    Ok(Command::Write { file: file(args)? })
}

fn write_quit(args: Args<'_>) -> Result<Command, CommandError> {
    Ok(Command::WriteQuit {
        file: file(args)?,
        force: args.bang,
    })
}

fn exit(args: Args<'_>) -> Result<Command, CommandError> {
    Ok(Command::Exit {
        file: file(args)?,
        force: args.bang,
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
fn file(args: Args<'_>) -> Result<Option<String>, CommandError> {
    let name = args.rest.trim();
    if name.is_empty() {
        Ok(None)
    } else if name.starts_with('!') || name.starts_with(">>") {
        let bang = if args.bang { "!" } else { "" };
        Err(CommandError::UnsupportedFile {
            command: format!("{}{bang}", args.typed),
            file: name.to_owned(),
        })
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

/// `:map` and its family: keys and what they map to make a mapping; keys alone show theirs.
fn map(args: Args<'_>) -> Result<Command, CommandError> {
    let noremap = args.name.ends_with("noremap");
    let modes = map_modes(&args, if noremap { "noremap" } else { "map" })?;
    let (nowait, rest) = map_arguments(args.rest)?;
    let (lhs, rhs) = split_keys(rest);
    let lhs = map_keys(lhs, "the keys to map")?;
    if rhs.is_empty() {
        return Ok(Command::ShowMaps { modes, lhs });
    }
    if lhs.is_empty() {
        return Err(CommandError::MissingKeys(args.name));
    }
    let rhs = map_keys(rhs, "what they map to")?;
    Ok(Command::Map {
        modes,
        noremap,
        nowait,
        lhs,
        rhs,
    })
}

fn unmap(args: Args<'_>) -> Result<Command, CommandError> {
    let modes = map_modes(&args, "unmap")?;
    let (_, rest) = map_arguments(args.rest)?;
    let (lhs, extra) = split_keys(rest);
    if !extra.is_empty() {
        return Err(CommandError::OnlyKeys(args.name));
    }
    let lhs = map_keys(lhs, "the keys to unmap")?;
    if lhs.is_empty() {
        return Err(CommandError::MissingKeys(args.name));
    }
    Ok(Command::Unmap { modes, lhs })
}

fn map_clear(args: Args<'_>) -> Result<Command, CommandError> {
    let modes = map_modes(&args, "mapclear")?;
    if args.rest.trim().is_empty() {
        Ok(Command::MapClear { modes })
    } else {
        Err(CommandError::NoArguments(args.name))
    }
}

/// The modes of a mapping command: from the letter before `base` in its name (`n` in
/// `nnoremap`), or without one, Normal, Visual and operator-pending mode, or with `!`
/// Insert and command-line mode.
fn map_modes(args: &Args<'_>, base: &str) -> Result<&'static [MapMode], CommandError> {
    let letter = args.name.strip_suffix(base).unwrap_or(args.name);
    match (letter, args.bang) {
        ("", false) => Ok(NVO),
        ("", true) => Ok(IC),
        (_, true) => Err(CommandError::NoBang(args.name)),
        ("n", false) => Ok(&[MapMode::Normal]),
        ("v" | "x", false) => Ok(&[MapMode::Visual]),
        ("o", false) => Ok(&[MapMode::OperatorPending]),
        ("i", false) => Ok(&[MapMode::Insert]),
        ("c", false) => Ok(&[MapMode::CommandLine]),
        _ => Err(CommandError::Unknown(args.name.to_owned())),
    }
}

/// Vim's map arguments that lawh does not take. `<buffer>`, `<script>` and `<unique>` change
/// which mappings apply; `<expr>` runs Vim script.
const UNSUPPORTED_ARGUMENTS: &[&str] = &["<buffer>", "<expr>", "<script>", "<unique>"];

/// Reads the map arguments at the start of `rest`, as Vim does (spelled exactly so): whether
/// `<nowait>` was given, and the rest after them. `<silent>` and `<special>` have no effect:
/// lawh shows nothing as a mapping runs and always reads `<>` notation.
fn map_arguments(rest: &str) -> Result<(bool, &str), CommandError> {
    let mut nowait = false;
    let mut rest = rest.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("<nowait>") {
            nowait = true;
            rest = after.trim_start();
        } else if let Some(after) = ["<silent>", "<special>"]
            .iter()
            .find_map(|arg| rest.strip_prefix(arg))
        {
            rest = after.trim_start();
        } else if let Some(arg) = UNSUPPORTED_ARGUMENTS.iter().find(|a| rest.starts_with(**a)) {
            return Err(CommandError::NotSupported((*arg).to_owned()));
        } else {
            return Ok((nowait, rest));
        }
    }
}

/// The keys to map, up to the first space, and the rest without spaces at its ends: Vim keeps
/// trailing spaces in what keys map to, but an invisible space at the end of a line should not
/// become keys (`<Space>` types one on purpose).
fn split_keys(rest: &str) -> (&str, &str) {
    let rest = rest.trim_start();
    let (lhs, rhs) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    (lhs, rhs.trim())
}

/// What a name in `<>` means in a mapping, besides a key.
#[derive(Clone, Copy)]
enum MapName {
    /// `<Leader>`.
    Leader,
    /// `<Nop>`: no keys.
    Nothing,
    /// A name Vim has and lawh does not: refused, not read as plain characters.
    Unsupported,
}

/// Names in `<>` with a meaning in mappings, lowercase (they are matched ignoring case).
const MAP_NAMES: &[(&str, MapName)] = &[
    ("<leader>", MapName::Leader),
    ("<nop>", MapName::Nothing),
    ("<cmd>", MapName::Unsupported),
    ("<localleader>", MapName::Unsupported),
    ("<plug>", MapName::Unsupported),
    ("<scriptcmd>", MapName::Unsupported),
    ("<sid>", MapName::Unsupported),
    ("<snr>", MapName::Unsupported),
];

/// Reads one side of a mapping (`what` names it in errors): keys in Vim notation, `<Leader>`
/// and `<Nop>`, at most [`MAX_MAP_KEYS`].
fn map_keys(text: &str, what: &'static str) -> Result<Vec<MapKey>, CommandError> {
    // ASCII lowercase keeps every byte offset.
    let lower = text.to_ascii_lowercase();
    let mut keys = Vec::new();
    let mut from = 0;
    for (at, _) in lower.match_indices('<') {
        if keys.len() > MAX_MAP_KEYS {
            return Err(CommandError::TooManyKeys(what));
        }
        if at < from {
            continue;
        }
        let tail = lower.get(at..).unwrap_or_default();
        let Some((name, meaning)) = MAP_NAMES.iter().find(|(name, _)| tail.starts_with(name))
        else {
            continue;
        };
        push_keys(&mut keys, text, from, at)?;
        match meaning {
            MapName::Leader => keys.push(MapKey::Leader),
            MapName::Nothing => {}
            MapName::Unsupported => {
                let typed = text.get(at..at + name.len()).unwrap_or(name);
                return Err(CommandError::NotSupported(typed.to_owned()));
            }
        }
        from = at + name.len();
    }
    push_keys(&mut keys, text, from, text.len())?;
    if keys.len() > MAX_MAP_KEYS {
        return Err(CommandError::TooManyKeys(what));
    }
    Ok(keys)
}

/// Reads bytes `from..to` of `text` as keys onto `keys`.
fn push_keys(
    keys: &mut Vec<MapKey>,
    text: &str,
    from: usize,
    to: usize,
) -> Result<(), CommandError> {
    let part = text.get(from..to).unwrap_or_default();
    let parsed = parse_keys(part).map_err(|error| CommandError::BadKeys {
        keys: text.to_owned(),
        error: shifted(error, from),
    })?;
    keys.extend(parsed.into_iter().map(MapKey::Key));
    Ok(())
}

/// `error` with its byte offset moved on by `by`, from a part of a text to the whole.
fn shifted(error: KeyParseError, by: usize) -> KeyParseError {
    match error {
        KeyParseError::BadModifiedKey { at, text } => {
            KeyParseError::BadModifiedKey { at: at + by, text }
        }
        KeyParseError::FunctionKey { at, text } => KeyParseError::FunctionKey { at: at + by, text },
        KeyParseError::ControlChar { at, c } => KeyParseError::ControlChar { at: at + by, c },
    }
}

/// `:se[t]`: options until the end of the line or a `"`, which starts a comment.
fn set(args: Args<'_>) -> Result<Command, CommandError> {
    args.no_bang()?;
    let words = args
        .rest
        .split_whitespace()
        .take_while(|word| !word.starts_with('"'));
    Ok(Command::Set(
        words.map(parse_set_arg).collect::<Result<_, _>>()?,
    ))
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
