//! Layouts of everyday formulas: box size, baseline and what gets drawn.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use std::fmt::Write as _;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use lawh_math::{
    MAX_LAYOUT_EXTENT_PT, MathEngine, MathError, MathItem, MathLayout, MathStatus, fonts,
};

/// Formulas built to be huge: each reaches far past [`MAX_LAYOUT_EXTENT_PT`].
/// The circle is the verifier's repro: zoomed in, it made the renderer
/// flatten an arc 2e21 pixels long.
const TOO_LARGE: [&str; 4] = [
    "#circle(radius: 1e18pt)",
    "#box(width: 1e9pt)",
    "#h(1e9pt)",
    "#line(length: 1e12pt)",
];

/// An `n` x `n` matrix of subscripted entries.
fn matrix(n: usize) -> String {
    let rows: Vec<String> = (1..=n)
        .map(|i| {
            let row: Vec<String> = (1..=n).map(|j| format!("a_({i},{j})")).collect();
            row.join(", ")
        })
        .collect();
    format!("mat({})", rows.join("; "))
}

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

#[test]
fn request_works_in_process_too() {
    let engine = MathEngine::new();
    let (tx, rx) = mpsc::channel();
    engine.set_waker(Arc::new(move || {
        let _ = tx.send(());
    }));
    assert_eq!(engine.request("x^2"), MathStatus::Pending);
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let MathStatus::Ready(layout) = engine.request("x^2") else {
        panic!("expected a layout after the waker fired");
    };
    assert!(Arc::ptr_eq(
        &layout,
        &engine.layout_blocking("x^2").unwrap()
    ));
}

#[test]
fn huge_layouts_are_too_large_and_cached() {
    let engine = MathEngine::new();
    for src in TOO_LARGE {
        assert_eq!(engine.layout(src), Err(MathError::TooLarge), "{src}");
        let started = Instant::now();
        assert_eq!(
            engine.request(src),
            MathStatus::Failed(MathError::TooLarge),
            "{src}"
        );
        assert!(started.elapsed() < Duration::from_millis(50), "{src}");
    }
}

#[test]
fn big_real_formulas_stay_well_under_the_extent_cap() {
    // Generous: a big matrix compiles in well under a second, but debug builds on a busy
    // machine are slow.
    let engine = MathEngine::new().with_timeout(Duration::from_secs(30));
    let sums = format!("{}0", "sum_(i=1)^n i + ".repeat(100));
    for src in [matrix(30), "sum_(i=1)^n".to_owned(), sums] {
        let layout = engine.layout(&src).unwrap();
        let extent = layout.width.max(layout.height);
        assert!(
            extent < MAX_LAYOUT_EXTENT_PT / 10.0,
            "{extent} pt: {}",
            &src[..40.min(src.len())]
        );
    }
}
