//! The math worker on its own, for tests: `MathEngine::with_worker` with
//! `env!("CARGO_BIN_EXE_lawh-math-worker")`. The app runs the same worker by
//! starting itself with a hidden flag.

fn main() -> std::process::ExitCode {
    lawh_math::worker_main()
}
