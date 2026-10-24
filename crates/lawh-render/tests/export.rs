//! Exports with the real renderer: what is framed and drawn, the PNG's format, and every cap.
//!
//! The `export_*` goldens in `tests/golden/` are whole exports, compared as the window's goldens
//! are (`tests/golden.rs`): at most [`MAX_DIFFERENT_PER_MILLION`] pixels in a million may differ
//! by more than [`CHANNEL_TOLERANCE`] in a channel. `LAWH_UPDATE_GOLDEN=1 cargo test -p
//! lawh-render --test export` rewrites them; look at every rewritten image before committing it.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use std::fmt::Write as _;
use std::io::Cursor;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use lawh_core::{Board, ExportBackground, ExportFormat, ExportOptions, ExportReport, ObjectId};
use lawh_render::{
    EXPORT_ARROW_HEAD_UNITS, EXPORT_MARGIN_UNITS, EXPORT_STROKE_UNITS, ExportError, ExportLimits,
    Exported, MAX_EXPORT_PIXELS, MAX_EXPORT_SIDE_PX, Palette, Renderer, Viewport,
};
use vello_cpu::Pixmap;

/// Largest per-channel difference that still counts as the same pixel.
const CHANNEL_TOLERANCE: u8 = 3;
/// Most pixels in a million that may differ by more than [`CHANNEL_TOLERANCE`].
const MAX_DIFFERENT_PER_MILLION: usize = 100;

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join(format!("{name}.png"))
}

/// Compares `png` with the golden `name`, or rewrites it with `LAWH_UPDATE_GOLDEN=1`.
fn check_golden(name: &str, png: &[u8]) {
    let path = golden_path(name);
    if std::env::var_os("LAWH_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        std::fs::write(&path, png).unwrap();
        return;
    }
    let expected = std::fs::read(&path).unwrap_or_else(|err| {
        panic!(
            "no golden image at {} ({err}); run with LAWH_UPDATE_GOLDEN=1 and review it",
            path.display()
        )
    });
    let expected = Pixmap::from_png(Cursor::new(expected)).unwrap();
    let actual = Pixmap::from_png(Cursor::new(png)).unwrap();
    assert_eq!(
        (actual.width(), actual.height()),
        (expected.width(), expected.height()),
        "{name}: image size changed"
    );
    let different = actual
        .data()
        .iter()
        .zip(expected.data())
        .filter(|(a, e)| {
            [(a.r, e.r), (a.g, e.g), (a.b, e.b), (a.a, e.a)]
                .iter()
                .any(|(x, y)| x.abs_diff(*y) > CHANNEL_TOLERANCE)
        })
        .count();
    let total = actual.data().len();
    let allowed = total * MAX_DIFFERENT_PER_MILLION / 1_000_000;
    if different > allowed {
        let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.actual.png"));
        std::fs::write(&out, png).unwrap();
        panic!(
            "{name}: {different} of {total} pixels differ from {} by more than \
             {CHANNEL_TOLERANCE} (at most {allowed} may); actual image written to {}",
            path.display(),
            out.display()
        );
    }
}

fn png_options() -> ExportOptions {
    ExportOptions::default()
}

fn scaled(hundredths: u32) -> ExportOptions {
    ExportOptions {
        png_scale_hundredths: hundredths,
        ..ExportOptions::default()
    }
}

fn export(board: &Board) -> Exported {
    Renderer::new_trusted()
        .export(board, None, &png_options())
        .unwrap()
}

fn export_with(board: &Board, limits: &ExportLimits) -> Result<Exported, ExportError> {
    Renderer::new_trusted().export_with_limits(board, None, &png_options(), limits)
}

/// A PNG decoded with the `png` crate: its size, colour type, whether it has an sRGB chunk, and
/// its pixels as they are in the file (RGB or RGBA, 8 bits a channel).
struct Decoded {
    width: u32,
    height: u32,
    color: png::ColorType,
    srgb: bool,
    data: Vec<u8>,
}

impl Decoded {
    fn channels(&self) -> usize {
        match self.color {
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            other => panic!("unexpected colour type {other:?}"),
        }
    }

    /// Every pixel as RGBA.
    fn pixels(&self) -> impl Iterator<Item = [u8; 4]> + '_ {
        let channels = self.channels();
        self.data.chunks_exact(channels).map(move |px| {
            if channels == 3 {
                [px[0], px[1], px[2], 255]
            } else {
                [px[0], px[1], px[2], px[3]]
            }
        })
    }

    fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let channels = self.channels();
        let i = (y as usize * self.width as usize + x as usize) * channels;
        let px = &self.data[i..i + channels];
        if channels == 3 {
            [px[0], px[1], px[2], 255]
        } else {
            [px[0], px[1], px[2], px[3]]
        }
    }

    fn count(&self, what: impl Fn([u8; 4]) -> bool) -> usize {
        self.pixels().filter(|px| what(*px)).count()
    }
}

fn decode(bytes: &[u8]) -> Decoded {
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder.read_info().unwrap();
    let info = reader.info();
    let (width, height, color, srgb) = (
        info.width,
        info.height,
        info.color_type,
        info.srgb.is_some(),
    );
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    let mut data = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut data).unwrap();
    data.truncate(frame.buffer_size());
    Decoded {
        width,
        height,
        color,
        srgb,
        data,
    }
}

fn paper() -> [u8; 4] {
    Palette::LIGHT.paper.to_rgba8().to_u8_array()
}

/// Red and dark: the error colour, not the ink.
fn is_red(px: [u8; 4]) -> bool {
    px[3] > 200 && px[0] > 150 && px[1] < 110 && px[2] < 110
}

/// Dark and grey: the ink.
fn is_ink(px: [u8; 4]) -> bool {
    px[3] > 200 && px[0] < 100 && px[0].abs_diff(px[1]) < 10 && px[1].abs_diff(px[2]) < 10
}

/// A pixel position, rounded.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "positions inside a small image"
)]
fn whole(px: f64) -> u32 {
    px.round() as u32
}

fn id(name: &str) -> ObjectId {
    ObjectId::new(name).unwrap()
}

/// Text at three sizes, a formula beside text at the same `y` (its baseline on the text's), a
/// formula at size 2, symbols Inter lacks (set in Libertinus Math, as `:sym` puts them in
/// text), a circle, a line and an arrow.
fn mixed_board() -> Board {
    common::board(
        "lawh version=1\n\
         text id=t1 x=0 y=0 size=0.5 \"Size 0.5: x\"\n\
         text id=t2 x=0 y=1 \"Size 1: x =\"\n\
         math id=m1 x=3.5 y=1 \"x = pi r^2\"\n\
         text id=t3 x=0 y=2.5 size=2 \"p \u{2227} q, \u{2220}ABC\"\n\
         math id=m2 x=0 y=5 size=2 \"sum_(i=1)^n i = (n(n+1))/2\"\n\
         ellipse id=e1 x=12 y=2 rx=1.5 ry=1\n\
         line id=l1 x1=10 y1=5 x2=14 y2=5\n\
         arrow id=a1 x1=10 y1=6 x2=14 y2=7\n",
    )
}

#[test]
fn the_area_board_exports_as_its_golden() {
    let exported = export(&common::area_board());
    assert_eq!(exported.report, ExportReport::default());
    check_golden("export_area", &exported.bytes);
}

#[test]
fn sizes_math_and_symbols_export_as_their_golden() {
    let exported = export(&mixed_board());
    assert_eq!(exported.report, ExportReport::default());
    check_golden("export_mixed", &exported.bytes);
}

/// The area board framed with its margin at 80 pixels a unit: the circle reaches from -3 to 3
/// (and half a stroke further), the formula and title further right and up. Paper all around,
/// with no grid dots: every pixel of the margin is exactly the paper colour.
#[test]
fn exports_frame_every_object_with_a_margin_and_no_grid() {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    renderer.resolve_math(&board);
    let objects = renderer.board_bounds(&board).unwrap();
    let exported = renderer.export(&board, None, &png_options()).unwrap();
    let image = decode(&exported.bytes);
    assert!(image.srgb, "no sRGB chunk");
    assert_eq!(image.color, png::ColorType::Rgb, "opaque, so RGB");
    // The board's bounds, half the circle's stroke past its left edge (the formula is
    // rightmost, the title topmost), and the margin, at 80 px a unit.
    let half = EXPORT_STROKE_UNITS / 2.0;
    let left_edge = objects.x0 - half - EXPORT_MARGIN_UNITS;
    let top_edge = objects.y0 - EXPORT_MARGIN_UNITS;
    let width = (objects.width() + half + 2.0 * EXPORT_MARGIN_UNITS) * 80.0;
    let height = (objects.height() + half + 2.0 * EXPORT_MARGIN_UNITS) * 80.0;
    assert!(
        (f64::from(image.width) - width).abs() <= 1.0
            && (f64::from(image.height) - height).abs() <= 1.0,
        "{}x{} px, expected about {width}x{height}",
        image.width,
        image.height
    );
    // The margin, less a pixel for anti-aliasing, is paper on every side.
    let margin = 79;
    let edge = (0..image.width)
        .flat_map(|x| (0..margin).flat_map(move |y| [(x, y), (x, image.height - 1 - y)]))
        .chain(
            (0..image.height)
                .flat_map(|y| (0..margin).flat_map(move |x| [(x, y), (image.width - 1 - x, y)])),
        );
    for (x, y) in edge {
        assert_eq!(image.at(x, y), paper(), "at {x}, {y}");
    }
    // Ink inside: the circle's leftmost point, at y = 0, half its stroke in from the margin.
    let row = whole((0.0 - top_edge) * 80.0);
    let left = whole((-3.0 - left_edge) * 80.0);
    assert!(
        (left - 2..=left + 2).any(|x| is_ink(image.at(x, row))),
        "no circle at {left}, {row}"
    );
    // Only paper and ink and their blends: no other colour, no grid grey.
    assert!(image.pixels().all(|px| px[0].abs_diff(px[2]) <= 3));
}

#[test]
fn a_transparent_export_has_no_paper() {
    let options = ExportOptions {
        background: ExportBackground::None,
        ..ExportOptions::default()
    };
    let opaque = export(&common::area_board());
    let exported = Renderer::new_trusted()
        .export(&common::area_board(), None, &options)
        .unwrap();
    let image = decode(&exported.bytes);
    let opaque = decode(&opaque.bytes);
    assert_eq!((image.width, image.height), (opaque.width, opaque.height));
    assert_eq!(image.color, png::ColorType::Rgba);
    assert!(image.srgb);
    assert_eq!(image.at(0, 0), [0, 0, 0, 0]);
    let clear = image.count(|px| px[3] == 0);
    let all = (image.width * image.height) as usize;
    assert!(clear > all * 9 / 10, "{clear} clear pixels of {all}");
    // The ink is there, opaque, where the paper export has it in full (its anti-aliased edges
    // are see-through).
    let inked: Vec<usize> = opaque
        .pixels()
        .enumerate()
        .filter(|(_, px)| px[0] < 40)
        .map(|(i, _)| i)
        .collect();
    assert!(inked.len() > 500, "{}", inked.len());
    let alpha: Vec<u8> = image.pixels().map(|px| px[3]).collect();
    assert!(inked.iter().all(|&i| alpha[i] > 220));
}

#[test]
fn a_selection_exports_only_its_objects_framed_to_them() {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    // The circle alone: 6 units across with its stroke, and the margin.
    let circle = renderer
        .export(&board, Some(&[id("c1")]), &png_options())
        .unwrap();
    let image = decode(&circle.bytes);
    let side = (6.0 + EXPORT_STROKE_UNITS + 2.0 * EXPORT_MARGIN_UNITS) * 80.0;
    for px in [image.width, image.height] {
        assert!(
            (f64::from(px) - side).abs() <= 1.0,
            "{px} px, expected {side}"
        );
    }
    // A circle and nothing else: its centre, where the line and the "r" are on the board, is
    // paper.
    let (cx, cy) = (image.width / 2, image.height / 2);
    for dx in 0..80 {
        assert_eq!(
            image.at(cx + dx, cy),
            paper(),
            "{dx} px right of the centre"
        );
    }
    // Its leftmost point is half a stroke (2 px) past the margin.
    assert!((80..=84).any(|x| is_ink(image.at(x, cy))));

    // The formula and the line, in board order whatever the selection's order: framed to both.
    let whole = decode(&renderer.export(&board, None, &png_options()).unwrap().bytes);
    let two = decode(
        &renderer
            .export(&board, Some(&[id("m1"), id("l1")]), &png_options())
            .unwrap()
            .bytes,
    );
    assert!(two.width < whole.width && two.height < whole.height);
    // An id the board doesn't have is left out.
    let same = decode(
        &renderer
            .export(
                &board,
                Some(&[id("m1"), id("zz"), id("l1")]),
                &png_options(),
            )
            .unwrap()
            .bytes,
    );
    assert_eq!(same.data, two.data);
}

#[test]
fn nothing_to_export_is_an_error() {
    let mut renderer = Renderer::new_trusted();
    let options = png_options();
    assert_eq!(
        renderer.export(&Board::new(), None, &options),
        Err(ExportError::Empty)
    );
    let board = common::area_board();
    assert_eq!(
        renderer.export(&board, Some(&[]), &options),
        Err(ExportError::Empty)
    );
    assert_eq!(
        renderer.export(&board, Some(&[id("nope")]), &options),
        Err(ExportError::Empty)
    );
    assert_eq!(ExportError::Empty.to_string(), "nothing to export");
}

#[test]
fn the_scale_sets_the_pixels_a_unit_within_its_range() {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    let mut size = |hundredths| {
        let exported = renderer.export(&board, None, &scaled(hundredths)).unwrap();
        assert_eq!(exported.report.shrunk, None);
        let image = decode(&exported.bytes);
        (image.width, image.height)
    };
    let (w1, h1) = size(100);
    let (w2, h2) = size(200);
    assert!(
        w2.abs_diff(2 * w1) <= 2 && h2.abs_diff(2 * h1) <= 2,
        "{w1}x{h1} {w2}x{h2}"
    );
    // Scales outside 0.25 to 8 are kept inside.
    assert_eq!(size(0), size(25));
    assert_eq!(size(10_000), size(800));
    let (w8, _) = size(800);
    assert!(w8.abs_diff(8 * w1) <= 8, "{w1} {w8}");
}

#[test]
fn boards_too_large_shrink_to_fit_both_caps() {
    let board = common::limits_board();
    let exported = export(&board);
    let shrunk = exported.report.shrunk.expect("shrunk");
    assert!(shrunk > 0.0 && shrunk < 1e-3, "{shrunk}");
    let image = decode(&exported.bytes);
    assert!(image.width <= MAX_EXPORT_SIDE_PX && image.height <= MAX_EXPORT_SIDE_PX);
    assert!(u64::from(image.width) * u64::from(image.height) <= MAX_EXPORT_PIXELS);
    // As large as the pixel cap lets it be, the frame's shape: from x = -1e6 to the second
    // ellipse's right at 2e6, from y = -1e6 to the many-line text's foot (2,500 lines of half a
    // unit and more, past 1e6), with the margin.
    assert!(
        u64::from(image.width) * u64::from(image.height) > MAX_EXPORT_PIXELS / 100 * 98,
        "{}x{}",
        image.width,
        image.height
    );
    assert!(
        image.width > image.height,
        "{}x{}",
        image.width,
        image.height
    );
    // Everything is drawn at the same scale, outlines too: shrunk this far (a three-hundredth
    // of a pixel a unit), the 0.05-unit outlines are far under a pixel, and the text is too
    // small to draw.

    // Smaller caps, a wide board: the side cap.
    let wide = common::board(
        "lawh version=1\nline id=l1 x1=0 y1=0 x2=5000 y2=0\nline id=l2 x1=0 y1=1 x2=5000 y2=1\n",
    );
    let limits = ExportLimits {
        max_side_px: 2_000,
        max_pixels: 1_000_000,
        ..ExportLimits::DEFAULT
    };
    let exported = export_with(&wide, &limits).unwrap();
    let image = decode(&exported.bytes);
    assert!(
        image.width <= 2_000 && image.width >= 1_995,
        "{}",
        image.width
    );
    let shrunk = exported.report.shrunk.unwrap();
    // 80 px a unit shrunk to about 2,000 px over 5,002 units.
    assert!(
        (shrunk * 80.0 - 2_000.0 / 5_002.05).abs() < 0.01,
        "{shrunk}"
    );
    // A square board: the pixel cap. At about 10 px a unit, its outline is half a pixel wide:
    // faint, but there all around.
    let square = common::board("lawh version=1\nrect id=r1 x=0 y=0 w=100 h=100\n");
    let exported = export_with(&square, &limits).unwrap();
    let image = decode(&exported.bytes);
    assert!(u64::from(image.width) * u64::from(image.height) <= 1_000_000);
    assert!(image.width >= 990, "{}", image.width);
    let outline = image.count(|px| px != paper());
    assert!(outline > 3_500, "{outline} pixels of outline");
}

#[test]
fn a_failing_formula_is_drawn_red_and_counted() {
    let board = common::board(
        "lawh version=1\n\
         math id=m1 x=0 y=0 \"A = pi r^2\"\n\
         math id=m2 x=0 y=2 \"frac(1,\"\n\
         math id=m3 x=0 y=4 \"frac(1,\"\n",
    );
    let exported = export(&board);
    assert_eq!(exported.report.failed_formulas, 2);
    assert_eq!(exported.report.shrunk, None);
    let image = decode(&exported.bytes);
    let red = image.count(is_red);
    let ink = image.count(is_ink);
    assert!(red > 100 && ink > 100, "red {red}, ink {ink}");
    // The good formula on top, the failed sources below it: no red in the top third.
    let third = image.height / 3;
    assert!((0..third).all(|y| (0..image.width).all(|x| !is_red(image.at(x, y)))));
}

/// A formula that takes longer than the math engine's timeout fails on its own: it is drawn as
/// its source and counted, and the export goes on.
#[test]
fn a_formula_that_times_out_is_drawn_red_and_counted() {
    let engine = lawh_math::MathEngine::new().with_timeout(Duration::from_millis(1));
    let mut renderer = Renderer::with_math(engine);
    let board = common::board(
        "lawh version=1\n\
         math id=m1 x=0 y=0 \"#range(20000).map(i => str(i)).join()\"\n\
         circle id=c1 x=0 y=3 r=1\n",
    );
    let exported = renderer.export(&board, None, &png_options()).unwrap();
    assert_eq!(exported.report.failed_formulas, 1);
    let image = decode(&exported.bytes);
    assert!(image.count(is_red) > 50);
}

#[test]
fn the_math_wait_stops_an_export() {
    let board = common::area_board();
    let limits = ExportLimits {
        math_wait: Duration::ZERO,
        ..ExportLimits::DEFAULT
    };
    assert_eq!(export_with(&board, &limits), Err(ExportError::MathWait));
    // A board without math never waits.
    let shapes = common::board("lawh version=1\ncircle id=c1 x=0 y=0 r=1\n");
    assert!(export_with(&shapes, &limits).is_ok());
    let message = ExportError::MathWait.to_string();
    assert!(
        message.starts_with("export stopped: ") && message.contains("10 s"),
        "{message}"
    );
}

#[test]
fn the_budget_stops_an_export() {
    let board = common::area_board();
    // The area board at the default scale costs a few ms; its image alone 0.6 ns a pixel.
    for budget_ns in [0.0, 1e5, 2e5] {
        let limits = ExportLimits {
            budget_ns,
            ..ExportLimits::DEFAULT
        };
        assert_eq!(
            export_with(&board, &limits),
            Err(ExportError::OverBudget),
            "{budget_ns}"
        );
    }
    let limits = ExportLimits {
        budget_ns: 1e8,
        ..ExportLimits::DEFAULT
    };
    assert!(export_with(&board, &limits).is_ok());
    // 500 circles one inside the other, filling a 2,000 x 2,000 image with outlines: about
    // 85 ms estimated, over the 30 ms given here, which the image alone (2.4 ms) is not.
    let mut src = String::from("lawh version=1\n");
    for i in 0..500 {
        let r = 10 + i;
        writeln!(src, "circle id=c{i} x=0 y=0 r={r}").unwrap();
    }
    let piled = common::board(&src);
    let limits = ExportLimits {
        budget_ns: 3e7,
        max_pixels: 4_000_000,
        ..ExportLimits::DEFAULT
    };
    assert_eq!(export_with(&piled, &limits), Err(ExportError::OverBudget));
    assert_eq!(
        ExportError::OverBudget.to_string(),
        "export stopped: drawing it costs too much"
    );
}

#[test]
fn the_byte_cap_stops_an_export() {
    let board = common::area_board();
    let full = export(&board).bytes.len();
    let limits = ExportLimits {
        max_bytes: full - 1,
        ..ExportLimits::DEFAULT
    };
    match export_with(&board, &limits) {
        Err(ExportError::TooLarge { bytes }) => assert!(bytes > full - 1, "{bytes}"),
        other => panic!("{other:?}"),
    }
    let limits = ExportLimits {
        max_bytes: full,
        ..ExportLimits::DEFAULT
    };
    assert_eq!(export_with(&board, &limits).unwrap().bytes.len(), full);
    let message = ExportError::TooLarge { bytes: 1 }.to_string();
    assert_eq!(message, "export stopped: the file is over 64 MiB");
}

#[test]
fn limits_above_the_defaults_change_nothing() {
    let board = common::area_board();
    let loose = ExportLimits {
        budget_ns: f64::INFINITY,
        math_wait: Duration::from_secs(1_000),
        max_pixels: u64::MAX,
        max_side_px: u32::MAX,
        max_bytes: usize::MAX,
    };
    let exported = export_with(&board, &loose).unwrap();
    assert_eq!(exported, export(&board));
    // A board past the pixel caps still shrinks to the defaults.
    let wide = common::board("lawh version=1\nline id=l1 x1=0 y1=0 x2=1000 y2=0\n");
    let image = decode(&export_with(&wide, &loose).unwrap().bytes);
    assert!(image.width <= MAX_EXPORT_SIDE_PX);
}

#[test]
fn exports_are_the_same_every_time() {
    let board = mixed_board();
    let first = export(&board);
    for _ in 0..2 {
        assert_eq!(export(&board), first);
    }
    // And from a renderer that has drawn the board in the window first.
    let mut renderer = Renderer::new_trusted();
    renderer.render(
        &board,
        &Viewport::new(kurbo::Point::ZERO, 2.0, 300, 200, 1.0),
    );
    assert_eq!(
        renderer.export(&board, None, &png_options()).unwrap(),
        first
    );
}

#[test]
fn an_export_leaves_the_window_as_it_was() {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    renderer.resolve_math(&board);
    let vp = Viewport::fit(renderer.board_bounds(&board).unwrap(), 640, 480, 1.0);
    let before = renderer.render(&board, &vp).to_vec();
    let stats = renderer.last_frame();
    renderer.export(&board, None, &scaled(800)).unwrap();
    assert_eq!(renderer.render(&board, &vp), before.as_slice());
    assert_eq!(renderer.last_frame().drawn, stats.drawn);
}

#[test]
fn svg_and_pdf_are_not_written_yet() {
    let board = common::area_board();
    for format in [ExportFormat::Svg, ExportFormat::Pdf] {
        let options = ExportOptions {
            format,
            ..ExportOptions::default()
        };
        assert_eq!(
            Renderer::new_trusted().export(&board, None, &options),
            Err(ExportError::Unsupported(format))
        );
    }
    assert_eq!(
        ExportError::Unsupported(ExportFormat::Pdf).to_string(),
        "pdf export is not available yet"
    );
}

#[test]
fn error_messages_are_short_lowercase_phrases() {
    for err in [
        ExportError::Empty,
        ExportError::OverBudget,
        ExportError::MathWait,
        ExportError::TooLarge { bytes: 70_000_000 },
        ExportError::Png("bad".to_owned()),
        ExportError::Unsupported(ExportFormat::Svg),
    ] {
        let message = err.to_string();
        let first = message.chars().next().unwrap();
        assert!(first.is_lowercase(), "{message}");
        assert!(
            !message.contains('\u{2014}') && !message.ends_with('.'),
            "{message}"
        );
        assert!(message.len() < 60, "{message}");
    }
}

#[test]
fn the_export_sizes_are_the_window_at_zoom_1() {
    assert!((EXPORT_STROKE_UNITS - 0.05).abs() < 1e-12);
    assert!((EXPORT_ARROW_HEAD_UNITS - 0.3).abs() < 1e-12);
    assert!((EXPORT_MARGIN_UNITS - 1.0).abs() < 1e-12);
}

/// Peak resident memory of this process, in bytes (Linux), after resetting it.
#[cfg(target_os = "linux")]
fn reset_peak() {
    std::fs::write("/proc/self/clear_refs", "5").unwrap();
}

#[cfg(target_os = "linux")]
fn peak_bytes() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status
        .lines()
        .find(|line| line.starts_with("VmHWM:"))
        .unwrap();
    let kb: u64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    kb * 1024
}

/// A board at lawh-core's caps for an export: 250 texts of 160 characters (the board's 40,000)
/// spread over the whole coordinate range, and 250 circles a million units across, so the image
/// is at the pixel cap and full of outlines.
fn hostile_board() -> Board {
    let mut src = String::from("lawh version=1\n");
    let line = "lorem ipsum dolor sit amet, consectetur adipiscing elit ".repeat(3);
    let text: String = line.chars().take(160).collect();
    for i in 0..250 {
        let x = -1e6 + f64::from(i) * 8_000.0;
        writeln!(src, "text id=t{i} x={x} y={x} \"{text}\"").unwrap();
        writeln!(src, "circle id=c{i} x={x} y=0 r={}", 1e6 - f64::from(i)).unwrap();
    }
    common::board(&src)
}

/// Times and peak memory of exports, for `docs/research/m4-measurements.md`. Run in release on
/// a quiet machine: `cargo test --release -p lawh-render --test export -- --ignored
/// --test-threads=1 --nocapture measure_exports`.
#[test]
#[ignore = "a measurement: run in release with --test-threads=1"]
#[cfg(target_os = "linux")]
fn measure_exports() {
    fn row(name: &str, took: Duration, peak: u64, result: &Result<Exported, ExportError>) {
        match result {
            Ok(exported) => {
                let image = decode(&exported.bytes);
                println!(
                    "{name}: {took:?}, peak {} MB, {}x{} px, {} bytes, shrunk {:?}",
                    peak / 1_000_000,
                    image.width,
                    image.height,
                    exported.bytes.len(),
                    exported.report.shrunk
                );
            }
            Err(err) => println!("{name}: {took:?}, peak {} MB, {err}", peak / 1_000_000),
        }
    }
    let boards = [
        ("area.lawh", common::area_board()),
        ("limits_board", common::limits_board()),
        ("hostile 500 objects", hostile_board()),
    ];
    for (name, board) in &boards {
        for hundredths in [200, 800] {
            let mut renderer = Renderer::new_trusted();
            // Math compiled first, so the times are drawing and encoding.
            renderer.resolve_math(board);
            reset_peak();
            let started = Instant::now();
            let result = renderer.export(board, None, &scaled(hundredths));
            let took = started.elapsed();
            row(
                &format!("{name} at {hundredths}"),
                took,
                peak_bytes(),
                &result,
            );
        }
    }
    // 500 formulas that fail, each its own, compiled as part of the export (in process here;
    // the app's worker adds its round trips).
    let mut src = String::from("lawh version=1\n");
    for i in 0..500 {
        let (x, y) = (f64::from(i % 25) * 6.0, f64::from(i / 25) * 2.0);
        writeln!(src, "math id=m{i} x={x} y={y} \"frac({i},\"").unwrap();
    }
    let failing = common::board(&src);
    let mut renderer = Renderer::new_trusted();
    reset_peak();
    let started = Instant::now();
    let result = renderer.export(&failing, None, &png_options());
    let took = started.elapsed();
    row(
        "500 failing formulas, cold, at 200",
        took,
        peak_bytes(),
        &result,
    );
    if let Ok(exported) = &result {
        assert_eq!(exported.report.failed_formulas, 500);
    }
}
