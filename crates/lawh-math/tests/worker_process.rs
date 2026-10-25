//! The worker process as the OS sees it (Linux only): its memory cap holds,
//! and dropping the engine ends it. Kept in its own test binary so the only
//! math workers this process starts are the ones these tests look at.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]
#![cfg(target_os = "linux")]

use std::fs;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use lawh_math::{
    DEFAULT_TIMEOUT, MathEngine, MathError, MathStatus, WORKER_MEMORY_LIMIT_BYTES, WorkerCommand,
};

/// The tests look for "the" worker among this process's children, so they
/// take turns.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const WORKER: &str = env!("CARGO_BIN_EXE_lawh-math-worker");

/// How long a test waits for the worker to crash or be killed before giving up: only a guard
/// against a test that would otherwise hang. A busy machine (other builds, swap) can push a crash
/// or a kill seconds past [`DEFAULT_TIMEOUT`]; how long a compile may take is checked in
/// `tests/timeout.rs` and `tests/worker.rs`, not here.
const PATIENCE: Duration = DEFAULT_TIMEOUT.saturating_add(Duration::from_secs(30));

fn engine() -> MathEngine {
    MathEngine::with_worker(WorkerCommand::new(WORKER))
}

/// The pid of this process's math worker, if one is running.
fn worker_pid() -> Option<u32> {
    let me = std::process::id();
    let worker = fs::canonicalize(WORKER).unwrap();
    fs::read_dir("/proc").unwrap().find_map(|entry| {
        let pid: u32 = entry.ok()?.file_name().to_str()?.parse().ok()?;
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // "pid (comm) state ppid ...": comm may hold spaces and parentheses.
        let parent: u32 = stat
            .rsplit_once(')')?
            .1
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()?;
        let exe = fs::read_link(format!("/proc/{pid}/exe")).ok()?;
        (parent == me && exe == worker).then_some(pid)
    })
}

/// A `kB` field of `/proc/<pid>/status`, in bytes.
fn status_bytes(pid: u32, field: &str) -> Option<u64> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|line| line.starts_with(field))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

#[test]
fn the_worker_runs_under_the_memory_cap() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    let engine = engine();
    engine.layout_blocking("A = pi r^2").unwrap();
    let pid = worker_pid().expect("a running worker");

    let limits = fs::read_to_string(format!("/proc/{pid}/limits")).unwrap();
    let cap = WORKER_MEMORY_LIMIT_BYTES.to_string();
    let address_space = limits
        .lines()
        .find(|line| line.starts_with("Max address space"))
        .unwrap();
    assert_eq!(
        address_space
            .split_whitespace()
            .skip(3)
            .take(2)
            .collect::<Vec<_>>(),
        [cap.as_str(), cap.as_str()],
        "{address_space}"
    );
    let core = limits
        .lines()
        .find(|line| line.starts_with("Max core file size"))
        .unwrap();
    assert_eq!(
        core.split_whitespace().skip(4).take(2).collect::<Vec<_>>(),
        ["0", "0"],
        "{core}"
    );

    // Watch the worker while it tries to allocate gigabytes.
    let hungry = "#range(100000000).len()";
    assert_eq!(engine.request(hungry), MathStatus::Pending);
    let (mut peak_vm, mut peak_rss) = (0, 0);
    let started = Instant::now();
    let outcome = loop {
        // `VmPeak` is the kernel's peak, or the current address space when that is higher. Keep
        // the last reading, not the largest: to grow an allocation, the kernel moves it to a
        // bigger mapping (`mremap`) and counts both mappings for a moment, which a reading can
        // catch above the cap the kernel enforces (940 MiB under a 768 MiB cap in 3 of 40 runs,
        // 2026-10-11). The kernel's peak leaves that moment out, and the worker fills its last
        // mapping for a long while after the last move before it fails to grow it again.
        if let Some(vm) = status_bytes(pid, "VmPeak") {
            peak_vm = vm;
        }
        if let Some(rss) = status_bytes(pid, "VmHWM") {
            peak_rss = peak_rss.max(rss);
        }
        match engine.request(hungry) {
            MathStatus::Pending => std::thread::sleep(Duration::from_millis(1)),
            done => break done,
        }
        assert!(started.elapsed() < PATIENCE, "still pending");
    };
    println!(
        "{hungry}: {outcome:?} after {:?}; peak address space {:.0} MiB, peak RSS {:.0} MiB, cap {:.0} MiB",
        started.elapsed(),
        mib(peak_vm),
        mib(peak_rss),
        mib(WORKER_MEMORY_LIMIT_BYTES),
    );
    assert!(
        matches!(
            outcome,
            MathStatus::Failed(MathError::Crashed | MathError::Timeout)
        ),
        "{outcome:?}"
    );
    assert!(peak_vm <= WORKER_MEMORY_LIMIT_BYTES, "{peak_vm}");
    assert!(peak_rss <= WORKER_MEMORY_LIMIT_BYTES, "{peak_rss}");
    // The crashed worker was reaped before its error was cached.
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
}

#[test]
fn dropping_the_engine_ends_the_worker() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    let engine = engine();
    engine.layout_blocking("x").unwrap();
    let pid = worker_pid().expect("a running worker");
    drop(engine);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Path::new(&format!("/proc/{pid}")).exists() {
        assert!(Instant::now() < deadline, "worker {pid} still running");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Dropping the engine while its worker compiles kills the worker then, not when the compile
/// would have ended, and reaps it before the drop returns: a program that drops its engine and
/// exits leaves no worker running, nor one writing to its stderr afterwards.
#[test]
fn dropping_the_engine_mid_compile_kills_and_reaps_the_worker_at_once() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    // About ten seconds of compiling, well within this timeout.
    let engine = engine().with_timeout(Duration::from_secs(60));
    engine.layout_blocking("x").unwrap();
    let pid = worker_pid().expect("a running worker");
    let slow = "#for i in range(30000) { for j in range(3000) { } } x";
    let deadline = Instant::now() + Duration::from_millis(200);
    assert_eq!(engine.layout_until(slow, deadline), Err(MathError::Timeout));
    // The worker is still at it.
    assert!(Path::new(&format!("/proc/{pid}")).exists());
    let started = Instant::now();
    drop(engine);
    let took = started.elapsed();
    println!("dropped in {:.1} ms", took.as_secs_f64() * 1e3);
    // A kill and a wait: milliseconds; a busy machine can pause a thread for a while.
    assert!(took < Duration::from_millis(1500), "{took:?}");
    // Killed and reaped: not even a zombie is left.
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    assert_eq!(worker_pid(), None);
}

#[test]
fn a_timed_out_worker_is_killed_not_left_spinning() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    let engine = engine();
    engine.layout_blocking("x").unwrap();
    let pid = worker_pid().expect("a running worker");
    let endless = "#for i in range(10000) { for j in range(10000) { for k in range(10000) { } } }";
    assert_eq!(engine.layout_blocking(endless), Err(MathError::Timeout));
    // `layout_blocking` stops waiting a second past the timeout, even when a busy machine has
    // the engine still killing the worker then. The timeout is cached once the worker is gone.
    let started = Instant::now();
    let cached = loop {
        match engine.request(endless) {
            MathStatus::Pending => std::thread::sleep(Duration::from_millis(10)),
            done => break done,
        }
        assert!(started.elapsed() < PATIENCE, "still pending");
    };
    assert_eq!(cached, MathStatus::Failed(MathError::Timeout));
    // Killed and reaped before the timeout was cached.
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    assert_eq!(worker_pid(), None);
    // The next formula gets a new worker.
    engine.layout_blocking("y").unwrap();
    assert!(worker_pid().is_some_and(|new| new != pid));
}

/// Everyday formulas, for measuring the worker's normal footprint.
const EVERYDAY: &[&str] = &[
    "A = pi r^2",
    "x^2",
    "a/b + (x+1)/(y-1)",
    "sum_(i=1)^n i = (n(n+1))/2",
    "integral_0^infinity e^(-x^2) dif x = sqrt(pi)/2",
    "mat(1, 2, 3, 4; 5, 6, 7, 8; 9, 10, 11, 12; 13, 14, 15, 16)",
    "f(x) = cases(x &\"if\" x > 0, -x &\"otherwise\")",
    "lim_(n -> infinity) (1 + 1/n)^n = e",
    "f(x) = sum_(k=0)^infinity (f^((k))(a))/(k!) (x-a)^k",
    "nabla times bold(E) = - (partial bold(B))/(partial t)",
    "x &= (-b plus.minus sqrt(b^2 - 4 a c))/(2a) \\ y &= 1",
    "vec(1, 2, 3) dot vec(4, 5, 6) = 32",
    "binom(n, k) = n!/(k!(n-k)!)",
    "underbrace(1 + 2 + dots.c + n, n \"terms\")",
    "det mat(a, b; c, d) = a d - b c",
    "hat(x) + tilde(y) + dot(z) + arrow(v) + overline(w)",
    "#rotate(30deg, $x^2$) + #scale(150%)[$y$]",
    "#text(fill: blue)[E] = m c^2",
    "#for i in range(50) [$x_#i +$] 0",
    "P(A | B) = (P(B | A) P(A))/P(B)",
];

/// Prints the worker's peak address space and resident memory over
/// [`EVERYDAY`], which [`WORKER_MEMORY_LIMIT_BYTES`] is chosen from.
/// `cargo test -p lawh-math --test worker_process -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, not a check"]
fn measure_everyday_footprint() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    let engine = engine();
    engine.layout_blocking("x").unwrap();
    let pid = worker_pid().expect("a running worker");
    let idle_vm = status_bytes(pid, "VmPeak").unwrap();
    let idle_rss = status_bytes(pid, "VmHWM").unwrap();
    for src in EVERYDAY {
        engine.layout_blocking(src).unwrap();
    }
    // The second pass hits Typst's caches inside the worker, not ours.
    let engine2 = engine;
    for src in EVERYDAY {
        let _ = engine2.layout_blocking(&format!("{src} + 1"));
    }
    assert_eq!(worker_pid(), Some(pid), "the worker was restarted");
    println!(
        "idle: VmPeak {:.0} MiB, VmHWM {:.0} MiB; after {} formulas: VmPeak {:.0} MiB, VmHWM {:.0} MiB",
        mib(idle_vm),
        mib(idle_rss),
        EVERYDAY.len() * 2,
        mib(status_bytes(pid, "VmPeak").unwrap()),
        mib(status_bytes(pid, "VmHWM").unwrap()),
    );
}
