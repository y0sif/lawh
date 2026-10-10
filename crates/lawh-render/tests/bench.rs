//! Frame timing for `examples/area.lawh` at 1920x1080, scale factor 1.
//!
//! Ignored by default. Run with:
//! `nice -n 19 cargo test --release -p lawh-render --test bench -- --ignored --nocapture`

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use std::hint::black_box;
use std::time::{Duration, Instant};

use lawh_render::{Renderer, Viewport};

const WARM_UP: usize = 10;
const FRAMES: usize = 50;

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[test]
#[ignore = "timing only; run with --release -- --ignored --nocapture"]
fn area_1080p_frame_time() {
    let board = common::area_board();
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };

    // First frame as the app sees it after opening a file: a new renderer,
    // bounds for the fit, then one render. Neither waits for math: the
    // formula is queued and drawn as a placeholder.
    let start = Instant::now();
    let mut renderer = Renderer::new();
    let created = start.elapsed();
    let bounds = renderer.board_bounds(&board).unwrap();
    let bounded = start.elapsed();
    let vp = Viewport::fit(bounds, 1920, 1080, 1.0);
    black_box(renderer.render(&board, &vp));
    let first = start.elapsed();

    for _ in 0..WARM_UP {
        black_box(renderer.render(&board, &vp));
    }
    let mut times: Vec<Duration> = (0..FRAMES)
        .map(|_| {
            let t = Instant::now();
            black_box(renderer.render(&board, &vp));
            t.elapsed()
        })
        .collect();
    times.sort();
    let median = (times[FRAMES / 2 - 1] + times[FRAMES / 2]) / 2;

    println!(
        "[{profile}] first frame {:.1} ms (Renderer::new {:.1} ms, bounds {:.1} ms, render {:.1} ms)",
        ms(first),
        ms(created),
        ms(bounded.saturating_sub(created)),
        ms(first.saturating_sub(bounded)),
    );
    println!(
        "[{profile}] steady frame over {FRAMES} after {WARM_UP} warm-up: median {:.2} ms, min {:.2} ms, max {:.2} ms",
        ms(median),
        ms(times[0]),
        ms(times[FRAMES - 1]),
    );
}

/// Most a frame of [`common::limits_board`] may take in a release build.
const LIMITS_BUDGET: Duration = Duration::from_millis(100);

/// Frames of a board at the core's limits, at the app's highest zoom (20) and scale factor 2,
/// looking at the places where its huge shapes cross the view. Asserts the release budget for
/// every frame. Laying out the board's 200,000 characters of text happens once, when the board
/// is opened, and is timed separately.
#[test]
#[ignore = "timing only; run with --release -- --ignored --nocapture"]
fn limits_frame_time() {
    let board = common::limits_board();
    let mut renderer = Renderer::new();
    renderer.resolve_math(&board);
    let t = Instant::now();
    black_box(renderer.board_bounds(&board));
    println!(
        "limits: laying out the text once, for the bounds: {:.1} ms",
        ms(t.elapsed())
    );
    let mut worst = Duration::ZERO;
    for (x, y) in common::LIMITS_CENTERS {
        let vp = Viewport::new(kurbo::Point::new(x, y), 20.0, 2560, 1600, 2.0);
        let t = Instant::now();
        black_box(renderer.render(&board, &vp));
        let first = t.elapsed();
        let mut times: Vec<Duration> = (0..5)
            .map(|_| {
                let t = Instant::now();
                black_box(renderer.render(&board, &vp));
                t.elapsed()
            })
            .collect();
        times.sort();
        println!(
            "limits at ({x}, {y}), zoom 20, scale 2, 2560x1600: first {:.1} ms, then median {:.2} ms, max {:.2} ms",
            ms(first),
            ms(times[2]),
            ms(times[4]),
        );
        worst = worst.max(first).max(times[4]);
    }
    if !cfg!(debug_assertions) {
        assert!(
            worst < LIMITS_BUDGET,
            "worst frame {:.1} ms is over the {:.0} ms budget",
            ms(worst),
            ms(LIMITS_BUDGET)
        );
    }
}
