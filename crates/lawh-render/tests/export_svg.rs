//! SVG exports with the real renderer: drawn by an independent rasterizer (resvg) they match the
//! PNG export, their XML holds only geometry and references inside the file, no board text
//! reaches the file, and every cap holds.
//!
//! The snapshot in `tests/snapshots/` is a small board's whole file; `INSTA_UPDATE=always cargo
//! test -p lawh-render --test export_svg` rewrites it. Read the new file before committing it.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Cursor;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

use lawh_core::{
    Board, ExportBackground, ExportFormat, ExportOptions, ExportReport, Object, ObjectId, Point,
    Shape,
};
use lawh_render::{
    EXPORT_STROKE_UNITS, ExportError, ExportLimits, Exported, MAX_EXPORT_BYTES, MAX_EXPORT_SIDE_PX,
    Palette, Renderer, Viewport,
};

/// Most a pixel inside a shape, a glyph or the paper (its 3 x 3 neighbourhood the same colour
/// in the PNG, within 3 a channel) may differ between resvg's drawing of an SVG export and the
/// PNG export, a channel. Away from edges both draw the same colours: measured 0 on
/// [`mixed_board`]; 2 allows for rounding in blending a translucent colour.
const INTERIOR_TOLERANCE: u8 = 2;

/// Most a pixel on an edge (any other pixel) may differ, a channel. vello works out how much of
/// a pixel a shape covers exactly; resvg (tiny-skia) samples a pixel's coverage four times
/// along y, so on a horizontal edge it can be off by an eighth of the pixel either way, about
/// 30 of the 219 levels between paper and ink, more where edges meet. Measured on
/// [`mixed_board`]: at most 56, over 9,332 edge pixels of 227,652 (369 of them past 20). A
/// quarter of full coverage (64) allows that and still fails a glyph or a line one pixel off,
/// or another colour, which differ by up to 219 there.
const EDGE_TOLERANCE: u8 = 64;

fn svg_options() -> ExportOptions {
    ExportOptions {
        format: ExportFormat::Svg,
        ..ExportOptions::default()
    }
}

fn export(board: &Board) -> Exported {
    Renderer::new_trusted()
        .export(board, None, &svg_options())
        .unwrap()
}

fn export_with(board: &Board, limits: &ExportLimits) -> Result<Exported, ExportError> {
    Renderer::new_trusted().export_with_limits(board, None, &svg_options(), limits)
}

fn text(exported: &Exported) -> &str {
    std::str::from_utf8(&exported.bytes).unwrap()
}

fn id(name: &str) -> ObjectId {
    ObjectId::new(name).unwrap()
}

/// An image as straight (not premultiplied) RGBA pixels.
struct Image {
    width: u32,
    height: u32,
    pixels: Vec<[u8; 4]>,
}

/// `svg` drawn by resvg, `scale` times its nominal size.
fn rasterize(svg: &[u8], scale: f32) -> Image {
    let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default()).unwrap();
    let size = tree.size().to_int_size().scale_by(scale).unwrap();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height()).unwrap();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let pixels = pixmap
        .pixels()
        .iter()
        .map(|px| {
            let px = px.demultiply();
            [px.red(), px.green(), px.blue(), px.alpha()]
        })
        .collect();
    Image {
        width: size.width(),
        height: size.height(),
        pixels,
    }
}

/// A PNG export decoded.
fn decode_png(bytes: &[u8]) -> Image {
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder.read_info().unwrap();
    let mut data = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut data).unwrap();
    let channels = match frame.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other => panic!("{other:?}"),
    };
    let pixels = data[..frame.buffer_size()]
        .chunks_exact(channels)
        .map(|px| [px[0], px[1], px[2], if channels == 4 { px[3] } else { 255 }])
        .collect();
    Image {
        width: frame.width,
        height: frame.height,
        pixels,
    }
}

/// The largest difference between two pixels, a channel.
fn difference(a: [u8; 4], b: [u8; 4]) -> u8 {
    (0..4).map(|c| a[c].abs_diff(b[c])).max().unwrap()
}

/// Text at two sizes, a formula on a text's baseline, a translucent coloured formula, `∧` from
/// the fallback font, and every shape kind.
fn mixed_board() -> Board {
    common::board(
        "lawh version=1\n\
         text id=t1 x=0 y=0 \"Size 1: x =\"\n\
         math id=m1 x=3.5 y=0 \"x = pi r^2\"\n\
         text id=t2 x=0 y=1.5 size=2 \"p \u{2227} q\"\n\
         math id=m2 x=8 y=1.5 \"#text(fill: rgb(\\\"#0000ff80\\\"))[$sqrt(y)$]\"\n\
         ellipse id=e1 x=12 y=2 rx=1.5 ry=1\n\
         rect id=r1 x=9 y=4 w=3 h=1.5\n\
         line id=l1 x1=0 y1=5 x2=6 y2=5\n\
         arrow id=a1 x1=0 y1=6 x2=6 y2=7\n",
    )
}

/// Element names an export may hold.
const ELEMENTS: [&str; 6] = ["svg", "defs", "path", "g", "use", "rect"];

/// Checks `svg` is well-formed XML that holds only geometry: the elements of [`ELEMENTS`] in
/// the SVG namespace, glyph definitions with generated ids, each defined once, inside
/// `<defs>`, every reference `#` and one of them, defined before it is used. Returns how many
/// `<use>`s, and `<path>`s outside `<defs>`, it holds.
fn check_structure(svg: &str) -> (usize, usize) {
    // Only printable ASCII and line feeds: no character board text could have brought.
    assert!(
        svg.bytes().all(|b| b == b'\n' || (0x20..0x7F).contains(&b)),
        "a byte outside printable ASCII"
    );
    for word in ["NaN", "inf", "&", "'", "<!", "url("] {
        assert!(!svg.contains(word), "{word}");
    }
    let doc = roxmltree::Document::parse(svg).unwrap();
    let root = doc.root_element();
    assert_eq!(root.tag_name().name(), "svg");
    let mut defined = HashSet::new();
    let (mut uses, mut paths) = (0, 0);
    for node in doc.descendants().filter(roxmltree::Node::is_element) {
        let name = node.tag_name().name();
        assert!(ELEMENTS.contains(&name), "<{name}>");
        assert_eq!(
            node.tag_name().namespace(),
            Some("http://www.w3.org/2000/svg"),
            "<{name}>"
        );
        for attribute in node.attributes() {
            let value = attribute.value();
            match attribute.name() {
                "href" => {
                    assert_eq!(
                        attribute.namespace(),
                        Some("http://www.w3.org/1999/xlink"),
                        "{value}"
                    );
                    let target = value
                        .strip_prefix('#')
                        .expect("a reference inside the file");
                    assert!(
                        defined.contains(target),
                        "{value} not defined before its use"
                    );
                }
                "id" => {
                    let number = value.strip_prefix('g').unwrap();
                    assert!(number.bytes().all(|b| b.is_ascii_digit()), "{value}");
                    assert!(defined.insert(value.to_owned()), "{value} defined twice");
                    assert_eq!(name, "path");
                    assert_eq!(node.parent_element().unwrap().tag_name().name(), "defs");
                }
                other => assert!(attribute.namespace().is_none(), "{other}={value}"),
            }
        }
        let in_defs = node
            .ancestors()
            .any(|ancestor| ancestor.tag_name().name() == "defs");
        match name {
            "use" => uses += 1,
            "path" if !in_defs => paths += 1,
            _ => {}
        }
    }
    // Glyphs are numbered in the order they are defined.
    for i in 0..defined.len() {
        assert!(defined.contains(&format!("g{i}")), "g{i}");
    }
    (uses, paths)
}

/// The `width` and `height` of an SVG's root, and its `viewBox`'s.
fn sizes(svg: &str) -> ((f64, f64), (f64, f64)) {
    let doc = roxmltree::Document::parse(svg).unwrap();
    let root = doc.root_element();
    let get = |name| root.attribute(name).unwrap().parse::<f64>().unwrap();
    let view: Vec<f64> = root
        .attribute("viewBox")
        .unwrap()
        .split(' ')
        .map(|v| v.parse().unwrap())
        .collect();
    assert_eq!(view[..2], [0.0, 0.0]);
    ((get("width"), get("height")), (view[2], view[3]))
}

/// The `width` and `height` of an SVG's root, which its `viewBox` matches.
fn nominal_size(svg: &str) -> (f64, f64) {
    let doc = roxmltree::Document::parse(svg).unwrap();
    let root = doc.root_element();
    let get = |name| root.attribute(name).unwrap().parse::<f64>().unwrap();
    assert_eq!(
        root.attribute("viewBox").unwrap(),
        format!("0 0 {} {}", get("width"), get("height"))
    );
    (get("width"), get("height"))
}

#[test]
fn a_small_board_exports_as_its_snapshot() {
    let board = common::board(
        "lawh version=1\n\
         text id=t1 x=0 y=0 \"Hi\"\n\
         math id=m1 x=2 y=0 \"x^2\"\n\
         rect id=r1 x=0 y=2 w=2 h=1\n\
         ellipse id=e1 x=4 y=2.5 rx=0.5 ry=0.5\n\
         arrow id=a1 x1=0 y1=4 x2=3 y2=4\n",
    );
    let exported = export(&board);
    assert_eq!(exported.report, ExportReport::default());
    insta::assert_snapshot!(text(&exported));
}

/// resvg's drawing of an SVG export is the PNG export at the same scale (scale 1, 40 pixels a
/// unit): the same size, the same colours inside every shape, glyph and the paper, and edges
/// within what the two rasterizers' anti-aliasing tells apart.
#[test]
fn svg_draws_as_the_png_export_does() {
    let board = mixed_board();
    let mut renderer = Renderer::new_trusted();
    let svg = renderer.export(&board, None, &svg_options()).unwrap();
    let png_options = ExportOptions {
        png_scale_hundredths: 100,
        ..ExportOptions::default()
    };
    let png = renderer.export(&board, None, &png_options).unwrap();
    let drawn = rasterize(&svg.bytes, 1.0);
    let expected = decode_png(&png.bytes);
    assert_eq!(
        (drawn.width, drawn.height),
        (expected.width, expected.height)
    );
    assert_eq!(
        nominal_size(text(&svg)),
        (f64::from(expected.width), f64::from(expected.height))
    );
    let (w, h) = (expected.width as usize, expected.height as usize);
    let at = |x: usize, y: usize| expected.pixels[y * w + x];
    let (mut edges, mut worst_edge, mut worst_interior) = (0, 0, 0);
    for y in 0..h {
        for x in 0..w {
            let here = at(x, y);
            let edge = (y.saturating_sub(1)..(y + 2).min(h))
                .flat_map(|ny| (x.saturating_sub(1)..(x + 2).min(w)).map(move |nx| (nx, ny)))
                .any(|(nx, ny)| difference(at(nx, ny), here) > 3);
            let d = difference(drawn.pixels[y * w + x], here);
            if edge {
                edges += 1;
                worst_edge = worst_edge.max(d);
            } else {
                worst_interior = worst_interior.max(d);
            }
        }
    }
    println!(
        "{} pixels, {edges} on edges: worst difference {worst_edge} on an edge, \
         {worst_interior} elsewhere",
        w * h
    );
    assert!(worst_interior <= INTERIOR_TOLERANCE, "{worst_interior}");
    assert!(worst_edge <= EDGE_TOLERANCE, "{worst_edge}");
    // Something was drawn: the ink, the translucent blue, the paper.
    let ink = Palette::LIGHT.ink.to_rgba8().to_u8_array();
    let paper = Palette::LIGHT.paper.to_rgba8().to_u8_array();
    let count = |color: [u8; 4]| {
        drawn
            .pixels
            .iter()
            .filter(|px| difference(**px, color) <= 3)
            .count()
    };
    assert!(count(ink) > 1_000, "{}", count(ink));
    assert!(count(paper) > w * h / 2);
    assert!(drawn.pixels.iter().any(|px| px[2] > 200 && px[0] < 200));
}

#[test]
fn exports_hold_only_geometry_and_references_inside_the_file() {
    let exported = export(&mixed_board());
    let svg = text(&exported);
    let (uses, paths) = check_structure(svg);
    // A `<use>` for every glyph drawn: 9 in "Size 1: x =" (not the spaces), 3 in "p ∧ q", and
    // the formulas' (5 in `x = pi r^2`, 2 in `sqrt(y)`).
    assert!(uses >= 9 + 3 + 6, "{uses}");
    // The ellipse, the rect, the line, the arrow's shaft and head, and the root's bar.
    assert!(paths >= 6, "{paths}");
    // Colours from the palette, a translucent one with its opacity, strokes with round caps
    // and joins.
    assert!(svg.contains("fill=\"#1f1f1f\""));
    assert!(svg.contains("fill=\"#fafaf7\""));
    assert!(svg.contains("fill=\"#0000ff\" fill-opacity=\"0.502\""));
    let strokes = svg.matches(" stroke=\"").count();
    assert_eq!(strokes, svg.matches("stroke-linecap=\"round\"").count());
    assert_eq!(strokes, svg.matches("stroke-linejoin=\"round\"").count());
    assert!(strokes >= 5, "{strokes}");
    // The area board, and the board at lawh-core's limits.
    check_structure(text(&export(&common::area_board())));
    check_structure(text(&export(&common::limits_board())));
}

/// Board text never reaches the file, whatever it holds: markup, entities, a CDATA end,
/// quotes, and characters XML can't carry at all (U+0001, U+001F, U+FFFE), as text and as the
/// source of a formula that fails (drawn as its text).
#[test]
fn board_text_leaves_no_trace_in_the_file() {
    let hostile = "<script>alert(\"x\")</script> & &amp; ]]> 'q' \"q\" \u{1}\u{1F}\u{FFFE}\ta\r\nb";
    let mut board = Board::new();
    for (name, shape) in [
        (
            "t1",
            Shape::Text {
                at: Point::new(0.0, 0.0),
                text: hostile.to_owned(),
                size: 1.0,
            },
        ),
        (
            "m1",
            Shape::Math {
                at: Point::new(0.0, 3.0),
                src: hostile.to_owned(),
                size: 1.0,
            },
        ),
    ] {
        board.insert(Object::new(id(name), shape)).unwrap();
    }
    let exported = export(&board);
    assert_eq!(exported.report.failed_formulas, 1);
    let svg = text(&exported);
    let (uses, _) = check_structure(svg);
    assert!(uses > 20, "{uses}");
    for word in ["script", "alert", "]]>", "amp", "\t", "\r", "t1", "m1"] {
        assert!(!svg.contains(word), "{word:?}");
    }
    // The failed formula is drawn in the error colour.
    assert!(svg.contains("fill=\"#c0392b\""));
    // A reader that resolves the references takes it too.
    resvg::usvg::Tree::from_data(&exported.bytes, &resvg::usvg::Options::default()).unwrap();
}

#[test]
fn a_transparent_export_has_no_paper() {
    let board = common::area_board();
    let options = ExportOptions {
        background: ExportBackground::None,
        ..svg_options()
    };
    let clear = Renderer::new_trusted()
        .export(&board, None, &options)
        .unwrap();
    let paper = export(&board);
    assert!(!text(&clear).contains("<rect"));
    assert_eq!(text(&paper).matches("<rect").count(), 1);
    assert_eq!(nominal_size(text(&clear)), nominal_size(text(&paper)));
    let image = rasterize(&clear.bytes, 1.0);
    assert_eq!(image.pixels[0], [0, 0, 0, 0]);
    let see_through = image.pixels.iter().filter(|px| px[3] == 0).count();
    assert!(see_through > image.pixels.len() * 9 / 10, "{see_through}");
    assert!(image.pixels.iter().any(|px| px[3] == 255));
}

#[test]
fn a_selection_exports_only_its_objects_framed_to_them() {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    let circle = renderer
        .export(&board, Some(&[id("c1")]), &svg_options())
        .unwrap();
    let svg = text(&circle);
    assert_eq!(check_structure(svg), (0, 1));
    // 6 units across with its outline, and the margin, at 40 pixels a unit, in whole pixels.
    let side = ((6.0 + EXPORT_STROKE_UNITS + 2.0) * 40.0_f64).ceil();
    assert_eq!(nominal_size(svg), (side, side));
    // The formula and the title: their glyphs, framed to them.
    let two = renderer
        .export(&board, Some(&[id("m1"), id("t1")]), &svg_options())
        .unwrap();
    let (uses, paths) = check_structure(text(&two));
    assert!(uses > 10 && paths == 0, "{uses} {paths}");
    let whole = nominal_size(text(&export(&board)));
    let framed = nominal_size(text(&two));
    assert!(framed.1 < whole.1, "{framed:?} {whole:?}");
}

#[test]
fn the_byte_cap_stops_an_export_as_soon_as_it_is_passed() {
    let board = common::area_board();
    let full = export(&board).bytes.len();
    let limits = ExportLimits {
        max_bytes: full - 1,
        ..ExportLimits::DEFAULT
    };
    match export_with(&board, &limits) {
        Err(ExportError::TooLarge { bytes }) => assert_eq!(bytes, full),
        other => panic!("{other:?}"),
    }
    let limits = ExportLimits {
        max_bytes: full,
        ..ExportLimits::DEFAULT
    };
    assert_eq!(export_with(&board, &limits).unwrap().bytes.len(), full);
    // A board that writes megabytes stops a few bytes past a small cap: the file is counted as
    // it is written, not made first.
    let limits = ExportLimits {
        max_bytes: 10_000,
        ..ExportLimits::DEFAULT
    };
    match export_with(&texts_at_the_cap(), &limits) {
        Err(ExportError::TooLarge { bytes }) => assert!(bytes <= 10_000 + 400, "{bytes}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_budget_stops_an_export() {
    let board = common::area_board();
    for budget_ns in [0.0, 1e4, 3e4] {
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
        budget_ns: 1e6,
        ..ExportLimits::DEFAULT
    };
    assert!(export_with(&board, &limits).is_ok());
    // 40,000 characters of text, estimated at about 7 ms with their layout: over 2 ms.
    let limits = ExportLimits {
        budget_ns: 2e6,
        ..ExportLimits::DEFAULT
    };
    assert_eq!(
        export_with(&texts_at_the_cap(), &limits),
        Err(ExportError::OverBudget)
    );
}

#[test]
fn exports_are_the_same_every_time() {
    let board = mixed_board();
    let first = export(&board);
    assert_eq!(export(&board), first);
    // And from a renderer that has drawn the board in the window, and exported a PNG, first.
    let mut renderer = Renderer::new_trusted();
    renderer.render(&board, &Viewport::new(Point::ZERO, 2.0, 300, 200, 1.0));
    renderer
        .export(&board, None, &ExportOptions::default())
        .unwrap();
    assert_eq!(
        renderer.export(&board, None, &svg_options()).unwrap(),
        first
    );
}

/// The board at lawh-core's limits (shapes a million units across and out to 2,000,000 units,
/// a text of 5,000 characters on one line and one of 500 lines) exports within every cap, and
/// another SVG reader draws it.
#[test]
fn the_board_at_the_limits_exports_within_its_caps() {
    let board = common::limits_board();
    let exported = export(&board);
    assert!(exported.bytes.len() < MAX_EXPORT_BYTES);
    assert_eq!(exported.report.failed_formulas, 0);
    let svg = text(&exported);
    check_structure(svg);
    let ((width, height), (view_width, view_height)) = sizes(svg);
    // Drawn at 40 px a unit, from x = -1e6 to the second ellipse's right at 2e6 with the
    // margin, and from y = -1e6 to the many-line text's foot, past 1e6...
    assert!(
        view_width > 3e6 * 40.0 && view_width < 3.1e6 * 40.0,
        "{view_width}"
    );
    assert!(view_height > 2e6 * 40.0, "{view_height}");
    // ...and shown within the side cap, the shape kept.
    let shrunk = exported.report.shrunk.unwrap();
    assert!(
        (shrunk - f64::from(MAX_EXPORT_SIDE_PX) / view_width).abs() < 1e-12,
        "{shrunk}"
    );
    assert!(
        (width - f64::from(MAX_EXPORT_SIDE_PX)).abs() < 1e-9,
        "{width}"
    );
    assert!((height - view_height * shrunk).abs() < 0.01, "{height}");
    // resvg reads it, and draws the start of the long text (at x = -1,000,000, y = 0) at full
    // size, 40,000,000 pixels down the drawing. (It leaves out paths millions of pixels across,
    // such as the shapes here: tiny-skia's own limit.)
    let tree =
        resvg::usvg::Tree::from_data(&exported.bytes, &resvg::usvg::Options::default()).unwrap();
    // The frame starts at the rectangle at -1,000,000, half its outline and the margin out.
    let start = (1.0 + EXPORT_STROKE_UNITS / 2.0) * 40.0;
    let top = (1_000_000.0 + 1.0 + EXPORT_STROKE_UNITS / 2.0) * 40.0;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "pixels of the drawing and a scale, inside f32's range"
    )]
    let (x, y, unshrink) = (
        (start - 20.0) as f32,
        (top - 20.0) as f32,
        (1.0 / shrunk) as f32,
    );
    let mut pixmap = resvg::tiny_skia::Pixmap::new(400, 80).unwrap();
    // The file's size scaled back to the drawing's, then moved to the text.
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(unshrink, unshrink).post_translate(-x, -y),
        &mut pixmap.as_mut(),
    );
    let ink = pixmap
        .pixels()
        .iter()
        .filter(|px| px.alpha() > 200 && px.red() < 100)
        .count();
    assert!(ink > 200, "{ink} pixels of ink");
}

/// A board wider than the side cap at 40 px a unit keeps its drawing (the `viewBox`, in those
/// pixels) and shrinks its `width` and `height` to the cap, shape kept, which `ExportReport`
/// says; one within it is its own size.
#[test]
fn a_size_past_the_side_cap_shrinks_and_keeps_its_drawing() {
    // Two circles two million units apart: 80,000,162 pixels across at full size.
    let board = common::board(
        "lawh version=1\n\
         circle id=c1 x=-1000000 y=0 r=1\n\
         circle id=c2 x=1000000 y=0 r=1\n",
    );
    let exported = export(&board);
    let svg = text(&exported);
    check_structure(svg);
    let ((width, height), (view_width, view_height)) = sizes(svg);
    assert_eq!(view_width, 80_000_162.0);
    assert_eq!(view_height, 162.0);
    let shrunk = exported.report.shrunk.unwrap();
    assert!((shrunk - 16_384.0 / 80_000_162.0).abs() < 1e-15, "{shrunk}");
    assert_eq!(width, 16_384.0);
    // 162 pixels shrunk to 0.03, kept at a pixel: the drawing is centred in it.
    assert_eq!(height, 1.0);
    assert!(svg.contains(" width=\"16384\" height=\"1\" viewBox=\"0 0 80000162 162\""));
    // Both circles are there, at their places in the drawing.
    assert_eq!(svg.matches("<path ").count(), 2);
    // The cap through the seam: a frame of 12.05 units, 482 pixels, shown at 100.
    let small = common::board("lawh version=1\ncircle id=c1 x=0 y=0 r=5\n");
    let limits = ExportLimits {
        max_side_px: 100,
        ..ExportLimits::DEFAULT
    };
    let capped = export_with(&small, &limits).unwrap();
    let ((width, height), view) = sizes(text(&capped));
    assert_eq!(view, (482.0, 482.0));
    assert_eq!((width, height), (100.0, 100.0));
    assert!((capped.report.shrunk.unwrap() - 100.0 / 482.0).abs() < 1e-12);
    // Within the cap: no shrink, and the same drawing as with the cap.
    let full = export(&small);
    assert_eq!(full.report, ExportReport::default());
    assert_eq!(nominal_size(text(&full)), (482.0, 482.0));
    let header_end = |bytes: &[u8]| bytes.iter().position(|&b| b == b'>').unwrap();
    let rest = |bytes: &[u8]| {
        let svg = std::str::from_utf8(bytes).unwrap();
        let root = svg.find("<svg").unwrap();
        svg[root + header_end(&bytes[root..])..].to_owned()
    };
    assert_eq!(rest(&full.bytes), rest(&capped.bytes));
}

/// 250 texts of 160 characters: lawh-core's 40,000 characters a board.
fn texts_at_the_cap() -> Board {
    let line: String = "lorem ipsum dolor sit amet, consectetur adipiscing elit "
        .chars()
        .cycle()
        .take(160)
        .collect();
    let mut src = String::from("lawh version=1\n");
    for i in 0..250 {
        writeln!(src, "text id=t{i} x=0 y={} \"{line}\"", i * 2).unwrap();
    }
    common::board(&src)
}

/// 250 texts of 160 characters and 250 circles a million units across, spread over the whole
/// coordinate range: the hostile board of the PNG measurements (`tests/export.rs`).
#[cfg(target_os = "linux")]
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

/// Times, sizes and peak memory of SVG exports, for the M4 measurements. Run in release on a
/// quiet machine: `cargo test --release -p lawh-render --test export_svg -- --ignored
/// --test-threads=1 --nocapture measure_svg_exports`
#[test]
#[ignore = "a measurement: run in release with --test-threads=1"]
#[cfg(target_os = "linux")]
fn measure_svg_exports() {
    fn row(name: &str, times: &[Duration], peak: u64, result: &Result<Exported, ExportError>) {
        let times: Vec<String> = times
            .iter()
            .map(|took| format!("{:.1}", took.as_secs_f64() * 1e3))
            .collect();
        let times = times.join(" / ");
        let peak = peak / 1_000_000;
        match result {
            Ok(exported) => {
                let svg = text(exported);
                println!(
                    "{name}: {times} ms, peak {peak} MB, {} bytes, {} uses, {} glyphs defined, \
                     {} formulas failed",
                    exported.bytes.len(),
                    svg.matches("<use").count(),
                    svg.matches("<path id").count(),
                    exported.report.failed_formulas
                );
            }
            Err(err) => println!("{name}: {times} ms, peak {peak} MB, {err}"),
        }
    }
    let boards = [
        ("area.lawh", common::area_board()),
        ("40,000 chars (250 texts of 160)", texts_at_the_cap()),
        ("limits_board", common::limits_board()),
        ("hostile 500 objects", hostile_board()),
    ];
    for (name, board) in &boards {
        let mut renderer = Renderer::new_trusted();
        // Math compiled and text laid out first, so the times are drawing and writing.
        renderer.resolve_math(board);
        renderer.export(board, None, &svg_options()).unwrap();
        reset_peak();
        let mut times = Vec::new();
        let mut result = Err(ExportError::Empty);
        for _ in 0..3 {
            let started = Instant::now();
            result = renderer.export(board, None, &svg_options());
            times.push(started.elapsed());
        }
        row(name, &times, peak_bytes(), &result);
    }
    // 500 copies of a formula of 22,500 glyphs, under lawh-math's 25,000: 11 million glyphs
    // to write, so the file reaches its cap.
    let mut src = String::from("lawh version=1\n");
    for i in 0..500 {
        writeln!(
            src,
            "math id=m{i} x=0 y={} \"#range(150).map(_ => \\\"x\\\" * 150).join(linebreak())\"",
            i * 200
        )
        .unwrap();
    }
    let glyphs = common::board(&src);
    let mut renderer = Renderer::new_trusted();
    renderer.resolve_math(&glyphs);
    reset_peak();
    let started = Instant::now();
    let result = renderer.export(&glyphs, None, &svg_options());
    let took = started.elapsed();
    row(
        "500 formulas of 22,500 glyphs",
        &[took],
        peak_bytes(),
        &result,
    );
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
    let result = renderer.export(&failing, None, &svg_options());
    let took = started.elapsed();
    row("500 failing formulas, cold", &[took], peak_bytes(), &result);
}
