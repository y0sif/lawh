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
//! `LAWH_UPDATE_GOLDEN=1 cargo test -p lawh-render --test golden` rewrites
//! the images. Look at every rewritten image before committing it.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use std::io::Cursor;
use std::path::PathBuf;

use lawh_core::{Board, ObjectId};
use lawh_render::{Renderer, Viewport};
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
