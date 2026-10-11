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

#[test]
fn stuck_compiles_are_capped_and_the_refusal_is_not_cached() {
    let engine = MathEngine::new().with_timeout(Duration::from_millis(20));
    // Each takes a few seconds in a debug build (a few hundred ms in
    // release), far longer than the timeout, then finishes on its own.
    for k in 0..MAX_WORKERS {
        let slow = format!("#for i in range(1500) {{ for j in range(1500) {{ }} }} x_{k}");
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
