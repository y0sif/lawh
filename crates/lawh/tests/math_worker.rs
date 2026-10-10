//! The app binary as its own math worker (`lawh --math-worker`), driven through the engine and
//! renderer the window uses.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use std::time::{Duration, Instant};

use lawh_core::{Point, from_lawh};
use lawh_math::{MathEngine, MathError, WorkerCommand};
use lawh_render::{Renderer, Viewport};

/// Nests an array 100,000 deep, then prints it.
const HOSTILE: &str = "#let a = (); #for i in range(100000) { a = (a,) }; #repr(a).len()";

fn engine() -> MathEngine {
    MathEngine::with_worker(WorkerCommand::new(env!("CARGO_BIN_EXE_lawh")).arg("--math-worker"))
}

fn count(buf: &[u8], pick: impl Fn(&[u8; 4]) -> bool) -> usize {
    buf.as_chunks::<4>().0.iter().filter(|px| pick(px)).count()
}

#[test]
fn the_app_lays_out_math_like_the_in_process_engine() {
    let worker = engine();
    let local = MathEngine::new();
    for src in [
        "A = pi r^2",
        "sum_(i=1)^n i = (n(n+1))/2",
        "mat(1, 2; 3, 4)",
    ] {
        assert_eq!(
            worker.layout_blocking(src).unwrap(),
            local.layout_blocking(src).unwrap(),
            "{src}"
        );
    }
    assert!(matches!(
        worker.layout_blocking("frac(1,"),
        Err(MathError::Compile(_))
    ));
}

#[test]
fn a_hostile_formula_ends_red_and_the_rest_still_draws() {
    let board = from_lawh(&format!(
        "lawh version=1\nmath id=m1 x=0 y=0 \"{HOSTILE}\"\nmath id=m2 x=0 y=2 \"A = pi r^2\"\n"
    ))
    .unwrap();
    let mut renderer = Renderer::with_math(engine());
    let vp = Viewport::new(Point::new(4.0, 1.2), 1.5, 900, 300, 1.0);

    let started = Instant::now();
    renderer.render(&board, &vp);
    let first_frame = started.elapsed();
    assert_eq!(renderer.math_pending(), 2);
    assert!(first_frame < Duration::from_secs(1), "{first_frame:?}");

    // What the window does on each wake-up: draw again without waiting.
    while renderer.math_pending() > 0 {
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "math still pending"
        );
        std::thread::sleep(Duration::from_millis(20));
        let t = Instant::now();
        renderer.render(&board, &vp);
        assert!(
            t.elapsed() < Duration::from_secs(1),
            "a frame waited for math"
        );
    }
    let buf = renderer.render(&board, &vp);
    let red = count(buf, |px| px[0] > 150 && px[1] < 100 && px[2] < 100);
    let ink = count(buf, |px| px[0] < 80 && px[1] < 80 && px[2] < 80);
    assert!(red > 50, "the hostile formula is not red: {red} red pixels");
    assert!(ink > 50, "the good formula is not drawn: {ink} ink pixels");
}

#[test]
fn a_failed_worker_is_replaced_for_the_next_formula() {
    let engine = engine();
    let failed = engine.layout_blocking(HOSTILE).unwrap_err();
    assert!(
        matches!(failed, MathError::Crashed | MathError::Timeout),
        "{failed:?}"
    );
    engine.layout_blocking("x^2").unwrap();
}
