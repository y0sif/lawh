//! Viewport maths and renderer behaviour that doesn't need a golden image.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use std::time::{Duration, Instant};

use kurbo::{Point, Rect};
use lawh_core::{Board, Object, ObjectId, Shape};
use lawh_render::{BASE_PX_PER_UNIT, RenderError, Renderer, TEXT_SIZE_UNITS, Viewport};

const PAPER: [u8; 4] = [0xFA, 0xFA, 0xF7, 0xFF];

/// Pixels of a math error's red source text.
fn red_pixels(buf: &[u8]) -> usize {
    buf.as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[0] > 150 && px[1] < 100 && px[2] < 100)
        .count()
}

/// Pixels at the heart of a placeholder's grey (#9A9A94) source text.
fn grey_pixels(buf: &[u8]) -> usize {
    buf.as_chunks::<4>()
        .0
        .iter()
        .filter(|px| {
            (148..=162).contains(&px[0]) && px[0] == px[1] && px[0].checked_sub(px[2]) == Some(6)
        })
        .count()
}

/// Pixels of near-black ink.
fn ink_pixels(buf: &[u8]) -> usize {
    buf.as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[0] < 80 && px[1] < 80 && px[2] < 80)
        .count()
}

fn close(a: Point, b: Point) -> bool {
    (a - b).hypot() <= 1e-9 * (1.0 + a.to_vec2().hypot().max(b.to_vec2().hypot()))
}

#[test]
fn to_board_inverts_to_screen() {
    let viewports = [
        Viewport::new(Point::ZERO, 1.0, 800, 600, 1.0),
        Viewport::new(Point::new(3.5, -2.25), 2.5, 1920, 1080, 1.25),
        Viewport::new(Point::new(-1000.0, 40.0), 0.05, 333, 77, 2.0),
        Viewport::new(Point::new(1e6, -1e6), 50.0, 1, 1, 1.5),
    ];
    let points = [
        Point::ZERO,
        Point::new(1.0, -1.0),
        Point::new(-12.75, 3.125),
        Point::new(1e5, 1e5),
    ];
    for vp in viewports {
        assert!((vp.px_per_unit() - BASE_PX_PER_UNIT * vp.zoom * vp.scale_factor).abs() < 1e-12);
        let center = vp.to_screen(vp.center);
        assert!(close(
            center,
            Point::new(f64::from(vp.width) / 2.0, f64::from(vp.height) / 2.0)
        ));
        for p in points {
            assert!(close(vp.to_board(vp.to_screen(p)), p), "{vp:?} {p:?}");
            assert!(close(vp.transform() * p, vp.to_screen(p)), "{vp:?} {p:?}");
        }
    }
}

#[test]
fn screen_y_grows_down_like_board_y() {
    let vp = Viewport::new(Point::ZERO, 1.0, 800, 600, 1.0);
    let above = vp.to_screen(Point::new(0.0, -1.0));
    let below = vp.to_screen(Point::new(0.0, 1.0));
    assert!(above.y < below.y);
    assert!((below.y - above.y - 2.0 * BASE_PX_PER_UNIT).abs() < 1e-9);
}

#[test]
fn fit_contains_bounds() {
    let cases = [
        (Rect::new(-3.0, -5.0, 8.0, 3.0), 800, 600, 1.0),
        (Rect::new(0.0, 0.0, 100.0, 1.0), 800, 600, 1.0),
        (Rect::new(0.0, 0.0, 1.0, 100.0), 1920, 1080, 2.0),
        (Rect::new(-1e4, -1e4, 1e4, 1e4), 640, 480, 1.5),
        (Rect::new(5.0, 5.0, 4.0, 4.0), 300, 200, 1.0),
        (Rect::new(2.0, 2.0, 2.5, 2.1), 16, 16, 1.0),
    ];
    for (bounds, w, h, sf) in cases {
        let vp = Viewport::fit(bounds, w, h, sf);
        assert!(vp.zoom.is_finite() && vp.zoom > 0.0, "{vp:?}");
        let screen = Rect::new(0.0, 0.0, f64::from(w), f64::from(h));
        let shown = Rect::from_points(
            vp.to_screen(Point::new(bounds.x0, bounds.y0)),
            vp.to_screen(Point::new(bounds.x1, bounds.y1)),
        );
        assert!(
            screen.contains_rect(shown),
            "{bounds:?} at {w}x{h}: shown as {shown:?}"
        );
        assert!(close(vp.center, bounds.abs().center()));
    }
}

#[test]
fn fit_uses_the_space_and_keeps_a_margin() {
    let bounds = Rect::new(-3.0, -5.0, 8.0, 3.0);
    let vp = Viewport::fit(bounds, 800, 600, 1.0);
    let shown = Rect::from_points(
        vp.to_screen(Point::new(bounds.x0, bounds.y0)),
        vp.to_screen(Point::new(bounds.x1, bounds.y1)),
    );
    // Height is the tight side here: a margin above and below, but not much more.
    assert!(shown.y0 > 8.0 && shown.y0 < 60.0, "{shown:?}");
    assert!(shown.y1 < 592.0 && shown.y1 > 540.0, "{shown:?}");
}

#[test]
fn fit_of_a_point_or_garbage_is_usable() {
    for bounds in [
        Rect::new(1.0, 2.0, 1.0, 2.0),
        Rect::new(f64::NAN, 0.0, 1.0, 1.0),
        Rect::new(0.0, 0.0, f64::INFINITY, 1.0),
        Rect::new(-f64::MAX, -f64::MAX, f64::MAX, f64::MAX),
        Rect::new(f64::MAX / 2.0, f64::MAX / 2.0, f64::MAX, f64::MAX),
    ] {
        let vp = Viewport::fit(bounds, 800, 600, f64::NAN);
        assert!(vp.zoom.is_finite() && vp.zoom > 0.0, "{vp:?}");
        assert!(vp.center.is_finite(), "{vp:?}");
        assert!((vp.scale_factor - 1.0).abs() < f64::EPSILON);
    }
}

#[test]
fn render_returns_a_full_opaque_buffer() {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    for (w, h) in [(800, 600), (31, 17), (1, 1)] {
        let vp = Viewport::new(Point::ZERO, 1.0, w, h, 1.0);
        let buf = renderer.render(&board, &vp);
        assert_eq!(buf.len(), usize::from(w) * usize::from(h) * 4);
        assert!(
            buf.as_chunks::<4>().0.iter().all(|px| px[3] == 255),
            "{w}x{h}"
        );
    }
}

#[test]
fn empty_board_renders_paper_and_grid_only() {
    let mut renderer = Renderer::new_trusted();
    let vp = Viewport::new(Point::new(0.5, 0.5), 1.0, 200, 100, 1.0);
    let buf = renderer.render(&Board::new(), &vp);
    let paper = buf
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| **px == PAPER)
        .count();
    // Grid dots cover a tiny share of the image.
    assert!(paper * 100 > buf.len() / 4 * 95, "{paper}");
    assert!(paper < buf.len() / 4, "expected some grid dots");
}

#[test]
fn grid_is_hidden_when_zoomed_out() {
    let mut renderer = Renderer::new_trusted();
    // 0.25 * 40 = 10 px per unit, under the 12 px threshold.
    let vp = Viewport::new(Point::ZERO, 0.25, 200, 100, 1.0);
    let buf = renderer.render(&Board::new(), &vp);
    assert!(buf.as_chunks::<4>().0.iter().all(|px| *px == PAPER));
}

#[test]
fn failed_math_shows_its_source_in_red() {
    let board = common::board("lawh version=1\nmath id=m1 x=0 y=0 \"frac(1,\"\n");
    let mut renderer = Renderer::new_trusted();
    renderer.resolve_math(&board);
    let bounds = renderer.board_bounds(&board).unwrap();
    // The source text "frac(1," is laid out as text: wider than tall, one line.
    assert!(bounds.width() > bounds.height(), "{bounds:?}");
    assert!(bounds.height() < TEXT_SIZE_UNITS * 2.0, "{bounds:?}");

    let vp = Viewport::fit(bounds, 400, 200, 1.0);
    let buf = renderer.render(&board, &vp);
    let red = red_pixels(buf);
    assert!(red > 50, "expected red error text, found {red} red pixels");
    assert_eq!(grey_pixels(buf), 0);
    assert_eq!(renderer.math_pending(), 0);
}

#[test]
fn a_formula_too_large_to_draw_is_red_and_fast_at_every_zoom() {
    // The verifier's repro: zoomed in, this circle made the renderer flatten an
    // arc 2e21 pixels long, about 51 GB.
    let board = common::board("lawh version=1\nmath id=m1 x=0 y=0 \"#circle(radius: 1e18pt)\"\n");
    let mut renderer = Renderer::new_trusted();
    renderer.resolve_math(&board);
    let bounds = renderer.board_bounds(&board).unwrap();
    // Laid out as its source text, not as a circle 2e18 pt across.
    assert!(bounds.width() < 20.0 && bounds.height() < 1.0, "{bounds:?}");

    let fit = Viewport::fit(bounds, 800, 600, 2.0);
    let mut viewports = vec![fit];
    // Zoomed in on the start of the text, where the circle's arc would be.
    for zoom in [0.3, 1.0, 3.0, 12.0, 20.0] {
        viewports.push(Viewport::new(Point::new(0.4, 0.3), zoom, 800, 600, 2.0));
    }
    for vp in viewports {
        let started = Instant::now();
        let buf = renderer.render(&board, &vp);
        let took = started.elapsed();
        assert!(
            took < Duration::from_millis(500),
            "zoom {} took {took:?}",
            vp.zoom
        );
        let red = red_pixels(buf);
        assert!(red > 20, "zoom {}: {red} red pixels", vp.zoom);
        assert_eq!(ink_pixels(buf), 0, "zoom {}", vp.zoom);
    }
    assert_eq!(renderer.math_pending(), 0);
}

#[test]
fn math_still_compiling_is_a_grey_placeholder_until_resolved() {
    let board = common::board("lawh version=1\nmath id=m1 x=0 y=0 \"A = pi r^2\"\n");
    let mut renderer = Renderer::new_trusted();
    // A fresh engine has nothing cached: the first frame queues the formula and doesn't wait.
    let vp = Viewport::new(Point::new(1.2, 0.3), 4.0, 600, 200, 1.0);
    let buf = renderer.render(&board, &vp);
    let (grey, red, ink) = (grey_pixels(buf), red_pixels(buf), ink_pixels(buf));
    assert!(
        grey > 100,
        "expected grey placeholder text, found {grey} grey pixels"
    );
    assert_eq!((red, ink), (0, 0));
    assert_eq!(renderer.math_pending(), 1);

    renderer.resolve_math(&board);
    let buf = renderer.render(&board, &vp);
    let (grey, red, ink) = (grey_pixels(buf), red_pixels(buf), ink_pixels(buf));
    assert!(
        ink > 100,
        "expected the formula in ink, found {ink} ink pixels"
    );
    assert_eq!((grey, red), (0, 0));
    assert_eq!(renderer.math_pending(), 0);
}

#[test]
fn exports_wait_for_math() {
    let board = common::board(
        "lawh version=1\nmath id=m1 x=0 y=0 \"A = pi r^2\"\nmath id=m2 x=0 y=2 \"frac(1,\"\n",
    );
    let vp = Viewport::new(Point::new(1.2, 1.0), 2.0, 300, 200, 1.0);
    // A fresh renderer every time: nothing cached, yet every export is the same.
    let png = Renderer::new_trusted().render_png(&board, &vp).unwrap();
    for _ in 0..2 {
        assert_eq!(
            Renderer::new_trusted().render_png(&board, &vp).unwrap(),
            png
        );
    }
    let pixels = vello_cpu::Pixmap::from_png(std::io::Cursor::new(png)).unwrap();
    let buf = pixels.data_as_u8_slice();
    assert!(ink_pixels(buf) > 50 && red_pixels(buf) > 50);
    assert_eq!(grey_pixels(buf), 0);
}

#[test]
fn bounds_cover_every_object_kind() {
    let board = common::board(
        "lawh version=1\n\
         rect id=r1 x=-10 y=-10 w=4 h=2\n\
         ellipse id=e1 x=0 y=0 rx=3 ry=2\n\
         arrow id=a1 x1=0 y1=0 x2=20 y2=5\n\
         text id=t1 x=0 y=8 \"one\\ntwo\"\n",
    );
    let mut renderer = Renderer::new_trusted();
    let bounds = renderer.board_bounds(&board).unwrap();
    assert!((bounds.x0 - -10.0).abs() < 1e-9 && (bounds.y0 - -10.0).abs() < 1e-9);
    assert!((bounds.x1 - 20.0).abs() < 1e-9, "{bounds:?}");
    // Two lines of text starting at y = 8, each a bit taller than the em size.
    assert!(bounds.y1 > 8.0 + 2.0 * TEXT_SIZE_UNITS, "{bounds:?}");
    assert!(bounds.y1 < 8.0 + 4.0 * TEXT_SIZE_UNITS, "{bounds:?}");
    assert_eq!(renderer.board_bounds(&Board::new()), None);
}

#[test]
fn newlines_start_new_lines() {
    let mut renderer = Renderer::new_trusted();
    let one = common::board("lawh version=1\ntext id=t1 x=0 y=0 \"line\"\n");
    let two = common::board("lawh version=1\ntext id=t1 x=0 y=0 \"line\\nline\"\n");
    let one = renderer.board_bounds(&one).unwrap();
    let two = renderer.board_bounds(&two).unwrap();
    assert!(
        (two.height() - 2.0 * one.height()).abs() < 0.01,
        "{one:?} {two:?}"
    );
    assert!((two.width() - one.width()).abs() < 0.01, "{one:?} {two:?}");
}

#[test]
fn bounds_never_wait_for_math() {
    let board = common::board("lawh version=1\nmath id=m1 x=5 y=0 \"A = pi r^2\"\n");
    let source_as_text = common::board("lawh version=1\ntext id=t1 x=5 y=0 \"A = pi r^2\"\n");
    let mut renderer = Renderer::new_trusted();
    // Not compiled yet: the placeholder's extents.
    assert_eq!(
        renderer.board_bounds(&board),
        renderer.board_bounds(&source_as_text)
    );
    renderer.resolve_math(&board);
    assert_ne!(
        renderer.board_bounds(&board),
        renderer.board_bounds(&source_as_text)
    );
}

#[test]
fn objects_with_non_finite_numbers_have_no_bounds() {
    let mut renderer = Renderer::new_trusted();
    let id = ObjectId::new("x1").unwrap();
    for shape in [
        Shape::Rect {
            origin: Point::new(f64::NAN, 0.0),
            w: 1.0,
            h: 1.0,
        },
        Shape::Ellipse {
            center: Point::ZERO,
            rx: f64::INFINITY,
            ry: 1.0,
        },
        Shape::Line {
            a: Point::ZERO,
            b: Point::new(0.0, f64::NEG_INFINITY),
        },
        Shape::Text {
            at: Point::new(f64::NAN, f64::NAN),
            text: "t".to_owned(),
        },
    ] {
        let object = Object::new(id.clone(), shape);
        assert_eq!(renderer.object_bounds(&object), None, "{object:?}");
    }
}

#[test]
fn math_bounds_scale_with_the_text_size() {
    let board = common::board("lawh version=1\nmath id=m1 x=5 y=0 \"A = pi r^2\"\n");
    let mut renderer = Renderer::new_trusted();
    renderer.resolve_math(&board);
    let bounds = renderer.board_bounds(&board).unwrap();
    assert!((bounds.x0 - 5.0).abs() < 1e-9 && bounds.y0.abs() < 1e-9);
    // About one em tall and a few ems wide.
    assert!(bounds.height() > TEXT_SIZE_UNITS * 0.6 && bounds.height() < TEXT_SIZE_UNITS * 2.0);
    assert!(bounds.width() > TEXT_SIZE_UNITS * 2.0 && bounds.width() < TEXT_SIZE_UNITS * 8.0);
}

#[test]
fn degenerate_viewports_do_not_panic() {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    for vp in [
        Viewport::new(Point::ZERO, 0.0, 64, 64, 1.0),
        Viewport::new(Point::ZERO, -2.0, 64, 64, 1.0),
        Viewport::new(Point::ZERO, f64::NAN, 64, 64, 1.0),
        Viewport::new(Point::new(f64::INFINITY, 0.0), 1.0, 64, 64, 1.0),
        Viewport::new(Point::ZERO, 1.0, 64, 64, 0.0),
        Viewport::new(Point::ZERO, 1e-9, 64, 64, 1.0),
        Viewport::new(Point::ZERO, 1e6, 64, 64, 1.0),
        Viewport::new(Point::new(1e12, -1e12), 1.0, 64, 64, 1.0),
        Viewport::new(Point::new(1e10, 0.0), 1e300, 64, 64, 1.0),
        Viewport::new(Point::ZERO, 1e-300, 64, 64, 1e300),
    ] {
        let buf = renderer.render(&board, &vp);
        assert_eq!(buf.len(), 64 * 64 * 4, "{vp:?}");
    }
}

#[test]
fn absurd_zoom_or_scale_factor_draws_the_paper_only() {
    let board =
        common::board("lawh version=1\nrect id=r1 x=-1 y=-1 w=2 h=2\ntext id=t1 x=0 y=0 \"W\"\n");
    let mut renderer = Renderer::new_trusted();
    for vp in [
        // 40 * 1e6 pixels per unit: a text em would be 20 million pixels.
        Viewport::new(Point::ZERO, 1e6, 64, 64, 1.0),
        // An ordinary 40 pixels per unit, but strokes 2e300 pixels wide.
        Viewport::new(Point::ZERO, 1e-300, 64, 64, 1e300),
        Viewport::new(Point::ZERO, 1.0 / 1000.0, 64, 64, 1000.0),
    ] {
        let buf = renderer.render(&board, &vp);
        assert!(
            buf.as_chunks::<4>().0.iter().all(|px| *px == PAPER),
            "{vp:?}"
        );
    }
}

/// The pixel at the centre of `vp` is ink, and pixels `away` physical pixels
/// to either side along `normal` are paper.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the pixels looked at are a few pixels from the centre of a small image"
)]
fn ink_at_center_only(renderer: &mut Renderer, board: &Board, vp: &Viewport, normal: (f64, f64)) {
    let buf = renderer.render(board, vp).to_vec();
    let (cx, cy) = (f64::from(vp.width) / 2.0, f64::from(vp.height) / 2.0);
    let at = |dx: f64, dy: f64| common::pixel(&buf, vp.width, (cx + dx) as u16, (cy + dy) as u16);
    let center = at(0.0, 0.0);
    assert!(
        center[0] < 100,
        "no ink at the centre of {vp:?}: {center:?}"
    );
    let away = 5.0;
    for side in [-1.0, 1.0] {
        let px = at(side * away * normal.0, side * away * normal.1);
        assert_eq!(
            px, PAPER,
            "{vp:?}: expected paper {away} px off the outline"
        );
    }
}

#[test]
fn huge_shapes_are_drawn_exactly_where_they_cross_the_view() {
    // At zoom 20 and scale 2 a unit is 1,600 pixels: this circle is 1.6e9 pixels in radius
    // and the line 3.2e9 long, far past what f32 holds to the pixel.
    let board = common::board(
        "lawh version=1\n\
         circle id=c1 x=0 y=0 r=1000000\n\
         line id=l1 x1=-1000000 y1=-1000000 x2=1000000 y2=999999\n",
    );
    let mut renderer = Renderer::new_trusted();
    for degrees in [0.0_f64, 30.0, 135.0, 270.0] {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let on_circle = Point::new(1e6 * cos, 1e6 * sin);
        let vp = Viewport::new(on_circle, 20.0, 400, 300, 2.0);
        ink_at_center_only(&mut renderer, &board, &vp, (cos, sin));
    }
    // The line rises 1,999,999 over 2,000,000: at x = 0 it is at y = -0.5.
    let vp = Viewport::new(Point::new(0.0, -0.5), 20.0, 400, 300, 2.0);
    ink_at_center_only(&mut renderer, &board, &vp, (0.0, 1.0));
}

#[test]
fn boards_at_the_limits_render_everywhere() {
    let board = common::limits_board();
    let mut renderer = Renderer::new_trusted();
    renderer.resolve_math(&board);
    for (x, y) in common::LIMITS_CENTERS {
        let vp = Viewport::new(Point::new(x, y), 20.0, 320, 200, 2.0);
        let buf = renderer.render(&board, &vp);
        assert!(ink_pixels(buf) > 0, "nothing drawn at ({x}, {y})");
    }
    let bounds = renderer.board_bounds(&board).unwrap();
    let vp = Viewport::fit(bounds, 320, 200, 2.0);
    assert!(vp.center.is_finite() && vp.zoom.is_finite() && vp.zoom > 0.0);
    assert!(ink_pixels(renderer.render(&board, &vp)) > 0);
}

#[test]
fn empty_image_is_an_error_for_png_and_empty_for_pixels() {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    let vp = Viewport::new(Point::ZERO, 1.0, 0, 600, 1.0);
    assert_eq!(renderer.render(&board, &vp), &[] as &[u8]);
    assert_eq!(
        renderer.render_png(&board, &vp),
        Err(RenderError::EmptyImage {
            width: 0,
            height: 600
        })
    );
    // And it recovers at a real size.
    let vp = Viewport::new(Point::ZERO, 1.0, 10, 10, 1.0);
    assert_eq!(renderer.render(&board, &vp).len(), 400);
}

#[test]
fn png_round_trips_the_pixels() {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    let vp = Viewport::new(Point::new(1.0, -1.0), 1.2, 320, 240, 1.0);
    let png = renderer.render_png(&board, &vp).unwrap();
    let pixels = renderer.render(&board, &vp).to_vec();
    let decoded = vello_cpu::Pixmap::from_png(std::io::Cursor::new(png)).unwrap();
    assert_eq!(decoded.data_as_u8_slice(), pixels.as_slice());
    assert_eq!(common::pixel(&pixels, 320, 0, 0)[3], 255);
}

/// Pixels of the faint outline drawn in place of an object over the frame budget (#C8C8C2,
/// over paper where it covers a pixel partly). Grid dots (#D6D6D0) look alike, so callers look
/// where the outline is.
fn is_faint(px: [u8; 4]) -> bool {
    px[0] == px[1] && (0xC0..0xFA).contains(&px[0]) && px[2] < px[0]
}

/// An in-process engine with room for formulas that are slow in a debug build.
fn patient_renderer() -> Renderer {
    Renderer::with_math(lawh_math::MathEngine::new().with_timeout(Duration::from_secs(120)))
}

#[test]
fn the_review_repros_are_too_large_red_and_quick() {
    for src in [
        r#"#range(700).map(_ => "x" * 700).join(linebreak())"#,
        "#polygon(stroke: 100pt, ..range(100000).map(i => (calc.rem(i, 2) * 300pt, calc.rem(i, 3) * 300pt)))",
    ] {
        let engine = lawh_math::MathEngine::new().with_timeout(Duration::from_secs(120));
        assert_eq!(
            engine.layout(src),
            Err(lawh_math::MathError::TooLarge),
            "{src}"
        );
        let mut renderer = Renderer::with_math(engine);
        let board = common::board(&format!(
            "lawh version=1\nmath id=m1 x=0 y=0 \"{}\"\n",
            src.replace('"', "\\\"")
        ));
        renderer.resolve_math(&board);
        let bounds = renderer.board_bounds(&board).unwrap();
        for vp in [
            Viewport::fit(bounds, 1920, 1080, 1.0),
            Viewport::new(Point::new(0.5, 0.3), 20.0, 2880, 1800, 2.0),
        ] {
            let started = Instant::now();
            let buf = renderer.render(&board, &vp);
            let took = started.elapsed();
            assert!(
                took < Duration::from_secs(1),
                "{src} at {vp:?} took {took:?}"
            );
            assert!(red_pixels(buf) > 20, "{src} at {vp:?}");
            assert_eq!(ink_pixels(buf), 0, "{src} at {vp:?}");
        }
    }
}

#[test]
fn a_full_board_of_costly_shapes_stays_in_budget() {
    use std::fmt::Write as _;

    // As many screen-filling outlines as a board may hold, stacked: 70 ms a frame at 1920x1080
    // before the budget (`board_objects_frame_time` in `tests/bench.rs`).
    for template in [
        "circle id=o{i} x=0 y=0 r=100",
        "rect id=o{i} x=0 y=0 w=48 h=27",
        "arrow id=o{i} x1=0 y1=0 x2=48 y2=27",
    ] {
        let mut src = "lawh version=1\n".to_owned();
        for i in 0..lawh_core::board::MAX_OBJECTS {
            writeln!(src, "{}", template.replace("{i}", &i.to_string())).unwrap();
        }
        let board = common::board(&src);
        let mut renderer = Renderer::new_trusted();
        let bounds = renderer.board_bounds(&board).unwrap();
        for (width, height, scale) in [(1920, 1080, 1.0), (2880, 1800, 2.0)] {
            let vp = Viewport::fit(bounds, width, height, scale);
            for _ in 0..2 {
                let started = Instant::now();
                let buf = renderer.render(&board, &vp);
                let took = started.elapsed();
                assert!(ink_pixels(buf) > 1000, "{template}");
                let frame = renderer.last_frame();
                assert!(took < Duration::from_secs(1), "{template}: {took:?}");
                assert!(
                    frame.drawn > 0 && frame.downgraded > 0,
                    "{template}: {frame:?}"
                );
                assert_eq!(frame.drawn + frame.downgraded, board.len(), "{frame:?}");
                assert!(frame.estimated < Duration::from_millis(25), "{frame:?}");
            }
        }
    }
}

#[test]
fn a_capped_glyph_pile_up_becomes_an_outline() {
    // Glyphs stacked on one spot defeat culling: at the glyph cap this took 2.2 s a frame at zoom
    // 20 and scale factor 2 before the budget (lawh-math's `MAX_LAYOUT_GLYPHS`).
    let n = lawh_math::MAX_LAYOUT_GLYPHS - 1;
    let src = format!("#for i in range({n}) {{ box(width: 0pt)[x] }}");
    let board = common::board(&format!("lawh version=1\nmath id=m1 x=0 y=0 \"{src}\"\n"));
    let mut renderer = patient_renderer();
    renderer.resolve_math(&board);
    let bounds = renderer.board_bounds(&board).unwrap();
    // The pile is one glyph's ink: an `x` about half an em wide.
    assert!(bounds.width() < TEXT_SIZE_UNITS * 2.0, "{bounds:?}");
    for vp in [
        Viewport::new(bounds.center(), 20.0, 2880, 1800, 2.0),
        Viewport::new(bounds.center(), 20.0, 1920, 1080, 1.0),
        Viewport::fit(bounds, 1920, 1080, 1.0),
    ] {
        let started = Instant::now();
        let buf = renderer.render(&board, &vp).to_vec();
        let took = started.elapsed();
        assert!(took < Duration::from_secs(1), "{vp:?} took {took:?}");
        let frame = renderer.last_frame();
        assert_eq!((frame.drawn, frame.downgraded), (0, 1), "{vp:?}: {frame:?}");
        assert_eq!(ink_pixels(&buf), 0, "{vp:?}");
        // The outline of the pile's bounds, in the faint colour: its top edge, a little in from
        // its left end.
        let screen = vp.transform().transform_rect_bbox(bounds);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "on screen"
        )]
        let (x, y) = (
            (screen.x0 + 0.3 * screen.width()) as u16,
            screen.y0.floor() as u16,
        );
        let around: Vec<[u8; 4]> = (y.saturating_sub(1)..=y + 1)
            .map(|y| common::pixel(&buf, vp.width, x, y))
            .collect();
        assert!(around.iter().any(|px| is_faint(*px)), "{vp:?}: {around:?}");
    }
}

#[test]
fn objects_over_the_budget_become_outlines_in_board_order() {
    // A formula far over the budget: 5,000 edges across its 300x600 pt box. r0 is drawn before
    // it and r2 after; both cross the left edge of its outline.
    let polygon = "#polygon(fill: black, ..range(5000).map(i => (calc.rem(i, 2) * 300pt, calc.rem(i, 3) * 300pt)))";
    let board = common::board(&format!(
        "lawh version=1\n\
         rect id=r0 x=-2 y=2 w=12 h=4\n\
         math id=m1 x=0 y=0 \"{polygon}\"\n\
         rect id=r2 x=-2 y=10 w=12 h=4\n"
    ));
    let mut renderer = patient_renderer();
    renderer.resolve_math(&board);
    // 40 pixels per unit: the formula's box (7.5 x 15 units) spans x 250..550, y 100..700.
    let vp = Viewport::new(Point::new(3.75, 7.5), 1.0, 800, 800, 1.0);
    let buf = renderer.render(&board, &vp).to_vec();
    let frame = renderer.last_frame();
    assert_eq!(
        (frame.drawn, frame.downgraded, frame.left_out),
        (2, 1, 0),
        "{frame:?}"
    );
    let at = |x, y| common::pixel(&buf, 800, x, y);
    // Inside the formula's box, away from the rectangles: paper, not a black fill.
    assert_eq!(at(400, 420), PAPER);
    // The outline's left edge, at x = 250, between grid rows.
    assert!(is_faint(at(250, 440)), "{:?}", at(250, 440));
    // r0 is under the outline: where they cross, the faint line lightens r0's ink.
    assert!(at(250, 180)[0] > 80, "{:?}", at(250, 180));
    assert!(at(300, 180)[0] < 80, "{:?}", at(300, 180));
    // r2 is over it: its ink covers the outline.
    assert!(at(250, 500)[0] < 80, "{:?}", at(250, 500));
}

#[test]
fn objects_under_half_a_pixel_are_skipped() {
    let board = common::board(
        "lawh version=1\n\
         rect id=r1 x=0 y=0 w=0.001 h=0.001\n\
         line id=l1 x1=1 y1=1 x2=1.001 y2=1\n\
         rect id=r2 x=2 y=0 w=1 h=1\n",
    );
    let mut renderer = Renderer::new_trusted();
    let vp = Viewport::new(Point::new(1.5, 0.5), 1.0, 300, 100, 1.0);
    renderer.render(&board, &vp);
    let frame = renderer.last_frame();
    assert_eq!(
        (frame.drawn, frame.too_small, frame.downgraded),
        (1, 2, 0),
        "{frame:?}"
    );
}

#[test]
fn math_is_culled_and_fitted_by_its_ink() {
    // The formula's box is one `a`; its `b` is drawn 400 pt (10 units) to the right of it.
    let board = common::board("lawh version=1\nmath id=m1 x=0 y=0 \"a #move(dx: 400pt)[b]\"\n");
    let mut renderer = Renderer::new_trusted();
    renderer.resolve_math(&board);
    let bounds = renderer.board_bounds(&board).unwrap();
    assert!(
        bounds.x1 > 10.0,
        "the fit misses the moved glyph: {bounds:?}"
    );
    // Looking at the moved glyph only, far from the formula's box: it is drawn.
    let vp = Viewport::new(Point::new(bounds.x1 - 0.2, 0.3), 4.0, 200, 200, 1.0);
    assert!(ink_pixels(renderer.render(&board, &vp)) > 20);
    assert_eq!(renderer.last_frame().drawn, 1);
}

#[test]
fn only_math_on_screen_is_asked_for_nearest_first() {
    use std::sync::{Arc, Condvar, Mutex};

    // `slow` comes first on the board, off screen, and takes a second or more to compile. Asked
    // for first, it would hold up the formula in view.
    let slow = "#let s = 0; #for i in range(3000000) { s += i }; #s";
    let board = common::board(&format!(
        "lawh version=1\n\
         math id=slow x=1000 y=0 \"{slow}\"\n\
         math id=m1 x=0 y=0 \"A = pi r^2\"\n"
    ));
    let wakes = Arc::new((Mutex::new(0_usize), Condvar::new()));
    let engine = lawh_math::MathEngine::new().with_timeout(Duration::from_secs(120));
    let woken = Arc::clone(&wakes);
    engine.set_waker(Arc::new(move || {
        *woken.0.lock().unwrap() += 1;
        woken.1.notify_all();
    }));
    let mut renderer = Renderer::with_math(engine);
    let wait_for = |count: usize, within: Duration| {
        let (lock, changed) = &*wakes;
        let guard = lock.lock().unwrap();
        *changed
            .wait_timeout_while(guard, within, |wakes| *wakes < count)
            .unwrap()
            .0
    };

    // As the app opens a board: the fit asks for nothing, then the first frame asks for what it
    // shows.
    renderer.board_bounds(&board);
    assert_eq!(
        wait_for(1, Duration::from_millis(300)),
        0,
        "the fit asked for math"
    );
    let vp = Viewport::new(Point::new(1.2, 0.3), 4.0, 600, 200, 1.0);
    renderer.render(&board, &vp);
    assert_eq!(renderer.math_pending(), 1);
    assert_eq!(wait_for(1, Duration::from_secs(60)), 1);
    // The first formula to finish is the one in view.
    let buf = renderer.render(&board, &vp);
    assert!(ink_pixels(buf) > 100);
    assert_eq!((grey_pixels(buf), renderer.math_pending()), (0, 0));
    // The one off screen was never asked for.
    assert_eq!(wait_for(2, Duration::from_millis(500)), 1);

    // In view, it is asked for.
    let there = Viewport::new(Point::new(1001.0, 0.3), 4.0, 600, 200, 1.0);
    renderer.render(&board, &there);
    assert_eq!(renderer.math_pending(), 1);
}
