//! Frame timing for `examples/area.lawh` at 1920x1080, scale factor 1.
//!
//! Ignored by default. Run with:
//! `nice -n 19 cargo test --release -p lawh-render --test bench -- --ignored --nocapture`

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use std::hint::black_box;
use std::time::{Duration, Instant};

use lawh_render::{Renderer, Viewport};

const WARM_UP: usize = 10;
const FRAMES: usize = 50;

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[test]
#[ignore = "timing only; run with --release -- --ignored --nocapture"]
fn area_1080p_frame_time() {
    let board = common::area_board();
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };

    // First frame as the app sees it after opening a file: a new renderer,
    // bounds for the fit, then one render. Neither waits for math: the
    // formula is queued and drawn as a placeholder.
    let start = Instant::now();
    let mut renderer = Renderer::new();
    let created = start.elapsed();
    let bounds = renderer.board_bounds(&board).unwrap();
    let bounded = start.elapsed();
    let vp = Viewport::fit(bounds, 1920, 1080, 1.0);
    black_box(renderer.render(&board, &vp));
    let first = start.elapsed();

    for _ in 0..WARM_UP {
        black_box(renderer.render(&board, &vp));
    }
    let mut times: Vec<Duration> = (0..FRAMES)
        .map(|_| {
            let t = Instant::now();
            black_box(renderer.render(&board, &vp));
            t.elapsed()
        })
        .collect();
    times.sort();
    let median = (times[FRAMES / 2 - 1] + times[FRAMES / 2]) / 2;

    println!(
        "[{profile}] first frame {:.1} ms (Renderer::new {:.1} ms, bounds {:.1} ms, render {:.1} ms)",
        ms(first),
        ms(created),
        ms(bounded.saturating_sub(created)),
        ms(first.saturating_sub(bounded)),
    );
    println!(
        "[{profile}] steady frame over {FRAMES} after {WARM_UP} warm-up: median {:.2} ms, min {:.2} ms, max {:.2} ms",
        ms(median),
        ms(times[0]),
        ms(times[FRAMES - 1]),
    );
}

/// Most a frame of [`common::limits_board`] may take in a release build.
const LIMITS_BUDGET: Duration = Duration::from_millis(100);

/// Frames of a board at the core's limits, at the app's highest zoom (20) and scale factor 2,
/// looking at the places where its huge shapes cross the view. Asserts the release budget for
/// every frame. Laying out the board's 200,000 characters of text happens once, when the board
/// is opened, and is timed separately.
#[test]
#[ignore = "timing only; run with --release -- --ignored --nocapture"]
fn limits_frame_time() {
    let board = common::limits_board();
    let mut renderer = Renderer::new();
    renderer.resolve_math(&board);
    let t = Instant::now();
    black_box(renderer.board_bounds(&board));
    println!(
        "limits: laying out the text once, for the bounds: {:.1} ms",
        ms(t.elapsed())
    );
    let mut worst = Duration::ZERO;
    for (x, y) in common::LIMITS_CENTERS {
        let vp = Viewport::new(kurbo::Point::new(x, y), 20.0, 2560, 1600, 2.0);
        let t = Instant::now();
        black_box(renderer.render(&board, &vp));
        let first = t.elapsed();
        let mut times: Vec<Duration> = (0..5)
            .map(|_| {
                let t = Instant::now();
                black_box(renderer.render(&board, &vp));
                t.elapsed()
            })
            .collect();
        times.sort();
        println!(
            "limits at ({x}, {y}), zoom 20, scale 2, 2560x1600: first {:.1} ms, then median {:.2} ms, max {:.2} ms",
            ms(first),
            ms(times[2]),
            ms(times[4]),
        );
        worst = worst.max(first).max(times[4]);
    }
    if !cfg!(debug_assertions) {
        assert!(
            worst < LIMITS_BUDGET,
            "worst frame {:.1} ms is over the {:.0} ms budget",
            ms(worst),
            ms(LIMITS_BUDGET)
        );
    }
}

/// Median of `frames` renders of `board` through `vp`, after one warm-up render, and the first.
fn frame_time(
    renderer: &mut Renderer,
    board: &lawh_core::Board,
    vp: &Viewport,
    frames: usize,
) -> (Duration, Duration) {
    let t = Instant::now();
    black_box(renderer.render(board, vp));
    let first = t.elapsed();
    let mut times: Vec<Duration> = (0..frames)
        .map(|_| {
            let t = Instant::now();
            black_box(renderer.render(board, vp));
            t.elapsed()
        })
        .collect();
    times.sort();
    (first, times[frames / 2])
}

/// Formulas of about `n` glyphs or path elements, laid out normally, stacked on one spot, or
/// crossing the whole formula box.
fn math_cases(n: usize) -> [(&'static str, String); 7] {
    let side = if n.isqrt().pow(2) == n {
        n.isqrt()
    } else {
        n.isqrt() + 1
    };
    [
        (
            "glyph grid",
            format!(r#"#range({side}).map(_ => "x" * {side}).join(linebreak())"#),
        ),
        (
            "glyphs overlapping, one run",
            format!(r#"#text(tracking: -0.45em, "x" * {n})"#),
        ),
        (
            "glyphs stacked, own runs",
            format!("#for i in range({n}) {{ box(width: 0pt)[x] }}"),
        ),
        (
            "polygon stroke 100pt",
            format!(
                "#polygon(stroke: 100pt, ..range({n}).map(i => (calc.rem(i, 2) * 300pt, calc.rem(i, 3) * 300pt)))"
            ),
        ),
        (
            "polygon stroke 1pt",
            format!(
                "#polygon(stroke: 1pt, ..range({n}).map(i => (calc.rem(i, 2) * 300pt, calc.rem(i, 3) * 300pt)))"
            ),
        ),
        (
            "polygon fill",
            format!(
                "#polygon(fill: black, ..range({n}).map(i => (calc.rem(i, 2) * 300pt, calc.rem(i, 3) * 300pt)))"
            ),
        ),
        (
            "lines stacked, stroke 100pt",
            format!(
                "#for i in range({}) {{ box(width: 0pt, line(length: 300pt, angle: 45deg, stroke: 100pt)) }}",
                n / 2
            ),
        ),
    ]
}

/// Frame times of single formulas of growing complexity, which the math complexity caps
/// (`lawh_math::MAX_LAYOUT_GLYPHS`, `lawh_math::MAX_LAYOUT_PATH_ELEMENTS`) are chosen from:
/// fitted to 1920x1080, and at the app's highest zoom (20) at scale factors 1 and 2. Formulas
/// over the caps print `TooLarge`. `LAWH_BENCH_KINDS` picks cases by name prefix.
/// `LAWH_BENCH_SIZES=1000,10000 cargo test --release -p lawh-render --test bench -- --ignored --nocapture math_complexity`
#[test]
#[ignore = "timing only; run with --release -- --ignored --nocapture"]
fn math_complexity_frame_time() {
    use std::fmt::Write as _;

    use lawh_core::{Board, Object, ObjectId, Point, Shape};
    use lawh_math::{MathEngine, MathItem};

    let sizes: Vec<usize> = std::env::var("LAWH_BENCH_SIZES")
        .unwrap_or_else(|_| "1000,10000".to_owned())
        .split(',')
        .map(|s| s.trim().parse().unwrap())
        .collect();
    let kinds = std::env::var("LAWH_BENCH_KINDS").unwrap_or_default();
    for n in sizes {
        for (name, src) in math_cases(n) {
            if !kinds.is_empty() && !kinds.split(',').any(|k| name.starts_with(k)) {
                continue;
            }
            let mut board = Board::new();
            board
                .insert(Object::new(
                    ObjectId::new("m1").unwrap(),
                    Shape::Math {
                        at: Point::ZERO,
                        src: src.clone(),
                    },
                ))
                .unwrap();
            let mut renderer =
                Renderer::with_math(MathEngine::new().with_timeout(Duration::from_secs(120)));
            renderer.resolve_math(&board);
            let math = MathEngine::new().with_timeout(Duration::from_secs(120));
            let layout = match math.layout(&src) {
                Ok(layout) => layout,
                Err(err) => {
                    println!("{name} n={n}: {err:?}");
                    continue;
                }
            };
            // Counted as the caps count them: stroked elements twice.
            let (mut glyphs, mut elements) = (0, 0);
            for item in &layout.items {
                match item {
                    MathItem::Glyphs { glyphs: run, .. } => glyphs += run.len(),
                    MathItem::Path { path, stroke, .. } => {
                        let weight = if stroke.is_some() { 2 } else { 1 };
                        elements += path.elements().len() * weight;
                    }
                }
            }
            let bounds = renderer.board_bounds(&board).unwrap();
            let fit = Viewport::fit(bounds, 1920, 1080, 1.0);
            let center = bounds.center();
            let zoom1 = Viewport::new(center, 20.0, 1920, 1080, 1.0);
            let zoom2 = Viewport::new(center, 20.0, 2560, 1600, 2.0);
            let mut line =
                format!("{name:<28} n={n:>6}: {glyphs:>6} glyphs {elements:>6} elements |");
            for (label, vp) in [("fit", fit), ("zoom20", zoom1), ("zoom20@2x", zoom2)] {
                let (first, median) = frame_time(&mut renderer, &board, &vp, 3);
                write!(line, " {label} {:.1}/{:.1} ms", ms(first), ms(median)).unwrap();
            }
            println!("{line}");
        }
    }
}

/// Frame times of boards of `n` cheap objects that all reach the screen, which
/// `lawh_core::board::MAX_OBJECTS` is chosen from: fitted at 1920x1080 and scale factor 1, the
/// way the app first shows a board. Sizes over the cap are skipped. `LAWH_BENCH_KINDS` picks
/// cases by name prefix.
/// `LAWH_BENCH_SIZES=400,500 cargo test --release -p lawh-render --test bench -- --ignored --nocapture board_objects`
#[test]
#[ignore = "timing only; run with --release -- --ignored --nocapture"]
fn board_objects_frame_time() {
    use std::fmt::Write as _;

    let sizes: Vec<usize> = std::env::var("LAWH_BENCH_SIZES")
        .unwrap_or_else(|_| "400,500".to_owned())
        .split(',')
        .map(|s| s.trim().parse().unwrap())
        .collect();
    let kinds = std::env::var("LAWH_BENCH_KINDS").unwrap_or_default();
    let line = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor";
    let cases: [(&str, String); 9] = [
        (
            "circle r=1, stacked",
            "circle id=o{i} x=0 y=0 r=1".to_owned(),
        ),
        (
            "circle r=100, stacked",
            "circle id=o{i} x=0 y=0 r=100".to_owned(),
        ),
        (
            "rect 48x27, stacked",
            "rect id=o{i} x=0 y=0 w=48 h=27".to_owned(),
        ),
        (
            "rect 48x27 + 1 offset",
            "rect id=o{i} x=0 y=0 w=48 h=27\ncircle id=p{i} x=24 y=13 r=0.1".to_owned(),
        ),
        (
            "line diagonal, stacked",
            "line id=o{i} x1=0 y1=0 x2=48 y2=27".to_owned(),
        ),
        (
            "arrow diagonal, stacked",
            "arrow id=o{i} x1=0 y1=0 x2=48 y2=27".to_owned(),
        ),
        ("text x, stacked", "text id=o{i} x=0 y=0 \"x\"".to_owned()),
        (
            "text line, stacked",
            format!("text id=o{{i}} x=0 y=0 \"{line}\""),
        ),
        ("math x, stacked", "math id=o{i} x=0 y=0 \"x\"".to_owned()),
    ];
    for n in sizes {
        if n > lawh_core::board::MAX_OBJECTS {
            println!("n={n}: over MAX_OBJECTS, skipped");
            continue;
        }
        for (name, template) in &cases {
            if !kinds.is_empty() && !kinds.split(',').any(|k| name.starts_with(k)) {
                continue;
            }
            let mut src = "lawh version=1\n".to_owned();
            // Each template line is one object; the offset case has two per i.
            let per = template.lines().count();
            for i in 0..n / per {
                writeln!(src, "{}", template.replace("{i}", &i.to_string())).unwrap();
            }
            let board = common::board(&src);
            let mut renderer = Renderer::new();
            renderer.resolve_math(&board);
            let bounds = renderer.board_bounds(&board).unwrap();
            let fit = Viewport::fit(bounds, 1920, 1080, 1.0);
            let (first, median) = frame_time(&mut renderer, &board, &fit, 15);
            println!(
                "{name:<26} n={:>6} ({:>7} bytes): fitted 1920x1080 first {:.1} ms, median {:.1} ms",
                board.len(),
                src.len(),
                ms(first),
                ms(median)
            );
        }
    }
}
