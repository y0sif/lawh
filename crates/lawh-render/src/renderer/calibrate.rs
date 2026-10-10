//! Estimated against measured costs, for choosing the constants in `budget.rs`.
//!
//! Each test draws (or lays out) one kind of thing at many sizes and prints the estimate, the
//! measured time (past an empty frame, for drawing) and their ratio. Estimates lean high, so the
//! ratio stays under 1 where the cost is high. In a release build each test then asserts that no
//! row took more than [`MAX_RATIO`] times its estimate, give or take [`SLACK_MS`] of timer and
//! frame noise; a debug build is far slower and only prints. Ignored by default; run in release
//! (and on a quiet machine):
//! `nice -n 19 cargo test --release -p lawh-render --lib -- --ignored --nocapture calibrate`

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use std::hint::black_box;
use std::time::{Duration, Instant};

use kurbo::{Affine, BezPath, Circle, Point, Rect, Shape as _};
use lawh_core::board::{MAX_BOARD_TEXT_CHARS, MAX_OBJECTS, MAX_TEXT_CHARS, MAX_TEXT_LINES};
use lawh_core::{Board, Object, ObjectId, Shape};
use lawh_math::{MathEngine, MathGlyph, MathItem, MathLayout};

use super::{
    INK, MATH_SCALE, MathWait, Renderer, layout_bounds, math_to_board, text_box, text_cost,
    text_view,
};
use crate::budget::{self, Budget};
use crate::glyphs::{GlyphStats, GlyphTable};
use crate::text::TextShaper;
use crate::{Viewport, draw};

const BLACK: [u8; 4] = [0, 0, 0, 255];
const TRANSLUCENT: [u8; 4] = [0, 0, 0, 40];
const SCREENS: [(u16, u16, f64); 2] = [(1920, 1080, 1.0), (2880, 1800, 2.0)];

/// Most a row may take, as a multiple of its estimate, in release.
const MAX_RATIO: f64 = 1.2;
/// Noise allowed on top of [`MAX_RATIO`]: timer resolution, and the empty frame subtracted from
/// each measurement varying from run to run.
const SLACK_MS: f64 = 0.2;

/// Estimated and measured costs, printed as they come and checked at the end.
#[derive(Default)]
struct Rows {
    count: usize,
    over: Vec<String>,
    worst: f64,
}

impl Rows {
    fn add(&mut self, name: &str, estimate_ms: f64, took_ms: f64) {
        let ratio = took_ms / estimate_ms;
        println!(
            "{name:<64} est {estimate_ms:>9.3} ms  took {took_ms:>9.3} ms  ratio {ratio:>5.2}"
        );
        self.count += 1;
        if estimate_ms >= 1.0 {
            self.worst = self.worst.max(ratio);
        }
        if took_ms > MAX_RATIO * estimate_ms + SLACK_MS {
            self.over.push(format!(
                "{name}: est {estimate_ms:.3} ms, took {took_ms:.3} ms"
            ));
        }
    }

    /// In release, asserts that no row took more than [`MAX_RATIO`] times its estimate plus
    /// [`SLACK_MS`].
    fn check(self) {
        println!(
            "{} rows; worst ratio where the estimate is 1 ms or more: {:.2}",
            self.count, self.worst
        );
        if cfg!(debug_assertions) {
            println!("debug build: not checked");
            return;
        }
        assert!(
            self.over.is_empty(),
            "{} of {} rows took more than {MAX_RATIO} times their estimate + {SLACK_MS} ms:\n{}",
            self.over.len(),
            self.count,
            self.over.join("\n")
        );
    }
}

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

/// The median time of an empty frame through `vp`: the paper and the final render.
fn empty_frame(renderer: &mut Renderer, vp: &Viewport) -> Duration {
    median(9, || {
        renderer.begin_frame(vp);
        black_box(renderer.finish_frame());
    })
}

/// Adds the estimate and the measured time, past an empty frame, of drawing `layout` at the
/// board origin through `vp`.
fn row(renderer: &mut Renderer, rows: &mut Rows, name: &str, layout: &MathLayout, vp: &Viewport) {
    let clip = draw::clip_rect(vp);
    let estimate = renderer
        .math_cost(Point::ZERO, layout, vp, clip, f64::INFINITY)
        .ns
        / 1e6;
    let empty = empty_frame(renderer, vp);
    let took = median(5, || {
        renderer.begin_frame(vp);
        let mut budget = Budget::unlimited();
        assert!(renderer.draw_math(Point::ZERO, layout, vp, clip, &mut budget));
        black_box(renderer.finish_frame());
    });
    rows.add(name, estimate, ms(took.saturating_sub(empty)));
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
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
    let mut rows = Rows::default();
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
                row(
                    &mut renderer,
                    &mut rows,
                    &label,
                    &pile(*font, *id, em, count, at),
                    &vp,
                );
            }
        }
    }
    rows.check();
}

/// Fills and strokes from 2 to 20,000 pixels wide, opaque and translucent: screen-crossing
/// zigzags, and circles from 3 to 30,000 pixels in radius stacked at the centre.
#[test]
#[ignore = "timing only; run in release with --ignored --nocapture"]
fn calibrate_paths() {
    let mut rows = Rows::default();
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
                &mut rows,
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
                    &mut rows,
                    &name,
                    &paths(vec![circle; count], fill, stroke),
                    &vp,
                );
            }
        }
    }
    rows.check();
}

/// The same 5,000 glyphs as one run and as a run each: what setting up a run costs.
#[test]
#[ignore = "timing only; run in release with --ignored --nocapture"]
fn calibrate_runs() {
    let mut rows = Rows::default();
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
                &mut rows,
                &format!("{width}x{height} 5000 x em {em_px} one run"),
                &one,
                &vp,
            );
            row(
                &mut renderer,
                &mut rows,
                &format!("{width}x{height} 5000 x em {em_px} a run each"),
                &each,
                &vp,
            );
        }
    }
    rows.check();
}

/// What estimating alone costs, per glyph or path element looked at, on screen and off, per
/// chunk of a text's plan, and per byte of text hashed and looked up: against what the budget
/// charges for looking at an object that then isn't drawn.
#[test]
#[ignore = "timing only; run in release with --ignored --nocapture"]
fn calibrate_estimating() {
    let mut rows = Rows::default();
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
        let examined = renderer
            .math_cost(Point::ZERO, &layout, &vp, clip, f64::INFINITY)
            .examined();
        let took = median(9, || {
            black_box(renderer.math_cost(Point::ZERO, &layout, &vp, clip, f64::INFINITY));
        });
        rows.add(&format!("estimating {n} {name}"), examined / 1e6, ms(took));
    }
    // Text plans: a chunk a line, and long lines of many chunks.
    for (name, per_line) in [
        ("max lines", max_lines_per_line(MAX_TEXT_CHARS)),
        ("one line", usize::MAX),
    ] {
        let text = cycled(0, MAX_TEXT_CHARS, per_line, WORDS);
        let key = renderer.text.key(&text);
        renderer.text.lay_out(key, &text, &mut renderer.glyphs);
        let vp = Viewport::new(Point::new(5.0, 5.0), 0.3, 1920, 1080, 1.0);
        let view = text_view(Point::ZERO, &vp, clip).unwrap();
        let plan = &renderer.text.shaped(key, &text).unwrap().plan;
        let lines = view.lines(plan);
        let chunks = plan.chunks(lines.clone()).len();
        // What the budget charges for looking at every chunk.
        let (cost, _) = text_cost(plan, lines.clone(), &view, &vp, clip, f64::INFINITY);
        let examined = cost.examined();
        let took = median(9, || {
            black_box(text_cost(
                plan,
                lines.clone(),
                &view,
                &vp,
                clip,
                f64::INFINITY,
            ));
        });
        rows.add(
            &format!("estimating text, {name}: {chunks} chunks"),
            examined / 1e6,
            ms(took),
        );
    }
    // A board's worth of text, hashed once and looked up three times as a frame does.
    let text = cycled(0, MAX_BOARD_TEXT_CHARS, 75, "aα漢 ");
    let took = median(9, || {
        let key = renderer.text.key(&text);
        for _ in 0..3 {
            black_box(renderer.text.known(key, &text));
        }
    });
    rows.add(
        &format!("hashing and looking up {} bytes of text", text.len()),
        budget::hashing(text.len()) / 1e6,
        ms(took),
    );
    rows.check();
}

/// Real hostile formulas, 200 glyphs or elements deep, at the views the bench uses: fitted, and
/// at zoom 20 around the middle of their bounds.
#[test]
#[ignore = "timing only; run in release with --ignored --nocapture"]
fn calibrate_formulas() {
    let mut rows = Rows::default();
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
                row(
                    &mut renderer,
                    &mut rows,
                    &format!("{width}x{height} {label} {short}"),
                    &layout,
                    &vp,
                );
            }
        }
    }
    rows.check();
}

/// Words for the text calibrations.
const WORDS: &str = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor ";

/// Kinds of text, as the alphabets they cycle through: the costliest ones lawh-core's text caps
/// were measured with (`text_caps_worst_cases` in `tests/bench.rs`) and plainer ones.
const TEXT_KINDS: [(&str, &str); 12] = [
    ("words", WORDS),
    ("one word", "x"),
    ("digits and punctuation", "1,2.3;4:5!6?"),
    ("spaces", " "),
    ("cjk", "漢字仮名交じり文"),
    (
        "combining",
        "a\u{301}\u{302}\u{303}\u{304}\u{305}\u{306}\u{307}",
    ),
    ("bidi", "ab אב "),
    ("bidi per char", "aא"),
    ("scripts per char", "aαа漢ا"),
    ("arabic", "سلام عليكم "),
    ("emoji", "👨\u{200D}👩\u{200D}👧\u{FE0F}"),
    // Lines of nothing but their tag: every character a line break.
    ("newlines", "\n"),
];

/// `n` characters (line breaks included) cycling through `alphabet`, in lines of about
/// `per_line` characters, starting with `tag` so no two texts are the same. An alphabet of line
/// breaks makes a line per character, as many as a text may have.
fn cycled(tag: usize, n: usize, per_line: usize, alphabet: &str) -> String {
    let mut out = format!("{tag} ");
    if alphabet == "\n" {
        let breaks = n
            .min(MAX_TEXT_LINES - 1)
            .saturating_sub(out.chars().count());
        out.push_str(&"\n".repeat(breaks));
        return out;
    }
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

/// Characters a line for a text of `n` characters to have as many lines as a text may.
fn max_lines_per_line(n: usize) -> usize {
    n.div_ceil(MAX_TEXT_LINES - 1).max(2)
}

/// The ways [`TextKinds`] are broken into lines: as one line, in lines of 75 characters, and in
/// as many lines as a text may have.
fn text_shapes(n: usize) -> [(&'static str, usize); 3] {
    [
        ("one line", usize::MAX),
        ("75 a line", 75),
        ("max lines", max_lines_per_line(n)),
    ]
}

/// Resident memory of this process in bytes (Linux); 0 where it can't be read.
fn rss_bytes() -> f64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find(|line| line.starts_with("VmRSS:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|kb| kb.parse::<f64>().ok())
        .map_or(0.0, |kb| kb * 1024.0)
}

/// Laying out text: every kind and shape at several lengths up to the cap, each text new to the
/// cache, against `budget::layout`.
#[test]
#[ignore = "timing only; run in release with --ignored --nocapture"]
fn calibrate_text_layout() {
    let mut rows = Rows::default();
    let mut glyphs = GlyphTable::new();
    let mut tag = 0;
    for (kind, alphabet) in TEXT_KINDS {
        for n in [100, 1000, MAX_TEXT_CHARS] {
            for (shape, per_line) in text_shapes(n) {
                if alphabet == "\n" && shape != "one line" {
                    continue;
                }
                let mut shaper = TextShaper::new();
                let mut texts: Vec<String> = (0..9)
                    .map(|_| {
                        tag += 1;
                        cycled(tag, n, per_line, alphabet)
                    })
                    .collect();
                let known = {
                    let key = shaper.key(&texts[0]);
                    shaper.known(key, &texts[0])
                };
                let facts = known.facts;
                let estimate =
                    budget::layout(facts.chars, facts.non_ascii, facts.changes, facts.lines) / 1e6;
                // Two to warm up, then the median of seven.
                let mut times: Vec<Duration> = texts
                    .drain(..)
                    .map(|text| {
                        let key = shaper.key(&text);
                        let t = Instant::now();
                        black_box(shaper.lay_out(key, &text, &mut glyphs));
                        t.elapsed()
                    })
                    .skip(2)
                    .collect();
                times.sort();
                rows.add(
                    &format!(
                        "lay out {kind}, {shape}: {} chars {} lines",
                        facts.chars, facts.lines
                    ),
                    estimate,
                    ms(times[times.len() / 2]),
                );
            }
        }
    }
    rows.check();
}

/// For each kind of text, a board of it at the text caps laid out in a new cache: its resident
/// memory against the cache's estimate. Prints only. The allocator keeps memory it got for
/// earlier kinds, so run one kind per process for real numbers: `LAWH_CAL_KINDS` picks kinds by
/// name prefix.
/// `for k in words cjk bidi scripts newlines; do LAWH_CAL_KINDS=$k nice -n 19 cargo test --release -p lawh-render --lib -- --ignored --nocapture calibrate_text_memory; done`
#[test]
#[ignore = "memory only; run in release with --ignored --nocapture"]
fn calibrate_text_memory() {
    let kinds = std::env::var("LAWH_CAL_KINDS").unwrap_or_default();
    let mut glyphs = GlyphTable::new();
    for (kind, alphabet) in TEXT_KINDS {
        if !kinds.is_empty() && !kinds.split(',').any(|k| kind.starts_with(k)) {
            continue;
        }
        let (per_text, per_line) = if alphabet == "\n" {
            (MAX_TEXT_LINES - 1, usize::MAX)
        } else {
            (MAX_TEXT_CHARS, max_lines_per_line(MAX_TEXT_CHARS))
        };
        let count = (MAX_BOARD_TEXT_CHARS / per_text).min(MAX_OBJECTS);
        let texts: Vec<String> = (0..count)
            .map(|i| cycled(i, per_text, per_line, alphabet))
            .collect();
        let mut shaper = TextShaper::new();
        let before = rss_bytes();
        for text in &texts {
            let key = shaper.key(text);
            shaper.lay_out(key, text, &mut glyphs);
        }
        let measured = rss_bytes() - before;
        let chars: usize = texts.iter().map(|text| text.chars().count()).sum();
        println!(
            "memory of {kind:<24} {count} x {per_text}: {chars} chars, measured {:.1} MB, estimated {:.1} MB",
            measured / 1e6,
            shaper.bytes() as f64 / 1e6
        );
    }
}

/// The views the text calibrations draw through at `width` x `height` and `scale`, for text
/// whose bounds are `bounds`: fitted, at zoom 1 and 20 with the text's top-left corner near the
/// image's, at zoom 20 around the middle of the text, and at zoom 1 with the end of its longest
/// line near the image's left edge (the line is walked whole for a few glyphs).
fn text_views(bounds: Rect, width: u16, height: u16, scale: f64) -> [(&'static str, Viewport); 5] {
    let corner = |zoom: f64| {
        let unit = Viewport::new(Point::ZERO, zoom, width, height, scale).px_per_unit();
        let center = Point::new(
            f64::from(width) / 2.0 / unit - 1.0,
            f64::from(height) / 2.0 / unit - 1.0,
        );
        Viewport::new(center, zoom, width, height, scale)
    };
    [
        ("fit", Viewport::fit(bounds, width, height, scale)),
        ("corner z1", corner(1.0)),
        ("corner z20", corner(20.0)),
        (
            "middle z20",
            Viewport::new(bounds.center(), 20.0, width, height, scale),
        ),
        ("line end z1", {
            let vp = corner(1.0);
            let unit = vp.px_per_unit();
            Viewport {
                center: Point::new(bounds.x1 + f64::from(width) / 2.0 / unit - 1.0, vp.center.y),
                ..vp
            }
        }),
    ]
}

/// Whether `name` starts with one of the comma-separated prefixes in `LAWH_CAL_KINDS` (or it is
/// unset).
fn picked(name: &str) -> bool {
    let kinds = std::env::var("LAWH_CAL_KINDS").unwrap_or_default();
    kinds.is_empty() || kinds.split(',').any(|k| name.starts_with(k))
}

/// Drawing text: one text of each kind and shape at the cap, then boards of each kind at the
/// text caps (the bench's worst text boards), at the views the bench uses. One text is drawn
/// with `draw_text` and an unlimited budget, against `text_cost`; a board is drawn as an export
/// frame (nothing over its budget) once its text is laid out, against the frame's estimate.
/// `LAWH_CAL_KINDS` picks kinds (and boards, by their name) by prefix.
#[test]
#[ignore = "timing only; run in release with --ignored --nocapture"]
fn calibrate_text_drawing() {
    let mut rows = Rows::default();
    let mut renderer = Renderer::new_trusted();
    for (kind, alphabet) in TEXT_KINDS {
        if !picked(kind) {
            continue;
        }
        for (shape, per_line) in text_shapes(MAX_TEXT_CHARS) {
            if alphabet == "\n" && shape != "one line" {
                continue;
            }
            let text = cycled(0, MAX_TEXT_CHARS, per_line, alphabet);
            let key = renderer.text.key(&text);
            let (w, h) = renderer.text.lay_out(key, &text, &mut renderer.glyphs);
            let bounds = text_box(Point::ZERO, w, h);
            for (width, height, scale) in SCREENS {
                for (label, vp) in text_views(bounds, width, height, scale) {
                    let clip = draw::clip_rect(&vp);
                    let Some(view) = text_view(Point::ZERO, &vp, clip) else {
                        continue;
                    };
                    let plan = &renderer.text.shaped(key, &text).unwrap().plan;
                    let (cost, _) =
                        text_cost(plan, view.lines(plan), &view, &vp, clip, f64::INFINITY);
                    let estimate = cost.ns / 1e6;
                    let empty = empty_frame(&mut renderer, &vp);
                    let took = median(5, || {
                        renderer.begin_frame(&vp);
                        let mut budget = Budget::unlimited();
                        renderer.draw_text(Point::ZERO, key, &text, INK, &vp, clip, &mut budget);
                        black_box(renderer.finish_frame());
                    });
                    rows.add(
                        &format!("{width}x{height} {label} {kind}, {shape}"),
                        estimate,
                        ms(took.saturating_sub(empty)),
                    );
                }
            }
        }
    }
    for (name, board) in text_boards() {
        if !picked(&name) {
            continue;
        }
        let mut renderer = Renderer::new_trusted();
        let bounds = renderer.board_bounds(&board).unwrap();
        for (width, height, scale) in SCREENS {
            for (label, vp) in text_views(bounds, width, height, scale) {
                let empty = empty_frame(&mut renderer, &vp);
                let took = median(5, || {
                    black_box(renderer.draw_frame(&board, &vp, MathWait::Yes));
                });
                let stats = renderer.last_frame();
                assert_eq!(
                    (stats.downgraded, stats.laid_out, stats.text_pending),
                    (0, 0, 0),
                    "{name} {label}: {stats:?}"
                );
                rows.add(
                    &format!("{width}x{height} {label} board {name}"),
                    ms(stats.estimated),
                    ms(took.saturating_sub(empty)),
                );
            }
        }
    }
    rows.check();
}

/// Boards of each text kind at the text caps, stacked at the origin: as many texts of the most
/// characters (in the most lines) as the board may hold, and 500 texts of 80 characters of words.
fn text_boards() -> Vec<(String, Board)> {
    let board = |texts: Vec<String>| {
        let mut board = Board::new();
        for (i, text) in texts.into_iter().enumerate() {
            board
                .insert(Object::new(
                    ObjectId::new(format!("t{i}")).unwrap(),
                    Shape::Text {
                        at: Point::ZERO,
                        text,
                    },
                ))
                .unwrap();
        }
        board
    };
    let mut boards = Vec::new();
    for (kind, alphabet) in TEXT_KINDS {
        let (per_text, per_line) = if alphabet == "\n" {
            (MAX_TEXT_LINES - 1, usize::MAX)
        } else {
            (MAX_TEXT_CHARS, max_lines_per_line(MAX_TEXT_CHARS))
        };
        let count = (MAX_BOARD_TEXT_CHARS / per_text).min(MAX_OBJECTS);
        let texts = (0..count)
            .map(|i| cycled(i, per_text, per_line, alphabet))
            .collect();
        boards.push((format!("{kind} {count}x{per_text}"), board(texts)));
    }
    let per_text = MAX_BOARD_TEXT_CHARS / MAX_OBJECTS;
    let texts = (0..MAX_OBJECTS)
        .map(|i| cycled(i, per_text, usize::MAX, WORDS))
        .collect();
    boards.push((
        format!("small words {MAX_OBJECTS}x{per_text}"),
        board(texts),
    ));
    boards
}
