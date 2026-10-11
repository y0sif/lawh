//! Layouts of everyday formulas: box size, baseline and what gets drawn.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use std::fmt::Write as _;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use lawh_math::{
    MAX_LAYOUT_EXTENT_PT, MAX_LAYOUT_GLYPHS, MAX_LAYOUT_PATH_ELEMENTS, MathEngine, MathError,
    MathItem, MathLayout, MathStatus, fonts,
};

/// How long [`MathEngine::request`] may take for a cached formula. It takes microseconds, but a
/// busy machine (other builds, swap) can pause any thread for a while.
const CACHE_HIT_WITHIN: Duration = Duration::from_secs(1);

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

/// Formulas of a few dozen bytes that lay out far more than the complexity
/// caps allow: the review's repros.
const TOO_COMPLEX: [&str; 2] = [
    // 490,000 glyphs.
    r#"#range(700).map(_ => "x" * 700).join(linebreak())"#,
    // 100,001 stroked path elements.
    "#polygon(stroke: 100pt, ..range(100000).map(i => (calc.rem(i, 2) * 300pt, calc.rem(i, 3) * 300pt)))",
];

/// Size, baseline and ink (2 decimals) and the item list as kinds and counts,
/// which stays stable across small layout tweaks that don't matter.
fn summary(layout: &MathLayout) -> String {
    // Rounded first, so a tiny negative number doesn't print as -0.00.
    let ink = layout.ink;
    let [x0, y0, x1, y1] =
        [ink.x0, ink.y0, ink.x1, ink.y1].map(|v| (v * 100.0).round() / 100.0 + 0.0);
    let mut out = format!(
        "width: {:.2}\nheight: {:.2}\nbaseline: {:.2}\nink: {:.2}, {:.2} to {:.2}, {:.2}\nitems: {}\n",
        layout.width,
        layout.height,
        layout.baseline,
        x0,
        y0,
        x1,
        y1,
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
                assert!(
                    layout.ink.contains((glyph.x + 1.0, glyph.y - 1.0)),
                    "{glyph:?}"
                );
            }
        }
    }
    // An everyday formula's ink is its box, give or take a glyph's side bearing.
    let ink = layout.ink;
    assert!(ink.x0 > -1.0 && ink.y0 >= 0.0, "{ink:?}");
    assert!(
        ink.x1 < layout.width + 1.0 && ink.y1 <= layout.height,
        "{ink:?}"
    );
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
        assert!(started.elapsed() < CACHE_HIT_WITHIN, "{src}");
    }
}

#[test]
fn too_complex_layouts_are_too_large_and_cached() {
    // Generous: the 490,000 glyphs take about a second to lay out in release.
    let engine = MathEngine::new().with_timeout(Duration::from_secs(60));
    for src in TOO_COMPLEX {
        assert_eq!(engine.layout(src), Err(MathError::TooLarge), "{src}");
        let started = Instant::now();
        assert_eq!(
            engine.request(src),
            MathStatus::Failed(MathError::TooLarge),
            "{src}"
        );
        assert!(started.elapsed() < CACHE_HIT_WITHIN, "{src}");
    }
}

#[test]
fn heavy_real_formulas_stay_well_under_the_complexity_caps() {
    let engine = MathEngine::new().with_timeout(Duration::from_secs(30));
    for (name, src) in heavy_formulas() {
        let layout = engine.layout(&src);
        assert!(layout.is_ok(), "{name}: {layout:?}");
    }
    // The heavy formulas the caps were chosen from have at least five times
    // room on both.
    for name in [
        "mat 30x30",
        "sum chain x100",
        "cases x40",
        "integral chain x30",
    ] {
        let (_, src) = heavy_formulas()
            .into_iter()
            .find(|(n, _)| *n == name)
            .unwrap();
        let (glyphs, elements, stroked) = complexity(&engine.layout(&src).unwrap());
        assert!(glyphs * 5 <= MAX_LAYOUT_GLYPHS, "{name}: {glyphs} glyphs");
        assert!(
            (elements + stroked) * 5 <= MAX_LAYOUT_PATH_ELEMENTS,
            "{name}: {elements} elements, {stroked} stroked"
        );
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

/// Glyphs, path elements and stroked path elements in `layout`.
fn complexity(layout: &MathLayout) -> (usize, usize, usize) {
    let (mut glyphs, mut elements, mut stroked) = (0, 0, 0);
    for item in &layout.items {
        match item {
            MathItem::Glyphs { glyphs: run, .. } => glyphs += run.len(),
            MathItem::Path { path, stroke, .. } => {
                elements += path.elements().len();
                if stroke.is_some() {
                    stroked += path.elements().len();
                }
            }
        }
    }
    (glyphs, elements, stroked)
}

/// Heavy formulas a real board might hold, which the complexity caps are chosen from.
fn heavy_formulas() -> Vec<(&'static str, String)> {
    let cases: Vec<String> = (1..=40)
        .map(|k| format!("x^{k} + {k} x &\"if\" {k} <= x < {} ", k + 1))
        .collect();
    let derivation: Vec<String> = (1..=40)
        .map(|k| format!("&= integral_0^{k} (x^2 + {k} x)/(sqrt(1 + x^{k})) dif x"))
        .collect();
    let fractions: Vec<String> = (1..=20)
        .map(|i| {
            let row: Vec<String> = (1..=20).map(|j| format!("({i} x)/({j} + y)")).collect();
            row.join(", ")
        })
        .collect();
    vec![
        ("mat 30x30", matrix(30)),
        (
            "mat 20x20 fractions",
            format!("mat({})", fractions.join("; ")),
        ),
        (
            "sum chain x100",
            format!("{}0", "sum_(i=1)^n i + ".repeat(100)),
        ),
        (
            "integral chain x30",
            format!(
                "{}0",
                "integral_0^infinity (e^(-x^2) sin(k x))/(sqrt(1 + x^2)) dif x + ".repeat(30)
            ),
        ),
        ("cases x40", format!("f(x) = cases({})", cases.join(", "))),
        ("derivation x40", format!("I {}", derivation.join(" \\ "))),
        (
            "continued fraction x20",
            format!("{}1{}", "1 + 1/(".repeat(20), ")".repeat(20)),
        ),
        ("mat 30x30 fractions", {
            let rows: Vec<String> = (1..=30)
                .map(|i| {
                    (1..=30)
                        .map(|j| format!("{i}/{j}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .collect();
            format!("mat({})", rows.join("; "))
        }),
        (
            "rotated mat 10x10",
            format!("#rotate(30deg, ${}$)", matrix(10)),
        ),
        (
            "mirrored mat 10x10",
            format!("#scale(x: -100%, ${}$)", matrix(10)),
        ),
        (
            "rotated label",
            "#rotate(-90deg, reflow: true)[velocity of the particle in m/s] = v(t)".to_owned(),
        ),
        (
            "cancel x50",
            (1..=50)
                .map(|k| format!("cancel(x_{k})"))
                .collect::<Vec<_>>()
                .join(" + "),
        ),
        (
            "boxed x50",
            (1..=50)
                .map(|k| format!("#box(stroke: 0.5pt, inset: 2pt, $a_{k}$)"))
                .collect::<Vec<_>>()
                .join(" + "),
        ),
        (
            "taylor x60",
            (0..60)
                .map(|k| format!("(f^(({k}))(a))/({k}!) (x-a)^{k}"))
                .collect::<Vec<_>>()
                .join(" + "),
        ),
    ]
}

/// Prints the size of the heavy formulas above and of the review's hostile repros, which the
/// complexity caps are chosen from.
/// `cargo test --release -p lawh-math --test layout -- --ignored --nocapture measure`.
#[test]
#[ignore = "a measurement, not a check"]
fn measure_heavy_formulas() {
    let engine = MathEngine::new().with_timeout(Duration::from_secs(60));
    let hostile = [
        (
            "repro 700x700 glyphs",
            r#"#range(700).map(_ => "x" * 700).join(linebreak())"#.to_owned(),
        ),
        (
            "repro polygon 1e5",
            "#polygon(stroke: 100pt, ..range(100000).map(i => (calc.rem(i, 2) * 300pt, calc.rem(i, 3) * 300pt)))".to_owned(),
        ),
    ];
    for (name, src) in heavy_formulas().into_iter().chain(hostile) {
        let started = Instant::now();
        let outcome = engine.layout(&src);
        let took = started.elapsed();
        match outcome {
            Ok(layout) => {
                let (glyphs, elements, stroked) = complexity(&layout);
                println!(
                    "{name:<24} {:>7.1} ms  {glyphs:>7} glyphs {elements:>7} path elements ({stroked} stroked), items {}, {:.0} x {:.0} pt",
                    took.as_secs_f64() * 1000.0,
                    layout.items.len(),
                    layout.width,
                    layout.height,
                );
            }
            Err(err) => println!(
                "{name:<24} {:>7.1} ms  {err:?}",
                took.as_secs_f64() * 1000.0
            ),
        }
    }
}
