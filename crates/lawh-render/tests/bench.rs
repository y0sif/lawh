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
    let mut renderer = Renderer::new_trusted();
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
        "[{profile}] first frame {:.1} ms (Renderer::new_trusted {:.1} ms, bounds {:.1} ms, render {:.1} ms)",
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
    let mut renderer = Renderer::new_trusted();
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
fn math_cases(n: usize) -> Vec<(&'static str, String)> {
    let side = if n.isqrt().pow(2) == n {
        n.isqrt()
    } else {
        n.isqrt() + 1
    };
    vec![
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
        (
            "glyphs stacked translucent",
            format!("#for i in range({n}) {{ box(width: 0pt, text(fill: rgb(0, 0, 0, 20))[x]) }}"),
        ),
        (
            "big glyphs stacked 100pt W",
            format!("#for i in range({n}) {{ box(width: 0pt, text(size: 100pt)[W]) }}"),
        ),
        (
            "lines stacked translucent",
            format!(
                "#for i in range({}) {{ box(width: 0pt, line(length: 300pt, angle: 45deg, stroke: 100pt + rgb(0, 0, 0, 20))) }}",
                n / 2
            ),
        ),
        (
            "rects stacked translucent",
            format!(
                "#for i in range({}) {{ box(width: 0pt, rect(width: 300pt, height: 300pt, fill: rgb(0, 0, 0, 20))) }}",
                n / 5
            ),
        ),
        (
            "polygon stroke 2000pt",
            format!(
                "#polygon(stroke: 2000pt, ..range({n}).map(i => (calc.rem(i, 2) * 300pt, calc.rem(i, 3) * 300pt)))"
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
            let mut renderer = Renderer::new_trusted();
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

/// Most a window frame may take in a release build, whatever the board holds: two refreshes at
/// 60 Hz. The frame budget in lawh-render's `budget.rs` is chosen to keep every case below under
/// it.
const FRAME_TARGET: Duration = Duration::from_millis(33);

/// The hostile boards the frame budget is for: formulas at the complexity caps
/// (`lawh_math::MAX_LAYOUT_GLYPHS`, `lawh_math::MAX_LAYOUT_PATH_ELEMENTS`) and boards of
/// `lawh_core::board::MAX_OBJECTS` costly objects.
fn worst_case_boards() -> Vec<(String, lawh_core::Board)> {
    use std::fmt::Write as _;

    let mut boards = Vec::new();
    let formulas = [
        (24_999, "glyphs stacked, own runs"),
        (24_999, "big glyphs stacked 100pt W"),
        (24_999, "glyphs stacked translucent"),
        (24_999, "glyphs overlapping, one run"),
        (24_964, "glyph grid"),
        (4_999, "polygon stroke 100pt"),
        (4_999, "polygon stroke 1pt"),
        (9_998, "polygon fill"),
        (4_999, "lines stacked, stroke 100pt"),
        (4_999, "lines stacked translucent"),
        (4_999, "rects stacked translucent"),
        (4_999, "polygon stroke 2000pt"),
    ];
    for (n, kind) in formulas {
        let (_, src) = math_cases(n)
            .into_iter()
            .find(|(name, _)| *name == kind)
            .unwrap();
        let src = src.replace('"', "\\\"");
        boards.push((
            format!("{kind} n={n}"),
            common::board(&format!("lawh version=1\nmath id=m1 x=0 y=0 \"{src}\"\n")),
        ));
        // The same formula 500 times: each copy is over the budget on its own.
        let mut many = "lawh version=1\n".to_owned();
        for i in 0..lawh_core::board::MAX_OBJECTS {
            writeln!(many, "math id=m{i} x=0 y=0 \"{src}\"").unwrap();
        }
        boards.push((format!("500x {kind} n={n}"), common::board(&many)));
    }
    let line = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor";
    let objects = [
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
            "rects offset by a pixel",
            "rect id=o{i} x={i}e-3 y=0 w=48 h=27".to_owned(),
        ),
        (
            "line diagonal, stacked",
            "line id=o{i} x1=0 y1=0 x2=48 y2=27".to_owned(),
        ),
        (
            "arrow diagonal, stacked",
            "arrow id=o{i} x1=0 y1=0 x2=48 y2=27".to_owned(),
        ),
        (
            "text line, stacked",
            format!("text id=o{{i}} x=0 y=0 \"{line}\""),
        ),
        (
            "text lines, spread",
            format!("text id=o{{i}} x=0 y={{i}} \"{line}\""),
        ),
    ];
    for (name, template) in objects {
        let mut src = "lawh version=1\n".to_owned();
        for i in 0..lawh_core::board::MAX_OBJECTS {
            writeln!(src, "{}", template.replace("{i}", &i.to_string())).unwrap();
        }
        boards.push((format!("500x {name}"), common::board(&src)));
    }
    boards
}

/// The time of a frame right after the image changed size (the best of three, each after a
/// frame at another size), and the median of five frames after it.
fn resized_and_steady(
    renderer: &mut Renderer,
    board: &lawh_core::Board,
    vp: &Viewport,
) -> (Duration, Duration) {
    let tiny = Viewport {
        width: 64,
        height: 64,
        ..*vp
    };
    let resized = (0..3)
        .map(|_| {
            black_box(renderer.render(board, &tiny));
            let t = Instant::now();
            black_box(renderer.render(board, vp));
            t.elapsed()
        })
        .min()
        .unwrap();
    let mut times: Vec<Duration> = (0..5)
        .map(|_| {
            let t = Instant::now();
            black_box(renderer.render(board, vp));
            t.elapsed()
        })
        .collect();
    times.sort();
    (resized, times[2])
}

/// Frame times of [`worst_case_boards`], fitted and at the app's highest zoom (20), at
/// 1920x1080 and at 2880x1800 with scale factor 2: the frame right after a resize and the
/// steady frame after it. Asserts [`FRAME_TARGET`] for both in release. The very first frame of
/// a process is not timed: it also pays for loading code and fonts. `LAWH_BENCH_KINDS` picks
/// boards by name prefix.
/// `nice -n 19 cargo test --release -p lawh-render --test bench -- --ignored --nocapture frame_budget`
#[test]
#[ignore = "timing only; run with --release -- --ignored --nocapture"]
fn frame_budget_worst_cases() {
    use std::fmt::Write as _;

    let kinds = std::env::var("LAWH_BENCH_KINDS").unwrap_or_default();
    let mut worst = (Duration::ZERO, String::new());
    for (name, board) in worst_case_boards() {
        if !kinds.is_empty() && !kinds.split(',').any(|k| name.starts_with(k)) {
            continue;
        }
        let mut renderer = Renderer::new_trusted();
        renderer.resolve_math(&board);
        let bounds = renderer.board_bounds(&board).unwrap();
        let mut line = format!("{name:<44}");
        for (width, height, scale) in [(1920, 1080, 1.0), (2880, 1800, 2.0)] {
            let fit = Viewport::fit(bounds, width, height, scale);
            let zoom20 = Viewport::new(bounds.center(), 20.0, width, height, scale);
            for (label, vp) in [("fit", fit), ("zoom20", zoom20)] {
                let (resized, median) = resized_and_steady(&mut renderer, &board, &vp);
                let stats = renderer.last_frame();
                write!(
                    line,
                    " | {width}x{height} {label} {:.1}/{:.1} ms est {:.1} ({} drawn, {} downgraded)",
                    ms(resized),
                    ms(median),
                    ms(stats.estimated),
                    stats.drawn,
                    stats.downgraded,
                )
                .unwrap();
                let slowest = resized.max(median);
                if slowest > worst.0 {
                    worst = (slowest, format!("{name} {width}x{height} {label}"));
                }
            }
        }
        println!("{line}");
    }
    println!("worst frame: {:.1} ms ({})", ms(worst.0), worst.1);
    if !cfg!(debug_assertions) {
        assert!(
            worst.0 < FRAME_TARGET,
            "worst frame {:.1} ms ({}) is over the {:.0} ms target",
            ms(worst.0),
            worst.1,
            ms(FRAME_TARGET)
        );
    }
}

/// Frame times of an empty board at the zooms where the dot grid is densest.
#[test]
#[ignore = "timing only; run with --release -- --ignored --nocapture"]
fn grid_frame_time() {
    let board = lawh_core::Board::new();
    let mut renderer = Renderer::new_trusted();
    for (width, height, scale) in [(1920, 1080, 1.0), (2880, 1800, 2.0)] {
        for zoom in [0.1, 0.15, 0.3, 0.31, 0.6, 1.0] {
            let vp = Viewport::new(kurbo::Point::ZERO, zoom, width, height, scale);
            let (first, median) = frame_time(&mut renderer, &board, &vp, 5);
            println!(
                "empty {width}x{height} scale {scale} zoom {zoom} ({:.1} px/unit): first {:.1} ms, median {:.1} ms",
                vp.px_per_unit(),
                ms(first),
                ms(median)
            );
        }
    }
}
