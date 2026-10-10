//! The app's math worker as the OS sees it (Linux only): it ends with its engine, and how much
//! memory it maps. Kept in its own test binary so the only workers this process starts are the
//! ones these tests look at.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]
#![cfg(target_os = "linux")]

use std::fs;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use lawh_core::{Point, from_lawh};
use lawh_math::{MathEngine, WORKER_MEMORY_LIMIT_BYTES, WorkerCommand};
use lawh_render::{Renderer, Viewport};

/// The tests look for "the" worker among this process's children, so they take turns.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const LAWH: &str = env!("CARGO_BIN_EXE_lawh");

fn engine() -> MathEngine {
    MathEngine::with_worker(WorkerCommand::new(LAWH).arg("--math-worker"))
}

/// The pid of this process's math worker, if one is running.
fn worker_pid() -> Option<u32> {
    let me = std::process::id();
    let lawh = fs::canonicalize(LAWH).unwrap();
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
        (parent == me && exe == lawh).then_some(pid)
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

fn wait_until_gone(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Path::new(&format!("/proc/{pid}")).exists() {
        assert!(Instant::now() < deadline, "worker {pid} still running");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn dropping_the_renderer_ends_the_worker() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    let board = from_lawh("lawh version=1\nmath id=m1 x=0 y=0 \"A = pi r^2\"\n").unwrap();
    let mut renderer = Renderer::with_math(engine());
    renderer.resolve_math(&board);
    renderer.render(&board, &Viewport::new(Point::ZERO, 1.0, 64, 64, 1.0));
    let pid = worker_pid().expect("a running worker");
    drop(renderer);
    wait_until_gone(pid);
}

/// Everyday formulas, for measuring the worker's normal footprint (the same twenty as
/// `lawh-math`'s measurement, plus four).
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
    "e^(i pi) + 1 = 0",
    "sqrt(x^2 + y^2) <= |x| + |y|",
    "a_n = a_(n-1) + a_(n-2)",
    "lr((sum_(k=1)^n k)^2) = sum_(k=1)^n k^3",
];

/// Prints the app worker's peak address space and resident memory over [`EVERYDAY`] twice
/// (48 compiles), against [`WORKER_MEMORY_LIMIT_BYTES`]. Measure the release binary:
/// `cargo test --release -p lawh --test worker_process -- --ignored --nocapture`.
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
    // The second pass misses this engine's cache but hits Typst's inside the worker.
    for src in EVERYDAY {
        engine.layout_blocking(&format!("{src} + 1")).unwrap();
    }
    assert_eq!(worker_pid(), Some(pid), "the worker was restarted");
    let peak_vm = status_bytes(pid, "VmPeak").unwrap();
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!(
        "[{profile}] lawh --math-worker: idle VmPeak {:.0} MiB, VmHWM {:.0} MiB; after {} formulas \
         VmPeak {:.0} MiB, VmHWM {:.0} MiB; cap {:.0} MiB ({:.0}% used)",
        mib(idle_vm),
        mib(idle_rss),
        EVERYDAY.len() * 2,
        mib(peak_vm),
        mib(status_bytes(pid, "VmHWM").unwrap()),
        mib(WORKER_MEMORY_LIMIT_BYTES),
        peak_vm as f64 / WORKER_MEMORY_LIMIT_BYTES as f64 * 100.0,
    );
}
