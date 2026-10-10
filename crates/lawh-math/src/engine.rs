//! [`MathEngine`]: compiles formulas with a time limit, either in this process
//! (trusted input) or in a worker process that can be killed (anything from a
//! board), and caches every outcome, errors included.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use crate::compile::{Outcome, compile, outcome_name};
use crate::process::{ProcessBackend, WorkerCommand};
use crate::{MathError, MathLayout};

/// How long one compile may take by default.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

/// Stack of in-process compile threads. Typst recurses through nested content
/// and math; a big stack keeps deep but legitimate formulas from overflowing.
const IN_PROCESS_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Most compile threads one in-process engine keeps alive. Typst can't be
/// cancelled, so a timed-out compile keeps running detached until it finishes
/// on its own; past this many, new compiles are refused instead of piling up
/// threads. Process mode has no such limit: it kills the worker instead.
pub const MAX_WORKERS: usize = 4;

/// What [`MathEngine::layout_blocking`] waits for beyond the timeout: killing
/// a stuck worker and starting a new one.
const BLOCKING_MARGIN: Duration = Duration::from_secs(1);

/// Called by the engine whenever a queued compile finishes, from the
/// engine's own thread. Typically asks the UI to redraw.
pub type MathWaker = Arc<dyn Fn() + Send + Sync>;

/// Where a formula stands, from [`MathEngine::request`].
#[derive(Debug, Clone, PartialEq)]
pub enum MathStatus {
    /// Laid out.
    Ready(Arc<MathLayout>),
    /// Won't lay out; the error is cached, so asking again is free.
    Failed(MathError),
    /// Queued or compiling. The waker is called when it finishes.
    Pending,
}

impl From<Outcome> for MathStatus {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Ok(layout) => Self::Ready(layout),
            Err(err) => Self::Failed(err),
        }
    }
}

/// Compiles Typst math to [`MathLayout`]s, sandboxed, time-limited and cached
/// by source text.
///
/// Every outcome is cached, errors and timeouts included, so a hostile
/// formula costs one compile, not one per frame.
///
/// There are two modes:
///
/// - [`MathEngine::new`] compiles in this process, on a thread per formula.
///   A thread can't contain a stack overflow, can't be stopped and can't be
///   capped in memory, so this mode is **for trusted input only**: tests,
///   goldens, formulas lawh itself writes.
/// - [`MathEngine::with_worker`] compiles in a separate worker process (see
///   [`crate::worker_main`]) whose memory is capped. A formula that takes too
///   long gets the worker killed; one that crashes it gets
///   [`MathError::Crashed`]. Either way the next formula starts a fresh
///   worker. Use this for anything that comes from a board.
///
/// [`MathEngine::request`] never blocks, for the UI;
/// [`MathEngine::layout_blocking`] waits, for exports and tests. Both work in
/// either mode and share one cache.
pub struct MathEngine {
    timeout: Duration,
    mode: Mode,
    shared: Arc<Shared>,
    /// In-process compile threads alive, shared with the threads so they can
    /// count themselves out even after the engine is dropped.
    threads: Arc<AtomicUsize>,
}

#[derive(Debug, Clone)]
enum Mode {
    InProcess,
    Worker(WorkerCommand),
}

impl fmt::Debug for MathEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MathEngine")
            .field("timeout", &self.timeout)
            .field("mode", &self.mode)
            .field("cached", &self.shared.lock().cache.len())
            .finish_non_exhaustive()
    }
}

impl Default for MathEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MathEngine {
    fn drop(&mut self) {
        // The queue thread finishes its current compile, then stops and kills
        // the worker.
        self.shared.lock().shutdown = true;
        self.shared.changed.notify_all();
    }
}

impl MathEngine {
    /// An in-process engine with the [`DEFAULT_TIMEOUT`] and an empty cache.
    /// For trusted input only; see [`MathEngine`].
    pub fn new() -> Self {
        Self::with_mode(Mode::InProcess)
    }

    /// An engine that compiles in a worker process started with `command`,
    /// with the [`DEFAULT_TIMEOUT`] and an empty cache. The worker starts on
    /// the first compile.
    pub fn with_worker(command: WorkerCommand) -> Self {
        Self::with_mode(Mode::Worker(command))
    }

    fn with_mode(mode: Mode) -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            mode,
            shared: Arc::new(Shared::default()),
            threads: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Sets how long one compile may take before [`MathError::Timeout`].
    /// Set it before the first compile.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Calls `waker` whenever a queued compile finishes, so a UI that got
    /// [`MathStatus::Pending`] knows to ask again. Replaces any earlier waker.
    pub fn set_waker(&self, waker: MathWaker) {
        *self
            .shared
            .waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(waker);
    }

    /// Where `src` stands, without blocking: the cached layout or error, or
    /// [`MathStatus::Pending`] after queueing it (once; asking again while it
    /// is queued doesn't queue it twice).
    pub fn request(&self, src: &str) -> MathStatus {
        let mut state = self.shared.lock();
        if let Some(outcome) = state.cache.get(src) {
            return outcome.clone().into();
        }
        if let Err(err) = self.ensure_queue_thread(&mut state) {
            return MathStatus::Failed(err);
        }
        if !state.is_waiting(src) {
            state.queue.push_back(src.to_owned());
            state.queued.insert(src.to_owned());
            self.shared.changed.notify_all();
        }
        MathStatus::Pending
    }

    /// Lays out `src` (Typst math, without `$`), waiting for it if needed.
    ///
    /// In process mode this waits up to the timeout plus a margin for
    /// restarting the worker, after at most one compile already running; a
    /// wait that runs out returns [`MathError::Timeout`] without caching it,
    /// and the compile carries on in the background. In-process, it is
    /// [`MathEngine::layout`].
    pub fn layout_blocking(&self, src: &str) -> Result<Arc<MathLayout>, MathError> {
        match self.mode {
            Mode::InProcess => self.layout_in_process(src),
            Mode::Worker(_) => self.wait_for(src),
        }
    }

    /// Lays out `src` (Typst math, without `$`), compiling it on first use;
    /// the same source always returns the same `Arc`.
    ///
    /// In-process, this blocks for at most the timeout on a cache miss, and
    /// returns [`MathError::Timeout`] without caching it while
    /// [`MAX_WORKERS`] timed-out compiles are still running. In process mode
    /// it is [`MathEngine::layout_blocking`].
    pub fn layout(&self, src: &str) -> Result<Arc<MathLayout>, MathError> {
        self.layout_blocking(src)
    }

    fn layout_in_process(&self, src: &str) -> Outcome {
        if let Some(hit) = self.shared.lock().cache.get(src) {
            return hit.clone();
        }
        let started = Instant::now();
        let outcome = compile_on_thread(src, self.timeout, &self.threads)?;
        log_compile(&outcome, started.elapsed(), src.len());
        self.shared.store(src, outcome)
    }

    /// Queues `src` first in line and waits for it.
    fn wait_for(&self, src: &str) -> Outcome {
        let mut state = self.shared.lock();
        if let Some(hit) = state.cache.get(src) {
            return hit.clone();
        }
        self.ensure_queue_thread(&mut state)?;
        if !state.is_current(src) {
            if state.queued.contains(src) {
                state.queue.retain(|queued| queued != src);
            } else {
                state.queued.insert(src.to_owned());
            }
            state.queue.push_front(src.to_owned());
            self.shared.changed.notify_all();
        }

        let asked = Instant::now();
        let per_compile = self.timeout.saturating_add(BLOCKING_MARGIN);
        loop {
            if let Some(hit) = state.cache.get(src) {
                return hit.clone();
            }
            if !state.queue_thread {
                return Err(MathError::Unavailable(
                    "the math queue thread stopped".to_owned(),
                ));
            }
            // At most one compile runs ahead of this one.
            let deadline = match &state.current {
                Some((current, started)) if current == src => started.checked_add(per_compile),
                _ => asked.checked_add(per_compile.saturating_mul(2)),
            };
            let wait = match deadline {
                Some(deadline) => match deadline.checked_duration_since(Instant::now()) {
                    Some(left) if !left.is_zero() => Some(left),
                    _ => return Err(MathError::Timeout),
                },
                None => None,
            };
            state = match wait {
                Some(left) => {
                    self.shared
                        .changed
                        .wait_timeout(state, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                None => self
                    .shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
            };
        }
    }

    /// Starts the queue thread if it isn't running.
    fn ensure_queue_thread(&self, state: &mut State) -> Result<(), MathError> {
        if state.queue_thread {
            return Ok(());
        }
        let backend = match &self.mode {
            Mode::InProcess => Backend::InProcess {
                timeout: self.timeout,
                threads: Arc::clone(&self.threads),
            },
            Mode::Worker(command) => {
                Backend::Process(ProcessBackend::new(command.clone(), self.timeout))
            }
        };
        let shared = Arc::clone(&self.shared);
        thread::Builder::new()
            .name("lawh-math-queue".to_owned())
            .spawn(move || run_queue(&shared, backend))
            .map_err(|err| {
                MathError::Unavailable(format!("could not start the math queue thread: {err}"))
            })?;
        // The thread can't look at the state before this lock is released.
        state.queue_thread = true;
        Ok(())
    }
}

/// State shared between the engine and its queue thread.
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// Signalled when a compile finishes, a formula is queued, or the engine
    /// shuts down.
    changed: Condvar,
    waker: Mutex<Option<MathWaker>>,
}

#[derive(Default)]
struct State {
    cache: HashMap<String, Outcome>,
    /// Formulas waiting for the queue thread, in order.
    queue: VecDeque<String>,
    /// The same formulas, for de-duplication.
    queued: HashSet<String>,
    /// The formula the queue thread is compiling, and since when.
    current: Option<(String, Instant)>,
    queue_thread: bool,
    shutdown: bool,
}

impl State {
    fn is_current(&self, src: &str) -> bool {
        self.current
            .as_ref()
            .is_some_and(|(current, _)| current == src)
    }

    fn is_waiting(&self, src: &str) -> bool {
        self.queued.contains(src) || self.is_current(src)
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // Every update is a single insert or assignment, so a panic can't
        // leave the state half-written; a poisoned lock is safe to use.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Caches `outcome` unless another compile of `src` got there first, and
    /// returns what is cached, so every caller gets the same `Arc`.
    fn store(&self, src: &str, outcome: Outcome) -> Outcome {
        let stored = self
            .lock()
            .cache
            .entry(src.to_owned())
            .or_insert(outcome)
            .clone();
        self.changed.notify_all();
        stored
    }

    /// The next formula to compile, or `None` once the engine is dropped.
    fn next_job(&self) -> Option<String> {
        let mut state = self.lock();
        loop {
            if state.shutdown {
                return None;
            }
            if let Some(src) = state.queue.pop_front() {
                state.queued.remove(&src);
                state.current = Some((src.clone(), Instant::now()));
                return Some(src);
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn finish(&self, src: String, outcome: Outcome) {
        {
            let mut state = self.lock();
            state.current = None;
            state.cache.entry(src).or_insert(outcome);
        }
        self.changed.notify_all();
        let waker = self
            .waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(waker) = waker
            && catch_unwind(AssertUnwindSafe(|| waker())).is_err()
        {
            tracing::warn!("math waker panicked");
        }
    }
}

/// How the queue thread compiles.
enum Backend {
    InProcess {
        timeout: Duration,
        threads: Arc<AtomicUsize>,
    },
    Process(ProcessBackend),
}

impl Backend {
    fn compile(&mut self, src: &str) -> Outcome {
        match self {
            // A refusal (too many stuck threads) is cached here like a
            // timeout: retrying it every frame would only spin.
            Self::InProcess { timeout, threads } => {
                compile_on_thread(src, *timeout, threads).unwrap_or_else(Err)
            }
            Self::Process(backend) => backend.compile(src),
        }
    }
}

/// The queue thread: compiles queued formulas one at a time until the engine
/// is dropped, then drops the backend, which kills the worker.
fn run_queue(shared: &Shared, mut backend: Backend) {
    let _guard = QueueGuard(shared);
    while let Some(src) = shared.next_job() {
        let started = Instant::now();
        let outcome = backend.compile(&src);
        log_compile(&outcome, started.elapsed(), src.len());
        shared.finish(src, outcome);
    }
}

/// Marks the queue thread stopped when it ends, even by panic, so the next
/// request starts a new one; a formula it was compiling is cached as
/// [`MathError::Panicked`] instead of staying pending forever.
struct QueueGuard<'a>(&'a Shared);

impl Drop for QueueGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.queue_thread = false;
        if let Some((src, _)) = state.current.take() {
            state.cache.entry(src).or_insert(Err(MathError::Panicked));
        }
        drop(state);
        self.0.changed.notify_all();
    }
}

fn log_compile(outcome: &Outcome, elapsed: Duration, src_len: usize) {
    tracing::debug!(
        elapsed_ms = elapsed.as_secs_f64() * 1000.0,
        src_len,
        outcome = outcome_name(outcome),
        "math compiled"
    );
    if matches!(
        outcome,
        Err(MathError::Timeout | MathError::Panicked | MathError::Crashed)
    ) {
        tracing::warn!(outcome = outcome_name(outcome), "math compile failed");
    }
}

/// Runs one compile on a fresh thread of this process and waits up to
/// `timeout`. `Err` means the compile never started and must not be cached.
fn compile_on_thread(
    src: &str,
    timeout: Duration,
    threads: &Arc<AtomicUsize>,
) -> Result<Outcome, MathError> {
    let Some(slot) = ThreadSlot::claim(threads) else {
        tracing::warn!(
            threads = MAX_WORKERS,
            "math compile refused: too many timed-out compiles still running"
        );
        return Err(MathError::Timeout);
    };
    let (tx, rx) = mpsc::sync_channel(1);
    let owned = src.to_owned();
    let spawned = thread::Builder::new()
        .name("lawh-math".to_owned())
        .stack_size(IN_PROCESS_STACK_BYTES)
        .spawn(move || {
            let outcome = {
                // Count this thread out before reporting, so a caller
                // that sees the result can start the next compile.
                let _slot = slot;
                catch_unwind(AssertUnwindSafe(|| compile(&owned)))
                    .unwrap_or(Err(MathError::Panicked))
            };
            if tx.send(outcome).is_err() {
                tracing::debug!("math compile finished after its caller gave up");
            }
        });
    if let Err(err) = spawned {
        tracing::warn!(%err, "could not start a math compile thread");
        return Err(MathError::Compile(format!(
            "could not start the math compiler: {err}"
        )));
    }

    Ok(match rx.recv_timeout(timeout) {
        Ok(outcome) => outcome,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(MathError::Timeout),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(MathError::Panicked),
    })
}

/// One counted compile thread; dropping it counts the thread out.
struct ThreadSlot(Arc<AtomicUsize>);

impl ThreadSlot {
    fn claim(threads: &Arc<AtomicUsize>) -> Option<Self> {
        let alive_before = threads.fetch_add(1, Ordering::AcqRel);
        // Over the cap, the slot is dropped here and counts itself back out.
        let slot = Self(Arc::clone(threads));
        (alive_before < MAX_WORKERS).then_some(slot)
    }
}

impl Drop for ThreadSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
