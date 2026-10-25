//! PDF exports with the real renderer, read back with an independent reader (hayro): one page at
//! 30 pt a unit, board text as text in embedded font subsets that a viewer can copy, drawn as
//! the PNG export is, the same bytes every time, and every cap holds.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use std::fmt::Write as _;
use std::io::Cursor;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache,
    InterpreterSettings, Paint, PathDrawMode, SoftMask, interpret_page,
};
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::metadata::Metadata;
use hayro::hayro_syntax::object::{Array, Dict, Name, Stream};
use hayro::{RenderCache, RenderSettings, render};
use kurbo::{Affine, BezPath};
use lawh_core::{
    Board, ExportBackground, ExportFormat, ExportOptions, ExportReport, Object, ObjectId, Point,
    Shape,
};
use lawh_render::{
    EXPORT_STROKE_UNITS, ExportError, ExportLimits, Exported, MAX_EXPORT_BYTES, MAX_PDF_PAGE_PT,
    PDF_PT_PER_UNIT, Palette, Renderer, Viewport,
};

/// Most a pixel inside a shape, a glyph or the paper (its 3 x 3 neighbourhood the same colour in
/// the PNG, within 3 a channel) may differ between hayro's drawing of a PDF export and the PNG
/// export, a channel. Away from edges both draw the same colours: measured 0 on [`mixed_board`]
/// (hayro reads the ICC-based sRGB as sRGB); 2 allows for rounding in blending a translucent
/// colour, as the SVG test does.
const INTERIOR_TOLERANCE: u8 = 2;

/// Most a pixel on an edge (any other pixel) may differ, a channel. Measured on [`mixed_board`]:
/// at most 54, over 9,332 edge pixels of 227,652, and only 54 of them past 32: short rows along
/// the horizontal bars of the formula's glyphs (its `=` and `π`), the mark of a fraction of a
/// pixel between where the PNG's glyph drawing and hayro's drawing of the glyphs' paths put
/// them; board text, shapes and the paper are within 32. resvg against the PNG export differs
/// as much (56, `tests/export_svg.rs`). A quarter of full coverage (64), as there, still fails a
/// glyph or a line one pixel off, or another colour, which differ by up to 219 there.
const EDGE_TOLERANCE: u8 = 64;

fn pdf_options() -> ExportOptions {
    ExportOptions {
        format: ExportFormat::Pdf,
        ..ExportOptions::default()
    }
}

fn export(board: &Board) -> Exported {
    Renderer::new_trusted()
        .export(board, None, &pdf_options())
        .unwrap()
}

fn export_with(board: &Board, limits: &ExportLimits) -> Result<Exported, ExportError> {
    Renderer::new_trusted().export_with_limits(board, None, &pdf_options(), limits)
}

fn id(name: &str) -> ObjectId {
    ObjectId::new(name).unwrap()
}

/// Text at two sizes, a formula on a text's baseline, a translucent coloured formula, `∧` from
/// the fallback font, and every shape kind (the board of the SVG test).
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

/// What hayro's interpreter hands a device for a page: each glyph's text from its font's
/// `ToUnicode` map, and how many paths are drawn (math glyphs, shapes, the paper).
#[derive(Default)]
struct Reading {
    text: String,
    glyphs: usize,
    unmapped: usize,
    paths: usize,
}

impl<'a> Device<'a> for Reading {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {
        self.paths += 1;
    }
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(
        &mut self,
        glyph: &Glyph<'a>,
        _: Affine,
        _: Affine,
        _: &Paint<'a>,
        _: &GlyphDrawMode,
    ) {
        self.glyphs += 1;
        match glyph.as_unicode() {
            Some(BfString::Char(c)) => self.text.push(c),
            Some(BfString::String(s)) => self.text.push_str(&s),
            None => self.unmapped += 1,
        }
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// The one page of `pdf` as hayro reads it.
fn read(bytes: &[u8]) -> (Pdf, Reading) {
    let pdf = Pdf::new(bytes.to_vec()).unwrap();
    assert_eq!(pdf.pages().len(), 1);
    let mut reading = Reading::default();
    {
        let page = &pdf.pages()[0];
        let cache = InterpreterCache::new();
        let mut ctx = Context::new(
            Affine::IDENTITY,
            kurbo::Rect::new(0.0, 0.0, 1e9, 1e9),
            &cache,
            page.xref(),
            InterpreterSettings::default(),
        );
        interpret_page(page, &mut ctx, &mut reading);
    }
    (pdf, reading)
}

/// The page's media box, `[x0, y0, x1, y1]` in pt.
fn media_box(pdf: &Pdf) -> [f64; 4] {
    let mb = pdf.pages()[0].media_box();
    [mb.x0, mb.y0, mb.x1, mb.y1]
}

/// An image as straight (not premultiplied) RGBA pixels.
struct Picture {
    width: u32,
    height: u32,
    pixels: Vec<[u8; 4]>,
}

/// The page of `bytes` drawn by hayro at `px_per_pt`, onto a transparent background. Its size
/// comes from the media box, and must be small: hayro takes it as `u16`.
fn rasterize(bytes: &[u8], px_per_pt: f64) -> Picture {
    let pdf = Pdf::new(bytes.to_vec()).unwrap();
    let page = &pdf.pages()[0];
    let [x0, y0, x1, y1] = media_box(&pdf);
    let (width, height) = (
        ((x1 - x0) * px_per_pt).round(),
        ((y1 - y0) * px_per_pt).round(),
    );
    assert!(width <= 4_000.0 && height <= 4_000.0, "{width} x {height}");
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "small sizes, checked just above"
    )]
    let (width, height) = (width as u16, height as u16);
    #[expect(clippy::cast_possible_truncation, reason = "a small scale")]
    let scale = px_per_pt as f32;
    let pixmap = render(
        page,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &RenderSettings {
            x_scale: scale,
            y_scale: scale,
            width: Some(width),
            height: Some(height),
            ..RenderSettings::default()
        },
    );
    let pixels = pixmap
        .take_unpremultiplied()
        .into_iter()
        .map(|px| [px.r, px.g, px.b, px.a])
        .collect();
    Picture {
        width: u32::from(width),
        height: u32::from(height),
        pixels,
    }
}

/// A PNG export decoded.
fn decode_png(bytes: &[u8]) -> Picture {
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
    Picture {
        width: frame.width,
        height: frame.height,
        pixels,
    }
}

/// The largest difference between two pixels, a channel.
fn difference(a: [u8; 4], b: [u8; 4]) -> u8 {
    (0..4).map(|c| a[c].abs_diff(b[c])).max().unwrap()
}

/// The font dictionaries of the page's resources, by resource name.
fn fonts(pdf: &Pdf) -> Vec<Dict<'_>> {
    let raw = pdf.pages()[0].raw();
    let resources: Dict<'_> = raw.get(b"Resources").unwrap();
    let fonts: Dict<'_> = resources.get(b"Font").unwrap();
    fonts
        .keys()
        .map(|key| fonts.get::<Dict<'_>>(&*key).unwrap())
        .collect()
}

/// How many form `XObject`s the page's resources name: one a math glyph and colour.
fn form_xobjects(pdf: &Pdf) -> usize {
    let raw = pdf.pages()[0].raw();
    let resources: Dict<'_> = raw.get(b"Resources").unwrap();
    resources
        .get::<Dict<'_>>(b"XObject")
        .map_or(0, |objects| objects.keys().count())
}

#[test]
fn a_pdf_is_one_page_of_text_in_embedded_subsets() {
    // Text at sizes 1 and 2, `∧` from the fallback font, two lines, and a formula.
    let board = common::board(
        "lawh version=1\n\
         text id=t1 x=0 y=0 \"Size 1: x =\"\n\
         math id=m1 x=4 y=0 \"x = pi r^2\"\n\
         text id=t2 x=0 y=1.5 size=2 \"p \u{2227} q\"\n\
         text id=t3 x=0 y=4 \"two\\nlines\"\n",
    );
    let mut renderer = Renderer::new_trusted();
    let exported = renderer.export(&board, None, &pdf_options()).unwrap();
    assert_eq!(exported.report, ExportReport::default());
    assert!(exported.bytes.starts_with(b"%PDF-1.7"));
    let (pdf, reading) = read(&exported.bytes);

    // At 30 pt a unit: the frame, rounded up to whole 0.75 pt pixels.
    let (frame, _) = renderer.record_export(&board, None).unwrap();
    let [x0, y0, x1, y1] = media_box(&pdf);
    assert_eq!((x0, y0), (0.0, 0.0));
    for (side, units) in [(x1, frame.width()), (y1, frame.height())] {
        let full = units * PDF_PT_PER_UNIT;
        assert!(
            side >= full - 1e-3 && side < full + 0.75,
            "{side} for {full}"
        );
        assert!((side / 0.75 - (side / 0.75).round()).abs() < 1e-3, "{side}");
    }

    // The board text, every glyph mapped to the text it shows; the formula is drawn as paths.
    assert_eq!(reading.unmapped, 0);
    assert_eq!(reading.text, "Size 1: x =p \u{2227} qtwolines");
    assert!(reading.paths >= 5, "{}", reading.paths);

    // Both bundled fonts, embedded as subsets (a six-letter tag), each far smaller than the font,
    // with a ToUnicode map.
    let fonts = fonts(&pdf);
    assert_eq!(fonts.len(), 2);
    let full_sizes: Vec<usize> = lawh_math::fonts().iter().map(|f| f.data.len()).collect();
    for font in &fonts {
        assert_eq!(font.get::<Name<'_>>(b"Subtype").unwrap().as_str(), "Type0");
        assert!(font.contains_key(b"ToUnicode"));
        let base = font.get::<Name<'_>>(b"BaseFont").unwrap();
        let (tag, name) = base.as_str().split_once('+').unwrap();
        assert!(
            tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()),
            "{tag}"
        );
        assert!(
            name.starts_with("Inter") || name.starts_with("LibertinusMath"),
            "{name}"
        );
        let descendants: Array<'_> = font.get(b"DescendantFonts").unwrap();
        let cid: Dict<'_> = descendants.iter::<Dict<'_>>().next().unwrap();
        let descriptor: Dict<'_> = cid.get(b"FontDescriptor").unwrap();
        let file: Stream<'_> = descriptor
            .get(b"FontFile2")
            .or_else(|| descriptor.get(b"FontFile3"))
            .unwrap();
        let subset = file.decoded().unwrap().len();
        let full = if name.starts_with("Inter") {
            full_sizes[0]
        } else {
            full_sizes[1]
        };
        assert!(subset * 10 < full, "{name}: {subset} of {full} bytes");
    }

    // No metadata: no title, producer or dates, no XMP.
    assert_eq!(pdf.metadata(), &Metadata::default());
    let has = |word: &[u8]| exported.bytes.windows(word.len()).any(|w| w == word);
    assert!(!has(b"/Metadata") && !has(b"CreationDate") && !has(b"/Producer"));
}

/// hayro's drawing of a PDF export is the PNG export at the same scale (scale 1, 40 pixels a
/// unit, 4/3 of a pixel a pt): the same size, the same colours inside every shape, glyph and the
/// paper, and edges within what the two drawings' anti-aliasing tells apart.
#[test]
fn pdf_draws_as_the_png_export_does() {
    let board = mixed_board();
    let mut renderer = Renderer::new_trusted();
    let pdf = renderer.export(&board, None, &pdf_options()).unwrap();
    let png_options = ExportOptions {
        png_scale_hundredths: 100,
        ..ExportOptions::default()
    };
    let png = renderer.export(&board, None, &png_options).unwrap();
    let drawn = rasterize(&pdf.bytes, 4.0 / 3.0);
    let expected = decode_png(&png.bytes);
    assert_eq!(
        (drawn.width, drawn.height),
        (expected.width, expected.height)
    );
    let (w, h) = (expected.width as usize, expected.height as usize);
    let at = |x: usize, y: usize| expected.pixels[y * w + x];
    let (mut edges, mut worst_edge, mut worst_interior, mut past_32) = (0, 0, 0, 0);
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
                if d > 32 {
                    past_32 += 1;
                }
            } else {
                worst_interior = worst_interior.max(d);
            }
        }
    }
    println!(
        "{} pixels, {edges} on edges: worst difference {worst_edge} on an edge ({past_32} past \
         32), {worst_interior} elsewhere",
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
fn exports_are_the_same_every_time() {
    let board = mixed_board();
    let first = export(&board);
    assert_eq!(export(&board), first);
    // And from a renderer that has drawn the board in the window, and exported it to PNG and to
    // SVG, first.
    let mut renderer = Renderer::new_trusted();
    renderer.render(&board, &Viewport::new(Point::ZERO, 2.0, 300, 200, 1.0));
    for format in [ExportFormat::Png, ExportFormat::Svg] {
        let options = ExportOptions {
            format,
            ..ExportOptions::default()
        };
        renderer.export(&board, None, &options).unwrap();
    }
    assert_eq!(
        renderer.export(&board, None, &pdf_options()).unwrap(),
        first
    );
}

#[test]
fn a_transparent_export_has_no_paper() {
    let board = common::area_board();
    let options = ExportOptions {
        background: ExportBackground::None,
        ..pdf_options()
    };
    let clear = Renderer::new_trusted()
        .export(&board, None, &options)
        .unwrap();
    let paper = export(&board);
    let (clear_pdf, clear_reading) = read(&clear.bytes);
    let (paper_pdf, paper_reading) = read(&paper.bytes);
    assert_eq!(media_box(&clear_pdf), media_box(&paper_pdf));
    assert_eq!(clear_reading.paths + 1, paper_reading.paths);
    assert_eq!(clear_reading.text, paper_reading.text);
    let image = rasterize(&clear.bytes, 4.0 / 3.0);
    assert_eq!(image.pixels[0], [0, 0, 0, 0]);
    let see_through = image.pixels.iter().filter(|px| px[3] == 0).count();
    assert!(see_through > image.pixels.len() * 9 / 10, "{see_through}");
    assert!(image.pixels.iter().any(|px| px[3] == 255));
    let image = rasterize(&paper.bytes, 4.0 / 3.0);
    assert!(image.pixels.iter().all(|px| px[3] == 255));
}

#[test]
fn a_selection_exports_only_its_objects_framed_to_them() {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    let circle = renderer
        .export(&board, Some(&[id("c1")]), &pdf_options())
        .unwrap();
    let (pdf, reading) = read(&circle.bytes);
    // The paper and the circle.
    assert_eq!((reading.glyphs, reading.paths), (0, 2));
    // 6 units across with its outline, and the margin, at 40 pixels a unit, in whole pixels.
    let side = ((6.0 + EXPORT_STROKE_UNITS + 2.0) * 40.0_f64).ceil() * 0.75;
    assert_eq!(media_box(&pdf), [0.0, 0.0, side, side]);
    // The formula and the title: the title's text, the formula's glyphs as paths.
    let two = renderer
        .export(&board, Some(&[id("m1"), id("t1")]), &pdf_options())
        .unwrap();
    let (pdf, reading) = read(&two.bytes);
    assert_eq!(reading.text, "Area of a circle");
    assert!(reading.paths > 5, "{}", reading.paths);
    let whole = media_box(&read(&export(&board).bytes).0);
    assert!(media_box(&pdf)[3] < whole[3]);
}

/// A formula's glyphs are each written once, whatever how many times they are drawn.
#[test]
fn math_glyphs_are_written_once_and_drawn_where_used() {
    let formula = |count: usize| {
        let mut src = String::from("lawh version=1\n");
        for i in 0..count {
            writeln!(src, "math id=m{i} x=0 y={} \"sum_(i=1)^n x_i^2\"", i * 3).unwrap();
        }
        common::board(&src)
    };
    let one = export(&formula(1));
    let many = export(&formula(40));
    let (one_pdf, one_reading) = read(&one.bytes);
    let (many_pdf, many_reading) = read(&many.bytes);
    // The same glyphs defined, forty times the paths drawn.
    assert_eq!(form_xobjects(&one_pdf), form_xobjects(&many_pdf));
    assert!(form_xobjects(&one_pdf) >= 5);
    assert_eq!((one_reading.paths - 1) * 40, many_reading.paths - 1);
    // Forty uses take far less than forty definitions would.
    assert!(
        many.bytes.len() < one.bytes.len() * 3,
        "{} {}",
        one.bytes.len(),
        many.bytes.len()
    );
    // A glyph drawn in another colour is another form.
    let coloured = export(&common::board(
        "lawh version=1\n\
         math id=m1 x=0 y=0 \"x\"\n\
         math id=m2 x=0 y=2 \"#text(fill: red)[$x$]\"\n",
    ));
    assert_eq!(form_xobjects(&read(&coloured.bytes).0), 2);
}

/// The board at lawh-core's limits (shapes a million units across and out to 2,000,000 units, a
/// text of 5,000 characters on one line and one of 500 lines) shrinks to fit the page cap.
#[test]
fn the_board_at_the_limits_shrinks_to_fit_the_page() {
    let board = common::limits_board();
    let mut renderer = Renderer::new_trusted();
    let exported = renderer.export(&board, None, &pdf_options()).unwrap();
    assert!(exported.bytes.len() < MAX_EXPORT_BYTES);
    let (pdf, reading) = read(&exported.bytes);
    let [_, _, width, height] = media_box(&pdf);
    assert!(width <= MAX_PDF_PAGE_PT && height <= MAX_PDF_PAGE_PT);
    assert!(
        width.max(height) > MAX_PDF_PAGE_PT - 1.0,
        "{width} {height}"
    );
    // Shrunk from 30 pt a unit to fit the longest side, about 3,000,000 units.
    let (frame, _) = renderer.record_export(&board, None).unwrap();
    let factor = exported.report.shrunk.unwrap();
    let expected = MAX_PDF_PAGE_PT / (frame.width().max(frame.height()) * PDF_PT_PER_UNIT);
    assert!(
        factor <= expected * (1.0 + 1e-12) && factor > expected * 0.99,
        "{factor} {expected}"
    );
    assert_eq!(exported.report.failed_formulas, 0);
    // Its text is still there to copy, however small.
    assert!(
        reading.text.starts_with("lorem ipsum"),
        "{}",
        &reading.text[..40]
    );
    assert!(reading.text.chars().count() > 5_000);

    // A page cap of the test seam shrinks a small board too.
    let limits = ExportLimits {
        max_page_pt: 200.0,
        ..ExportLimits::DEFAULT
    };
    let small = renderer
        .export_with_limits(&common::area_board(), None, &pdf_options(), &limits)
        .unwrap();
    let [_, _, width, height] = media_box(&read(&small.bytes).0);
    assert!(width.max(height) <= 200.0 && width.max(height) > 199.0);
    let factor = small.report.shrunk.unwrap();
    assert!(factor > 0.4 && factor < 0.7, "{factor}");
}

#[test]
fn the_byte_cap_stops_an_export() {
    let board = common::area_board();
    let full = export(&board).bytes.len();
    // The file, checked when it is done.
    let limits = ExportLimits {
        max_bytes: full - 1,
        ..ExportLimits::DEFAULT
    };
    assert_eq!(
        export_with(&board, &limits),
        Err(ExportError::TooLarge { bytes: full })
    );
    let limits = ExportLimits {
        max_bytes: full,
        ..ExportLimits::DEFAULT
    };
    assert_eq!(export_with(&board, &limits).unwrap().bytes.len(), full);
    // A board that writes megabytes of content stops as soon as its estimate passes a small cap:
    // the content is counted as it is drawn, not once the file is made.
    let limits = ExportLimits {
        max_bytes: 10_000,
        ..ExportLimits::DEFAULT
    };
    match export_with(&texts_at_the_cap(), &limits) {
        Err(ExportError::TooLarge { bytes }) => assert!(bytes <= 10_000 + 4_000, "{bytes}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_budget_stops_an_export() {
    let board = common::area_board();
    // The area board is estimated at about 0.9 ms (0.2 ms of it the file's own work).
    for budget_ns in [0.0, 1e4, 3e5] {
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
        budget_ns: 1e7,
        ..ExportLimits::DEFAULT
    };
    assert!(export_with(&board, &limits).is_ok());
    // 40,000 characters of text: over 5 ms.
    let limits = ExportLimits {
        budget_ns: 5e6,
        ..ExportLimits::DEFAULT
    };
    assert_eq!(
        export_with(&texts_at_the_cap(), &limits),
        Err(ExportError::OverBudget)
    );
}

/// Board text that holds markup, quotes, the characters PDF strings escape and control
/// characters exports as any other, as text and as the source of a formula that fails (drawn as
/// its text, in red): its printable text is there to copy, and nothing of it is read as PDF.
#[test]
fn board_text_with_markup_quotes_and_control_characters_is_text() {
    let hostile = "<script>alert(\"x\")</script> & &amp; ]]> 'q' (unbalanced \\ ) ) \
                   /Name %comment\u{1}\u{1F}\u{FFFE}\ta\r\nb";
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
        // A letter with two combining marks: one cluster, its text on its first glyph.
        (
            "t2",
            Shape::Text {
                at: Point::new(0.0, 6.0),
                text: "a\u{301}\u{302}b".to_owned(),
                size: 1.0,
            },
        ),
    ] {
        board.insert(Object::new(id(name), shape)).unwrap();
    }
    let exported = export(&board);
    assert_eq!(exported.report.failed_formulas, 1);
    let (_, reading) = read(&exported.bytes);
    let printable =
        "<script>alert(\"x\")</script> & &amp; ]]> 'q' (unbalanced \\ ) ) /Name %comment";
    assert_eq!(
        reading.text.matches(printable).count(),
        2,
        "{}",
        reading.text
    );
    assert!(
        reading.text.ends_with("a\u{301}\u{302}b"),
        "{}",
        reading.text
    );
    // The failed formula is drawn in the error colour.
    let image = rasterize(&exported.bytes, 4.0 / 3.0);
    let red = Palette::LIGHT.error.to_rgba8().to_u8_array();
    let reds = image
        .pixels
        .iter()
        .filter(|px| difference(**px, red) <= 3)
        .count();
    assert!(reds > 100, "{reds}");
}

#[test]
fn a_failing_formula_is_drawn_red_and_counted() {
    let board = common::board(
        "lawh version=1\n\
         math id=m1 x=0 y=0 \"frac(1,\"\n\
         math id=m2 x=0 y=2 \"x^2 + y^2 = z^2\"\n",
    );
    let exported = export(&board);
    assert_eq!(exported.report.failed_formulas, 1);
    let (_, reading) = read(&exported.bytes);
    // Its source as text; the formula that compiled as paths.
    assert_eq!(reading.text, "frac(1,");
    assert!(reading.paths >= 3);
    let image = rasterize(&exported.bytes, 4.0 / 3.0);
    let red = Palette::LIGHT.error.to_rgba8().to_u8_array();
    let ink = Palette::LIGHT.ink.to_rgba8().to_u8_array();
    let count = |color: [u8; 4]| {
        image
            .pixels
            .iter()
            .filter(|px| difference(**px, color) <= 3)
            .count()
    };
    assert!(
        count(red) > 50 && count(ink) > 20,
        "{} {}",
        count(red),
        count(ink)
    );
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
/// coordinate range: the hostile board of the PNG and SVG measurements.
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

/// Times, sizes and peak memory of PDF exports, for the M4 measurements. Run in release on a
/// quiet machine: `cargo test --release -p lawh-render --test export_pdf -- --ignored
/// --test-threads=1 --nocapture measure_pdf_exports`
#[test]
#[ignore = "a measurement: run in release with --test-threads=1"]
#[cfg(target_os = "linux")]
fn measure_pdf_exports() {
    fn row(name: &str, times: &[Duration], peak: u64, result: &Result<Exported, ExportError>) {
        let times: Vec<String> = times
            .iter()
            .map(|took| format!("{:.1}", took.as_secs_f64() * 1e3))
            .collect();
        let times = times.join(" / ");
        let peak = peak / 1_000_000;
        match result {
            Ok(exported) => println!(
                "{name}: {times} ms, peak {peak} MB, {} bytes, {:?}",
                exported.bytes.len(),
                exported.report
            ),
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
        renderer.export(board, None, &pdf_options()).unwrap();
        reset_peak();
        let mut times = Vec::new();
        let mut result = Err(ExportError::Empty);
        for _ in 0..3 {
            let started = Instant::now();
            result = renderer.export(board, None, &pdf_options());
            times.push(started.elapsed());
        }
        row(name, &times, peak_bytes(), &result);
    }
    // 500 copies of a formula of 22,500 glyphs, under lawh-math's 25,000: 11 million glyphs to
    // draw, so the content reaches its cap.
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
    let result = renderer.export(&glyphs, None, &pdf_options());
    let took = started.elapsed();
    row(
        "500 formulas of 22,500 glyphs",
        &[took],
        peak_bytes(),
        &result,
    );
    // 500 formulas that fail, each its own, compiled as part of the export (in process here; the
    // app's worker adds its round trips).
    let mut src = String::from("lawh version=1\n");
    for i in 0..500 {
        let (x, y) = (f64::from(i % 25) * 6.0, f64::from(i / 25) * 2.0);
        writeln!(src, "math id=m{i} x={x} y={y} \"frac({i},\"").unwrap();
    }
    let failing = common::board(&src);
    let mut renderer = Renderer::new_trusted();
    reset_peak();
    let started = Instant::now();
    let result = renderer.export(&failing, None, &pdf_options());
    let took = started.elapsed();
    row("500 failing formulas, cold", &[took], peak_bytes(), &result);
}
