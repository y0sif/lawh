//! The math worker process: compiles formulas for a [`crate::MathEngine`] in
//! another process, so a formula that overflows the stack, runs out of memory
//! or never ends takes down only this process, which the engine kills and
//! restarts.

use std::io::{self, Write as _};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::MathError;
use crate::compile::{Outcome, compile};
use crate::protocol;

/// Address space the worker process may map, set as `RLIMIT_AS` on Linux when
/// it starts. An allocation past it fails and aborts the worker, which the
/// engine reports as [`MathError::Crashed`].
///
/// Address space counts every mapping, not just memory in use: the binary and
/// the two 64 MiB thread stacks are most of an idle worker. Measured on Linux
/// `x86_64` (2026-10-10, debug test worker, `measure_everyday_footprint` in
/// `tests/worker_process.rs`): 173 MiB peak address space and 21 MiB peak
/// resident over 40 everyday formulas. The cap is about four times that peak.
pub const WORKER_MEMORY_LIMIT_BYTES: u64 = 768 * 1024 * 1024;

/// Stack of the compile thread, and of Typst's own threads through
/// `RUST_MIN_STACK` (set by the engine when it starts the worker). Typst
/// recurses through nested content; a big stack keeps deep but legitimate
/// formulas working, and anything deeper crashes only the worker.
pub(crate) const WORKER_STACK_BYTES: usize = 64 * 1024 * 1024;

/// How long past the engine's timeout the worker waits for a compile before
/// exiting on its own. The engine kills it at the timeout; this only matters
/// if the engine itself is gone.
const SELF_EXIT_GRACE: Duration = Duration::from_secs(1);

/// Entry point of the math worker process.
///
/// Call it first thing from `main` in a process started by
/// [`crate::MathEngine::with_worker`], and return its exit code. It caps the
/// process's memory, announces itself on stdout with a versioned handshake
/// line, then answers compile requests from stdin on stdout until stdin
/// closes. Stdout carries the protocol only; nothing else in the process may
/// write to it.
pub fn worker_main() -> ExitCode {
    std::panic::set_hook(Box::new(|info| {
        tracing::error!(%info, "math worker panicked");
    }));
    limit_resources();

    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    if let Err(err) = output
        .write_all(&protocol::handshake())
        .and_then(|()| output.flush())
    {
        tracing::warn!(%err, "math worker could not write its handshake");
        return ExitCode::FAILURE;
    }

    let (jobs, results) = match start_compiler() {
        Ok(channels) => channels,
        Err(err) => {
            tracing::warn!(%err, "math worker could not start its compile thread");
            return ExitCode::FAILURE;
        }
    };

    loop {
        let frame = match protocol::read_frame(&mut input) {
            Ok(Some(frame)) => frame,
            // The engine closed our stdin: it is done with us.
            Ok(None) => return ExitCode::SUCCESS,
            Err(err) => {
                tracing::warn!(%err, "math worker got a bad request frame");
                return ExitCode::FAILURE;
            }
        };
        let (timeout, src) = match protocol::decode_request(&frame) {
            Ok(request) => request,
            Err(err) => {
                tracing::warn!(%err, "math worker got a bad request");
                return ExitCode::FAILURE;
            }
        };
        if jobs.send(src.to_owned()).is_err() {
            return ExitCode::FAILURE;
        }
        // Returning from main ends the process, the stuck compile thread
        // included.
        let Ok(outcome) = results.recv_timeout(timeout.saturating_add(SELF_EXIT_GRACE)) else {
            tracing::warn!("math worker exits: compile outlived the engine's timeout");
            return ExitCode::FAILURE;
        };
        if let Err(err) = protocol::write_frame(&mut output, &protocol::encode_response(&outcome)) {
            tracing::warn!(%err, "math worker could not write a response");
            return ExitCode::FAILURE;
        }
    }
}

/// Starts the thread that compiles one formula at a time.
fn start_compiler() -> io::Result<(mpsc::Sender<String>, mpsc::Receiver<Outcome>)> {
    let (job_tx, job_rx) = mpsc::channel::<String>();
    let (result_tx, result_rx) = mpsc::channel();
    thread::Builder::new()
        .name("lawh-math-compile".to_owned())
        .stack_size(WORKER_STACK_BYTES)
        .spawn(move || {
            for src in job_rx {
                let outcome = catch_unwind(AssertUnwindSafe(|| compile(&src)))
                    .unwrap_or(Err(MathError::Panicked));
                if result_tx.send(outcome).is_err() {
                    return;
                }
            }
        })?;
    Ok((job_tx, result_rx))
}

/// Caps the address space at [`WORKER_MEMORY_LIMIT_BYTES`] (Linux only) and
/// turns off core dumps, so a crash doesn't write the worker's memory to disk.
/// Both the soft and hard limits are lowered, so nothing in the process can
/// raise them again.
///
/// macOS refuses `RLIMIT_AS` with `EINVAL` (GitHub's macOS 26 runner, CI run
/// 40216100811), so it isn't asked for there; on macOS, as on Windows, the
/// engine's kill on timeout is what bounds a runaway compile.
#[cfg(unix)]
fn limit_resources() {
    use rlimit::Resource;

    let address_space =
        cfg!(target_os = "linux").then_some((Resource::AS, WORKER_MEMORY_LIMIT_BYTES));
    for (resource, cap) in address_space.into_iter().chain([(Resource::CORE, 0)]) {
        let lowered = resource
            .get()
            .and_then(|(_, hard)| resource.set(cap.min(hard), cap.min(hard)));
        if let Err(err) = lowered {
            tracing::warn!(%err, limit = resource.as_name(), "math worker could not set a limit");
        }
    }
}

/// Windows has no address-space limit a process can set on itself without a
/// Job Object (unsafe FFI). The engine's kill on timeout bounds how long a
/// runaway compile can allocate.
#[cfg(not(unix))]
fn limit_resources() {}
