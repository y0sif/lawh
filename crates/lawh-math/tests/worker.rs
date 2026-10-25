//! Process mode: formulas from a board compile in a worker process that is
//! killed when it takes too long and restarted when it dies, so no formula
//! can crash, hang or exhaust lawh itself.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use std::fmt::Write as _;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use lawh_math::{DEFAULT_TIMEOUT, MathEngine, MathError, MathItem, MathStatus, WorkerCommand};

fn worker() -> WorkerCommand {
    WorkerCommand::new(env!("CARGO_BIN_EXE_lawh-math-worker"))
}

fn engine() -> MathEngine {
    MathEngine::with_worker(worker())
}

/// How long a failing formula may take: the timeout, then killing the worker
/// and starting a new one. Generous: a busy machine (other builds, swap) can
/// make that take seconds, and a formula that is never stopped takes forever.
const FAIL_WITHIN: Duration = DEFAULT_TIMEOUT.saturating_add(Duration::from_secs(5));

/// How long a call that must not wait for a compile may take. It takes
/// microseconds, but a busy machine can pause any thread for a while; waiting
/// for a compile here would take the whole timeout.
const NO_WAIT: Duration = DEFAULT_TIMEOUT.checked_div(2).unwrap();

/// A PNG header claiming 40000x40000 pixels, like the verifier's PNG bomb.
fn png_bomb() -> String {
    let mut bytes = vec![137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82];
    bytes.extend_from_slice(&40000u32.to_be_bytes());
    bytes.extend_from_slice(&40000u32.to_be_bytes());
    bytes.extend_from_slice(&[1, 0, 0, 0, 0, 121, 119, 51, 168]);
    let mut src = "#image(bytes((".to_owned();
    for b in bytes {
        write!(src, "{b},").unwrap();
    }
    src.push_str(")))");
    src
}

/// Formulas that crashed, hung or ran away with memory when Typst ran on a
/// thread of the app (verifier repros, M1).
fn hostile() -> Vec<(&'static str, String)> {
    [
        // Stack overflows.
        (
            "xml_deep",
            r#"#xml(bytes("<a>" * 200000 + "</a>" * 200000)).len()"#,
        ),
        (
            "nest_content",
            "#let c = [x]; #for i in range(100000) { c = box(c) }; #c",
        ),
        (
            "nest_arrays_repr",
            "#let a = (); #for i in range(100000) { a = (a,) }; #repr(a).len()",
        ),
        (
            "flatten_deep",
            "#let a = (); #for i in range(1000000) { a = (a,) }; #a.flatten().len()",
        ),
        // Memory.
        ("huge_range", "#range(100000000).len()"),
        ("huge_range2", "#range(1000000000).len()"),
        ("huge_range3", "#range(9223372036854775807).len()"),
        ("huge_str2", r#"#("a" * 3000000000).len()"#),
        (
            "huge_str3",
            r#"#let s = "a" * 100000000; #(s + s + s + s + s + s + s + s).len()"#,
        ),
        (
            "expo_content",
            "#let f(n) = if n == 0 [x] else [#f(n - 1)#f(n - 1)]; #f(28)",
        ),
        (
            "push_loop",
            "#let a = (); #for i in range(200000000) { a.push(i) }; #a.len()",
        ),
        // Time.
        (
            "nested_loops",
            "#for i in range(10000) { for j in range(10000) { } }",
        ),
        (
            "endless",
            "#for i in range(10000) { for j in range(10000) { for k in range(10000) { } } }",
        ),
        // Panicked in-process.
        (
            "cbor_hugearr",
            "#cbor(bytes((0x9b, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff))).len()",
        ),
    ]
    .into_iter()
    .map(|(name, src)| (name, src.to_owned()))
    .chain([("png_bomb", png_bomb())])
    .collect()
}

#[test]
fn hostile_formulas_fail_in_time_and_the_worker_recovers() {
    let engine = engine();
    for (name, src) in hostile() {
        let started = Instant::now();
        let outcome = engine.layout_blocking(&src);
        let took = started.elapsed();
        assert!(
            matches!(
                outcome,
                Err(MathError::Compile(_) | MathError::Timeout | MathError::Crashed)
            ),
            "{name}: {outcome:?}"
        );
        assert!(took < FAIL_WITHIN, "{name} took {took:?}");
    }

    // A new worker serves the next formula.
    assert!(engine.layout_blocking("A = pi r^2").is_ok());

    // Every failure is cached: asking again costs nothing.
    for (name, src) in hostile() {
        let started = Instant::now();
        assert!(
            matches!(engine.request(&src), MathStatus::Failed(_)),
            "{name}"
        );
        assert!(started.elapsed() < NO_WAIT, "{name}");
    }
}

#[test]
fn request_is_pending_until_the_waker_fires() {
    let engine = engine();
    let (tx, rx) = mpsc::channel();
    engine.set_waker(Arc::new(move || {
        let _ = tx.send(());
    }));

    let started = Instant::now();
    assert_eq!(engine.request("A = pi r^2"), MathStatus::Pending);
    // Asking again doesn't queue it twice.
    assert_eq!(engine.request("A = pi r^2"), MathStatus::Pending);
    assert!(started.elapsed() < NO_WAIT);

    rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let MathStatus::Ready(first) = engine.request("A = pi r^2") else {
        panic!("expected a layout after the waker fired");
    };
    let MathStatus::Ready(second) = engine.request("A = pi r^2") else {
        panic!("expected the cached layout");
    };
    assert!(Arc::ptr_eq(&first, &second));
    // One compile, one wake.
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn a_hostile_request_does_not_block_the_caller() {
    let engine = engine();
    let (tx, rx) = mpsc::channel();
    engine.set_waker(Arc::new(move || {
        let _ = tx.send(());
    }));
    let endless = "#for i in range(10000) { for j in range(10000) { for k in range(10000) { } } }";

    let started = Instant::now();
    assert_eq!(engine.request(endless), MathStatus::Pending);
    assert_eq!(engine.request("x^2"), MathStatus::Pending);
    assert!(started.elapsed() < NO_WAIT);

    // The endless loop times out, then x^2 compiles on a new worker.
    let deadline = Instant::now() + FAIL_WITHIN + Duration::from_secs(5);
    while !matches!(engine.request("x^2"), MathStatus::Ready(_)) {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "x^2 never compiled");
        let _ = rx.recv_timeout(left);
    }
    assert_eq!(
        engine.request(endless),
        MathStatus::Failed(MathError::Timeout)
    );
}

#[test]
fn worker_layouts_match_in_process_layouts_exactly() {
    let in_process = MathEngine::new();
    let worker = engine();
    for src in [
        "A = pi r^2",
        "x^2",
        "a/b",
        "sum_(i=1)^n i",
        "mat(1, 2; 3, 4)",
        "integral_0^infinity e^(-x^2) dif x = sqrt(pi)/2",
        "#rotate(45deg, $x$)",
        "#text(fill: red)[y] + #line(length: 1em)",
        "x #place(dx: 300pt, dy: -200pt)[y]",
        "#nope",
    ] {
        assert_eq!(worker.layout_blocking(src), in_process.layout(src), "{src}");
    }
}

#[test]
fn huge_layouts_are_too_large_in_the_worker_too() {
    let in_process = MathEngine::new();
    let worker = engine();
    for src in [
        "#circle(radius: 1e18pt)",
        "#box(width: 1e9pt)",
        "#h(1e9pt)",
        "#line(length: 1e12pt)",
    ] {
        let started = Instant::now();
        assert_eq!(
            worker.layout_blocking(src),
            Err(MathError::TooLarge),
            "{src}"
        );
        assert!(started.elapsed() < FAIL_WITHIN, "{src}");
        assert_eq!(in_process.layout(src), Err(MathError::TooLarge), "{src}");
        // Cached like any failure.
        let started = Instant::now();
        assert_eq!(
            worker.request(src),
            MathStatus::Failed(MathError::TooLarge),
            "{src}"
        );
        assert!(started.elapsed() < NO_WAIT, "{src}");
    }
    // The worker is still the same healthy one.
    assert!(worker.layout_blocking("A = pi r^2").is_ok());
}

#[test]
fn too_complex_layouts_are_too_large_in_the_worker_too() {
    // Generous: the review's 490,000 glyphs take about a second to lay out in
    // release, longer in a debug worker on a busy machine.
    let worker = MathEngine::with_worker(worker()).with_timeout(Duration::from_secs(60));
    for src in [
        r#"#range(700).map(_ => "x" * 700).join(linebreak())"#,
        "#polygon(stroke: 100pt, ..range(100000).map(i => (calc.rem(i, 2) * 300pt, calc.rem(i, 3) * 300pt)))",
    ] {
        assert_eq!(
            worker.layout_blocking(src),
            Err(MathError::TooLarge),
            "{src}"
        );
        assert_eq!(
            worker.request(src),
            MathStatus::Failed(MathError::TooLarge),
            "{src}"
        );
    }
    assert!(worker.layout_blocking("A = pi r^2").is_ok());
}

#[test]
fn ink_survives_the_trip_from_the_worker() {
    let in_process = MathEngine::new();
    let worker = engine();
    for src in [
        "x #place(dx: 300pt, dy: -200pt)[y]",
        "#move(dx: -50pt)[$x$]",
        "#line(length: 100pt, stroke: 10pt)",
        "#h(1em)",
    ] {
        let from_worker = worker.layout_blocking(src).unwrap();
        assert_eq!(
            from_worker.ink,
            in_process.layout(src).unwrap().ink,
            "{src}"
        );
    }
}

#[test]
fn big_real_formulas_lay_out_in_the_worker() {
    let rows: Vec<String> = (1..=30)
        .map(|i| {
            let row: Vec<String> = (1..=30).map(|j| format!("a_({i},{j})")).collect();
            row.join(", ")
        })
        .collect();
    let matrix = format!("mat({})", rows.join("; "));
    // Generous: a debug worker on a busy machine is slow.
    let timeout = Duration::from_secs(30);
    let in_process = MathEngine::new().with_timeout(timeout);
    let worker = MathEngine::with_worker(worker()).with_timeout(timeout);
    for src in [matrix.as_str(), "sum_(i=1)^n"] {
        let layout = worker.layout_blocking(src);
        assert!(layout.is_ok(), "{layout:?}");
        assert_eq!(layout, in_process.layout(src));
    }
}

#[test]
fn layouts_of_small_items_come_back_from_the_worker() {
    // Each rotated em space is an empty filled path: 10 bytes on the wire, under the 21 of a
    // glyph run, which the item count used to be checked against. It came back `Crashed`.
    let src = r#"#for i in range(2000) { rotate(1deg, text("\u{2003}")) }"#;
    // Generous: a debug worker on a busy machine is slow.
    let timeout = Duration::from_secs(30);
    let in_process = MathEngine::new().with_timeout(timeout).layout(src).unwrap();
    let empty_paths = in_process
        .items
        .iter()
        .filter(|item| matches!(item, MathItem::Path { path, .. } if path.elements().is_empty()))
        .count();
    assert_eq!(empty_paths, 2000);
    let worker = MathEngine::with_worker(worker()).with_timeout(timeout);
    let from_worker = worker.layout_blocking(src);
    assert!(
        from_worker.as_ref() == Ok(&in_process),
        "{:?}",
        from_worker.map(|layout| layout.items.len())
    );
}

/// A wait with a deadline ends there, without killing the worker or caching a failure: the
/// compile carries on in the worker and is cached when it finishes, so an export that ran out of
/// time doesn't mark a slow but sound formula as broken.
#[test]
fn a_wait_until_a_deadline_leaves_the_worker_compiling() {
    // Nine million loop iterations: a second or so, well within this timeout.
    let slow = "#for i in range(3000) { for j in range(3000) { } } x";
    let engine = MathEngine::with_worker(worker()).with_timeout(Duration::from_secs(60));
    // The worker started, so the deadline below doesn't include starting it.
    engine.layout_blocking("x").unwrap();
    let wait = Duration::from_millis(100);
    let started = Instant::now();
    assert_eq!(
        engine.layout_until(slow, started + wait),
        Err(MathError::Timeout)
    );
    let took = started.elapsed();
    println!(
        "waited {:.1} ms for a deadline {wait:?} away",
        took.as_secs_f64() * 1e3
    );
    // Waking up takes microseconds; a busy machine can pause a thread for a while.
    assert!(took >= wait && took < wait + NO_WAIT, "{took:?}");
    assert_eq!(engine.request(slow), MathStatus::Pending);
    let later = Instant::now() + Duration::from_secs(60);
    assert!(engine.layout_until(slow, later).is_ok());
    // One compile of each: the slow one was not started again.
    assert_eq!(engine.compiles(), 2);
}

#[test]
fn layout_in_worker_mode_goes_through_the_worker() {
    let engine = engine();
    let first = engine.layout("A = pi r^2").unwrap();
    let second = engine.layout_blocking("A = pi r^2").unwrap();
    assert!(Arc::ptr_eq(&first, &second));
}

#[test]
fn a_missing_worker_is_unavailable() {
    let missing = std::env::temp_dir().join("lawh-no-such-math-worker");
    let engine = MathEngine::with_worker(WorkerCommand::new(missing));
    let started = Instant::now();
    assert!(matches!(
        engine.layout_blocking("x"),
        Err(MathError::Unavailable(_))
    ));
    assert!(matches!(
        engine.request("y"),
        MathStatus::Pending | MathStatus::Failed(MathError::Unavailable(_))
    ));
    // Not after waiting for the timeout or for a handshake.
    assert!(started.elapsed() < DEFAULT_TIMEOUT);
}

#[test]
fn a_program_that_is_not_the_worker_is_unavailable() {
    // This test binary lists its tests on stdout instead of a handshake.
    let not_a_worker = WorkerCommand::new(std::env::current_exe().unwrap()).arg("--list");
    let engine = MathEngine::with_worker(not_a_worker);
    match engine.layout_blocking("x") {
        Err(MathError::Unavailable(message)) => {
            assert!(message.contains("not a lawh math worker"), "{message}");
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

/// Runs every `*.typ` formula in `$LAWH_MATH_REPROS` through a worker and
/// prints what each did and how long it took. Not part of the suite:
/// `LAWH_MATH_REPROS=dir cargo test -p lawh-math --test worker -- --ignored --nocapture run_repro`.
#[test]
#[ignore = "needs a directory of formulas in LAWH_MATH_REPROS"]
fn run_repro_directory() {
    let dir = std::env::var_os("LAWH_MATH_REPROS").expect("set LAWH_MATH_REPROS");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "typ"))
        .collect();
    paths.sort();
    let engine = engine();
    for path in paths {
        let src = std::fs::read_to_string(&path).unwrap();
        let started = Instant::now();
        let outcome = engine.layout_blocking(&src);
        let took = started.elapsed();
        let summary = match &outcome {
            Ok(layout) => format!("Ready {} items", layout.items.len()),
            Err(MathError::Compile(message)) => {
                format!("Compile {}", message.chars().take(70).collect::<String>())
            }
            Err(err) => format!("{err:?}"),
        };
        println!(
            "{:<20} {:>6} ms  {summary}",
            path.file_stem().unwrap().to_string_lossy(),
            took.as_millis()
        );
    }
    assert!(engine.layout_blocking("A = pi r^2").is_ok());
}

/// The review's churn: 40 different formulas at the glyph cap, each a run per glyph (about
/// 2 MB cached), all asked for on every frame as if all were on screen: together they don't fit
/// in `MAX_CACHED_LAYOUT_BYTES`. Before `MathStatus::Evicted` they were compiled again as fast as
/// the worker could (2,099 compiles in 55 s). Now the engine compiles a layout pushed out again
/// at most `MAX_RECOMPILE_BURST` at once and one per `RECOMPILE_INTERVAL`, oldest pushed out
/// first. Runs for a minute and prints the compiles, the rate, and how many formulas were drawn.
/// `nice -n 19 cargo test --release -p lawh-math --test worker -- --ignored --nocapture churn`
#[test]
#[ignore = "a measurement, not a check; slow in debug"]
fn measure_churn_of_formulas_that_do_not_fit_the_cache() {
    // Before the engine, so `allowed` below counts no less than its limiter does.
    let started = Instant::now();
    let worker = MathEngine::with_worker(worker()).with_timeout(Duration::from_secs(10));
    let sources: Vec<String> = (0..40)
        .map(|k| format!("#for i in range({}) {{ box(width: 0pt)[x] }}", 24_999 - k))
        .collect();
    let mut frames = 0;
    let mut first_compiles = None;
    let mut drawn = vec![false; sources.len()];
    let statuses = loop {
        frames += 1;
        let statuses: Vec<MathStatus> = sources.iter().map(|src| worker.request(src)).collect();
        if first_compiles.is_none() && !statuses.contains(&MathStatus::Pending) {
            first_compiles = Some((Instant::now(), worker.compiles()));
        }
        if first_compiles.is_some() {
            for (drawn, status) in drawn.iter_mut().zip(&statuses) {
                *drawn |= matches!(status, MathStatus::Ready(_));
            }
        }
        if started.elapsed() > Duration::from_secs(60) {
            break statuses;
        }
        std::thread::sleep(Duration::from_millis(16));
    };
    let count = |f: fn(&MathStatus) -> bool| statuses.iter().filter(|s| f(s)).count();
    let (settled_at, first) = first_compiles.expect("the first compiles never finished");
    let again = worker.compiles() - first;
    let span = settled_at.elapsed().as_secs_f64();
    println!(
        "{first} compiles in {:.1} s until nothing was pending, then {again} more in {span:.1} s \
         ({:.2} a second); {frames} frames; {} of {} formulas drawn since; at the end {} ready, \
         {} outlines, {} failed",
        settled_at.duration_since(started).as_secs_f64(),
        again as f64 / span,
        drawn.iter().filter(|drawn| **drawn).count(),
        sources.len(),
        count(|s| matches!(s, MathStatus::Ready(_))),
        count(|s| matches!(s, MathStatus::Evicted(_))),
        count(|s| matches!(s, MathStatus::Failed(_))),
    );
    // Compiles counted before the time they are checked against.
    let compiles = worker.compiles();
    let allowed = u64::from(lawh_math::MAX_RECOMPILE_BURST)
        + u64::try_from(
            started
                .elapsed()
                .as_nanos()
                .div_ceil(lawh_math::RECOMPILE_INTERVAL.as_nanos()),
        )
        .unwrap();
    assert!(compiles <= 40 + allowed, "{compiles} > 40 + {allowed}");
}
