//! Golden images: render example boards and compare them with reviewed PNGs
//! in `tests/golden/`, with a small per-pixel tolerance because CPU float
//! maths can differ between machines.
//!
//! `LAWH_UPDATE_GOLDEN=1 cargo test -p lawh-render --test golden` rewrites
//! the images. Look at every rewritten image before committing it.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use std::io::Cursor;
use std::path::PathBuf;

use lawh_core::ObjectId;
use lawh_render::{Renderer, Viewport};
use vello_cpu::Pixmap;

/// Largest per-channel difference that still counts as the same pixel.
const CHANNEL_TOLERANCE: u8 = 3;
/// Share of pixels that must be the same.
const MIN_MATCHING: f64 = 0.995;

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join(format!("{name}.png"))
}

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

    let same = actual
        .data()
        .iter()
        .zip(expected.data())
        .filter(|(a, e)| {
            [(a.r, e.r), (a.g, e.g), (a.b, e.b), (a.a, e.a)]
                .iter()
                .all(|(x, y)| x.abs_diff(*y) <= CHANNEL_TOLERANCE)
        })
        .count();
    let share = same as f64 / actual.data().len() as f64;
    if share < MIN_MATCHING {
        let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.actual.png"));
        std::fs::write(&out, png).unwrap();
        panic!(
            "{name}: only {:.3}% of pixels match {} (need {:.1}%); actual image written to {}",
            share * 100.0,
            path.display(),
            MIN_MATCHING * 100.0,
            out.display()
        );
    }
}

#[test]
fn area_fit_800x600() {
    let board = common::area_board();
    let mut renderer = Renderer::new();
    let bounds = renderer.board_bounds(&board).unwrap();
    let vp = Viewport::fit(bounds, 800, 600, 1.0);
    let png = renderer.render_png(&board, &vp).unwrap();
    check_golden("area", &png);
}

#[test]
fn area_formula_at_zoom_3() {
    let board = common::area_board();
    let mut renderer = Renderer::new();
    let formula = board.get(&ObjectId::new("m1").unwrap()).unwrap();
    let center = renderer.object_bounds(formula).unwrap().center();
    let vp = Viewport::new(center, 3.0, 800, 600, 1.0);
    let png = renderer.render_png(&board, &vp).unwrap();
    check_golden("area_zoom3", &png);
}
