//! Typst's symbol names, for `:sym`.
//!
//! The list comes from `codex`, the crate Typst itself takes `sym` from, pinned to the version
//! Typst 0.15.1 uses, so every name in it is one the worker compiles. It is read from codex's
//! static tables and checked against the bundled fonts' character maps in this process: no
//! Typst runs to make it, and nothing in it comes from a board.

use std::cmp::Ordering;

use codex::{Def, Module};
use typst::text::is_default_ignorable;

use crate::fonts::{MATH_INDEX, TEXT_INDEX, face};

/// A symbol Typst math knows by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolInfo {
    /// Its full name in Typst math, modifiers included: `alpha`, `arrow.r.long`,
    /// `gender.female`.
    pub name: String,
    /// What it stands for: one character, or a few (a character and a variation selector, say).
    pub value: String,
    /// The bundled fonts draw it: Typst lays it out without a missing-glyph box (see
    /// [`symbols`]).
    pub drawable: bool,
}

/// Every symbol Typst math knows by name, in the order lawh-core's `:sym` keeps a list in.
///
/// One entry for each variant of each symbol in codex's `sym` module, the module Typst math
/// puts its names in, and in its sub-modules (`gender`, `control`), which Typst math exposes
/// under their own names (`gender.female`). Deprecated symbols and variants are left out: Typst
/// still compiles them but warns, and each has a name that replaces it.
///
/// The order is a dictionary's, as `name_order` in lawh-core's `engine/sym.rs` sorts: ignoring
/// the case of ASCII letters, then a small letter before its capital (`alpha`, `Alpha`, `beta`).
/// No name is in it twice. The engine keeps such a list without copying it.
///
/// [`SymbolInfo::drawable`] says whether the bundled fonts have a glyph for the value, in the
/// order Typst tries them for math: Libertinus Math, the font math is set in, then Inter, the
/// one font left for Typst's fallback. Shaping one symbol, Typst takes the first of them with a
/// glyph for every character, leaving out the characters it drops while shaping (Unicode's
/// default ignorables: variation selectors, joiners, marks of direction); with neither, it draws
/// Libertinus Math's missing-glyph box for each character. So a space counts as drawable (it
/// is drawn as a space, see `draws`), and so does a value that is all default ignorables (`zws`,
/// `zwj`, `wj`): Typst draws nothing for it, and no box.
///
/// With codex 0.3.0 that is 1,189 names: 1,142 in `sym` itself and 47 in its sub-modules. The
/// fonts draw 732 (7 of them as nothing); 457 would draw the box. The tests in
/// `tests/symbols.rs` compile every name and check its flag against the glyphs Typst lays out.
///
/// Built on each call, from codex's static tables and the bundled fonts' character maps (parsed
/// once per process, shared with the rest of lawh-math): the app calls it once, at start, and
/// moves what it keeps into the engine's list. Measured on Linux `x86_64` (2026-10-12,
/// `measure_symbol_list` in `tests/symbols.rs`): 0.7 ms a call in release (1.0 ms the first,
/// which parses the fonts), 3.7 ms in debug (4.5 to 6.3 ms the first). The list holds 83 KB:
/// 1,189 entries of 56 bytes, 12.8 KB of names and 3.6 KB of values.
pub fn symbols() -> Vec<SymbolInfo> {
    let mut list = Vec::new();
    // Modules still to read, with the prefix their names take.
    let mut modules: Vec<(String, Module)> = vec![(String::new(), codex::SYM)];
    while let Some((prefix, module)) = modules.pop() {
        for (name, binding) in module.iter() {
            if binding.deprecation.is_some() {
                continue;
            }
            let full = format!("{prefix}{name}");
            match binding.def {
                Def::Module(inner) => modules.push((format!("{full}."), inner)),
                Def::Symbol(symbol) => {
                    for (modifiers, value, deprecation) in symbol.variants() {
                        if deprecation.is_some() {
                            continue;
                        }
                        let name = if modifiers.is_empty() {
                            full.clone()
                        } else {
                            format!("{full}.{}", modifiers.as_str())
                        };
                        list.push(SymbolInfo {
                            name,
                            value: value.to_owned(),
                            drawable: drawable(value),
                        });
                    }
                }
            }
        }
    }
    list.sort_by(|a, b| name_order(&a.name, &b.name));
    list.dedup_by(|a, b| a.name == b.name);
    list.shrink_to_fit();
    list
}

/// Typst draws `value` with no missing-glyph box: one of the bundled fonts, tried in Typst's
/// order for math, has a glyph for each of its characters other than default ignorables (see
/// [`symbols`]).
fn drawable(value: &str) -> bool {
    [MATH_INDEX, TEXT_INDEX].into_iter().any(|index| {
        face(index).is_some_and(|face| {
            value
                .chars()
                .filter(|&c| !is_default_ignorable(c))
                .all(|c| draws(face, c))
        })
    })
}

/// Typst's shaper (rustybuzz) draws `c` in `face` without a missing-glyph box: the font maps
/// it to a glyph, or `c` is a space the shaper draws as the font's U+0020 at its own width (as
/// Libertinus Math draws U+205F, the medium mathematical space, which it has no glyph for).
///
/// Not followed here: the shaper also draws a character the font has no glyph for from the
/// characters it decomposes into, when the font has those. No value in codex 0.3.0 needs that,
/// and the layout test in `tests/symbols.rs` would fail on one that did.
fn draws(face: &ttf_parser::Face<'_>, c: char) -> bool {
    let mapped = |c: char| face.glyph_index(c).is_some_and(|id| id.0 != 0);
    mapped(c) || (is_fallback_space(c) && mapped(' '))
}

/// The spaces rustybuzz draws as U+0020 when a font has no glyph for them: every space
/// separator but the Ogham space mark, which is visible (`space_fallback` in its
/// `hb/unicode.rs`).
fn is_fallback_space(c: char) -> bool {
    matches!(
        c,
        '\u{A0}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

/// Dictionary order, as lawh-core's `:sym` keeps its list (`name_order` in its
/// `engine/sym.rs`): the names compared with ASCII letters in small letters, then with the case
/// of ASCII letters swapped, so a small letter comes before its capital. Only equal names
/// compare equal.
fn name_order(a: &str, b: &str) -> Ordering {
    let swapped = |c: u8| {
        if c.is_ascii_alphabetic() { c ^ 0x20 } else { c }
    };
    a.bytes()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
        .then_with(|| a.bytes().map(swapped).cmp(b.bytes().map(swapped)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_order_is_a_dictionary_order() {
        let sorted = [
            "alpha",
            "Alpha",
            "arrow.r",
            "arrow.r.long",
            "beta",
            "pi",
            "Pi",
            "pi.alt",
        ];
        for pair in sorted.windows(2) {
            assert_eq!(name_order(pair[0], pair[1]), Ordering::Less, "{pair:?}");
            assert_eq!(name_order(pair[1], pair[0]), Ordering::Greater, "{pair:?}");
        }
        assert_eq!(name_order("Pi", "Pi"), Ordering::Equal);
    }

    #[test]
    fn spaces_the_math_font_lacks_are_drawn_as_its_space() {
        let math = face(MATH_INDEX).unwrap();
        assert_eq!(math.glyph_index('\u{205F}'), None);
        assert!(draws(math, '\u{205F}'));
        assert!(
            !is_fallback_space('\u{1680}'),
            "the Ogham space mark is visible"
        );
        assert!(
            !is_fallback_space(' '),
            "U+0020 is the glyph the others fall back to"
        );
    }

    #[test]
    fn drawable_skips_default_ignorables_and_needs_one_font_for_the_rest() {
        assert!(drawable("α"));
        assert!(drawable("\u{200B}"), "zero width space: drawn as nothing");
        assert!(drawable("∅\u{FE00}"), "with a variation selector");
        assert!(!drawable("\u{2400}"), "symbol for null: in neither font");
        assert!(!drawable("α\u{2400}"));
    }
}
