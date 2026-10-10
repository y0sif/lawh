//! Layouts of everyday formulas: box size, baseline and what gets drawn.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use std::fmt::Write as _;
use std::sync::Arc;

use lawh_math::{MathEngine, MathItem, MathLayout, fonts};

/// Size, baseline (2 decimals) and the item list as kinds and counts, which
/// stays stable across small layout tweaks that don't matter.
fn summary(layout: &MathLayout) -> String {
    let mut out = format!(
        "width: {:.2}\nheight: {:.2}\nbaseline: {:.2}\nitems: {}\n",
        layout.width,
        layout.height,
        layout.baseline,
        layout.items.len()
    );
    for item in &layout.items {
        match item {
            MathItem::Glyphs {
                font, size, glyphs, ..
            } => {
                let family = fonts()[*font].family;
                writeln!(out, "- glyphs: {} x {family} {size:.1}pt", glyphs.len()).unwrap();
            }
            MathItem::Path { path, fill, stroke } => {
                let fill = if fill.is_some() { " fill" } else { "" };
                let stroke = stroke.map_or(String::new(), |(w, _)| format!(" stroke {w:.2}pt"));
                writeln!(
                    out,
                    "- path: {} elements{fill}{stroke}",
                    path.elements().len()
                )
                .unwrap();
            }
        }
    }
    out
}

fn assert_fonts_bundled(layout: &MathLayout) {
    for item in &layout.items {
        if let MathItem::Glyphs { font, .. } = item {
            assert!(*font < fonts().len(), "font index {font} is not bundled");
        }
    }
}

fn check(name: &str, src: &str) -> Arc<MathLayout> {
    let layout = MathEngine::new().layout(src).unwrap();
    assert_fonts_bundled(&layout);
    assert!(layout.width > 0.0 && layout.height > 0.0);
    assert!((0.0..=layout.height).contains(&layout.baseline));
    insta::assert_snapshot!(name, summary(&layout), src);
    layout
}

#[test]
fn area_of_a_circle() {
    check("area_of_a_circle", "A = pi r^2");
}

#[test]
fn superscript() {
    check("superscript", "x^2");
}

#[test]
fn fraction_has_a_bar() {
    let layout = check("fraction", "a/b");
    assert!(
        layout
            .items
            .iter()
            .any(|item| matches!(item, MathItem::Path { .. }))
    );
}

#[test]
fn sum_with_limits() {
    check("sum_with_limits", "sum_(i=1)^n i");
}

#[test]
fn glyphs_sit_inside_the_box() {
    let layout = MathEngine::new().layout("A = pi r^2").unwrap();
    for item in &layout.items {
        if let MathItem::Glyphs { glyphs, .. } = item {
            for glyph in glyphs {
                assert!((0.0..=layout.width).contains(&glyph.x), "{glyph:?}");
                assert!((0.0..=layout.height).contains(&glyph.y), "{glyph:?}");
            }
        }
    }
}

#[test]
fn same_source_returns_the_same_arc() {
    let engine = MathEngine::new();
    let first = engine.layout("A = pi r^2").unwrap();
    let second = engine.layout("A = pi r^2").unwrap();
    assert!(Arc::ptr_eq(&first, &second));
}

#[test]
fn errors_are_cached_too() {
    let engine = MathEngine::new();
    let first = engine.layout("#nope").unwrap_err();
    let second = engine.layout("#nope").unwrap_err();
    assert_eq!(first, second);
}
