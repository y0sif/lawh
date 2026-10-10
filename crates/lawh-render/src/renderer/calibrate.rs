//! Estimated against measured costs, for choosing the constants in `budget.rs`.
//!
//! Each test draws one kind of primitive at many sizes and prints the estimate, the measured
//! time past an empty frame, and their ratio, which should stay under about 1 where the cost is
//! high. Ignored by default; run in release:
//! `nice -n 19 cargo test --release -p lawh-render --lib -- --ignored --nocapture calibrate`

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use std::time::{Duration, Instant};

use kurbo::{Affine, BezPath, Circle, Point, Rect, Shape as _};
use lawh_math::{MathEngine, MathGlyph, MathItem, MathLayout};

use super::{MATH_SCALE, Renderer, layout_bounds, math_to_board};
use crate::budget::{self, Budget};
use crate::glyphs::{GlyphStats, GlyphTable};
use crate::{Viewport, draw};

const BLACK: [u8; 4] = [0, 0, 0, 255];
const TRANSLUCENT: [u8; 4] = [0, 0, 0, 40];
const SCREENS: [(u16, u16, f64); 2] = [(1920, 1080, 1.0), (2880, 1800, 2.0)];

/// Median time of `frames` runs of `draw`, after two warm-ups.
fn median(frames: usize, mut draw: impl FnMut()) -> Duration {
    draw();
    draw();
    let mut times: Vec<Duration> = (0..frames)
        .map(|_| {
            let t = Instant::now();
            draw();
            t.elapsed()
        })
        .collect();
    times.sort();
    times[frames / 2]
}

/// Prints the estimate and the measured time, past an empty frame, of drawing `layout` at the
/// board origin through `vp`, both in ms, and their ratio.
fn row(renderer: &mut Renderer, name: &str, layout: &MathLayout, vp: &Viewport) {
    let clip = draw::clip_rect(vp);
    let estimate = renderer
        .math_cost(Point::ZERO, layout, vp, clip, f64::INFINITY)
        .ns
        / 1e6;
    let empty = median(9, || {
        renderer.begin_frame(vp);
        std::hint::black_box(renderer.finish_frame());
    });
    let took = median(5, || {
        renderer.begin_frame(vp);
        let mut budget = Budget::unlimited();
        assert!(renderer.draw_math(Point::ZERO, layout, vp, clip, &mut budget));
        std::hint::black_box(renderer.finish_frame());
    });
    let took = took.saturating_sub(empty).as_secs_f64() * 1e3;
    println!(
        "{name:<56} est {estimate:>9.3} ms  took {took:>9.3} ms  ratio {:>5.2}",
        took / estimate
    );
}

/// A viewport at zoom 1 where layout pt land on screen at `scale` pixels each, from the
/// top-left corner.
fn pixel_viewport(width: u16, height: u16, scale: f64) -> Viewport {
    Viewport::new(
        Point::new(
            f64::from(width) / 2.0 / scale * MATH_SCALE,
            f64::from(height) / 2.0 / scale * MATH_SCALE,
        ),
        1.0,
        width,
        height,
        scale,
    )
}

fn layout(items: Vec<MathItem>) -> MathLayout {
    MathLayout {
        width: 1.0,
        height: 1.0,
        baseline: 1.0,
        ink: Rect::ZERO,
        items,
    }
}

fn run(font: usize, size: f64, glyphs: Vec<MathGlyph>) -> MathItem {
    MathItem::Glyphs {
        font,
        size,
        fill: BLACK,
        glyphs,
    }
}

/// `count` copies of glyph `id`, stacked at `at`.
fn pile(font: usize, id: u32, size: f64, count: usize, at: Point) -> MathLayout {
    let glyph = MathGlyph {
        id,
        x: at.x,
        y: at.y,
    };
    layout(vec![run(font, size, vec![glyph; count])])
}

fn paths(paths: Vec<BezPath>, fill: Option<[u8; 4]>, stroke: Option<(f64, [u8; 4])>) -> MathLayout {
    layout(
        paths
            .into_iter()
            .map(|path| MathItem::Path { path, fill, stroke })
            .collect(),
    )
}

/// `n` lines from side to side of a `w` x `h` screen, at scattered heights.
fn zigzag(n: u32, w: f64, h: f64) -> BezPath {
    let mut path = BezPath::new();
    path.move_to((0.0, 0.0));
    for i in 0..n {
        let x = if i % 2 == 0 { w } else { 0.0 };
        path.line_to((x, h * f64::from((i * 7919) % 101) / 100.0));
    }
    path.close_path();
    path
}

fn glyph_id(font: usize, c: char) -> u32 {
    let face = ttf_parser::Face::parse(lawh_math::fonts()[font].data, 0).unwrap();
    u32::from(face.glyph_index(c).unwrap().0)
}

/// Plain glyphs and the bundled fonts' most complex ones (longest outline, most elements,
/// largest box), stacked about 30 ms deep by the estimate (`LAWH_CAL_COUNT` sets the depth), at
/// em sizes from 8 to 65,536 pixels.
#[test]
#[ignore = "timing only; run in release with --ignored --nocapture"]
fn calibrate_glyphs() {
    let mut renderer = Renderer::new_trusted();
    let mut table = GlyphTable::new();
    let mut cases = vec![
        ("Inter x".to_owned(), 0, glyph_id(0, 'x')),
        ("Inter W".to_owned(), 0, glyph_id(0, 'W')),
        ("Libertinus x".to_owned(), 1, glyph_id(1, 'x')),
    ];
    for (label, list) in [
        ("longest", table.most_complex(|s| s.outline_em)),
        ("busiest", table.most_complex(GlyphStats::elements)),
        ("widest", table.most_complex(|s| s.area_em)),
    ] {
        for (font, id) in list {
            cases.push((format!("{label} f{font} g{id}"), font, id));
        }
    }
    let depth: Option<usize> = std::env::var("LAWH_CAL_COUNT")
        .ok()
        .map(|n| n.parse().unwrap());
    for (width, height, scale) in SCREENS {
        let vp = pixel_viewport(width, height, scale);
        let (w, h) = (f64::from(width) / scale, f64::from(height) / scale);
        for (name, font, id) in &cases {
            let stats = table.stats(*font, *id);
            for em in [8.0, 32.0, 128.0, 512.0, 2048.0, 8192.0, 32768.0] {
                let em_px = em * scale;
                let one = budget::glyph(stats, em_px, f64::from(width), f64::from(height));
                let count = depth.unwrap_or_else(|| {
                    let fit = 30e6 / one;
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "clamped to a small positive count"
                    )]
                    let count = fit.clamp(1.0, 25_000.0) as usize;
                    count
                });
                // Origin left of and below the centre, so the glyph covers the middle.
                let at = Point::new(w / 2.0 - 0.3 * em, h / 2.0 + 0.3 * em);
                let label = format!(
                    "{width}x{height} {name} em {em_px} px x{count} [{} {} {:.3} {:.3}]",
                    stats.lines, stats.curves, stats.outline_em, stats.area_em
                );
                row(&mut renderer, &label, &pile(*font, *id, em, count, at), &vp);
            }
        }
    }
}

/// Fills and strokes from 2 to 20,000 pixels wide, opaque and translucent: screen-crossing
/// zigzags, and circles from 3 to 30,000 pixels in radius stacked at the centre.
#[test]
#[ignore = "timing only; run in release with --ignored --nocapture"]
fn calibrate_paths() {
    let mut renderer = Renderer::new_trusted();
    for (width, height, scale) in SCREENS {
        let vp = pixel_viewport(width, height, scale);
        let (w, h) = (f64::from(width) / scale, f64::from(height) / scale);
        for (fill, stroke, label) in [
            (Some(BLACK), None, "fill"),
            (Some(TRANSLUCENT), None, "fill translucent"),
            (None, Some((2.0, BLACK)), "stroke 2"),
            (None, Some((20.0, BLACK)), "stroke 20"),
            (None, Some((200.0, BLACK)), "stroke 200"),
            (None, Some((2000.0, BLACK)), "stroke 2000"),
            (None, Some((20000.0, BLACK)), "stroke 20000"),
            (None, Some((200.0, TRANSLUCENT)), "stroke 200 translucent"),
        ] {
            let name = format!("{width}x{height} zigzag 1000 {label}");
            row(
                &mut renderer,
                &name,
                &paths(vec![zigzag(1000, w, h)], fill, stroke),
                &vp,
            );
            for r in [3.0, 30.0, 300.0, 3000.0, 30000.0] {
                let circle = Circle::new((w / 2.0, h / 2.0), r).to_path(0.1);
                let deep = if r < 100.0 { 10 } else { 1 };
                let count = ((2000 / circle.elements().len()).max(1) * deep).min(2000);
                let name = format!("{width}x{height} circle r {r} x{count} {label}");
                row(
                    &mut renderer,
                    &name,
                    &paths(vec![circle; count], fill, stroke),
                    &vp,
                );
            }
        }
    }
}

/// The same 5,000 glyphs as one run and as a run each: what setting up a run costs.
#[test]
#[ignore = "timing only; run in release with --ignored --nocapture"]
fn calibrate_runs() {
    let mut renderer = Renderer::new_trusted();
    let x = glyph_id(0, 'x');
    for (width, height, scale) in SCREENS {
        let vp = pixel_viewport(width, height, scale);
        for em in [4_u32, 8, 16, 32] {
            let per_row = 1800 / em;
            let size = f64::from(em);
            let glyphs: Vec<MathGlyph> = (0..5000)
                .map(|i| MathGlyph {
                    id: x,
                    x: 10.0 + f64::from(i % per_row) * size * 0.55,
                    y: 20.0 + f64::from(i / per_row) * size * 1.2,
                })
                .collect();
            let one = layout(vec![run(0, size, glyphs.clone())]);
            let each = layout(glyphs.iter().map(|g| run(0, size, vec![*g])).collect());
            let em_px = size * scale;
            row(
                &mut renderer,
                &format!("{width}x{height} 5000 x em {em_px} one run"),
                &one,
                &vp,
            );
            row(
                &mut renderer,
                &format!("{width}x{height} 5000 x em {em_px} a run each"),
                &each,
                &vp,
            );
        }
    }
}

/// What estimating alone costs, per glyph or path element looked at, on screen and off.
#[test]
#[ignore = "timing only; run in release with --ignored --nocapture"]
fn calibrate_estimating() {
    let mut renderer = Renderer::new_trusted();
    let vp = pixel_viewport(1920, 1080, 1.0);
    let clip = draw::clip_rect(&vp);
    let n: u32 = 25_000;
    let x = glyph_id(0, 'x');
    let on = zigzag(n, 1920.0, 1080.0);
    let mut off = on.clone();
    off.apply_affine(Affine::translate((1e5, 0.0)));
    let count = usize::try_from(n).unwrap();
    for (name, layout) in [
        (
            "glyphs on screen",
            pile(0, x, 20.0, count, Point::new(960.0, 540.0)),
        ),
        (
            "glyphs off screen",
            pile(0, x, 20.0, count, Point::new(-1e5, 540.0)),
        ),
        ("elements on screen", paths(vec![on], Some(BLACK), None)),
        ("elements off screen", paths(vec![off], Some(BLACK), None)),
    ] {
        let took = median(9, || {
            std::hint::black_box(renderer.math_cost(
                Point::ZERO,
                &layout,
                &vp,
                clip,
                f64::INFINITY,
            ));
        });
        let each = took.as_secs_f64() * 1e9 / f64::from(n);
        println!("estimating {name}: {each:.1} ns each");
    }
}

/// Real hostile formulas, 200 glyphs or elements deep, at the views the bench uses: fitted, and
/// at zoom 20 around the middle of their bounds.
#[test]
#[ignore = "timing only; run in release with --ignored --nocapture"]
fn calibrate_formulas() {
    let mut renderer = Renderer::new_trusted();
    let math = MathEngine::new().with_timeout(Duration::from_secs(60));
    let polygon = "..range(200).map(i => (calc.rem(i, 2) * 300pt, calc.rem(i, 3) * 300pt))";
    for src in [
        "#for i in range(200) { box(width: 0pt)[x] }".to_owned(),
        "#for i in range(200) { box(width: 0pt, text(size: 100pt)[W]) }".to_owned(),
        "#for i in range(200) { box(width: 0pt, text(fill: rgb(0, 0, 0, 20))[x]) }".to_owned(),
        r#"#text(tracking: -0.45em, "x" * 2000)"#.to_owned(),
        r#"#range(30).map(_ => "x" * 30).join(linebreak())"#.to_owned(),
        format!("#polygon(stroke: 100pt, {polygon})"),
        format!("#polygon(fill: black, {polygon})"),
        format!("#polygon(stroke: 2000pt, {polygon})"),
        "#for i in range(200) { box(width: 0pt, line(length: 300pt, angle: 45deg, stroke: 100pt)) }"
            .to_owned(),
        "#for i in range(200) { box(width: 0pt, line(length: 300pt, angle: 45deg, stroke: 100pt + rgb(0, 0, 0, 20))) }"
            .to_owned(),
        "#for i in range(200) { box(width: 0pt, rect(width: 300pt, height: 300pt, fill: rgb(0, 0, 0, 20))) }"
            .to_owned(),
    ] {
        let layout = math.layout(&src).unwrap();
        let bounds = math_to_board(Point::ZERO).transform_rect_bbox(layout_bounds(&layout));
        let short: String = src.chars().take(48).collect();
        for (width, height, scale) in [(1920, 1080, 1.0), (2560, 1600, 2.0), (2880, 1800, 2.0)] {
            let fit = Viewport::fit(bounds, width, height, scale);
            let zoom = Viewport::new(bounds.center(), 20.0, width, height, scale);
            for (label, vp) in [("fit", fit), ("zoom20", zoom)] {
                row(&mut renderer, &format!("{width}x{height} {label} {short}"), &layout, &vp);
            }
        }
    }
}
