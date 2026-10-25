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

/// How long each stuck compile below should run: far longer than the
/// `MAX_WORKERS` 20 ms timeouts it must outlast, far shorter than the test's
/// 120 s deadline.
const STUCK_FOR: Duration = Duration::from_secs(3);

/// Outer loop count for a formula that runs about [`STUCK_FOR`] on this
/// machine. A fixed loop doesn't work: 1500 x 1500 iterations outlasted the
/// timeouts on CI's Linux runners but finished first on its macOS arm64 one
/// (CI run 40217935423). So time 1000 x 1000 iterations here and scale.
fn stuck_rows() -> u64 {
    let engine = MathEngine::new().with_timeout(Duration::from_secs(60));
    // A first compile pays one-time costs (fonts, the library); keep them out
    // of the timing.
    engine.layout("z").unwrap();
    let started = Instant::now();
    engine
        .layout("#for i in range(1000) { for j in range(1000) { } } z")
        .unwrap();
    let measured = started.elapsed().max(Duration::from_millis(1));
    let rows = 1000.0 * STUCK_FOR.as_secs_f64() / measured.as_secs_f64();
    // Bounds: at least the calibration itself, and at most 100x it, so a
    // calibration that ran unusually fast can't make the formulas outlast the
    // deadline if the machine slows down afterwards. Measured here (Linux
    // x86_64, test profile): 106 to 128 ms for 1000 x 1000, so about 25,000
    // rows.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 1000..=100000 first"
    )]
    let rows = rows.clamp(1000.0, 100_000.0) as u64;
    rows
}

#[test]
fn stuck_compiles_are_capped_and_the_refusal_is_not_cached() {
    let rows = stuck_rows();
    let engine = MathEngine::new().with_timeout(Duration::from_millis(20));
    // Each runs about STUCK_FOR, far longer than the timeout, then finishes
    // on its own.
    for k in 0..MAX_WORKERS {
        let slow = format!("#for i in range({rows}) {{ for j in range(1000) {{ }} }} x_{k}");
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
