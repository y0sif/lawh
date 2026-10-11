//! Settings that change how the engine reads keys, and `:set`.
//!
//! `:se[t]` takes options as Vim does: `{option}={value}` (or `:`) sets one, `{option}?` or
//! `{option}` alone shows its value, `{option}&` puts back its default, and `:set` alone shows
//! them all. Several may follow one `:set`; a word starting with `"` ends the line (a comment).
//! The options are `escape` (`board` or `block`), `leader` (one key in Vim notation) and
//! `timeoutlen` (`tm`, milliseconds).

use super::Engine;
use super::command::CommandError;
use crate::key::{Key, parse_keys};

/// Longest `timeoutlen`, in milliseconds (10 s).
///
/// `timeoutlen` is how long keys that start a longer mapping wait for the rest. Nobody pauses
/// 10 s inside one mapping, so a larger value can only be a typo (`timeoutlen=100000`), which
/// would leave typed keys stuck on the status line for minutes.
pub const MAX_TIMEOUTLEN: u64 = 10_000;

/// Where `<Esc>` goes from Insert mode in a text block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Escape {
    /// Back to where Insert mode was entered from: the board (with the block as the current
    /// object) or block-Normal mode.
    #[default]
    Board,
    /// Always to block-Normal mode; `<Esc>` there goes on to the board.
    Block,
}

impl Escape {
    /// The value as `:set` takes it: `board` or `block`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Board => "board",
            Self::Block => "block",
        }
    }
}

/// The engine's settings. `:set` changes them, as does
/// [`Engine::settings_mut`](super::Engine::settings_mut).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub escape: Escape,
    /// The key `<Leader>` stands for in mappings, when a mapping is made: `\` by default.
    pub leader: Key,
    /// How long keys that start a longer mapping wait for the rest, in milliseconds (see
    /// [`Effect::ArmTimer`](super::Effect::ArmTimer)): 1000 by default, at most
    /// [`MAX_TIMEOUTLEN`].
    pub timeoutlen: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            escape: Escape::Board,
            leader: Key::char('\\'),
            timeoutlen: 1000,
        }
    }
}

/// An option `:set` knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionName {
    Escape,
    Leader,
    TimeoutLen,
}

impl OptionName {
    /// Every option, in the order `:set` alone shows them.
    const ALL: [Self; 3] = [Self::Escape, Self::Leader, Self::TimeoutLen];

    /// The option's name in `:set`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Escape => "escape",
            Self::Leader => "leader",
            Self::TimeoutLen => "timeoutlen",
        }
    }

    /// The option of a name in `:set`, or its short name (`tm`).
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "escape" => Some(Self::Escape),
            "leader" => Some(Self::Leader),
            "timeoutlen" | "tm" => Some(Self::TimeoutLen),
            _ => None,
        }
    }
}

/// A value for an option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionValue {
    Escape(Escape),
    Leader(Key),
    TimeoutLen(u64),
}

/// One argument of `:set`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetArg {
    /// `{option}?` or `{option}`: show its value.
    Show(OptionName),
    /// `{option}&`: back to its default.
    Reset(OptionName),
    /// `{option}={value}` or `{option}:{value}`.
    Assign(OptionValue),
}

/// Reads one word of `:set`.
pub(super) fn parse_set_arg(word: &str) -> Result<SetArg, CommandError> {
    let name_len = word
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(word.len());
    let (name, rest) = word.split_at(name_len);
    let option = OptionName::from_name(name).ok_or_else(|| {
        CommandError::UnknownOption(if name.is_empty() { word } else { name }.to_owned())
    })?;
    let value = match rest {
        "" | "?" => return Ok(SetArg::Show(option)),
        "&" => return Ok(SetArg::Reset(option)),
        _ => match rest.strip_prefix(['=', ':']) {
            Some(value) => value,
            None => return Err(CommandError::BadSetArgument(word.to_owned())),
        },
    };
    let bad = |usage: String| CommandError::BadValue {
        option: option.name(),
        value: value.to_owned(),
        usage,
    };
    let value = match option {
        OptionName::Escape => match value {
            "board" => OptionValue::Escape(Escape::Board),
            "block" => OptionValue::Escape(Escape::Block),
            _ => return Err(bad("`board` or `block`".to_owned())),
        },
        OptionName::Leader => match parse_keys(value).as_deref() {
            Ok([key]) => OptionValue::Leader(*key),
            _ => return Err(bad("one key, such as `<Space>` or `,`".to_owned())),
        },
        OptionName::TimeoutLen => {
            let ms = Some(value)
                .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|ms| *ms <= MAX_TIMEOUTLEN);
            match ms {
                Some(ms) => OptionValue::TimeoutLen(ms),
                None => return Err(bad(format!("milliseconds from 0 to {MAX_TIMEOUTLEN}"))),
            }
        }
    };
    Ok(SetArg::Assign(value))
}

impl Settings {
    /// `option` and its value as `:set` takes them: `timeoutlen=1000`, `leader=<Space>`.
    fn show(&self, option: OptionName) -> String {
        let value = match option {
            OptionName::Escape => self.escape.name().to_owned(),
            OptionName::Leader => self.leader.to_string(),
            OptionName::TimeoutLen => self.timeoutlen.to_string(),
        };
        format!("{}={value}", option.name())
    }

    fn reset(&mut self, option: OptionName) {
        let default = Self::default();
        match option {
            OptionName::Escape => self.escape = default.escape,
            OptionName::Leader => self.leader = default.leader,
            OptionName::TimeoutLen => self.timeoutlen = default.timeoutlen,
        }
    }

    fn assign(&mut self, value: OptionValue) {
        match value {
            OptionValue::Escape(escape) => self.escape = escape,
            OptionValue::Leader(key) => self.leader = key,
            OptionValue::TimeoutLen(ms) => self.timeoutlen = ms,
        }
    }
}

impl Engine {
    /// Runs `:set`: the values it shows, if any (all of them for `:set` alone).
    pub(super) fn set(&mut self, args: &[SetArg]) -> Option<String> {
        let shown: Vec<String> = if args.is_empty() {
            OptionName::ALL
                .iter()
                .map(|option| self.settings.show(*option))
                .collect()
        } else {
            let mut shown = Vec::new();
            for arg in args {
                match *arg {
                    SetArg::Show(option) => shown.push(self.settings.show(option)),
                    SetArg::Reset(option) => self.settings.reset(option),
                    SetArg::Assign(value) => self.settings.assign(value),
                }
            }
            shown
        };
        (!shown.is_empty()).then(|| shown.join("  "))
    }
}
