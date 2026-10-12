//! Golden images: render example boards and compare them with reviewed PNGs
//! in `tests/golden/`.
//!
//! The comparison is strict enough that one wrong glyph fails it: at most
//! [`MAX_DIFFERENT_PER_MILLION`] pixels in a million may differ by more than
//! [`CHANNEL_TOLERANCE`] in any channel. The tolerance absorbs the last-bit
//! float differences between machines (x86 and ARM) along anti-aliased edges;
//! a changed glyph changes whole strokes. The `a_wrong_glyph_fails_*` tests
//! prove it against the real goldens.
//!
//! The `overlay_*` goldens are window frames ([`Renderer::render_with`]) with the cursor, a
//! Visual selection, a caret, jump labels, `:sym` completions or the status bar drawn over the
//! area board. The `sizes` golden is text and math at sizes 0.5, 1, 2 and 4, and `text_symbols`
//! text with symbols Inter lacks, set in Libertinus Math.
//!
//! `LAWH_UPDATE_GOLDEN=1 cargo test -p lawh-render --test golden` rewrites
//! the images. Look at every rewritten image before committing it.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use std::io::Cursor;
use std::path::PathBuf;

use kurbo::{Point, Rect};
use lawh_core::engine::{JumpLabel, Symbol, anchor};
use lawh_core::{Board, ObjectId};
use lawh_render::{Caret, Overlay, OverlayCursor, Renderer, StatusLine, Viewport};
use vello_cpu::Pixmap;

/// Largest per-channel difference that still counts as the same pixel.
const CHANNEL_TOLERANCE: u8 = 3;
/// Most pixels in a million that may differ by more than [`CHANNEL_TOLERANCE`]:
/// 0.01 %, 48 pixels of an 800x600 image.
const MAX_DIFFERENT_PER_MILLION: usize = 100;

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join(format!("{name}.png"))
}

/// How many pixels of `png` differ from the golden `name` by more than the
/// tolerance, out of how many.
fn different_pixels(name: &str, png: &[u8]) -> (usize, usize) {
    let path = golden_path(name);
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
    (different, actual.data().len())
}

/// `Err` with a description when `png` doesn't match the golden `name`.
fn compare(name: &str, png: &[u8]) -> Result<(), String> {
    let (different, total) = different_pixels(name, png);
    let allowed = total * MAX_DIFFERENT_PER_MILLION / 1_000_000;
    if different > allowed {
        return Err(format!(
            "{name}: {different} of {total} pixels differ from {} by more than {CHANNEL_TOLERANCE} \
             (at most {allowed} may)",
            golden_path(name).display()
        ));
    }
    Ok(())
}

fn check_golden(name: &str, png: &[u8]) {
    if std::env::var_os("LAWH_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        std::fs::write(golden_path(name), png).unwrap();
        return;
    }
    if let Err(message) = compare(name, png) {
        let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.actual.png"));
        std::fs::write(&out, png).unwrap();
        panic!("{message}; actual image written to {}", out.display());
    }
}

/// The area board fitted to 800x600, as the `area` golden draws it.
fn area_fit_png(board: &Board) -> Vec<u8> {
    let mut renderer = Renderer::new_trusted();
    renderer.resolve_math(board);
    let bounds = renderer.board_bounds(board).unwrap();
    let vp = Viewport::fit(bounds, 800, 600, 1.0);
    renderer.render_png(board, &vp).unwrap()
}

/// The area board at zoom 3 around its formula, as the `area_zoom3` golden
/// draws it. The view is centred where the real formula is, so a mutated
/// formula is compared at the same place.
fn area_zoom3_png(board: &Board) -> Vec<u8> {
    let mut renderer = Renderer::new_trusted();
    let real = common::area_board();
    renderer.resolve_math(&real);
    let formula = real.get(&ObjectId::new("m1").unwrap()).unwrap();
    let center = renderer.object_bounds(formula).unwrap().center();
    let vp = Viewport::new(center, 3.0, 800, 600, 1.0);
    renderer.render_png(board, &vp).unwrap()
}

#[test]
fn area_fit_800x600() {
    check_golden("area", &area_fit_png(&common::area_board()));
}

#[test]
fn area_formula_at_zoom_3() {
    check_golden("area_zoom3", &area_zoom3_png(&common::area_board()));
}

/// The frame budget never touches an everyday board: every object of the area example is drawn
/// in full in window frames, right after a resize (when the budget is smallest) and after, at
/// every size and zoom the app shows.
#[test]
fn the_area_board_is_never_downgraded() {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    renderer.resolve_math(&board);
    let bounds = renderer.board_bounds(&board).unwrap();
    for (width, height, scale) in [(800, 600, 1.0), (1920, 1080, 1.0), (2880, 1800, 2.0)] {
        let fit = Viewport::fit(bounds, width, height, scale);
        for vp in [
            fit,
            Viewport { zoom: 0.1, ..fit },
            Viewport { zoom: 3.0, ..fit },
            Viewport { zoom: 20.0, ..fit },
        ] {
            for _ in 0..2 {
                renderer.render(&board, &vp);
                let frame = renderer.last_frame();
                assert_eq!(frame.downgraded, 0, "{vp:?}: {frame:?}");
                assert_eq!(frame.math_pending, 0, "{vp:?}");
            }
        }
    }
}

/// The area example with one line of its source replaced.
fn mutated_area(from: &str, to: &str) -> Board {
    let src = include_str!("../../../examples/area.lawh");
    assert!(src.contains(from), "{from:?} is not in the example");
    common::board(&src.replace(from, to))
}

/// One glyph changed in the area example: the label `r` → `x`, the formula's
/// `r` → `x`, and its `r^2` → `r^3`.
fn one_glyph_mutations() -> [(&'static str, Board); 3] {
    [
        ("label r -> x", mutated_area("\"r\"", "\"x\"")),
        (
            "formula r -> x",
            mutated_area("\"A = pi r^2\"", "\"A = pi x^2\""),
        ),
        (
            "formula r^2 -> r^3",
            mutated_area("\"A = pi r^2\"", "\"A = pi r^3\""),
        ),
    ]
}

#[test]
fn a_wrong_glyph_fails_the_fitted_golden() {
    for (what, board) in one_glyph_mutations() {
        let png = area_fit_png(&board);
        let (different, total) = different_pixels("area", &png);
        println!("area, {what}: {different} of {total} pixels differ");
        assert!(
            compare("area", &png).is_err(),
            "{what}: the mutated board passed the golden ({different} pixels differ)"
        );
    }
}

#[test]
fn a_wrong_glyph_fails_the_zoomed_golden() {
    // The label is outside this view, so only the formula mutations apply.
    for (what, board) in one_glyph_mutations().into_iter().skip(1) {
        let png = area_zoom3_png(&board);
        let (different, total) = different_pixels("area_zoom3", &png);
        println!("area_zoom3, {what}: {different} of {total} pixels differ");
        assert!(
            compare("area_zoom3", &png).is_err(),
            "{what}: the mutated board passed the golden ({different} pixels differ)"
        );
    }
}

/// `image`, a frame of `vp` from the renderer (premultiplied RGBA8), as PNG.
fn png_of(image: &[u8], vp: &Viewport) -> Vec<u8> {
    let mut pixmap = Pixmap::new(vp.width, vp.height);
    pixmap.data_as_u8_slice_mut().copy_from_slice(image);
    pixmap.into_png().unwrap()
}

/// The area board in a window frame through `vp` (`None`: fitted to 800x600, as the `area`
/// golden draws it), with `overlay` over it.
fn overlay_png(vp: Option<Viewport>, overlay: &Overlay<'_>) -> Vec<u8> {
    let board = common::area_board();
    let mut renderer = Renderer::new_trusted();
    renderer.resolve_math(&board);
    let vp =
        vp.unwrap_or_else(|| Viewport::fit(renderer.board_bounds(&board).unwrap(), 800, 600, 1.0));
    // The second frame at this size, with the whole budget, as the window draws it.
    renderer.render_with(&board, &vp, overlay);
    let image = renderer.render_with(&board, &vp, overlay).to_vec();
    let frame = renderer.last_frame();
    assert!(
        !frame.incomplete() && frame.downgraded == 0 && frame.math_pending == 0,
        "{frame:?}"
    );
    png_of(&image, &vp)
}

fn id(name: &str) -> ObjectId {
    ObjectId::new(name).unwrap()
}

/// The right of the status line in these goldens: the file and the cursor's position.
fn right(at: &str) -> String {
    format!("area.lawh  {at}")
}

#[test]
fn overlay_cursor_on_an_object() {
    let c1 = id("c1");
    let right = right("0,0");
    let overlay = Overlay {
        cursor: OverlayCursor::Object(&c1),
        status: StatusLine {
            left: "NORMAL",
            right: &right,
            error: false,
        },
        ..Overlay::default()
    };
    check_golden("overlay_normal", &overlay_png(None, &overlay));
}

#[test]
fn overlay_visual_selection() {
    let c1 = id("c1");
    // The circle's bounds, which hold the radius line and its label too.
    let selection = [id("c1"), id("l1"), id("t2")];
    let right = right("0,0");
    let overlay = Overlay {
        cursor: OverlayCursor::Object(&c1),
        selection: &selection,
        visual: Some(Rect::new(-3.0, -3.0, 3.0, 3.0)),
        status: StatusLine {
            left: "-- VISUAL --",
            right: &right,
            error: false,
        },
        ..Overlay::default()
    };
    check_golden("overlay_visual", &overlay_png(None, &overlay));
}

#[test]
fn overlay_insert_in_a_word() {
    let t1 = id("t1");
    let right = right("0,-5");
    let overlay = Overlay {
        cursor: OverlayCursor::Object(&t1),
        // "Ar|ea of a circle"
        caret: Some(Caret {
            id: &t1,
            byte: 2,
            block: false,
        }),
        status: StatusLine {
            left: "-- INSERT --",
            right: &right,
            error: false,
        },
        ..Overlay::default()
    };
    let vp = Viewport::new(Point::new(2.0, -4.7), 3.0, 800, 600, 1.0);
    check_golden("overlay_insert", &overlay_png(Some(vp), &overlay));
}

#[test]
fn overlay_math_block_being_edited() {
    let m1 = id("m1");
    let right = right("5,0");
    let overlay = Overlay {
        cursor: OverlayCursor::Object(&m1),
        // A block on the "p" of "A = pi r^2", drawn as its source.
        caret: Some(Caret {
            id: &m1,
            byte: 4,
            block: true,
        }),
        status: StatusLine {
            left: "",
            right: &right,
            error: false,
        },
        ..Overlay::default()
    };
    let vp = Viewport::new(Point::new(6.2, 0.3), 3.0, 800, 600, 1.0);
    check_golden("overlay_math_edit", &overlay_png(Some(vp), &overlay));
}

#[test]
fn overlay_command_line() {
    let right = right("-4,-3");
    let overlay = Overlay {
        cursor: OverlayCursor::Grid(Point::new(-4.0, -3.0)),
        status: StatusLine {
            left: "NORMAL",
            right: &right,
            error: false,
        },
        command_line: Some(("ci 3", 4)),
        ..Overlay::default()
    };
    check_golden("overlay_command_line", &overlay_png(None, &overlay));
}

#[test]
fn overlay_error_message() {
    let right = right("-4,-3");
    let overlay = Overlay {
        cursor: OverlayCursor::Grid(Point::new(-4.0, -3.0)),
        status: StatusLine {
            left: "radius must be above 0",
            right: &right,
            error: true,
        },
        ..Overlay::default()
    };
    check_golden("overlay_error", &overlay_png(None, &overlay));
}

/// Text and math at sizes 0.5, 1, 2 and 4, each formula beside a text at the same `y`: the
/// formula's baseline on the text's, its `x` as tall as the text's.
#[test]
fn text_and_math_at_four_sizes() {
    let board = common::board(
        "lawh version=1\n\
         text id=t1 x=0 y=0 size=0.5 \"Size 0.5: x\"\n\
         math id=m1 x=2.5 y=0 size=0.5 \"x = pi r^2\"\n\
         text id=t2 x=0 y=1 \"Size 1: x\"\n\
         math id=m2 x=3 y=1 \"x = pi r^2\"\n\
         text id=t3 x=0 y=2.5 size=2 \"Size 2: x\"\n\
         math id=m3 x=5.5 y=2.5 size=2 \"x = pi r^2\"\n\
         text id=t4 x=0 y=5 size=4 \"Size 4: x\"\n\
         math id=m4 x=10.5 y=5 size=4 \"x^2\"\n",
    );
    let mut renderer = Renderer::new_trusted();
    renderer.resolve_math(&board);
    let bounds = renderer.board_bounds(&board).unwrap();
    let vp = Viewport::fit(bounds, 800, 600, 1.0);
    check_golden("sizes", &renderer.render_png(&board, &vp).unwrap());
}

/// Symbols in text, as `:sym` puts them there: `∧`, `∠` and `⊞` are Libertinus Math's, which
/// text falls back to for characters Inter lacks, set between Inter's letters on one baseline.
#[test]
fn text_with_symbols_from_the_math_font() {
    let board = common::board(
        "lawh version=1\n\
         text id=t1 x=0 y=0 \"p \u{2227} q, \u{2220}ABC, A \u{229E} B\"\n\
         text id=t2 x=0 y=1 size=2 \"\u{2227}\u{2220}\u{229E} xyz\"\n",
    );
    let mut renderer = Renderer::new_trusted();
    let bounds = renderer.board_bounds(&board).unwrap();
    let vp = Viewport::fit(bounds, 800, 300, 1.0);
    check_golden("text_symbols", &renderer.render_png(&board, &vp).unwrap());
}

/// A label on each object of the area board but the current one (`t2`), in the order the
/// engine gives them (nearest the cursor first), as `texts` gives them out.
fn area_labels(texts: &[&str]) -> Vec<JumpLabel> {
    let board = common::area_board();
    let order = ["l1", "c1", "m1", "t1"];
    order
        .iter()
        .zip(texts)
        .map(|(name, text)| {
            let object = board.get(&id(name)).unwrap();
            JumpLabel {
                id: object.id.clone(),
                at: anchor(&object.shape),
                text: (*text).to_owned(),
            }
        })
        .collect()
}

/// Labels with nothing typed: one-letter labels for the two nearest objects, two-letter for the
/// rest. The view leaves the circle's centre and the title's corner off its left edge, so their
/// labels are kept inside the window; the line starts where the circle's centre is, so its
/// label (nearer) covers the circle's.
#[test]
fn overlay_jump_labels() {
    let t2 = id("t2");
    let labels = area_labels(&["a", "s", "df", "dg"]);
    let overlay = Overlay {
        cursor: OverlayCursor::Object(&t2),
        labels: Some((&labels, "")),
        status: StatusLine {
            left: "NORMAL",
            right: "s  area.lawh  1,-1",
            error: false,
        },
        ..Overlay::default()
    };
    let vp = Viewport::new(Point::new(7.0, -0.5), 1.5, 800, 600, 1.0);
    check_golden("overlay_labels", &overlay_png(Some(vp), &overlay));
}

/// The same labels after `d` was typed: only the two that start with it, their `d` dimmer.
#[test]
fn overlay_jump_labels_with_a_typed_character() {
    let t2 = id("t2");
    let labels = area_labels(&["a", "s", "df", "dg"]);
    let overlay = Overlay {
        cursor: OverlayCursor::Object(&t2),
        labels: Some((&labels, "d")),
        status: StatusLine {
            left: "NORMAL",
            right: "sd  area.lawh  1,-1",
            error: false,
        },
        ..Overlay::default()
    };
    check_golden("overlay_labels_typed", &overlay_png(None, &overlay));
}

/// `:sym plus` with its rows above the status bar, the second selected (`<Tab>` put its name
/// on the line); `⨁`, `∔` and `⊞` come from Libertinus Math, which the status font falls back
/// to, the others from Inter.
#[test]
fn overlay_sym_completions() {
    let rows: Vec<Symbol> = [
        ("plus", "+"),
        ("plus.circle", "\u{2295}"),
        ("plus.circle.big", "\u{2A01}"),
        ("plus.dot", "\u{2214}"),
        ("plus.minus", "\u{B1}"),
        ("plus.square", "\u{229E}"),
    ]
    .iter()
    .map(|(name, value)| Symbol {
        name: (*name).to_owned(),
        value: (*value).to_owned(),
    })
    .collect();
    let overlay = Overlay {
        cursor: OverlayCursor::Grid(Point::new(-4.0, -3.0)),
        command_line: Some(("sym plus.circle", 15)),
        completions: Some((&rows, Some(1))),
        status: StatusLine {
            left: "",
            right: "area.lawh  -4,-3",
            error: false,
        },
        ..Overlay::default()
    };
    check_golden("overlay_completions", &overlay_png(None, &overlay));
}
