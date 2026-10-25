//! Typst can loop and can't be cancelled: a slow formula times out, is never
//! compiled twice, and can't pile up threads.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use std::time::{Duration, Instant};

use lawh_math::{DEFAULT_TIMEOUT, MAX_WORKERS, MathEngine, MathError};

/// 10^12 loop iterations: never finishes in practice.
const ENDLESS: &str =
    "#for i in range(10000) { for j in range(10000) { for k in range(10000) { } } }";

#[test]
fn endless_loop_times_out_and_is_cached() {
    let engine = MathEngine::new();

    let started = Instant::now();
    assert_eq!(engine.layout(ENDLESS), Err(MathError::Timeout));
    let first = started.elapsed();
    // Generous: a busy machine (other builds, swap) can wake the caller seconds late. Without
    // the timeout this would never return.
    assert!(
        first < DEFAULT_TIMEOUT + Duration::from_secs(5),
        "took {first:?}"
    );

    let started = Instant::now();
    assert_eq!(engine.layout(ENDLESS), Err(MathError::Timeout));
    let second = started.elapsed();
    // Compiling it again would take the whole timeout; a cache hit takes microseconds, but a
    // busy machine can pause any thread for a while.
    assert!(second < DEFAULT_TIMEOUT / 2, "cache hit took {second:?}");
}

/// The engine's timeout in the test below. Long enough for a trivial formula
/// to compile while every core is busy: at 20 ms, CI's Windows runner timed
/// `y` out for real (run 40219261659), and a real timeout is cached, so it
/// never compiled. Short enough that `MAX_WORKERS` of them (2 s) pass well
/// within [`STUCK_FOR`].
const TIMEOUT: Duration = Duration::from_millis(500);

/// How long each stuck compile below should run: five times the
/// `MAX_WORKERS` timeouts it must outlast (2 s), and far shorter than the
/// test's 120 s deadline. The scaling is rough: aiming at 6 s, two runs here
/// took 3.4 s and 6.3 s in all, so the margin covers a calibration off by 2x.
const STUCK_FOR: Duration = Duration::from_secs(10);

/// Side of a square loop that runs about [`STUCK_FOR`] on this machine. A
/// fixed loop doesn't work: 1500 x 1500 iterations outlasted the timeouts on
/// CI's Linux runners but finished first on its macOS arm64 one (CI run
/// 40217935423). So time 1000 x 1000 iterations here and scale.
fn stuck_side() -> u64 {
    let engine = MathEngine::new().with_timeout(Duration::from_secs(60));
    // A first compile pays one-time costs (fonts, the library); keep them out
    // of the timing.
    engine.layout("z").unwrap();
    let started = Instant::now();
    engine
        .layout("#for i in range(1000) { for j in range(1000) { } } z")
        .unwrap();
    let measured = started.elapsed().max(Duration::from_millis(1));
    let side = 1000.0 * (STUCK_FOR.as_secs_f64() / measured.as_secs_f64()).sqrt();
    // Bounds: at least the calibration itself, and at most 1000x its
    // iterations, so a calibration that ran unusually fast can't make the
    // formulas outlast the deadline if the machine slows down afterwards.
    // Measured here (Linux x86_64, test profile): 106 to 128 ms for
    // 1000 x 1000.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 1000..=31623 first"
    )]
    let side = side.clamp(1000.0, 31_623.0) as u64;
    side
}

#[test]
fn stuck_compiles_are_capped_and_the_refusal_is_not_cached() {
    let side = stuck_side();
    let engine = MathEngine::new().with_timeout(TIMEOUT);
    // Each runs about STUCK_FOR, far longer than the timeout, then finishes
    // on its own.
    for k in 0..MAX_WORKERS {
        let slow = format!("#for i in range({side}) {{ for j in range({side}) {{ }} }} x_{k}");
        assert_eq!(engine.layout(&slow), Err(MathError::Timeout));
    }

    // Every worker is busy, so even a trivial formula is refused without
    // being compiled (it would compile in a few ms).
    assert_eq!(engine.layout("y"), Err(MathError::Timeout));

    // The refusal isn't cached: once the workers finish, the same source
    // compiles.
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match engine.layout("y") {
            Ok(_) => break,
            Err(MathError::Timeout) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            other => panic!("expected the formula to compile eventually, got {other:?}"),
        }
    }
}
