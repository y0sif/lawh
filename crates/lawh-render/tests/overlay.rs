//! What the renderer tells the app about the board it draws: where the caret stands in a
//! block, which object a click hits, the bounds the engine measures with, and what the overlay
//! leaves of the board.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use kurbo::{Point, Rect};
use lawh_core::engine::Measure;
use lawh_core::{Board, Object, ObjectId, Shape};
use lawh_render::{
    Caret, Overlay, OverlayCursor, Renderer, StatusLine, Viewport, status_bar_height_px,
};

/// Where the test texts start.
const AT: Point = Point::new(2.0, 3.0);

fn id(name: &str) -> ObjectId {
    ObjectId::new(name).unwrap()
}

fn text(name: &str, at: Point, text: &str) -> Object {
    Object::new(
        id(name),
        Shape::Text {
            at,
            text: text.to_owned(),
            size: lawh_core::board::DEFAULT_TEXT_SIZE,
        },
    )
}

fn board_of(objects: impl IntoIterator<Item = Object>) -> Board {
    let mut board = Board::new();
    for object in objects {
        board.insert(object).unwrap();
    }
    board
}

/// The caret before `byte` of a text `t1` holding `content` at [`AT`].
fn caret_in(renderer: &mut Renderer, content: &str, byte: usize) -> Rect {
    let board = board_of([text("t1", AT, content)]);
    renderer.caret_rect(&board, &id("t1"), byte).unwrap()
}

/// The bounds of a text holding `content` at [`AT`].
fn text_bounds(renderer: &mut Renderer, content: &str) -> Rect {
    renderer.object_bounds(&text("t1", AT, content)).unwrap()
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn the_caret_stands_before_its_byte_one_line_high() {
    let mut renderer = Renderer::new_trusted();
    let line = text_bounds(&mut renderer, "hello world").height();
    let start = caret_in(&mut renderer, "hello world", 0);
    assert!(near(start.x0, AT.x) && near(start.y0, AT.y), "{start:?}");
    assert!(
        near(start.width(), 0.0) && near(start.height(), line),
        "{start:?}"
    );

    // In the middle of a word: as far in as the text before it reaches.
    let hello = text_bounds(&mut renderer, "hello").width();
    let middle = caret_in(&mut renderer, "hello world", 5);
    assert!(
        near(middle.x0, AT.x + hello),
        "{middle:?}, hello is {hello} wide"
    );
    assert!(near(middle.y0, AT.y) && near(middle.height(), line));

    // At the end: at the right edge of the text's bounds. Past the end, the same.
    let bounds = text_bounds(&mut renderer, "hello world");
    let end = caret_in(&mut renderer, "hello world", 11);
    assert!(near(end.x0, bounds.x1), "{end:?} {bounds:?}");
    assert_eq!(caret_in(&mut renderer, "hello world", 999), end);
}

#[test]
fn the_caret_on_a_second_line_stands_a_line_down() {
    let mut renderer = Renderer::new_trusted();
    let line = caret_in(&mut renderer, "ab\ncd", 0).height();
    // The start of the second line.
    let second = caret_in(&mut renderer, "ab\ncd", 3);
    assert!(near(second.x0, AT.x), "{second:?}");
    assert!(
        near(second.y0, AT.y + line) && near(second.height(), line),
        "{second:?}"
    );
    // The end of the first line, before its line break.
    let ab = text_bounds(&mut renderer, "ab").width();
    let first_end = caret_in(&mut renderer, "ab\ncd", 2);
    assert!(
        near(first_end.x0, AT.x + ab) && near(first_end.y0, AT.y),
        "{first_end:?}"
    );
    // An empty last line.
    let last = caret_in(&mut renderer, "ab\n", 3);
    assert!(
        near(last.x0, AT.x) && near(last.y0, AT.y + line),
        "{last:?}"
    );
}

#[test]
fn the_caret_in_an_empty_text_is_one_line_high_at_its_corner() {
    let mut renderer = Renderer::new_trusted();
    let line = caret_in(&mut renderer, "x", 0).height();
    assert!(line > 0.3, "{line}");
    let empty = caret_in(&mut renderer, "", 0);
    assert!(near(empty.x0, AT.x) && near(empty.y0, AT.y), "{empty:?}");
    assert!(
        near(empty.width(), 0.0) && near(empty.height(), line),
        "{empty:?}"
    );
}

#[test]
fn a_combining_accent_is_one_step_for_the_caret() {
    let mut renderer = Renderer::new_trusted();
    // "e" + U+0301 COMBINING ACUTE ACCENT, then "x": the accent takes no room of its own.
    let accented = "e\u{301}x";
    let start = caret_in(&mut renderer, accented, 0);
    assert!(near(start.x0, AT.x));
    // Inside the grapheme: at its start.
    assert_eq!(caret_in(&mut renderer, accented, 1), start);
    // After it: where the caret after a plain "e" stands.
    let after = caret_in(&mut renderer, accented, 3);
    let plain = caret_in(&mut renderer, "ex", 1);
    assert!(
        after.x0 > AT.x && near(after.x0, plain.x0),
        "{after:?} {plain:?}"
    );
    let end = caret_in(&mut renderer, accented, 4);
    assert!(near(end.x0, text_bounds(&mut renderer, accented).x1));
}

#[test]
fn a_symbol_from_the_math_font_is_one_step_for_the_caret_and_is_hit() {
    let mut renderer = Renderer::new_trusted();
    // U+2227 LOGICAL AND: Inter lacks it, so it is set in Libertinus Math.
    let content = "a\u{2227}b";
    let before = caret_in(&mut renderer, content, 1);
    let after = caret_in(&mut renderer, content, 4);
    let a = text_bounds(&mut renderer, "a").width();
    let a_and = text_bounds(&mut renderer, "a\u{2227}").width();
    assert!(near(before.x0, AT.x + a), "{before:?}, a is {a} wide");
    assert!(
        near(after.x0, AT.x + a_and),
        "{after:?}, a and is {a_and} wide"
    );
    // Inside its three bytes: at its start.
    assert_eq!(caret_in(&mut renderer, content, 2), before);
    assert_eq!(caret_in(&mut renderer, content, 3), before);
    // It is as wide as Libertinus Math's glyph, at text's em of 0.5 units.
    let face = ttf_parser::Face::parse(lawh_math::fonts()[1].data, 0).unwrap();
    let glyph = face.glyph_index('\u{2227}').unwrap();
    let advance = f64::from(face.glyph_hor_advance(glyph).unwrap());
    let em = advance / f64::from(face.units_per_em()) * lawh_render::TEXT_SIZE_UNITS;
    assert!(near(after.x0 - before.x0, em), "{before:?} {after:?} {em}");
    // The end of the text, and a line as high as the text.
    let bounds = text_bounds(&mut renderer, content);
    assert!(near(caret_in(&mut renderer, content, 5).x0, bounds.x1));
    assert!(
        near(after.height(), bounds.height()),
        "{after:?} {bounds:?}"
    );
    // A click on the symbol hits the text.
    let board = board_of([text("t1", AT, content)]);
    let on_symbol = Point::new(f64::midpoint(before.x0, after.x0), before.center().y);
    assert_eq!(renderer.hit_test(&board, on_symbol, 0.0), Some(id("t1")));
}

#[test]
fn only_texts_and_formulas_have_a_caret() {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    // A formula's caret is in its source, laid out as text.
    let math = renderer.caret_rect(&board, &id("m1"), 0).unwrap();
    assert!(near(math.x0, 5.0) && near(math.y0, 0.0), "{math:?}");
    assert_eq!(renderer.caret_rect(&board, &id("c1"), 0), None);
    assert_eq!(renderer.caret_rect(&board, &id("nope"), 0), None);
}

fn rect(name: &str, x: f64, y: f64, w: f64, h: f64) -> Object {
    Object::new(
        id(name),
        Shape::Rect {
            origin: Point::new(x, y),
            w,
            h,
        },
    )
}

#[test]
fn a_click_hits_the_topmost_object_under_it() {
    let board = board_of([
        rect("r1", 0.0, 0.0, 4.0, 4.0),
        rect("r2", 2.0, 2.0, 4.0, 4.0),
        text("t1", Point::new(10.0, 0.0), "hello"),
    ]);
    let mut renderer = Renderer::new_trusted();
    let hit = |renderer: &mut Renderer, x: f64, y: f64| {
        renderer
            .hit_test(&board, Point::new(x, y), 0.0)
            .map(|id| id.as_str().to_owned())
    };
    assert_eq!(hit(&mut renderer, 3.0, 3.0).as_deref(), Some("r2"));
    assert_eq!(hit(&mut renderer, 1.0, 1.0).as_deref(), Some("r1"));
    assert_eq!(hit(&mut renderer, 5.0, 5.0).as_deref(), Some("r2"));
    // Inside a text's laid-out bounds.
    let hello = renderer.object_bounds(&board.objects()[2]).unwrap();
    let inside = hello.center();
    assert_eq!(
        hit(&mut renderer, inside.x, inside.y).as_deref(),
        Some("t1")
    );
    // Empty space, and points that aren't.
    assert_eq!(hit(&mut renderer, 8.0, 8.0), None);
    assert_eq!(hit(&mut renderer, hello.x1 + 0.1, inside.y), None);
    assert_eq!(hit(&mut renderer, f64::NAN, 1.0), None);
}

#[test]
fn a_thin_line_is_hit_within_the_tolerance() {
    let board = board_of([Object::new(
        id("l1"),
        Shape::Line {
            a: Point::new(0.0, 0.0),
            b: Point::new(4.0, 0.0),
        },
    )]);
    let mut renderer = Renderer::new_trusted();
    let above = Point::new(2.0, -0.1);
    assert_eq!(renderer.hit_test(&board, above, 0.05), None);
    assert_eq!(renderer.hit_test(&board, above, 0.2), Some(id("l1")));
    // Past its end, within the tolerance too.
    assert_eq!(
        renderer.hit_test(&board, Point::new(4.15, 0.0), 0.2),
        Some(id("l1"))
    );
    // On the line itself, with no tolerance (or one that isn't a number).
    for tolerance in [0.0, -1.0, f64::NAN] {
        assert_eq!(
            renderer.hit_test(&board, Point::new(2.0, 0.0), tolerance),
            Some(id("l1")),
            "{tolerance}"
        );
        assert_eq!(renderer.hit_test(&board, above, tolerance), None);
    }
}

fn segment(name: &str, arrow: bool, a: (f64, f64), b: (f64, f64)) -> Object {
    let (a, b) = (Point::new(a.0, a.1), Point::new(b.0, b.1));
    let shape = if arrow {
        Shape::Arrow { a, b }
    } else {
        Shape::Line { a, b }
    };
    Object::new(id(name), shape)
}

fn ellipse(name: &str, x: f64, y: f64, rx: f64, ry: f64) -> Object {
    Object::new(
        id(name),
        Shape::Ellipse {
            center: Point::new(x, y),
            rx,
            ry,
        },
    )
}

/// The id `hit_test` returns at `(x, y)` within `tolerance`.
fn hit_at(
    renderer: &mut Renderer,
    board: &Board,
    x: f64,
    y: f64,
    tolerance: f64,
) -> Option<String> {
    renderer
        .hit_test(board, Point::new(x, y), tolerance)
        .map(|id| id.as_str().to_owned())
}

#[test]
fn a_click_on_a_text_inside_a_rectangle_selects_the_text() {
    // `:te hi` then `:re 6 2` at the same point: the rectangle is drawn on top of the text.
    let board = board_of([
        text("t1", Point::ZERO, "hi"),
        rect("r1", 0.0, 0.0, 6.0, 2.0),
    ]);
    let mut renderer = Renderer::new_trusted();
    let hi = renderer
        .object_bounds(&board.objects()[0])
        .unwrap()
        .center();
    assert_eq!(
        hit_at(&mut renderer, &board, hi.x, hi.y, 0.1).as_deref(),
        Some("t1")
    );
    // Inside the rectangle, away from the text.
    assert_eq!(
        hit_at(&mut renderer, &board, 4.0, 1.5, 0.1).as_deref(),
        Some("r1")
    );
}

#[test]
fn a_line_or_an_arrow_is_hit_near_its_segment_not_in_its_box() {
    // A long diagonal arrow drawn after a small rectangle inside its bounding box.
    let board = board_of([
        rect("r1", 7.0, 1.0, 1.0, 1.0),
        segment("a1", true, (0.0, 0.0), (10.0, 10.0)),
    ]);
    let mut renderer = Renderer::new_trusted();
    assert_eq!(
        hit_at(&mut renderer, &board, 7.5, 1.5, 0.1).as_deref(),
        Some("r1")
    );
    // In the arrow's box, 2 units from its shaft.
    assert_eq!(hit_at(&mut renderer, &board, 6.0, 3.0, 0.1), None);
    // 0.07 units from the shaft.
    assert_eq!(
        hit_at(&mut renderer, &board, 5.0, 5.1, 0.1).as_deref(),
        Some("a1")
    );
    assert_eq!(hit_at(&mut renderer, &board, 5.0, 5.2, 0.1), None);

    // A line across a rectangle drawn after it: near the line, the line (it covers no area).
    let board = board_of([
        segment("l1", false, (-1.0, 0.5), (11.0, 0.5)),
        rect("r2", 0.0, 0.0, 10.0, 10.0),
    ]);
    assert_eq!(
        hit_at(&mut renderer, &board, 5.0, 0.55, 0.1).as_deref(),
        Some("l1")
    );
    assert_eq!(
        hit_at(&mut renderer, &board, 5.0, 5.0, 0.1).as_deref(),
        Some("r2")
    );
}

#[test]
fn an_ellipse_is_hit_in_its_shape_not_in_the_corners_of_its_box() {
    let board = board_of([
        ellipse("e1", 0.0, 0.0, 4.0, 2.0),
        ellipse("e2", 10.0, 0.0, 1.0, 3.0),
    ]);
    let mut renderer = Renderer::new_trusted();
    let mut hit = |x: f64, y: f64| hit_at(&mut renderer, &board, x, y, 0.1);
    assert_eq!(hit(0.0, 0.0).as_deref(), Some("e1"));
    assert_eq!(hit(3.0, 1.0).as_deref(), Some("e1"));
    // The corner of its box.
    assert_eq!(hit(3.9, 1.9), None);
    // Just outside its outline, at the ends of its axes, within the tolerance and past it.
    assert_eq!(hit(4.05, 0.0).as_deref(), Some("e1"));
    assert_eq!(hit(4.2, 0.0), None);
    assert_eq!(hit(0.0, -2.05).as_deref(), Some("e1"));
    assert_eq!(hit(0.0, -2.2), None);
    // Off the outline at 45 degrees, along its normal: 0.08 units out, then 0.12.
    let on = Point::new(4.0 * 0.5_f64.sqrt(), 2.0 * 0.5_f64.sqrt());
    let normal = kurbo::Vec2::new(2.0, 4.0).normalize();
    let out = |d: f64| on + normal * d;
    assert_eq!(hit(out(0.08).x, out(0.08).y).as_deref(), Some("e1"));
    assert_eq!(hit(out(0.12).x, out(0.12).y), None);
    // A tall one (its long radius along y).
    assert_eq!(hit(10.0, 3.05).as_deref(), Some("e2"));
    assert_eq!(hit(10.95, 2.9), None);
    assert_eq!(hit(10.5, 0.0).as_deref(), Some("e2"));
}

#[test]
fn the_smallest_object_hit_wins_then_the_topmost() {
    let board = board_of([
        rect("small", 2.0, 2.0, 2.0, 2.0),
        ellipse("c1", 5.0, 5.0, 5.0, 5.0),
        rect("big", 0.0, 0.0, 10.0, 10.0),
        rect("same", 0.0, 0.0, 10.0, 10.0),
    ]);
    let mut renderer = Renderer::new_trusted();
    let mut hit = |x: f64, y: f64| hit_at(&mut renderer, &board, x, y, 0.0);
    assert_eq!(hit(3.0, 3.0).as_deref(), Some("small"));
    // A circle covers less than the square around it.
    assert_eq!(hit(5.0, 5.0).as_deref(), Some("c1"));
    // In the square's corner, outside the circle: the topmost of two equal squares.
    assert_eq!(hit(0.5, 0.5).as_deref(), Some("same"));
}

#[test]
fn the_renderer_measures_objects_as_object_bounds_does() {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    for resolved in [false, true] {
        if resolved {
            renderer.resolve_math(&board);
        }
        for object in board.objects() {
            let measured = Measure::bounds(&mut renderer, object);
            assert!(measured.is_some(), "{}", object.id.as_str());
            assert_eq!(
                measured,
                renderer.object_bounds(object),
                "{} (math resolved: {resolved})",
                object.id.as_str()
            );
        }
    }
    // Text and math boxes come from shaping and Typst, not a fixed width per character.
    let t1 = board.get(&id("t1")).unwrap();
    let fixed = lawh_core::FixedMeasure.bounds(t1).unwrap();
    assert_ne!(Measure::bounds(&mut renderer, t1), Some(fixed));
}

/// A text or (`math`) a formula `name` holding `content` at `at`, of size `size`.
fn sized(name: &str, at: Point, content: &str, size: f64, math: bool) -> Object {
    let shape = if math {
        Shape::Math {
            at,
            src: content.to_owned(),
            size,
        }
    } else {
        Shape::Text {
            at,
            text: content.to_owned(),
            size,
        }
    };
    Object::new(id(name), shape)
}

/// `rect` scaled by `size` about `at`.
fn scaled(rect: Rect, at: Point, size: f64) -> Rect {
    Rect::new(
        at.x + (rect.x0 - at.x) * size,
        at.y + (rect.y0 - at.y) * size,
        at.x + (rect.x1 - at.x) * size,
        at.y + (rect.y1 - at.y) * size,
    )
}

fn near_rect(a: Rect, b: Rect) -> bool {
    near(a.x0, b.x0) && near(a.y0, b.y0) && near(a.x1, b.x1) && near(a.y1, b.y1)
}

#[test]
fn the_caret_and_clicks_follow_the_size() {
    let mut renderer = Renderer::new_trusted();
    for math in [false, true] {
        let one = board_of([sized("b1", AT, "hello world", 1.0, math)]);
        let at_one = renderer.caret_rect(&one, &id("b1"), 5).unwrap();
        let block_bounds = renderer.object_bounds(&one.objects()[0]).unwrap();
        for size in [0.5, 2.0, 4.0] {
            let board = board_of([sized("b1", AT, "hello world", size, math)]);
            // A formula with the caret in it is drawn as its source, at its size.
            let caret = renderer.caret_rect(&board, &id("b1"), 5).unwrap();
            assert!(
                near_rect(caret, scaled(at_one, AT, size)),
                "size {size} (math: {math}): {caret:?}, at size 1 {at_one:?}"
            );
            // A click near the far corner of the sized block hits it; at size 1 that is past it.
            let corner = scaled(block_bounds, AT, size);
            let click = Point::new(
                AT.x + (corner.x1 - AT.x) * 0.95,
                AT.y + (corner.y1 - AT.y) * 0.95,
            );
            assert_eq!(renderer.hit_test(&board, click, 0.0), Some(id("b1")));
            if size > 1.0 {
                assert_eq!(renderer.hit_test(&one, click, 0.0), None, "size {size}");
            }
        }
    }
}

#[test]
fn the_engine_measures_text_and_math_by_their_size_and_the_math_scale() {
    let src = "A = pi r^2";
    let mut renderer = Renderer::new_trusted();
    let layout = lawh_math::MathEngine::new().layout_blocking(src).unwrap();
    // What the formula covers in pt: its box joined with its ink.
    let pt = Rect::new(0.0, 0.0, layout.width, layout.height).union(layout.ink);
    for size in [0.25, 0.5, 1.0, 2.0, 4.0, 16.0] {
        let text = sized("t1", AT, src, size, false);
        let math = sized("m1", AT, src, size, true);
        let board = board_of([text.clone(), math.clone()]);
        renderer.resolve_math(&board);
        let text_one = renderer
            .object_bounds(&sized("t1", AT, src, 1.0, false))
            .unwrap();
        let measured = Measure::bounds(&mut renderer, &text).unwrap();
        assert!(
            near_rect(measured, scaled(text_one, AT, size)),
            "size {size}"
        );
        // Math is drawn about 1.27 times as large as text of its size (a pt is 1/40 of a
        // grid unit at size 1 before that), so its x-height matches the text's.
        let measured = Measure::bounds(&mut renderer, &math).unwrap();
        let per_pt = measured.width() / pt.width() / size;
        assert!(
            (per_pt * 40.0 - 1.2725).abs() < 1e-9,
            "size {size}: {per_pt} units a pt"
        );
        assert!(near(measured.height() / pt.height() / size, per_pt));
        assert!(near(measured.x0, AT.x), "size {size}: {measured:?}");
    }
}

#[test]
fn the_overlay_leaves_the_board_as_render_draws_it_above_the_status_bar() {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    renderer.resolve_math(&board);
    let bounds = renderer.board_bounds(&board).unwrap();
    for scale in [1.0, 2.0] {
        let vp = Viewport::fit(bounds, 400, 300, scale);
        let plain = renderer.render(&board, &vp).to_vec();
        let with = renderer
            .render_with(&board, &vp, &Overlay::default())
            .to_vec();
        let bar = status_bar_height_px(scale) as usize;
        let above = (300 - bar) * 400 * 4;
        assert_eq!(plain[..above], with[..above], "scale {scale}");
        // The bar itself, a shade darker than the paper, along the bottom.
        let bottom = common::pixel(&with, 400, 200, 299);
        assert_ne!(bottom, common::pixel(&plain, 400, 200, 299));
        assert!(bottom[0] > 0xE0 && bottom[0] < 0xFA, "{bottom:?}");
        assert_eq!(renderer.last_frame().drawn, board.len());
    }
}

/// Pixels of the cursor's blue (#2563EB), anti-aliased edges aside.
fn blue_pixels(buf: &[u8]) -> usize {
    buf.as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[2] > 180 && px[0] < 120 && px[1] < 150)
        .count()
}

#[test]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the pixel looked at is inside a 600x300 image"
)]
fn the_caret_and_cursor_are_drawn_in_blue_where_they_are() {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    renderer.resolve_math(&board);
    let vp = Viewport::new(Point::new(2.0, -4.0), 3.0, 600, 300, 1.0);
    let t1 = id("t1");
    let plain = renderer
        .render_with(&board, &vp, &Overlay::default())
        .to_vec();
    assert_eq!(blue_pixels(&plain), 0);
    // A bar caret in "Area": as tall as a line of text at zoom 3 (0.5 units an em, 120 px a
    // unit), 2 px wide.
    let overlay = Overlay {
        caret: Some(Caret {
            id: &t1,
            byte: 2,
            block: false,
        }),
        ..Overlay::default()
    };
    let caret = renderer.render_with(&board, &vp, &overlay).to_vec();
    let bar = blue_pixels(&caret);
    assert!((60..400).contains(&bar), "{bar}");
    let at = renderer.caret_rect(&board, &t1, 2).unwrap();
    let screen = vp.to_screen(Point::new(at.x0, at.center().y));
    let (x, y) = (screen.x.round() as u16, screen.y.round() as u16);
    let px = common::pixel(&caret, 600, x, y);
    assert!(px[2] > 180 && px[0] < 120, "{px:?} at {x},{y}");
    // The grid cursor's cross, and the current object's outline, each in blue too.
    for cursor in [
        OverlayCursor::Grid(Point::new(0.0, -3.0)),
        OverlayCursor::Object(&t1),
    ] {
        let overlay = Overlay {
            cursor,
            status: StatusLine {
                left: "NORMAL",
                right: "0,-3",
                error: false,
            },
            ..Overlay::default()
        };
        let image = renderer.render_with(&board, &vp, &overlay);
        assert!(blue_pixels(image) > 20, "{cursor:?}");
    }
}

/// Pixels of dark ink or of the error red in the columns `x0..x1` of the status bar of a frame
/// `width` x `height` pixels.
fn status_text_pixels(image: &[u8], width: u16, height: u16, x0: u16, x1: u16) -> (usize, usize) {
    let bar = u16::try_from(status_bar_height_px(1.0)).unwrap();
    let (mut ink, mut red) = (0, 0);
    for y in height - bar..height {
        for x in x0..x1 {
            let px = common::pixel(image, width, x, y);
            ink += usize::from(px[0] < 100 && px[1] < 100 && px[2] < 100);
            red += usize::from(px[0] > 150 && px[1] < 100 && px[2] < 100);
        }
    }
    (ink, red)
}

#[test]
fn the_status_line_shows_both_sides_whatever_their_length() {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    let vp = Viewport::new(Point::ZERO, 1.0, 600, 200, 1.0);
    let long = format!("{}  3,4", "very/long/path/".repeat(100));
    for (left, right, error) in [
        ("NORMAL", "area.lawh  0,0", false),
        ("NORMAL", long.as_str(), false),
        ("no file name", long.as_str(), true),
    ] {
        let overlay = Overlay {
            status: StatusLine { left, right, error },
            ..Overlay::default()
        };
        let image = renderer.render_with(&board, &vp, &overlay).to_vec();
        let (ink, red) = status_text_pixels(&image, 600, 200, 0, 150);
        if error {
            assert!(red > 20 && ink == 0, "{left:?}: {ink} ink, {red} red");
        } else {
            assert!(ink > 20 && red == 0, "{left:?}: {ink} ink, {red} red");
        }
        let (ink, _) = status_text_pixels(&image, 600, 200, 450, 600);
        assert!(ink > 20, "{right:?}");
    }
    // A command line far longer than the bar keeps its caret in view, left of the right text.
    let line = "x".repeat(2_000);
    let overlay = Overlay {
        status: StatusLine {
            left: "",
            right: "area.lawh  0,0",
            error: false,
        },
        command_line: Some((&line, line.len())),
        ..Overlay::default()
    };
    let image = renderer.render_with(&board, &vp, &overlay).to_vec();
    let caret = status_blue_columns(&image, 600, 200);
    assert!(
        !caret.is_empty() && caret.iter().all(|x| (300..500).contains(x)),
        "{caret:?}"
    );
}

/// The columns of the status bar with the cursor's blue in them.
fn status_blue_columns(image: &[u8], width: u16, height: u16) -> Vec<u16> {
    let bar = u16::try_from(status_bar_height_px(1.0)).unwrap();
    (0..width)
        .filter(|x| {
            (height - bar..height).any(|y| {
                let px = common::pixel(image, width, *x, y);
                px[2] > 180 && px[0] < 120
            })
        })
        .collect()
}
