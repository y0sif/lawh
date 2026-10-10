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
/// every frame. Laying out the board's 10,000 characters of text happens once, when the board
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

/// A `kB` field of `/proc/self/status`, in MiB; 0 where there is none.
fn status_mib(field: &str) -> f64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find(|line| line.starts_with(field))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|kb| kb.parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

/// Resets this process's peak resident memory (`VmHWM`) to what it uses now (Linux).
fn reset_peak_rss() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

/// `n` characters of words in lines of about `per_line` characters (the line breaks count),
/// starting with `tag` so no two texts are the same.
fn words(tag: usize, n: usize, per_line: usize) -> String {
    cycled(tag, n, per_line, WORDS)
}

/// `n` characters cycling through `alphabet`, in lines of about `per_line` characters,
/// starting with `tag`.
fn cycled(tag: usize, n: usize, per_line: usize, alphabet: &str) -> String {
    let mut out = format!("{tag} ");
    let mut count = out.chars().count();
    let mut column = count;
    let mut chars = alphabet.chars().cycle();
    while count < n {
        if column + 1 >= per_line {
            out.push('\n');
            column = 0;
        } else {
            out.push(chars.next().unwrap());
            column += 1;
        }
        count += 1;
    }
    out
}

/// The text caps the worst boards are built to: from `LAWH_CAP_CHARS`, `LAWH_CAP_LINES` and
/// `LAWH_CAP_TOTAL`, or lawh-core's.
fn text_caps() -> (usize, usize, usize) {
    let var = |name: &str, default: usize| {
        std::env::var(name)
            .ok()
            .map_or(default, |v| v.trim().parse().unwrap())
    };
    (
        var("LAWH_CAP_CHARS", lawh_core::board::MAX_TEXT_CHARS),
        var("LAWH_CAP_LINES", lawh_core::board::MAX_TEXT_LINES),
        var("LAWH_CAP_TOTAL", lawh_core::board::MAX_BOARD_TEXT_CHARS),
    )
}

/// How [`text_worst_boards`] fills one board.
struct TextCase {
    name: &'static str,
    /// Characters in each text.
    per_text: usize,
    /// The `i`th text of `n` characters.
    content: Box<dyn Fn(usize, usize) -> String>,
    /// Failing math sources instead of texts.
    math: bool,
}

/// Words for the cases below.
const WORDS: &str = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor ";

/// The kinds of text [`text_worst_boards`] tries, for texts of at most `chars` characters and
/// `lines` lines, and boards of at most `total` characters.
fn text_cases(chars: usize, lines: usize, total: usize) -> Vec<TextCase> {
    // Lines just long enough that a text of `n` characters has fewer than `lines` of them.
    let per_line = move |n: usize| n.div_ceil(lines - 1).max(2);
    let case = |name: &'static str, per_text: usize, math: bool, content| TextCase {
        name,
        per_text,
        content,
        math,
    };
    // `alphabet` over as many lines as a text may have, or in lines of 75 characters or more.
    let cycling = |name: &'static str, alphabet: &'static str, max_lines: bool| {
        let content = move |i, n| {
            let line = if max_lines {
                per_line(n)
            } else {
                per_line(n).max(75)
            };
            cycled(i, n, line, alphabet)
        };
        case(name, chars, false, Box::new(content))
    };
    vec![
        // Line breaks only: the most lines per character.
        case(
            "newlines",
            chars.min(lines - 1),
            false,
            Box::new(|i, n| format!("{i}{}", "\n".repeat(n.saturating_sub(i.to_string().len())))),
        ),
        cycling("words, max lines", WORDS, true),
        case(
            "one line",
            chars,
            false,
            Box::new(|i, n| words(i, n, usize::MAX)),
        ),
        // The board's text spread over every object.
        case(
            "small texts",
            total / lawh_core::board::MAX_OBJECTS,
            false,
            Box::new(|i, n| words(i, n, 75)),
        ),
        // Characters the bundled font has no glyph for.
        cycling("cjk", "漢字仮名交じり文", false),
        // Combining marks stacked on one letter.
        cycling(
            "combining",
            "a\u{301}\u{302}\u{303}\u{304}\u{305}\u{306}\u{307}",
            false,
        ),
        // Right-to-left and left-to-right words in turn: a run per word.
        cycling("bidi", "ab אב ", false),
        // The direction changing at every character: a run per character.
        cycling("bidi per char", "aא", false),
        cycling("bidi per char, max lines", "aא", true),
        // The script changing at every character.
        cycling("scripts per char", "aαа漢ا", false),
        // Arabic, which joins its letters.
        cycling("arabic", "سلام عليكم ", false),
        // Emoji joined into families, which the font doesn't have.
        cycling("emoji", "👨\u{200D}👩\u{200D}👧\u{FE0F}", false),
        // Formulas that fail, drawn as their source.
        case(
            "failing math, newlines",
            chars.min(lines - 1),
            true,
            Box::new(|i, n| {
                let head = format!("#x{i}");
                let breaks = n.saturating_sub(head.len());
                format!("{head}{}", "\n".repeat(breaks))
            }),
        ),
        case(
            "failing math, words",
            chars,
            true,
            Box::new(move |i, n| format!("#x{}", words(i, n.saturating_sub(2), per_line(n)))),
        ),
    ]
}

/// The costliest boards of text and failing math the text caps allow, for the cases whose name
/// starts with one of the comma-separated `kinds` (all if empty): as many texts as the
/// board-wide cap and the object cap leave, each as long as the per-text caps allow, all
/// different (so no layout is shared), stacked on one spot so every one of them is on screen
/// in every view.
fn text_worst_boards(
    chars: usize,
    lines: usize,
    total: usize,
    kinds: &str,
) -> Vec<(String, lawh_core::Board)> {
    use lawh_core::{Board, Object, ObjectId, Point, Shape};

    let mut boards = Vec::new();
    for case in text_cases(chars, lines, total) {
        if !kinds.is_empty() && !kinds.split(',').any(|k| case.name.starts_with(k)) {
            continue;
        }
        let per_text = case.per_text.max(1);
        let count = (total / per_text).min(lawh_core::board::MAX_OBJECTS);
        let mut board = Board::new();
        for i in 0..count {
            let text = (case.content)(i, per_text);
            let shape = if case.math {
                Shape::Math {
                    at: Point::ZERO,
                    src: text,
                }
            } else {
                Shape::Text {
                    at: Point::ZERO,
                    text,
                }
            };
            board
                .insert(Object::new(ObjectId::new(format!("o{i}")).unwrap(), shape))
                .unwrap();
        }
        boards.push((format!("{} {count}x{per_text}", case.name), board));
    }
    boards
}

/// Opening and drawing the worst boards of text the text caps allow ([`text_worst_boards`]), which
/// `lawh_core::board::{MAX_TEXT_CHARS, MAX_TEXT_LINES, MAX_BOARD_TEXT_CHARS}` are chosen from:
/// reading the file, `board_bounds` on a new renderer (it lays out every text, as the app does on
/// open) with its extra peak memory, then frames fitted, at zoom 0.1, 1 and 20 at the texts'
/// corner, at 1920x1080 and at 2880x1800 with scale factor 2 (resized/steady, as
/// [`frame_budget_worst_cases`]). Failing math is compiled first, so it is drawn as its source.
/// Run one case per process for clean memory numbers; `LAWH_BENCH_KINDS` picks cases by name
/// prefix, `LAWH_CAP_CHARS`, `LAWH_CAP_LINES` and `LAWH_CAP_TOTAL` try other caps.
/// `LAWH_BENCH_KINDS=newlines nice -n 19 cargo test --release -p lawh-render --test bench -- --ignored --nocapture text_caps`
#[test]
#[ignore = "timing only; run with --release -- --ignored --nocapture"]
fn text_caps_worst_cases() {
    use std::fmt::Write as _;

    let (chars, lines, total) = text_caps();
    println!("caps: {chars} chars and {lines} lines a text, {total} chars a board");
    let kinds = std::env::var("LAWH_BENCH_KINDS").unwrap_or_default();
    let mut worst_frame = (Duration::ZERO, String::new());
    let mut worst_open = (Duration::ZERO, String::new());
    for (name, board) in text_worst_boards(chars, lines, total, &kinds) {
        let file = lawh_core::to_lawh(&board);
        let t = Instant::now();
        let read = lawh_core::from_lawh(&file);
        let parsed = t.elapsed();
        if let Ok(read) = read {
            assert_eq!(read, board);
        } else {
            println!("{name}: the file does not read back: {read:?}");
        }

        let mut renderer = Renderer::new_trusted();
        let has_math = board
            .objects()
            .iter()
            .any(|o| matches!(o.shape, lawh_core::Shape::Math { .. }));
        if has_math {
            renderer.resolve_math(&board);
        }
        // Not math: a new renderer for the bounds, as on open.
        let mut fresh = Renderer::new_trusted();
        black_box(fresh.board_bounds(&lawh_core::Board::default()));
        let rss_before = status_mib("VmRSS:");
        reset_peak_rss();
        let t = Instant::now();
        let bounds = fresh.board_bounds(&board).unwrap();
        let bounded = t.elapsed();
        let peak_extra = status_mib("VmHWM:") - rss_before;
        let kept_extra = status_mib("VmRSS:") - rss_before;
        drop(fresh);
        if bounded > worst_open.0 {
            worst_open = (bounded, name.clone());
        }
        println!(
            "{name:<34} file {:>8} bytes, read {:.1} ms | board_bounds {:.1} ms, +{:.0} MiB peak, +{:.0} MiB kept",
            file.len(),
            ms(parsed),
            ms(bounded),
            peak_extra,
            kept_extra,
        );
        renderer.board_bounds(&board);
        for (width, height, scale) in [(1920u16, 1080u16, 1.0), (2880, 1800, 2.0)] {
            let mut line = format!("    {width}x{height}@{scale}");
            let fit = Viewport::fit(bounds, width, height, scale);
            let corner = |zoom: f64| {
                let vp = Viewport::new(kurbo::Point::ZERO, zoom, width, height, scale);
                let unit = vp.px_per_unit();
                // The texts' top-left corner a little in from the view's.
                let center = kurbo::Point::new(
                    f64::from(width) / 2.0 / unit - 1.0,
                    f64::from(height) / 2.0 / unit - 1.0,
                );
                Viewport::new(center, zoom, width, height, scale)
            };
            for (label, vp) in [
                ("fit", fit),
                ("z0.1", corner(0.1)),
                ("z1", corner(1.0)),
                ("z20", corner(20.0)),
            ] {
                let (resized, median) = resized_and_steady(&mut renderer, &board, &vp);
                let stats = renderer.last_frame();
                write!(
                    line,
                    " | {label} {:.1}/{:.1} ms ({}d {}g)",
                    ms(resized),
                    ms(median),
                    stats.drawn,
                    stats.downgraded,
                )
                .unwrap();
                let slowest = resized.max(median);
                if slowest > worst_frame.0 {
                    worst_frame = (slowest, format!("{name} {width}x{height} {label}"));
                }
            }
            println!("{line}");
        }
    }
    println!(
        "worst board_bounds: {:.1} ms ({}); worst frame: {:.1} ms ({})",
        ms(worst_open.0),
        worst_open.1,
        ms(worst_frame.0),
        worst_frame.1
    );
}

/// Time to read `.lawh` files of `LAWH_READ_BYTES` bytes (default `MAX_FILE_BYTES`) that hold
/// no objects, or few: the cost of `from_lawh` alone, which grows with the file, not the board.
/// `LAWH_READ_BYTES=1048576 nice -n 19 cargo test --release -p lawh-render --test bench -- --ignored --nocapture read_time`
#[test]
#[ignore = "timing only; run with --release -- --ignored --nocapture"]
fn read_time_of_filler_files() {
    let max: usize = std::env::var("LAWH_READ_BYTES")
        .map_or(lawh_core::format::MAX_FILE_BYTES, |v| {
            v.trim().parse().unwrap()
        });
    let header = "lawh version=1\n";
    let fill = |unit: &str| {
        let mut s = header.to_owned();
        while s.len() + unit.len() <= max {
            s.push_str(unit);
        }
        s
    };
    let cases = [
        ("spaces", fill(" ")),
        ("block comments", fill("/* lorem ipsum dolor sit amet */\n")),
        ("line breaks", fill("\n")),
        ("short line comments", fill("//\n")),
        (
            "line comments",
            fill("// lorem ipsum dolor sit amet consectetur adipiscing elit\n"),
        ),
        ("slashdashed nodes", fill("/-r\n")),
        ("slashdashed rects", fill("/-rect id=r1 x=0 y=0 w=1 h=1\n")),
        (
            "slashdashed texts",
            fill(&format!("/-text id=t1 x=0 y=0 \"{}\"\n", "a".repeat(1000))),
        ),
    ];
    for (name, src) in cases {
        let t = Instant::now();
        let read = lawh_core::from_lawh(&src);
        println!(
            "{name:<20} {:>8} bytes: {:>8.1} ms ({})",
            src.len(),
            ms(t.elapsed()),
            if read.is_ok() { "read" } else { "error" }
        );
    }
}
