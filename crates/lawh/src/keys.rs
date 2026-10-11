//! Keyboard events from winit as engine keys ([`lawh_core::Key`], Vim notation).
//!
//! The engine reads typed characters, so the app takes winit's logical key: the character the
//! keyboard layout makes, with Shift, `AltGr` and Option already applied. That keeps Vim keys on
//! the typed character on every layout (`:` is `:` whether it is Shift+`;` or a key of its
//! own), as `CLAUDE.md` asks. Only Ctrl and Cmd make chords:
//!
//! - Linux never reports `AltGr` as Alt, Windows reports `AltGr` as Ctrl+Alt, and macOS Option
//!   types characters. So Alt with a character, and Ctrl+Alt with one, are the typed character,
//!   never a chord: otherwise `{` on a German keyboard (`AltGr`+7) would be `<C-M-{>`.
//! - Ctrl without Alt and a character is `<C-x>` (a letter lowercased, `Ctrl+[` is `<Esc>`).
//! - Cmd (the Super or Windows key elsewhere) and a character is `<D-x>`.
//!
//! The one key the app takes before the engine is Cmd+Q on macOS ([`is_close_request`]); every
//! other chord goes to the engine, Super+Q elsewhere included. When app shortcuts come, they go through one modifier setting
//! (Ctrl, or Cmd on macOS) as `CLAUDE.md` says, and are taken before the engine sees the key.
//!
//! The caller drops key releases and synthetic presses (the keys winit reports as held when the
//! window gains focus); repeats are kept, so a held `j` keeps moving.

use lawh_core::{Key, KeyCode, Mods};
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey};

/// What one key press is for the engine.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Translated {
    /// Keys to feed, in order: none for a key the engine has no use for (a modifier alone), and
    /// several when a layout types several characters with one key.
    pub keys: Vec<Key>,
    /// A dead key was taken as its own character: the caller calls `Window::reset_dead_keys`,
    /// so the next key is not composed with it.
    pub reset_dead_keys: bool,
}

/// The engine keys for a key press with logical key `logical` while `mods` are held.
///
/// `text_mode` is true when the engine takes text (Insert mode and the command line): a dead
/// key is then left to compose with the next key (`^` then `e` types `ê`). Elsewhere (the
/// Normal-like modes) a dead key is a key of its own, as in Vim: `^` moves at once, and the
/// caller resets the dead key so it doesn't change the next character.
pub fn translate(logical: &WinitKey, mods: ModifiersState, text_mode: bool) -> Translated {
    match logical {
        WinitKey::Named(NamedKey::Space) => Translated {
            keys: typed(' ', mods).into_iter().collect(),
            reset_dead_keys: false,
        },
        WinitKey::Named(named) => Translated {
            keys: named_code(*named)
                .map(|code| Key::new(code, named_mods(mods)))
                .into_iter()
                .collect(),
            reset_dead_keys: false,
        },
        WinitKey::Character(text) => Translated {
            keys: text.chars().filter_map(|c| typed(c, mods)).collect(),
            reset_dead_keys: false,
        },
        WinitKey::Dead(Some(c)) if !text_mode => {
            let keys: Vec<Key> = typed(*c, mods).into_iter().collect();
            Translated {
                reset_dead_keys: !keys.is_empty(),
                keys,
            }
        }
        WinitKey::Dead(_) | WinitKey::Unidentified(_) => Translated::default(),
    }
}

/// True for Cmd+Q alone (`<D-q>`): on macOS the app takes it as a close request, as the
/// window's close button, which warns once about unwritten changes. macOS's default menu would
/// quit on it without any warning; lawh has no such menu (see `main`), so the key arrives here.
/// Cmd+Shift+Q (`<D-Q>`, log out on macOS) and other chords are not it. The caller asks only on
/// macOS: elsewhere the key is Super+Q, which is the desktop's, not a quit key.
pub fn is_close_request(keys: &[Key]) -> bool {
    let cmd_q = Key::new(
        KeyCode::Char('q'),
        Mods {
            cmd: true,
            ..Mods::NONE
        },
    );
    matches!(keys, [key] if *key == cmd_q)
}

/// The key for typed character `c` with `mods` held: a Ctrl or Cmd chord, or the character
/// itself. `None` for a control character, which is never typed text.
fn typed(c: char, mods: ModifiersState) -> Option<Key> {
    if c.is_control() {
        return None;
    }
    let chord = Mods {
        ctrl: mods.control_key() && !mods.alt_key(),
        cmd: mods.super_key(),
        ..Mods::NONE
    };
    // `Key::new` lowercases a letter with Ctrl and makes `Ctrl+[` `<Esc>`.
    Some(Key::new(KeyCode::Char(c), chord))
}

/// Modifiers on a key without a character (`<S-Tab>`, `<C-Left>`): all of them, as Vim keeps
/// them.
fn named_mods(mods: ModifiersState) -> Mods {
    Mods {
        ctrl: mods.control_key(),
        alt: mods.alt_key(),
        shift: mods.shift_key(),
        cmd: mods.super_key(),
    }
}

/// The engine's key for a named key, if it has one. Space is a character (see [`translate`]).
fn named_code(named: NamedKey) -> Option<KeyCode> {
    let code = match named {
        NamedKey::Enter => KeyCode::Enter,
        NamedKey::Escape => KeyCode::Esc,
        NamedKey::Backspace => KeyCode::Backspace,
        NamedKey::Tab => KeyCode::Tab,
        NamedKey::Delete => KeyCode::Delete,
        NamedKey::ArrowUp => KeyCode::Up,
        NamedKey::ArrowDown => KeyCode::Down,
        NamedKey::ArrowLeft => KeyCode::Left,
        NamedKey::ArrowRight => KeyCode::Right,
        NamedKey::Home => KeyCode::Home,
        NamedKey::End => KeyCode::End,
        NamedKey::PageUp => KeyCode::PageUp,
        NamedKey::PageDown => KeyCode::PageDown,
        NamedKey::Insert => KeyCode::Insert,
        NamedKey::F1 => KeyCode::F(1),
        NamedKey::F2 => KeyCode::F(2),
        NamedKey::F3 => KeyCode::F(3),
        NamedKey::F4 => KeyCode::F(4),
        NamedKey::F5 => KeyCode::F(5),
        NamedKey::F6 => KeyCode::F(6),
        NamedKey::F7 => KeyCode::F(7),
        NamedKey::F8 => KeyCode::F(8),
        NamedKey::F9 => KeyCode::F(9),
        NamedKey::F10 => KeyCode::F(10),
        NamedKey::F11 => KeyCode::F(11),
        NamedKey::F12 => KeyCode::F(12),
        NamedKey::F13 => KeyCode::F(13),
        NamedKey::F14 => KeyCode::F(14),
        NamedKey::F15 => KeyCode::F(15),
        NamedKey::F16 => KeyCode::F(16),
        NamedKey::F17 => KeyCode::F(17),
        NamedKey::F18 => KeyCode::F(18),
        NamedKey::F19 => KeyCode::F(19),
        NamedKey::F20 => KeyCode::F(20),
        NamedKey::F21 => KeyCode::F(21),
        NamedKey::F22 => KeyCode::F(22),
        NamedKey::F23 => KeyCode::F(23),
        NamedKey::F24 => KeyCode::F(24),
        _ => return None,
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use lawh_core::{key::format_keys, parse_keys};
    use winit::keyboard::{NativeKey, SmolStr};

    use super::*;

    const NONE: ModifiersState = ModifiersState::empty();
    const SHIFT: ModifiersState = ModifiersState::SHIFT;
    const CTRL: ModifiersState = ModifiersState::CONTROL;
    const ALT: ModifiersState = ModifiersState::ALT;
    const SUPER: ModifiersState = ModifiersState::SUPER;

    fn ch(s: &str) -> WinitKey {
        WinitKey::Character(SmolStr::new(s))
    }

    fn named(key: NamedKey) -> WinitKey {
        WinitKey::Named(key)
    }

    /// The keys for one press, in Vim notation.
    fn vim(logical: &WinitKey, mods: ModifiersState, text_mode: bool) -> String {
        format_keys(&translate(logical, mods, text_mode).keys)
    }

    /// One row per arm of `translate`, in both kinds of mode unless the mode matters.
    #[test]
    fn every_kind_of_key() {
        let rows: &[(WinitKey, ModifiersState, &str)] = &[
            // Named keys, Space as a character.
            (named(NamedKey::Space), NONE, "<Space>"),
            (named(NamedKey::Space), SHIFT, "<Space>"),
            (named(NamedKey::Space), ALT, "<Space>"),
            (named(NamedKey::Space), CTRL, "<C-Space>"),
            (named(NamedKey::Enter), NONE, "<CR>"),
            (named(NamedKey::Escape), NONE, "<Esc>"),
            (named(NamedKey::Backspace), NONE, "<BS>"),
            (named(NamedKey::Tab), NONE, "<Tab>"),
            (named(NamedKey::Tab), SHIFT, "<S-Tab>"),
            (named(NamedKey::Delete), NONE, "<Del>"),
            (named(NamedKey::ArrowUp), NONE, "<Up>"),
            (named(NamedKey::ArrowDown), NONE, "<Down>"),
            (named(NamedKey::ArrowLeft), CTRL, "<C-Left>"),
            (named(NamedKey::ArrowRight), ALT, "<M-Right>"),
            (named(NamedKey::Home), NONE, "<Home>"),
            (named(NamedKey::End), NONE, "<End>"),
            (named(NamedKey::PageUp), NONE, "<PageUp>"),
            (named(NamedKey::PageDown), NONE, "<PageDown>"),
            (named(NamedKey::Insert), NONE, "<Insert>"),
            (named(NamedKey::F1), NONE, "<F1>"),
            (named(NamedKey::F12), SUPER, "<D-F12>"),
            (named(NamedKey::F24), NONE, "<F24>"),
            // Named keys the engine has no use for.
            (named(NamedKey::F25), NONE, ""),
            (named(NamedKey::Shift), SHIFT, ""),
            (named(NamedKey::Control), CTRL, ""),
            (named(NamedKey::Alt), ALT, ""),
            (named(NamedKey::Super), SUPER, ""),
            (named(NamedKey::CapsLock), NONE, ""),
            (named(NamedKey::AudioVolumeUp), NONE, ""),
            // Characters.
            (ch("a"), NONE, "a"),
            (ch("A"), SHIFT, "A"),
            (ch("<"), SHIFT, "<lt>"),
            (ch("é"), NONE, "é"),
            (ch("ab"), NONE, "ab"),
            (ch("r"), CTRL, "<C-r>"),
            (ch("R"), CTRL | SHIFT, "<C-r>"),
            (ch("["), CTRL, "<Esc>"),
            (ch("6"), CTRL, "<C-6>"),
            (ch("r"), CTRL | ALT, "r"),
            (ch("r"), ALT, "r"),
            (ch("s"), SUPER, "<D-s>"),
            (ch("S"), SUPER | SHIFT, "<D-S>"),
            (ch("s"), SUPER | CTRL, "<C-D-s>"),
            // Control characters are never typed text.
            (ch("\u{12}"), CTRL, ""),
            (ch("\r"), NONE, ""),
            (ch("a\u{7f}b"), NONE, "ab"),
            // Neither are keys the platform can't name.
            (WinitKey::Unidentified(NativeKey::Unidentified), NONE, ""),
            (WinitKey::Dead(None), NONE, ""),
        ];
        for (logical, mods, want) in rows {
            for text_mode in [false, true] {
                let got = translate(logical, *mods, text_mode);
                assert_eq!(
                    format_keys(&got.keys),
                    *want,
                    "{logical:?} {mods:?} text_mode={text_mode}"
                );
                assert!(!got.reset_dead_keys, "{logical:?}");
                // What the app feeds is what Vim notation reads back.
                assert_eq!(parse_keys(want).unwrap(), got.keys, "{want}");
            }
        }
    }

    #[test]
    fn cmd_q_alone_is_a_close_request() {
        for text_mode in [false, true] {
            let keys = |logical: &WinitKey, mods| translate(logical, mods, text_mode).keys;
            assert!(is_close_request(&keys(&ch("q"), SUPER)));
            for (logical, mods) in [
                (ch("q"), NONE),
                (ch("q"), CTRL),
                (ch("Q"), SUPER | SHIFT),
                (ch("q"), SUPER | CTRL),
                (ch("w"), SUPER),
                (ch("qq"), SUPER),
            ] {
                assert!(
                    !is_close_request(&keys(&logical, mods)),
                    "{logical:?} {mods:?}"
                );
            }
        }
        assert!(is_close_request(&parse_keys("<D-q>").unwrap()));
    }

    #[test]
    fn a_dead_key_is_a_key_on_the_board_and_composes_in_text() {
        let circumflex = WinitKey::Dead(Some('^'));
        assert_eq!(
            translate(&circumflex, NONE, false),
            Translated {
                keys: vec![Key::char('^')],
                reset_dead_keys: true,
            }
        );
        assert_eq!(translate(&circumflex, NONE, true), Translated::default());
        // Ctrl makes a chord of it, as with any character.
        assert_eq!(vim(&WinitKey::Dead(Some('^')), CTRL, false), "<C-^>");
    }

    /// A press as a layout reports it: the logical key and the modifiers held.
    struct Press {
        logical: WinitKey,
        mods: ModifiersState,
        /// The engine keys in Normal mode, and in Insert mode or on the command line.
        normal: &'static str,
        text: &'static str,
    }

    fn press(logical: WinitKey, mods: ModifiersState, both: &'static str) -> Press {
        Press {
            logical,
            mods,
            normal: both,
            text: both,
        }
    }

    fn check(layout: &str, presses: &[Press]) {
        for p in presses {
            assert_eq!(
                vim(&p.logical, p.mods, false),
                p.normal,
                "{layout}: {:?} {:?} in Normal mode",
                p.logical,
                p.mods
            );
            assert_eq!(
                vim(&p.logical, p.mods, true),
                p.text,
                "{layout}: {:?} {:?} in Insert mode",
                p.logical,
                p.mods
            );
        }
    }

    /// What winit reports on common layouts (winit 0.30: the logical key carries Shift and
    /// `AltGr`; Linux reports `AltGr` as no modifier, Windows as Ctrl+Alt).
    #[test]
    fn layouts_type_their_characters() {
        check(
            "US",
            &[
                press(ch("h"), NONE, "h"),
                press(ch("J"), SHIFT, "J"),
                press(ch(":"), SHIFT, ":"),
                press(ch("$"), SHIFT, "$"),
                press(ch("0"), NONE, "0"),
                press(ch("r"), CTRL, "<C-r>"),
                press(ch("R"), CTRL | SHIFT, "<C-r>"),
                press(ch("r"), CTRL | ALT, "r"),
                press(ch("s"), SUPER, "<D-s>"),
            ],
        );
        // German: AltGr+7 8 Q + ß < (Linux reports no modifier, Windows Ctrl+Alt).
        for altgr in [NONE, CTRL | ALT] {
            check(
                "German",
                &[
                    press(ch("{"), altgr, "{"),
                    press(ch("["), altgr, "["),
                    press(ch("@"), altgr, "@"),
                    press(ch("~"), altgr, "~"),
                    press(ch("|"), altgr, "<Bar>"),
                    press(ch("\\"), altgr, "<Bslash>"),
                    press(ch("ö"), NONE, "ö"),
                    press(ch("z"), NONE, "z"),
                ],
            );
        }
        // French AZERTY: the digit row types symbols, Shift types the digits.
        check(
            "French AZERTY",
            &[
                press(ch("&"), NONE, "&"),
                press(ch("é"), NONE, "é"),
                press(ch("1"), SHIFT, "1"),
                press(ch("3"), SHIFT, "3"),
                press(ch("0"), SHIFT, "0"),
                press(ch("a"), NONE, "a"),
                press(ch("q"), CTRL, "<C-q>"),
            ],
        );
        // US-International: ^ " ' are dead keys; in text they compose (the next key reports
        // the composed character), on the board they are keys of their own.
        check(
            "US-International",
            &[
                Press {
                    logical: WinitKey::Dead(Some('^')),
                    mods: SHIFT,
                    normal: "^",
                    text: "",
                },
                Press {
                    logical: WinitKey::Dead(Some('"')),
                    mods: SHIFT,
                    normal: "\"",
                    text: "",
                },
                Press {
                    logical: WinitKey::Dead(Some('\'')),
                    mods: NONE,
                    normal: "'",
                    text: "",
                },
                press(ch("ê"), NONE, "ê"),
                press(ch("ë"), SHIFT, "ë"),
                press(ch("^"), NONE, "^"),
            ],
        );
        // Dvorak: the keys are where the layout puts them; only the character counts.
        check(
            "Dvorak",
            &[
                press(ch("h"), NONE, "h"),
                press(ch("t"), NONE, "t"),
                press(ch("n"), NONE, "n"),
                press(ch("s"), NONE, "s"),
                press(ch(":"), SHIFT, ":"),
                press(ch("r"), CTRL, "<C-r>"),
            ],
        );
    }
}
