//! The engine's side of the worker process: start it, check its handshake,
//! send one formula at a time, and kill it when it takes too long, dies or
//! sends something malformed.

use std::ffi::OsString;
use std::io::{self, BufReader};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use crate::MathError;
use crate::compile::Outcome;
use crate::protocol;
use crate::worker::WORKER_STACK_BYTES;

/// How to start the math worker: a program whose `main` calls
/// [`crate::worker_main`], with its arguments.
///
/// The app starts itself with a hidden flag; tests use the
/// `lawh-math-worker` binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerCommand {
    /// The executable.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<OsString>,
}

impl WorkerCommand {
    /// `program` with no arguments.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    /// Adds one argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }
}

/// How long a new worker has to announce itself. A normal start takes far less; the limit only
/// ends a start that hangs, so math doesn't wait on it forever.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Failed starts in a row after which a formula gets
/// [`MathError::Unavailable`]. Each later formula tries once more. A few retries ride out a
/// passing failure; a worker that can't start at all then costs each later formula one try, not
/// three.
const MAX_SPAWN_FAILURES: u32 = 3;

/// What the reader thread passes on: a handshake line or frame, or why the
/// worker's output ended.
type FromWorker = Result<Vec<u8>, String>;

/// Compiles formulas in a worker process, one at a time. The worker is
/// started on first use and after every kill.
pub(crate) struct ProcessBackend {
    command: WorkerCommand,
    timeout: Duration,
    worker: Option<Worker>,
    spawn_failures: u32,
}

impl ProcessBackend {
    pub(crate) fn new(command: WorkerCommand, timeout: Duration) -> Self {
        Self {
            command,
            timeout,
            worker: None,
            spawn_failures: 0,
        }
    }

    pub(crate) fn compile(&mut self, src: &str) -> Outcome {
        let Some(request) = protocol::encode_request(src, self.timeout) else {
            return Err(MathError::Compile("the formula is too long".to_owned()));
        };
        // A worker that died while idle never saw this formula, so a failed
        // send is retried once on a fresh worker.
        for _ in 0..2 {
            match self.worker()?.send(&request) {
                Ok(()) => return self.receive(),
                Err(err) => {
                    tracing::debug!(%err, "math worker was gone; starting a new one");
                    self.worker = None;
                }
            }
        }
        Err(MathError::Crashed)
    }

    /// Waits up to the timeout for the response to the request just sent.
    /// Anything but a well-formed response kills the worker.
    fn receive(&mut self) -> Outcome {
        let Some(worker) = &self.worker else {
            return Err(MathError::Crashed);
        };
        let failure = match worker.output.recv_timeout(self.timeout) {
            Ok(Ok(frame)) => match protocol::decode_response(&frame) {
                Ok(outcome) => return outcome,
                Err(err) => {
                    tracing::warn!(%err, "math worker sent a malformed response");
                    MathError::Crashed
                }
            },
            Ok(Err(err)) => {
                tracing::warn!(%err, "math worker died");
                MathError::Crashed
            }
            Err(RecvTimeoutError::Disconnected) => MathError::Crashed,
            Err(RecvTimeoutError::Timeout) => {
                tracing::warn!(timeout = ?self.timeout, "math worker killed: compile too slow");
                MathError::Timeout
            }
        };
        // Dropping the worker kills and reaps it.
        self.worker = None;
        Err(failure)
    }

    /// The running worker, started if needed.
    fn worker(&mut self) -> Result<&mut Worker, MathError> {
        if self.worker.as_mut().is_some_and(Worker::has_exited) {
            tracing::debug!("math worker exited while idle");
            self.worker = None;
        }
        if self.worker.is_none() {
            let attempts = MAX_SPAWN_FAILURES
                .saturating_sub(self.spawn_failures)
                .max(1);
            let mut last_error = String::new();
            for _ in 0..attempts {
                match Worker::spawn(&self.command) {
                    Ok(worker) => {
                        self.spawn_failures = 0;
                        self.worker = Some(worker);
                        break;
                    }
                    Err(err) => {
                        self.spawn_failures = self.spawn_failures.saturating_add(1);
                        tracing::warn!(
                            %err,
                            failures = self.spawn_failures,
                            "could not start the math worker"
                        );
                        last_error = err;
                    }
                }
            }
            if self.worker.is_none() {
                return Err(MathError::Unavailable(last_error));
            }
        }
        self.worker
            .as_mut()
            .ok_or_else(|| MathError::Unavailable("no math worker".to_owned()))
    }
}

/// A running worker process. Dropping it kills and reaps the process; the
/// reader thread then sees the end of the output and finishes.
struct Worker {
    child: Child,
    stdin: ChildStdin,
    /// Handshake and frames from the reader thread, in order.
    output: mpsc::Receiver<FromWorker>,
}

impl Worker {
    fn spawn(command: &WorkerCommand) -> Result<Self, String> {
        let mut child = Command::new(&command.program)
            .args(&command.args)
            // One Typst thread, not one per core: a runaway compile keeps at
            // most one core busy until it is killed.
            .env("RAYON_NUM_THREADS", "1")
            .env("RUST_MIN_STACK", WORKER_STACK_BYTES.to_string())
            // One glibc malloc arena instead of one reserved 64 MiB region
            // per thread, so the worker's address space, which its memory cap
            // limits, tracks the memory it actually uses.
            .env("MALLOC_ARENA_MAX", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|err| format!("could not run {}: {err}", command.program.display()))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            kill(&mut child);
            return Err("the math worker has no stdin or stdout".to_owned());
        };
        let (tx, output) = mpsc::channel();
        let worker = Self {
            child,
            stdin,
            output,
        };
        thread::Builder::new()
            .name("lawh-math-reader".to_owned())
            .spawn(move || read_worker(stdout, &tx))
            .map_err(|err| format!("could not start a reader thread: {err}"))?;

        match worker.output.recv_timeout(HANDSHAKE_TIMEOUT) {
            Ok(Ok(line)) if line == protocol::handshake() => Ok(worker),
            Ok(Ok(line)) => Err(format!(
                "{} is not a lawh math worker of protocol version {} (it said {:?})",
                command.program.display(),
                protocol::PROTOCOL_VERSION,
                String::from_utf8_lossy(&line).trim_end(),
            )),
            Ok(Err(err)) => Err(format!(
                "the math worker exited before its handshake: {err}"
            )),
            Err(_) => Err(format!(
                "the math worker sent no handshake within {HANDSHAKE_TIMEOUT:?}"
            )),
        }
    }

    fn send(&mut self, request: &[u8]) -> io::Result<()> {
        protocol::write_frame(&mut self.stdin, request)
    }

    fn has_exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        kill(&mut self.child);
    }
}

/// Kills `child` (a no-op if it already exited) and reaps it.
fn kill(child: &mut Child) {
    if let Err(err) = child.kill() {
        tracing::debug!(%err, "math worker already gone");
    }
    match child.wait() {
        Ok(status) => tracing::debug!(%status, "math worker ended"),
        Err(err) => tracing::warn!(%err, "could not reap the math worker"),
    }
}

/// Reads the handshake line, then frames, until the worker's output ends or
/// the engine stops listening.
fn read_worker(stdout: ChildStdout, tx: &mpsc::Sender<FromWorker>) {
    let mut input = BufReader::new(stdout);
    let handshake = protocol::read_handshake(&mut input).map_err(|err| err.to_string());
    let ok = handshake.is_ok();
    if tx.send(handshake).is_err() || !ok {
        return;
    }
    loop {
        let frame = match protocol::read_frame(&mut input) {
            Ok(Some(frame)) => Ok(frame),
            Ok(None) => Err("its output closed".to_owned()),
            Err(err) => Err(err.to_string()),
        };
        let ended = frame.is_err();
        if tx.send(frame).is_err() || ended {
            return;
        }
    }
}
