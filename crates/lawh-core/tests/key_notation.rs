//! Vim key notation: every key name and modifier, the canonical forms, literal `<`, errors, and
//! printing then reading back any keys.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use lawh_core::key::format_keys;
use lawh_core::{Key, KeyCode, KeyParseError, Mods, parse_keys};
use proptest::prelude::*;

/// Cases for the property tests: `LAWH_PROPTEST_CASES` if set, otherwise `default`.
fn cases(default: u32) -> u32 {
    std::env::var("LAWH_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// The one key `s` names.
fn one(s: &str) -> Key {
    match parse_keys(s) {
        Ok(keys) if keys.len() == 1 => keys[0],
        other => panic!("{s:?} is not one key: {other:?}"),
    }
}

/// `code` with the modifiers named in `mods`, as letters: `"CS"` is Ctrl and Shift. Not made
/// canonical.
fn key(code: KeyCode, mods: &str) -> Key {
    Key {
        code,
        mods: Mods {
            ctrl: mods.contains('C'),
            alt: mods.contains('M'),
            shift: mods.contains('S'),
            cmd: mods.contains('D'),
        },
    }
}

fn chars(s: &str) -> Vec<Key> {
    s.chars().map(Key::char).collect()
}

/// Every key name, written as the contract spells it, with the key it must give and how that
/// key prints. A copy-paste slip in the name table would bind the wrong key silently, so each
/// name is pinned here on its own line.
#[test]
fn every_key_name_reads_and_prints() {
    let table: &[(&str, KeyCode, &str)] = &[
        ("<Esc>", KeyCode::Esc, "<Esc>"),
        ("<CR>", KeyCode::Enter, "<CR>"),
        ("<Enter>", KeyCode::Enter, "<CR>"),
        ("<Return>", KeyCode::Enter, "<CR>"),
        ("<Tab>", KeyCode::Tab, "<Tab>"),
        ("<BS>", KeyCode::Backspace, "<BS>"),
        ("<Del>", KeyCode::Delete, "<Del>"),
        ("<Space>", KeyCode::Char(' '), "<Space>"),
        ("<lt>", KeyCode::Char('<'), "<lt>"),
        ("<Bar>", KeyCode::Char('|'), "<Bar>"),
        ("<Bslash>", KeyCode::Char('\\'), "<Bslash>"),
        ("<Up>", KeyCode::Up, "<Up>"),
        ("<Down>", KeyCode::Down, "<Down>"),
        ("<Left>", KeyCode::Left, "<Left>"),
        ("<Right>", KeyCode::Right, "<Right>"),
        ("<Home>", KeyCode::Home, "<Home>"),
        ("<End>", KeyCode::End, "<End>"),
        ("<PageUp>", KeyCode::PageUp, "<PageUp>"),
        ("<PageDown>", KeyCode::PageDown, "<PageDown>"),
        ("<Insert>", KeyCode::Insert, "<Insert>"),
        ("<F1>", KeyCode::F(1), "<F1>"),
        ("<F9>", KeyCode::F(9), "<F9>"),
        ("<F10>", KeyCode::F(10), "<F10>"),
        ("<F12>", KeyCode::F(12), "<F12>"),
        ("<F24>", KeyCode::F(24), "<F24>"),
    ];
    for (name, code, printed) in table {
        let expected = Key::named(*code);
        assert_eq!(one(name), expected, "{name}");
        assert_eq!(expected.to_string(), *printed, "{name}");
    }
    for n in 1..=24 {
        assert_eq!(one(&format!("<F{n}>")), Key::named(KeyCode::F(n)));
        assert_eq!(Key::named(KeyCode::F(n)).to_string(), format!("<F{n}>"));
    }
}

#[test]
fn every_modifier_prefix_reads_in_either_case() {
    // On a named key, where every modifier stays.
    let table: &[(&str, Key)] = &[
        ("<C-Tab>", key(KeyCode::Tab, "C")),
        ("<c-Tab>", key(KeyCode::Tab, "C")),
        ("<M-Tab>", key(KeyCode::Tab, "M")),
        ("<m-Tab>", key(KeyCode::Tab, "M")),
        ("<A-Tab>", key(KeyCode::Tab, "M")),
        ("<a-Tab>", key(KeyCode::Tab, "M")),
        ("<S-Tab>", key(KeyCode::Tab, "S")),
        ("<s-Tab>", key(KeyCode::Tab, "S")),
        ("<D-Tab>", key(KeyCode::Tab, "D")),
        ("<d-Tab>", key(KeyCode::Tab, "D")),
        ("<C-S-Tab>", key(KeyCode::Tab, "CS")),
        ("<S-C-Tab>", key(KeyCode::Tab, "CS")),
        ("<C-M-S-D-F5>", key(KeyCode::F(5), "CMSD")),
        ("<C-C-Tab>", key(KeyCode::Tab, "C")),
    ];
    for (name, expected) in table {
        assert_eq!(one(name), *expected, "{name}");
    }
    // Printed with C, M, S, D in that order, Alt as M.
    assert_eq!(one("<D-S-A-C-F5>").to_string(), "<C-M-S-D-F5>");
    assert_eq!(one("<A-x>").to_string(), "<M-x>");
    assert_eq!(one("<D-s>"), key(KeyCode::Char('s'), "D"));
}

#[test]
fn names_ignore_case() {
    for (a, b) in [
        ("<esc>", "<Esc>"),
        ("<ESC>", "<Esc>"),
        ("<eSc>", "<Esc>"),
        ("<cr>", "<CR>"),
        ("<enter>", "<CR>"),
        ("<RETURN>", "<CR>"),
        ("<bs>", "<BS>"),
        ("<pageup>", "<PageUp>"),
        ("<PAGEDOWN>", "<PageDown>"),
        ("<space>", "<Space>"),
        ("<LT>", "<lt>"),
        ("<bar>", "<Bar>"),
        ("<BSLASH>", "<Bslash>"),
        ("<f1>", "<F1>"),
        ("<c-s-tab>", "<C-S-Tab>"),
    ] {
        assert_eq!(one(a), one(b), "{a} {b}");
    }
}

#[test]
fn canonical_forms() {
    // Ctrl with a letter is the lowercase letter.
    assert_eq!(one("<C-R>"), one("<C-r>"));
    assert_eq!(one("<C-R>"), Key::ctrl('r'));
    assert_eq!(Key::ctrl('R'), Key::ctrl('r'));
    assert_eq!(one("<C-R>").to_string(), "<C-r>");
    // `<C-[>` is Esc.
    assert_eq!(one("<C-[>"), Key::named(KeyCode::Esc));
    assert_eq!(Key::ctrl('['), Key::named(KeyCode::Esc));
    assert_eq!(one("<M-C-[>"), key(KeyCode::Esc, "M"));
    // Shift with a character is the character: ASCII letters become uppercase.
    assert_eq!(one("<S-a>"), Key::char('A'));
    assert_eq!(one("<S-A>"), Key::char('A'));
    assert_eq!(one("<S-1>"), Key::char('1'));
    assert_eq!(one("<S-é>"), Key::char('é'));
    assert_eq!(one("<S-Space>"), Key::char(' '));
    assert_eq!(one("<S-lt>"), Key::char('<'));
    assert_eq!(one("<C-S-a>"), Key::ctrl('a'));
    assert_eq!(one("<M-S-a>"), key(KeyCode::Char('A'), "M"));
    // Other modifiers keep the case they are given.
    assert_eq!(one("<M-A>").to_string(), "<M-A>");
    assert_eq!(one("<D-A>").to_string(), "<D-A>");
    // Shift stays on keys without a character.
    assert_eq!(one("<S-Tab>").to_string(), "<S-Tab>");
    assert_eq!(one("<S-F1>").to_string(), "<S-F1>");
    // Ctrl folds ASCII letters only.
    assert_eq!(one("<C-É>").to_string(), "<C-É>");
    // `Key::new` gives the same keys as the parser, and canonical keys stay as they are.
    assert_eq!(
        Key::new(
            KeyCode::Char('a'),
            Mods {
                shift: true,
                ctrl: true,
                ..Mods::NONE
            }
        ),
        Key::ctrl('a')
    );
    assert_eq!(Key::ctrl('a').canonical(), Key::ctrl('a'));
    assert_eq!(key(KeyCode::Char('R'), "C").canonical(), Key::ctrl('r'));
}

#[test]
fn single_characters_with_modifiers() {
    for (name, c, printed) in [
        ("<C-->", '-', "<C-->"),
        ("<C->>", '>', "<C->>"),
        ("<C-<>", '<', "<C-lt>"),
        ("<C-lt>", '<', "<C-lt>"),
        ("<C- >", ' ', "<C-Space>"),
        ("<C-Space>", ' ', "<C-Space>"),
        ("<C-|>", '|', "<C-Bar>"),
        ("<C-\\>", '\\', "<C-Bslash>"),
        ("<C-]>", ']', "<C-]>"),
        ("<C-é>", 'é', "<C-é>"),
        ("<C-1>", '1', "<C-1>"),
    ] {
        let k = one(name);
        assert_eq!(k, Key::new(KeyCode::Char(c), Mods::CTRL), "{name}");
        assert_eq!(k.to_string(), printed, "{name}");
    }
    // A modifier letter followed by `>` is the key, not another modifier.
    assert_eq!(one("<M-S>"), key(KeyCode::Char('S'), "M"));
    assert_eq!(one("<C-S>"), Key::ctrl('s'));
}

#[test]
fn plain_characters_and_a_script() {
    assert_eq!(parse_keys(""), Ok(Vec::new()));
    assert_eq!(parse_keys("ab"), Ok(chars("ab")));
    assert_eq!(parse_keys("é😀 >-"), Ok(chars("é😀 >-")));
    let mut expected = chars("3l2j:ci 2");
    expected.push(Key::named(KeyCode::Enter));
    assert_eq!(parse_keys("3l2j:ci 2<CR>"), Ok(expected));
    assert_eq!(
        parse_keys("<Esc><C-r>u"),
        Ok(vec![
            Key::named(KeyCode::Esc),
            Key::ctrl('r'),
            Key::char('u')
        ])
    );
}

#[test]
fn a_lt_that_starts_no_name_is_literal() {
    for literal in [
        "<", "a<b", "<>", "<Nope>", "<a>", "<<", "<F>", "<F1x>", "<Esc", "< Esc>", "<é>",
    ] {
        assert_eq!(parse_keys(literal), Ok(chars(literal)), "{literal}");
    }
    // Reading goes on after a literal `<`.
    let mut expected = chars("<");
    expected.push(Key::named(KeyCode::Esc));
    assert_eq!(parse_keys("<<Esc>"), Ok(expected));
    let mut expected = chars("a<");
    expected.push(Key::named(KeyCode::Tab));
    expected.extend(chars(">"));
    assert_eq!(parse_keys("a<<Tab>>"), Ok(expected));
}

#[test]
fn mistakes_are_errors() {
    let bad_modified = |at: usize, text: &str| {
        Err(KeyParseError::BadModifiedKey {
            at,
            text: text.to_owned(),
        })
    };
    assert_eq!(parse_keys("<C-Nope>"), bad_modified(0, "<C-Nope>"));
    assert_eq!(parse_keys("ab<C->"), bad_modified(2, "<C->"));
    assert_eq!(parse_keys("<C-"), bad_modified(0, "<C-"));
    assert_eq!(parse_keys("<C-r"), bad_modified(0, "<C-r"));
    assert_eq!(parse_keys("<C-ab"), bad_modified(0, "<C-ab"));
    assert_eq!(parse_keys("<C-a b>"), bad_modified(0, "<C-a "));
    assert_eq!(parse_keys("<S-->x<C-S->"), bad_modified(6, "<C-S->"));
    let function = |at: usize, text: &str| {
        Err(KeyParseError::FunctionKey {
            at,
            text: text.to_owned(),
        })
    };
    assert_eq!(parse_keys("<F0>"), function(0, "<F0>"));
    assert_eq!(parse_keys("x<F25>"), function(1, "<F25>"));
    assert_eq!(parse_keys("<f300>"), function(0, "<f300>"));
    assert_eq!(parse_keys("<C-F25>"), function(0, "<C-F25>"));
    assert_eq!(
        parse_keys("a\tb"),
        Err(KeyParseError::ControlChar { at: 1, c: '\t' })
    );
    assert_eq!(
        parse_keys("é\n"),
        Err(KeyParseError::ControlChar { at: 2, c: '\n' })
    );
    assert_eq!(
        parse_keys("<C-\u{1b}>"),
        Err(KeyParseError::ControlChar { at: 3, c: '\u{1b}' })
    );
    assert_eq!(
        parse_keys("\u{85}"),
        Err(KeyParseError::ControlChar { at: 0, c: '\u{85}' })
    );

    // The messages show what was typed.
    let message = |s: &str| parse_keys(s).unwrap_err().to_string();
    assert_eq!(
        message("<C-Nope>"),
        "`<C-Nope>` at byte 0 is not a key: modifiers need a key name and `>`, as in `<C-r>`"
    );
    assert_eq!(
        message("<F25>"),
        "`<F25>` at byte 0 is not a key: function keys go from <F1> to <F24>"
    );
    assert_eq!(
        message("a\t"),
        "control character U+0009 at byte 1: write it in <> notation, such as <Tab> or <CR>"
    );
}

/// Any canonical key: characters (special ones often), named keys and function keys, with any
/// modifiers.
fn any_key() -> impl Strategy<Value = Key> {
    let code = prop_oneof![
        3 => any::<char>().prop_filter("printable", |c| !c.is_control()).prop_map(KeyCode::Char),
        2 => prop::sample::select(vec![
            '<', '>', '-', ' ', '|', '\\', '[', ']', 'C', 'M', 'A', 'S', 'D', 'F', 'c', 's', 'é',
        ])
        .prop_map(KeyCode::Char),
        2 => prop::sample::select(vec![
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
        ]),
        1 => (1_u8..=24).prop_map(KeyCode::F),
    ];
    let mods = (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()).prop_map(
        |(ctrl, alt, shift, cmd)| Mods {
            ctrl,
            alt,
            shift,
            cmd,
        },
    );
    (code, mods).prop_map(|(code, mods)| Key::new(code, mods))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(1024)))]

    #[test]
    fn printed_keys_read_back_the_same(keys in prop::collection::vec(any_key(), 0..12)) {
        for k in &keys {
            prop_assert_eq!(k.canonical(), *k);
        }
        let printed = format_keys(&keys);
        prop_assert_eq!(parse_keys(&printed), Ok(keys), "{}", printed);
    }

    /// Whatever a script holds, reading it never panics, and keys it reads print as a script
    /// that reads back the same.
    #[test]
    fn any_script_reads_or_fails_cleanly(
        script in prop::collection::vec(
            prop_oneof![
                any::<char>(),
                prop::sample::select(vec!['<', '>', '-', 'C', 'S', 'M', 'F', '1', '2', 'a', 'E']),
            ],
            0..24,
        )
        .prop_map(String::from_iter)
    ) {
        if let Ok(keys) = parse_keys(&script) {
            prop_assert_eq!(parse_keys(&format_keys(&keys)), Ok(keys));
        }
    }
}
