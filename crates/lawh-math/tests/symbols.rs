//! The symbol list for `:sym`: Typst's names from codex, and whether the bundled fonts draw them.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use std::cmp::Ordering;
use std::hint::black_box;
use std::time::{Duration, Instant};

use lawh_math::{MathEngine, MathItem, SymbolInfo, symbols};
use typst::text::is_default_ignorable;

/// The sub-modules of codex's `sym`, whose names Typst math reads as `gender.female`.
const SUB_MODULES: [&str; 2] = ["gender.", "control."];

/// Longest name lawh-core's `:sym` keeps (`MAX_SYM_QUERY_CHARS` in its `engine/sym.rs`).
const ENGINE_MAX_NAME_CHARS: usize = 64;

/// lawh-core's order for its symbol list (`name_order` in its `engine/sym.rs`), copied here:
/// lawh-math does not depend on lawh-core.
fn engine_order(a: &str, b: &str) -> Ordering {
    let swapped = |c: u8| {
        if c.is_ascii_alphabetic() { c ^ 0x20 } else { c }
    };
    a.bytes()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
        .then_with(|| a.bytes().map(swapped).cmp(b.bytes().map(swapped)))
}

/// A value that is all default ignorables, which Typst drops while shaping: it draws nothing.
fn invisible(symbol: &SymbolInfo) -> bool {
    symbol.value.chars().all(is_default_ignorable)
}

fn find<'a>(list: &'a [SymbolInfo], name: &str) -> Option<&'a SymbolInfo> {
    list.iter().find(|s| s.name == name)
}

/// The counts for codex 0.3.0 (Typst 0.15.1) and the bundled Libertinus Math and Inter, so a
/// bump of either is noticed: 1,189 names not deprecated, 1,142 in `sym` itself (1,159 with
/// the deprecated ones) and 47 in its sub-modules (12 in `gender`, 35 in `control`). The fonts
/// draw 732 of them: 725 with glyphs and 7 that draw nothing (`zws`, `zwj`, `zwnj`, `wj`,
/// `lrm`, `rlm`, `hyph.soft`). 457 draw Libertinus Math's missing-glyph box (419 in `sym`
/// itself, 38 in the sub-modules).
#[test]
fn counts_are_codex_0_3_0_and_the_bundled_fonts() {
    let list = symbols();
    let in_sub_modules = |s: &&SymbolInfo| SUB_MODULES.iter().any(|m| s.name.starts_with(m));
    let drawable = list.iter().filter(|s| s.drawable).count();
    let drawn_as_nothing = list.iter().filter(|s| invisible(s)).count();
    let missing = list.iter().filter(|s| !s.drawable).count();
    let sub = list.iter().filter(in_sub_modules).count();
    let sub_missing = list
        .iter()
        .filter(in_sub_modules)
        .filter(|s| !s.drawable)
        .count();
    assert_eq!(list.len(), 1_189);
    assert_eq!(sub, 47);
    // `sym` itself, deprecated variants and symbols included, as codex lists it: 17 are left out.
    let with_deprecated: usize = codex::SYM
        .iter()
        .map(|(_, binding)| match binding.def {
            codex::Def::Symbol(symbol) => symbol.variants().count(),
            codex::Def::Module(_) => 0,
        })
        .sum();
    assert_eq!(with_deprecated, 1_159);
    assert_eq!(drawable, 732);
    assert_eq!(drawn_as_nothing, 7);
    assert!(list.iter().filter(|s| invisible(s)).all(|s| s.drawable));
    assert_eq!(missing, 457);
    assert_eq!(sub_missing, 38);
}

#[test]
fn names_are_unique_in_the_engines_order_and_usable_by_it() {
    let list = symbols();
    for pair in list.windows(2) {
        assert_eq!(
            engine_order(&pair[0].name, &pair[1].name),
            Ordering::Less,
            "{} before {}",
            pair[0].name,
            pair[1].name
        );
    }
    // What lawh-core's `set_symbols` keeps (`usable` in its `engine/sym.rs`).
    for s in &list {
        let chars = s.name.chars().count();
        assert!((1..=ENGINE_MAX_NAME_CHARS).contains(&chars), "{}", s.name);
        assert!(
            !s.name.chars().any(|c| c.is_whitespace() || c.is_control()),
            "{}",
            s.name
        );
        assert!(!s.value.is_empty(), "{}", s.name);
        assert!(!s.value.chars().any(char::is_control), "{}", s.name);
    }
    let longest = list.iter().map(|s| s.name.len()).max();
    assert_eq!(longest, Some(28), "arrow.l.twohead.tail.dstruck");
    let at = |name: &str| list.iter().position(|s| s.name == name).unwrap();
    assert_eq!(at("alpha") + 1, at("Alpha"));
    assert!(at("Alpha") < at("beta"));
    assert_eq!(at("pi") + 1, at("Pi"));
    assert!(at("arrow.r") < at("arrow.r.long"));
}

#[test]
fn names_carry_their_modifiers_and_values() {
    let list = symbols();
    let value = |name: &str| find(&list, name).map(|s| s.value.as_str());
    assert_eq!(value("alpha"), Some("α"));
    assert_eq!(value("Alpha"), Some("Α"));
    assert_eq!(value("arrow.r.long"), Some("⟶"));
    assert_eq!(value("space.thin"), Some("\u{2009}"));
    assert_eq!(value("emptyset.zero"), Some("∅\u{FE00}"));
    assert_eq!(value("gender.female"), Some("♀\u{FE0E}"));
    assert_eq!(value("control.nul"), Some("␀"));
    // Deprecated: a whole symbol, and variants that have new names.
    assert_eq!(value("join"), None);
    assert_eq!(value("gt.tri"), None);
    assert_eq!(value("gt.closed"), Some("⊳"));
    // Modules are not symbols.
    assert_eq!(value("gender"), None);
    assert_eq!(value("control"), None);

    let drawable = |name: &str| find(&list, name).map(|s| s.drawable);
    assert_eq!(drawable("alpha"), Some(true));
    // Libertinus Math has no glyph for it but draws its space; Inter has one.
    assert_eq!(drawable("space.med"), Some(true));
    assert_eq!(drawable("zws"), Some(true));
    assert_eq!(drawable("control.nul"), Some(false));
}

/// Every name compiles as a formula of its own, as `:sym` puts it in a math block, through the
/// in-process engine (trusted input: the names are codex's). Its layout has a missing-glyph
/// glyph (id 0) exactly when the list says the fonts can't draw it, and draws nothing at all for
/// the invisible ones. 0.6 s in debug for the 1,189 names.
#[test]
fn every_name_compiles_and_draws_as_its_flag_says() {
    let engine = MathEngine::new();
    let mut drawn_nothing = 0;
    for s in symbols() {
        let layout = engine
            .layout(&s.name)
            .unwrap_or_else(|err| panic!("{}: {err}", s.name));
        let mut ids = Vec::new();
        for item in &layout.items {
            match item {
                MathItem::Glyphs { glyphs, .. } => ids.extend(glyphs.iter().map(|g| g.id)),
                // A glyph drawn as an outline would hide its id from this check.
                MathItem::Path { .. } => panic!("{}: drawn as a path", s.name),
            }
        }
        let boxed = ids.contains(&0);
        assert_eq!(
            boxed, !s.drawable,
            "{} ({:?}): glyphs {ids:?}",
            s.name, s.value
        );
        if ids.is_empty() {
            assert!(invisible(&s), "{} draws nothing", s.name);
            drawn_nothing += 1;
        }
    }
    assert_eq!(drawn_nothing, 7);
}

/// How long [`symbols`] takes and how much memory its list holds. Read the numbers; nothing is
/// asserted.
#[test]
#[ignore = "timing: run in release and in debug, one thread, on a quiet machine"]
fn measure_symbol_list() {
    const RUNS: usize = 50;
    let started = Instant::now();
    let list = symbols();
    let first = started.elapsed();
    let mut best = Duration::MAX;
    let mut total = Duration::ZERO;
    for _ in 0..RUNS {
        let started = Instant::now();
        let again = black_box(symbols());
        let took = started.elapsed();
        best = best.min(took);
        total += took;
        assert_eq!(again.len(), list.len());
    }
    let names: usize = list.iter().map(|s| s.name.len()).sum();
    let values: usize = list.iter().map(|s| s.value.len()).sum();
    let entries = list.capacity() * size_of::<SymbolInfo>();
    println!(
        "symbols(): {} names; first call {first:?}, then best {best:?}, mean {:?} of {RUNS}",
        list.len(),
        total / u32::try_from(RUNS).unwrap(),
    );
    println!(
        "memory: {} bytes = {} entries ({} x {} bytes) + names {names} + values {values}",
        entries + names + values,
        entries,
        list.capacity(),
        size_of::<SymbolInfo>(),
    );
}
