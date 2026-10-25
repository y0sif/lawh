//! Settings that change how the engine reads keys, and `:set`.
//!
//! `:se[t]` takes options as Vim does: `{option}={value}` (or `:`) sets one, `{option}?` or
//! `{option}` alone shows its value, `{option}&` puts back its default, and `:set` alone shows
//! them all. Several may follow one `:set`; a word starting with `"` ends the line (a comment).
//! The options are `escape` (`board` or `block`), `exportbg` (what `:export` draws behind the
//! board: `paper` or `none`), `jumplabels` (the characters of jump labels), `leader` (one key in
//! Vim notation), `pngscale` (the scale of a PNG export), `size` (the size of new text and math
//! blocks) and `timeoutlen` (`tm`, milliseconds).

use super::Engine;
use super::command::CommandError;
use super::jump::{LabelChars, MAX_LABEL_CHARS, MIN_LABEL_CHARS};
use crate::board::{MAX_TEXT_SIZE, MIN_TEXT_SIZE, is_valid_text_size};
use crate::export::{
    DEFAULT_PNG_SCALE_HUNDREDTHS, ExportBackground, ExportFormat, ExportOptions,
    MAX_PNG_SCALE_HUNDREDTHS, MIN_PNG_SCALE_HUNDREDTHS,
};
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
    /// The characters jump labels are made of, nearest object first (see [`LabelChars`]):
    /// [`DEFAULT_JUMP_LABELS`](super::DEFAULT_JUMP_LABELS) by default.
    pub jumplabels: LabelChars,
    /// The key `<Leader>` stands for in mappings, when a mapping is made: `\` by default.
    pub leader: Key,
    /// How long keys that start a longer mapping wait for the rest, in milliseconds (see
    /// [`Effect::ArmTimer`](super::Effect::ArmTimer)): 1000 by default, at most
    /// [`MAX_TIMEOUTLEN`].
    pub timeoutlen: u64,
    /// The size of new text and math blocks (`:set size`), in hundredths: 100 is size 1 (see
    /// [`DEFAULT_TEXT_SIZE`](crate::board::DEFAULT_TEXT_SIZE)), 150 is 1.5. Hundredths keep
    /// [`OptionValue`] `Eq`, which a float would not be; [`Settings::size`] gives the size. A
    /// value outside [`MIN_TEXT_SIZE`] to [`MAX_TEXT_SIZE`], which only this field can hold
    /// (`:set` refuses one), makes the board refuse new blocks, with a message.
    pub size_hundredths: u32,
    /// The scale of a PNG export (`:set pngscale`), in hundredths as
    /// [`Settings::size_hundredths`] is: 200, a scale of 2, by default
    /// ([`DEFAULT_PNG_SCALE_HUNDREDTHS`]). `:set` takes [`MIN_PNG_SCALE_HUNDREDTHS`] to
    /// [`MAX_PNG_SCALE_HUNDREDTHS`]; the renderer keeps a value this field holds outside them
    /// within them.
    pub pngscale_hundredths: u32,
    /// What an export draws behind the board's objects (`:set exportbg`): the paper by default.
    pub exportbg: ExportBackground,
}

impl Settings {
    /// The size of new text and math blocks: [`Settings::size_hundredths`] as a size.
    pub fn size(&self) -> f64 {
        f64::from(self.size_hundredths) / 100.0
    }

    /// How `:export` exports to `format`: with the `exportbg` and `pngscale` options.
    pub fn export_options(&self, format: ExportFormat) -> ExportOptions {
        ExportOptions {
            format,
            background: self.exportbg,
            png_scale_hundredths: self.pngscale_hundredths,
        }
    }
}

/// [`DEFAULT_TEXT_SIZE`](crate::board::DEFAULT_TEXT_SIZE) in hundredths, the default of
/// `:set size`.
const DEFAULT_SIZE_HUNDREDTHS: u32 = 100;

impl Default for Settings {
    fn default() -> Self {
        Self {
            escape: Escape::Board,
            jumplabels: LabelChars::default(),
            leader: Key::char('\\'),
            timeoutlen: 1000,
            size_hundredths: DEFAULT_SIZE_HUNDREDTHS,
            pngscale_hundredths: DEFAULT_PNG_SCALE_HUNDREDTHS,
            exportbg: ExportBackground::Paper,
        }
    }
}

/// An option `:set` knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionName {
    Escape,
    ExportBg,
    JumpLabels,
    Leader,
    PngScale,
    Size,
    TimeoutLen,
}

impl OptionName {
    /// Every option, in the order `:set` alone shows them.
    const ALL: [Self; 7] = [
        Self::Escape,
        Self::ExportBg,
        Self::JumpLabels,
        Self::Leader,
        Self::PngScale,
        Self::Size,
        Self::TimeoutLen,
    ];

    /// The option's name in `:set`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Escape => "escape",
            Self::ExportBg => "exportbg",
            Self::JumpLabels => "jumplabels",
            Self::Leader => "leader",
            Self::PngScale => "pngscale",
            Self::Size => "size",
            Self::TimeoutLen => "timeoutlen",
        }
    }

    /// The option of a name in `:set`, or its short name (`tm`).
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "escape" => Some(Self::Escape),
            "exportbg" => Some(Self::ExportBg),
            "jumplabels" => Some(Self::JumpLabels),
            "leader" => Some(Self::Leader),
            "pngscale" => Some(Self::PngScale),
            "size" => Some(Self::Size),
            "timeoutlen" | "tm" => Some(Self::TimeoutLen),
            _ => None,
        }
    }
}

/// A value for an option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "a value stays `Copy` and `Eq`, so `jumplabels` is a fixed array; `:set` makes few"
)]
pub enum OptionValue {
    Escape(Escape),
    ExportBg(ExportBackground),
    JumpLabels(LabelChars),
    Leader(Key),
    /// The PNG export's scale in hundredths (see [`Settings::pngscale_hundredths`]).
    PngScale(u32),
    /// The size of new blocks in hundredths (see [`Settings::size_hundredths`]).
    Size(u32),
    TimeoutLen(u64),
}

/// One argument of `:set`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "an `OptionValue` stays `Copy`, so `jumplabels` is a fixed array; `:set` makes few"
)]
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
        OptionName::ExportBg => match value {
            "paper" => OptionValue::ExportBg(ExportBackground::Paper),
            "none" => OptionValue::ExportBg(ExportBackground::None),
            _ => return Err(bad("`paper` or `none`".to_owned())),
        },
        OptionName::JumpLabels => match LabelChars::new(value) {
            Some(chars) => OptionValue::JumpLabels(chars),
            None => {
                return Err(bad(format!(
                    "{MIN_LABEL_CHARS} to {MAX_LABEL_CHARS} different printable characters, \
                     such as `asdfghjkl`"
                )));
            }
        },
        OptionName::Leader => match parse_keys(value).as_deref() {
            Ok([key]) => OptionValue::Leader(*key),
            _ => return Err(bad("one key, such as `<Space>` or `,`".to_owned())),
        },
        OptionName::PngScale => match parse_png_scale(value) {
            Some(h) => OptionValue::PngScale(h),
            None => {
                return Err(bad(format!(
                    "a scale from {} to {}, with at most two decimals",
                    show_hundredths(MIN_PNG_SCALE_HUNDREDTHS),
                    show_hundredths(MAX_PNG_SCALE_HUNDREDTHS)
                )));
            }
        },
        OptionName::Size => {
            match hundredths(value).filter(|h| is_valid_text_size(hundredths_size(*h))) {
                Some(h) => OptionValue::Size(h),
                None => {
                    return Err(bad(format!(
                        "a size from {MIN_TEXT_SIZE} to {MAX_TEXT_SIZE}, with at most two decimals"
                    )));
                }
            }
        }
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

/// A PNG export's scale as `:set pngscale` takes it, for the app's command line too: a number
/// with at most two decimals (`2`, `1.5`, `.25`, `+2`) from 0.25 to 8, in hundredths (see
/// [`Settings::pngscale_hundredths`]). `None` for anything else.
pub fn parse_png_scale(value: &str) -> Option<u32> {
    hundredths(value).filter(|h| (MIN_PNG_SCALE_HUNDREDTHS..=MAX_PNG_SCALE_HUNDREDTHS).contains(h))
}

/// A size in hundredths as a size: 150 is 1.5.
fn hundredths_size(h: u32) -> f64 {
    f64::from(h) / 100.0
}

/// A number with at most two decimals (`1`, `1.5`, `.25`, `+2`), in hundredths, read from its
/// digits so that no float rounding comes in: `None` for anything else, or past `u32`.
fn hundredths(value: &str) -> Option<u32> {
    let unsigned = value.strip_prefix('+').unwrap_or(value);
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if (whole.is_empty() && fraction.is_empty())
        || fraction.len() > 2
        || !digits(whole)
        || !digits(fraction)
    {
        return None;
    }
    let whole: u32 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let fraction: u32 = format!("{fraction:0<2}").parse().ok()?;
    whole.checked_mul(100)?.checked_add(fraction)
}

/// A size or scale in hundredths as `:set` shows it: `1`, `1.5`, `0.25`.
fn show_hundredths(h: u32) -> String {
    hundredths_size(h).to_string()
}

impl Settings {
    /// `option` and its value as `:set` takes them: `timeoutlen=1000`, `leader=<Space>`.
    fn show(&self, option: OptionName) -> String {
        let value = match option {
            OptionName::Escape => self.escape.name().to_owned(),
            OptionName::ExportBg => self.exportbg.name().to_owned(),
            OptionName::JumpLabels => self.jumplabels.to_string(),
            OptionName::Leader => self.leader.to_string(),
            OptionName::PngScale => show_hundredths(self.pngscale_hundredths),
            OptionName::Size => show_hundredths(self.size_hundredths),
            OptionName::TimeoutLen => self.timeoutlen.to_string(),
        };
        format!("{}={value}", option.name())
    }

    fn reset(&mut self, option: OptionName) {
        let default = Self::default();
        match option {
            OptionName::Escape => self.escape = default.escape,
            OptionName::ExportBg => self.exportbg = default.exportbg,
            OptionName::JumpLabels => self.jumplabels = default.jumplabels,
            OptionName::Leader => self.leader = default.leader,
            OptionName::PngScale => self.pngscale_hundredths = default.pngscale_hundredths,
            OptionName::Size => self.size_hundredths = default.size_hundredths,
            OptionName::TimeoutLen => self.timeoutlen = default.timeoutlen,
        }
    }

    fn assign(&mut self, value: OptionValue) {
        match value {
            OptionValue::Escape(escape) => self.escape = escape,
            OptionValue::ExportBg(background) => self.exportbg = background,
            OptionValue::JumpLabels(chars) => self.jumplabels = chars,
            OptionValue::Leader(key) => self.leader = key,
            OptionValue::PngScale(h) => self.pngscale_hundredths = h,
            OptionValue::Size(h) => self.size_hundredths = h,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::DEFAULT_TEXT_SIZE;

    #[test]
    fn sizes_are_read_in_hundredths_from_their_digits() {
        for (typed, h) in [
            ("1", 100),
            ("1.5", 150),
            ("1.50", 150),
            (".25", 25),
            ("0.29", 29),
            ("+2", 200),
            ("16.", 1600),
            ("007", 700),
        ] {
            assert_eq!(hundredths(typed), Some(h), "{typed}");
        }
        for bad in [
            "",
            ".",
            "+",
            "-1",
            "1.234",
            "1e2",
            "x",
            "1.2.3",
            "99999999999",
        ] {
            assert_eq!(hundredths(bad), None, "{bad}");
        }
        assert_eq!(show_hundredths(25), "0.25");
        assert_eq!(show_hundredths(100), "1");
        assert_eq!(show_hundredths(1600), "16");
        assert_eq!(Settings::default().size(), DEFAULT_TEXT_SIZE);
        assert_eq!(
            hundredths_size(DEFAULT_SIZE_HUNDREDTHS),
            DEFAULT_TEXT_SIZE,
            "the default is the board's"
        );
    }

    #[test]
    fn png_scales_are_hundredths_from_a_quarter_to_eight() {
        for (typed, h) in [
            ("2", 200),
            (".25", 25),
            ("8", 800),
            ("1.5", 150),
            ("+0.75", 75),
        ] {
            assert_eq!(parse_png_scale(typed), Some(h), "{typed}");
        }
        for bad in ["0.24", "8.01", "0", "-2", "1.234", "2e0", "", "x"] {
            assert_eq!(parse_png_scale(bad), None, "{bad}");
        }
    }

    #[test]
    fn export_options_come_from_the_settings() {
        let settings = Settings::default();
        assert_eq!(
            settings.export_options(ExportFormat::Png),
            ExportOptions::default(),
            "the defaults are the export's"
        );
        let settings = Settings {
            pngscale_hundredths: 25,
            exportbg: ExportBackground::None,
            ..Settings::default()
        };
        assert_eq!(
            settings.export_options(ExportFormat::Pdf),
            ExportOptions {
                format: ExportFormat::Pdf,
                background: ExportBackground::None,
                png_scale_hundredths: 25,
            }
        );
    }
}
