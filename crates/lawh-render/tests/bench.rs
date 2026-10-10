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
    // bounds for the fit (compiles the math), then one render.
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
        "[{profile}] first frame {:.1} ms (Renderer::new {:.1} ms, bounds incl. math compile {:.1} ms, render {:.1} ms)",
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
