//! Keys and Vim key notation: `<Esc>`, `<C-r>`, `<S-Tab>`, `3l2j:ci 2<CR>`.
//!
//! The engine, its test scripts and (later) key mappings all read keys in this notation.
//!
//! - A plain character is itself.
//! - `<...>` names a key, ignoring case: `<Esc> <CR> <Enter> <Return> <Tab> <BS> <Del> <Space>
//!   <lt> <Bar> <Bslash> <Up> <Down> <Left> <Right> <Home> <End> <PageUp> <PageDown> <Insert>`
//!   and `<F1>` to `<F24>`.
//! - Modifiers go before the key and stack: `C-` Ctrl, `M-` or `A-` Alt, `S-` Shift, `D-` Cmd,
//!   as in `<C-S-Tab>`. With a modifier, one character is a key too: `<C-r>`, `<M-->`, `<C->>`.
//!
//! Keys are kept in one canonical form, so equal keys compare equal (see [`Key::new`]): Ctrl
//! with an ASCII letter is the lowercase letter (`<C-R>` is `<C-r>`), `<C-[>` is `<Esc>`, and
//! Shift with a character is the character itself (`<S-a>` is `A`, `<S-1>` is `1`).
//!
//! A `<` that does not start a key name is a literal `<`, as in Vim: `a<b`, `<>` and `<Nope>`
//! are all plain characters. Three mistakes are errors instead (see [`KeyParseError`]):
//! modifiers without a key name and `>` (`<C-Nope>`, `<C->`, `<C-r`), a function key past
//! `<F24>`, and a raw control character, which would be invisible in a script.

use std::fmt::{self, Display, Formatter, Write as _};

/// One key press: a key and the modifiers held with it.
///
/// Build keys with [`Key::new`] (or [`Key::char`], [`Key::ctrl`], [`Key::named`]) so they are
/// canonical: the engine compares keys as they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub mods: Mods,
}

/// Which key: a character as typed, or a key with no character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    /// A typed character, Shift already applied (`A`, not Shift + `a`). Space is `Char(' ')`.
    Char(char),
    Esc,
    Enter,
    Tab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    /// A function key, `F(1)` to `F(24)`.
    F(u8),
}

/// Modifiers held with a key. `cmd` is the macOS Command key (`D-` in Vim notation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent modifier keys, named as in Vim notation"
)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
}

impl Mods {
    /// No modifiers.
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
        cmd: false,
    };

    /// Ctrl alone.
    pub const CTRL: Self = Self {
        ctrl: true,
        ..Self::NONE
    };

    pub const fn is_empty(self) -> bool {
        !(self.ctrl || self.alt || self.shift || self.cmd)
    }
}

/// Highest function key, `<F24>`.
const MAX_F: u8 = 24;

impl Key {
    /// The canonical key for `code` with `mods`:
    /// - Shift with a character is dropped, an ASCII letter becoming uppercase (`<S-a>` is `A`):
    ///   a typed character already carries Shift.
    /// - Ctrl with an ASCII letter is the lowercase letter (`<C-R>` is `<C-r>`), so `<C-S-r>` is
    ///   `<C-r>` too.
    /// - Ctrl with `[` is `<Esc>`, keeping any other modifiers.
    ///
    /// Shift stays on keys without a character (`<S-Tab>`).
    pub const fn new(code: KeyCode, mods: Mods) -> Self {
        let mut mods = mods;
        let code = match code {
            KeyCode::Char(c) => {
                let mut c = c;
                if mods.shift {
                    c = c.to_ascii_uppercase();
                    mods.shift = false;
                }
                if mods.ctrl && c == '[' {
                    mods.ctrl = false;
                    KeyCode::Esc
                } else {
                    if mods.ctrl {
                        c = c.to_ascii_lowercase();
                    }
                    KeyCode::Char(c)
                }
            }
            other => other,
        };
        Self { code, mods }
    }

    /// A character typed without modifiers.
    pub const fn char(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            mods: Mods::NONE,
        }
    }

    /// Ctrl with a character, canonical: `Key::ctrl('R')` is `<C-r>`, `Key::ctrl('[')` is `<Esc>`.
    pub const fn ctrl(c: char) -> Self {
        Self::new(KeyCode::Char(c), Mods::CTRL)
    }

    /// A key without modifiers, such as `Key::named(KeyCode::Esc)`.
    pub const fn named(code: KeyCode) -> Self {
        Self {
            code,
            mods: Mods::NONE,
        }
    }

    /// This key in canonical form (see [`Key::new`]).
    #[must_use]
    pub const fn canonical(self) -> Self {
        Self::new(self.code, self.mods)
    }
}

/// Key names in `<...>`, matched ignoring case. A key with several names prints the first.
const NAMES: &[(&str, KeyCode)] = &[
    ("Esc", KeyCode::Esc),
    ("CR", KeyCode::Enter),
    ("Enter", KeyCode::Enter),
    ("Return", KeyCode::Enter),
    ("Tab", KeyCode::Tab),
    ("BS", KeyCode::Backspace),
    ("Del", KeyCode::Delete),
    ("Space", KeyCode::Char(' ')),
    ("lt", KeyCode::Char('<')),
    ("Bar", KeyCode::Char('|')),
    ("Bslash", KeyCode::Char('\\')),
    ("Up", KeyCode::Up),
    ("Down", KeyCode::Down),
    ("Left", KeyCode::Left),
    ("Right", KeyCode::Right),
    ("Home", KeyCode::Home),
    ("End", KeyCode::End),
    ("PageUp", KeyCode::PageUp),
    ("PageDown", KeyCode::PageDown),
    ("Insert", KeyCode::Insert),
];

fn name_of(code: KeyCode) -> Option<&'static str> {
    NAMES
        .iter()
        .find(|(_, c)| *c == code)
        .map(|(name, _)| *name)
}

impl Display for Key {
    /// Vim notation in canonical form: `a`, `<lt>`, `<Space>`, `<C-r>`, `<M-S-F5>`. For a
    /// canonical key `k`, `parse_keys(&k.to_string())` is `[k]`.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let name = name_of(self.code);
        if let (KeyCode::Char(c), None, true) = (self.code, name, self.mods.is_empty()) {
            return f.write_char(c);
        }
        f.write_char('<')?;
        let Mods {
            ctrl,
            alt,
            shift,
            cmd,
        } = self.mods;
        for (held, prefix) in [(ctrl, "C-"), (alt, "M-"), (shift, "S-"), (cmd, "D-")] {
            if held {
                f.write_str(prefix)?;
            }
        }
        match (self.code, name) {
            (_, Some(name)) => f.write_str(name)?,
            (KeyCode::F(n), None) => write!(f, "F{n}")?,
            (KeyCode::Char(c), None) => f.write_char(c)?,
            (code, None) => write!(f, "{code:?}")?,
        }
        f.write_char('>')
    }
}

/// `keys` in Vim notation, one after another: `3d<Esc>`.
pub fn format_keys(keys: &[Key]) -> String {
    keys.iter().map(ToString::to_string).collect()
}

/// Why a key script could not be read. Positions are byte offsets into it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyParseError {
    /// Modifiers not followed by a key and `>`, such as `<C-Nope>`, `<C->` or `<C-r` at the end.
    /// `text` is the script from the `<` to where reading stopped.
    #[error("`{text}` at byte {at} is not a key: modifiers need a key name and `>`, as in `<C-r>`")]
    BadModifiedKey { at: usize, text: String },
    /// `<F0>`, `<F25>` and the like.
    #[error("`{text}` at byte {at} is not a key: function keys go from <F1> to <F{MAX_F}>")]
    FunctionKey { at: usize, text: String },
    /// A raw control character (tab, newline, escape...), which a script would hide.
    #[error(
        "control character U+{code:04X} at byte {at}: write it in <> notation, such as <Tab> \
         or <CR>",
        code = u32::from(*.c)
    )]
    ControlChar { at: usize, c: char },
}

/// Reads a script of keys in Vim notation.
pub fn parse_keys(s: &str) -> Result<Vec<Key>, KeyParseError> {
    let mut keys = Vec::new();
    let mut at = 0;
    while let Some(rest) = s.get(at..) {
        let Some(c) = rest.chars().next() else { break };
        if c == '<'
            && let Some((key, len)) = bracketed(rest, at)?
        {
            keys.push(key);
            at += len;
            continue;
        }
        if c.is_control() {
            return Err(KeyParseError::ControlChar { at, c });
        }
        keys.push(Key::char(c));
        at += c.len_utf8();
    }
    Ok(keys)
}

/// Reads the `<...>` key at the start of `s` (which starts with `<`, at byte `at` of the
/// script): the key and its length in bytes, or `None` when the `<` is a literal character.
fn bracketed(s: &str, at: usize) -> Result<Option<(Key, usize)>, KeyParseError> {
    let body = s.get(1..).unwrap_or_default();
    let bytes = body.as_bytes();
    let mut mods = Mods::NONE;
    let mut i = 0;
    while let (Some(&m), Some(b'-')) = (bytes.get(i), bytes.get(i + 1)) {
        match m.to_ascii_uppercase() {
            b'C' => mods.ctrl = true,
            b'M' | b'A' => mods.alt = true,
            b'S' => mods.shift = true,
            b'D' => mods.cmd = true,
            _ => break,
        }
        i += 2;
    }
    let modified = i > 0;
    // `i` follows ASCII bytes only, so it is a character boundary.
    let key_part = body.get(i..).unwrap_or_default();
    let mut chars = key_part.chars();
    if modified && let (Some(c), Some('>')) = (chars.next(), chars.next()) {
        if c.is_control() {
            return Err(KeyParseError::ControlChar { at: at + 1 + i, c });
        }
        let len = 1 + i + c.len_utf8() + 1;
        return Ok(Some((Key::new(KeyCode::Char(c), mods), len)));
    }
    let name_len = key_part
        .bytes()
        .take_while(u8::is_ascii_alphanumeric)
        .count();
    let name = key_part.get(..name_len).unwrap_or_default();
    let closed = key_part.as_bytes().get(name_len) == Some(&b'>');
    // The `<`, the modifiers, the name and the `>` if there is one.
    let len = 1 + i + name_len + usize::from(closed);
    let text = || s.get(..len).unwrap_or_default().to_owned();
    let code = if closed { key_code(name) } else { None };
    match code {
        Some(Ok(code)) => Ok(Some((Key::new(code, mods), len))),
        Some(Err(())) => Err(KeyParseError::FunctionKey { at, text: text() }),
        None if modified => {
            // Show the character that stopped the name too, when there is one.
            let shown = key_part.get(name_len..).and_then(|r| r.chars().next());
            let mut text = text();
            if let (false, Some(c)) = (closed, shown) {
                text.push(c);
            }
            Err(KeyParseError::BadModifiedKey { at, text })
        }
        None => Ok(None),
    }
}

/// The key named `name` (between `<` and `>`, without modifiers): `None` when it is not a
/// name, `Err` for a function key out of range such as `F25`.
fn key_code(name: &str) -> Option<Result<KeyCode, ()>> {
    if let Some((_, code)) = NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        return Some(Ok(*code));
    }
    let digits = name.strip_prefix(['F', 'f'])?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(match digits.parse::<u8>() {
        Ok(n @ 1..=MAX_F) => Ok(KeyCode::F(n)),
        _ => Err(()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_prints_back_as_itself_or_its_first_spelling() {
        for (name, code) in NAMES {
            let key = Key::named(*code);
            let printed = key.to_string();
            assert_eq!(parse_keys(&printed), Ok(vec![key]), "{name}");
            let first = name_of(*code).unwrap();
            assert_eq!(printed, format!("<{first}>"), "{name}");
        }
    }

    #[test]
    fn every_code_without_a_character_has_a_name() {
        // So `Display` never reaches its `{code:?}` fallback.
        for code in [
            KeyCode::Esc,
            KeyCode::Enter,
            KeyCode::Tab,
            KeyCode::Backspace,
            KeyCode::Delete,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Insert,
        ] {
            assert!(name_of(code).is_some(), "{code:?}");
        }
    }
}
