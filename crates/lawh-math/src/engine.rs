//! [`MathEngine`]: compiles formulas with a time limit, either in this process
//! (trusted input) or in a worker process that can be killed (anything from a
//! board), and caches outcomes, errors included, within fixed memory bounds.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use crate::compile::{Outcome, compile, outcome_name};
use crate::layout::approx_bytes;
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

/// Most memory the layouts one engine caches may take, in bytes, estimated
/// from their glyph and path element counts and source lengths. Least
/// recently used layouts are dropped first.
///
/// A layout within the complexity caps ([`crate::MAX_LAYOUT_GLYPHS`],
/// [`crate::MAX_LAYOUT_PATH_ELEMENTS`]) takes at most about 3 MB with its
/// source, so about twenty of the largest fit; an everyday formula takes a
/// few kilobytes. A board with more formulas that big than fit has them
/// compiled again as it is drawn.
pub const MAX_CACHED_LAYOUT_BYTES: usize = 64 * 1024 * 1024;

/// Most failed formulas one engine remembers, so it doesn't compile them again
/// on every frame. Least recently used failures are dropped first.
///
/// Well above the number of objects a board may hold (500), so every failing
/// formula on one board stays cached.
pub const MAX_CACHED_FAILURES: usize = 4096;

/// Most memory the failures one engine remembers may take, in bytes: their
/// sources and error messages. Above the largest `.lawh` file (8 MiB) plus a
/// full message for each of a board's objects, so every failing formula on one
/// board stays cached.
pub const MAX_CACHED_FAILURE_BYTES: usize = 16 * 1024 * 1024;

/// Longest compile error message the engine keeps, in bytes; longer ones are
/// cut at a character boundary and end in `…`. Typst can report thousands of
/// errors for one long formula; a status line shows the first few.
pub const MAX_ERROR_MESSAGE_BYTES: usize = 1024;

/// Called by the engine whenever a queued compile finishes, from the
/// engine's own thread. Typically asks the UI to redraw.
pub type MathWaker = Arc<dyn Fn() + Send + Sync>;

/// Where a formula stands, from [`MathEngine::request`].
#[derive(Debug, Clone, PartialEq)]
pub enum MathStatus {
    /// Laid out.
    Ready(Arc<MathLayout>),
    /// Won't lay out; the error is cached, so asking again is free (unless
    /// [`MAX_CACHED_FAILURES`] later failures have pushed it out).
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
/// formula costs one compile, not one per frame. The caches are bounded:
/// layouts by [`MAX_CACHED_LAYOUT_BYTES`], failures by
/// [`MAX_CACHED_FAILURES`] and [`MAX_CACHED_FAILURE_BYTES`], least recently
/// used first out. A formula pushed out is compiled again when next asked
/// for.
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
            .field("cached", &self.shared.lock().cached_len())
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
            shared: Arc::new(Shared::new(CacheLimits::DEFAULT)),
            threads: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// The same engine with other cache limits, for tests of eviction. Set
    /// before the first compile.
    #[cfg(test)]
    fn with_cache_limits(mut self, limits: CacheLimits) -> Self {
        self.shared = Arc::new(Shared::new(limits));
        self
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
        if let Some(outcome) = state.cached(src) {
            return outcome.into();
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
        if let Some(hit) = self.shared.lock().cached(src) {
            return hit;
        }
        let started = Instant::now();
        let outcome = compile_on_thread(src, self.timeout, &self.threads)?;
        log_compile(&outcome, started.elapsed(), src.len());
        self.shared.store(src, outcome)
    }

    /// Queues `src` first in line and waits for it.
    fn wait_for(&self, src: &str) -> Outcome {
        let mut state = self.shared.lock();
        if let Some(hit) = state.cached(src) {
            return hit;
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
            if let Some(hit) = state.cached(src) {
                return hit;
            }
            if !state.queue_thread {
                return Err(MathError::Unavailable(
                    "the math queue thread stopped".to_owned(),
                ));
            }
            // Compiled, then pushed out of the cache by later results before
            // this thread looked: ask again, first in line.
            if !state.is_waiting(src) {
                state.queued.insert(src.to_owned());
                state.queue.push_front(src.to_owned());
                self.shared.changed.notify_all();
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
struct Shared {
    state: Mutex<State>,
    /// Signalled when a compile finishes, a formula is queued, or the engine
    /// shuts down.
    changed: Condvar,
    waker: Mutex<Option<MathWaker>>,
}

/// How much each of an engine's caches may hold.
#[derive(Debug, Clone, Copy)]
struct CacheLimits {
    layout_bytes: usize,
    failures: usize,
    failure_bytes: usize,
}

impl CacheLimits {
    const DEFAULT: Self = Self {
        layout_bytes: MAX_CACHED_LAYOUT_BYTES,
        failures: MAX_CACHED_FAILURES,
        failure_bytes: MAX_CACHED_FAILURE_BYTES,
    };
}

struct State {
    /// Laid-out formulas, bounded by bytes.
    ready: Lru<Arc<MathLayout>>,
    /// Failed formulas, bounded by count and bytes.
    failed: Lru<MathError>,
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
    fn new(limits: CacheLimits) -> Self {
        Self {
            ready: Lru::new(usize::MAX, limits.layout_bytes),
            failed: Lru::new(limits.failures, limits.failure_bytes),
            queue: VecDeque::new(),
            queued: HashSet::new(),
            current: None,
            queue_thread: false,
            shutdown: false,
        }
    }

    /// The cached outcome of `src`, marked as just used.
    fn cached(&mut self, src: &str) -> Option<Outcome> {
        if let Some(layout) = self.ready.get(src) {
            return Some(Ok(layout));
        }
        self.failed.get(src).map(Err)
    }

    fn cached_len(&self) -> usize {
        self.ready.len() + self.failed.len()
    }

    /// Caches `outcome` unless `src` is already cached (another compile got
    /// there first), and returns what is cached. Compile error messages are
    /// cut to [`MAX_ERROR_MESSAGE_BYTES`] first.
    fn store(&mut self, src: &str, outcome: Outcome) -> Outcome {
        if let Some(hit) = self.cached(src) {
            return hit;
        }
        match outcome {
            Ok(layout) => {
                let bytes = src.len().saturating_add(approx_bytes(&layout));
                Ok(self.ready.insert(src, layout, bytes))
            }
            Err(err) => {
                let err = match err {
                    MathError::Compile(message) => MathError::Compile(shorten(message)),
                    other => other,
                };
                let bytes = src.len().saturating_add(error_bytes(&err));
                Err(self.failed.insert(src, err, bytes))
            }
        }
    }

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
    fn new(limits: CacheLimits) -> Self {
        Self {
            state: Mutex::new(State::new(limits)),
            changed: Condvar::new(),
            waker: Mutex::new(None),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // No update panics (plain map operations and saturating sums), so a
        // panic can't leave the state half-written; a poisoned lock is safe
        // to use.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Caches `outcome` unless another compile of `src` got there first, and
    /// returns what is cached, so every caller gets the same `Arc`.
    fn store(&self, src: &str, outcome: Outcome) -> Outcome {
        let stored = self.lock().store(src, outcome);
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

    fn finish(&self, src: &str, outcome: Outcome) {
        {
            let mut state = self.lock();
            state.current = None;
            // Waiters read the outcome from the cache.
            let _ = state.store(src, outcome);
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
        shared.finish(&src, outcome);
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
            let _ = state.store(&src, Err(MathError::Panicked));
        }
        drop(state);
        self.0.changed.notify_all();
    }
}

/// A map from formula source to `V` that drops its least recently used
/// entries to stay within a number of entries and an estimate of bytes. The
/// entry just inserted is never dropped, so a single entry over the byte
/// limit is still kept, alone.
struct Lru<V> {
    entries: HashMap<Arc<str>, Entry<V>>,
    /// Every key by when it was last used, oldest first.
    by_use: BTreeMap<u64, Arc<str>>,
    /// Counts up on every use.
    clock: u64,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

struct Entry<V> {
    value: V,
    bytes: usize,
    used: u64,
}

impl<V: Clone> Lru<V> {
    fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            by_use: BTreeMap::new(),
            clock: 0,
            bytes: 0,
            max_entries,
            max_bytes,
        }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// The value for `key`, marked as just used.
    fn get(&mut self, key: &str) -> Option<V> {
        let now = self.tick();
        let entry = self.entries.get_mut(key)?;
        if let Some(key) = self.by_use.remove(&entry.used) {
            self.by_use.insert(now, key);
        }
        entry.used = now;
        Some(entry.value.clone())
    }

    /// Adds `value`, about `bytes` big, for a `key` that isn't in the map,
    /// then drops the least recently used other entries until the map is
    /// within its limits. Returns `value`.
    fn insert(&mut self, key: &str, value: V, bytes: usize) -> V {
        let now = self.tick();
        let key: Arc<str> = Arc::from(key);
        self.by_use.insert(now, Arc::clone(&key));
        let old = self.entries.insert(
            key,
            Entry {
                value: value.clone(),
                bytes,
                used: now,
            },
        );
        if let Some(old) = old {
            // Callers check first, but stay consistent if one didn't.
            self.by_use.remove(&old.used);
            self.bytes = self.bytes.saturating_sub(old.bytes);
        }
        self.bytes = self.bytes.saturating_add(bytes);
        while self.entries.len() > self.max_entries.max(1) || self.bytes > self.max_bytes {
            let Some((&oldest, _)) = self.by_use.first_key_value() else {
                break;
            };
            if oldest == now {
                break;
            }
            if let Some(key) = self.by_use.remove(&oldest)
                && let Some(entry) = self.entries.remove(&key)
            {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
            }
        }
        value
    }
}

/// `message` cut to at most [`MAX_ERROR_MESSAGE_BYTES`], at a character
/// boundary, ending in `…` if it was cut.
fn shorten(mut message: String) -> String {
    if message.len() <= MAX_ERROR_MESSAGE_BYTES {
        return message;
    }
    let ellipsis = '…';
    let mut end = MAX_ERROR_MESSAGE_BYTES - ellipsis.len_utf8();
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message.truncate(end);
    message.push(ellipsis);
    message
}

/// About how many bytes a cached error takes.
fn error_bytes(err: &MathError) -> usize {
    let text = match err {
        MathError::Compile(message) | MathError::Unavailable(message) => message.len(),
        MathError::Timeout | MathError::Panicked | MathError::Crashed | MathError::TooLarge => 0,
    };
    size_of::<MathError>().saturating_add(text)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_drops_the_least_recently_used_first() {
        let mut lru = Lru::new(3, usize::MAX);
        for key in ["a", "b", "c"] {
            lru.insert(key, key.to_owned(), 1);
        }
        // Using `a` makes `b` the oldest.
        assert_eq!(lru.get("a").as_deref(), Some("a"));
        lru.insert("d", "d".to_owned(), 1);
        assert_eq!(lru.get("b"), None);
        for key in ["a", "c", "d"] {
            assert_eq!(lru.get(key).as_deref(), Some(key));
        }
        assert_eq!((lru.len(), lru.bytes, lru.by_use.len()), (3, 3, 3));
    }

    #[test]
    fn lru_stays_within_its_bytes_but_keeps_the_newest() {
        let mut lru = Lru::new(usize::MAX, 10);
        lru.insert("a", 1, 4);
        lru.insert("b", 2, 4);
        lru.insert("c", 3, 4);
        assert_eq!(
            (lru.get("a"), lru.get("b"), lru.get("c")),
            (None, Some(2), Some(3))
        );
        assert_eq!(lru.bytes, 8);
        // Bigger than the whole budget: kept, alone, until the next insert.
        lru.insert("big", 4, 100);
        assert_eq!((lru.len(), lru.bytes, lru.get("big")), (1, 100, Some(4)));
        lru.insert("e", 5, 1);
        assert_eq!((lru.len(), lru.bytes, lru.get("e")), (1, 1, Some(5)));
        assert_eq!(lru.by_use.len(), 1);
    }

    /// The cache bytes of a one-glyph formula such as `x`.
    fn one_glyph_bytes() -> usize {
        1 + approx_bytes(&MathEngine::new().layout("x").unwrap())
    }

    #[test]
    fn evicted_layouts_are_compiled_again() {
        let one = one_glyph_bytes();
        let engine = MathEngine::new().with_cache_limits(CacheLimits {
            layout_bytes: one + one / 2,
            ..CacheLimits::DEFAULT
        });
        let first = engine.layout("x").unwrap();
        assert!(Arc::ptr_eq(&first, &engine.layout("x").unwrap()));
        engine.layout("y").unwrap();
        assert_eq!(engine.shared.lock().cached_len(), 1);
        // `x` was pushed out: compiled again, the same layout in a new `Arc`.
        let again = engine.layout("x").unwrap();
        assert!(!Arc::ptr_eq(&first, &again));
        assert_eq!(first, again);

        // `request` queues a formula that was pushed out, and the waker reports it.
        let (tx, rx) = mpsc::channel();
        engine.set_waker(Arc::new(move || {
            let _ = tx.send(());
        }));
        assert_eq!(engine.request("y"), MathStatus::Pending);
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(engine.request("y"), MathStatus::Ready(_)));
        assert_eq!(engine.request("x"), MathStatus::Pending);
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
    }

    #[test]
    fn waiting_for_an_evicted_layout_compiles_it_again() {
        // `wait_for` is how worker engines block; it runs the same in process.
        let one = one_glyph_bytes();
        let engine = MathEngine::new().with_cache_limits(CacheLimits {
            layout_bytes: one + one / 2,
            ..CacheLimits::DEFAULT
        });
        let first = engine.wait_for("x").unwrap();
        engine.wait_for("y").unwrap();
        let again = engine.wait_for("x").unwrap();
        assert!(!Arc::ptr_eq(&first, &again));
        assert_eq!(first, again);
    }

    #[test]
    fn evicted_failures_are_compiled_again() {
        let engine = MathEngine::new().with_cache_limits(CacheLimits {
            failures: 2,
            ..CacheLimits::DEFAULT
        });
        for src in ["#a1", "#b1", "#c1"] {
            assert!(
                matches!(engine.layout(src), Err(MathError::Compile(_))),
                "{src}"
            );
        }
        assert_eq!(engine.shared.lock().failed.len(), 2);
        assert!(matches!(engine.request("#c1"), MathStatus::Failed(_)));
        let (tx, rx) = mpsc::channel();
        engine.set_waker(Arc::new(move || {
            let _ = tx.send(());
        }));
        assert_eq!(engine.request("#a1"), MathStatus::Pending);
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(
            engine.request("#a1"),
            MathStatus::Failed(MathError::Compile(_))
        ));
    }

    #[test]
    fn failures_stay_within_their_bytes() {
        let engine = MathEngine::new().with_cache_limits(CacheLimits {
            failure_bytes: 3 * 1024,
            ..CacheLimits::DEFAULT
        });
        let mut state = engine.shared.lock();
        for i in 0..10 {
            let src = format!("{i}{}", "x".repeat(999));
            let _ = state.store(&src, Err(MathError::Timeout));
        }
        assert_eq!(state.failed.len(), 2);
        assert!(state.failed.bytes <= 3 * 1024, "{}", state.failed.bytes);
    }

    #[test]
    fn long_error_messages_are_cut_at_a_character_boundary() {
        assert_eq!(shorten("short".to_owned()), "short");
        let fits = "x".repeat(MAX_ERROR_MESSAGE_BYTES);
        assert_eq!(shorten(fits.clone()), fits);
        let cut = shorten("é".repeat(MAX_ERROR_MESSAGE_BYTES));
        assert!(cut.len() <= MAX_ERROR_MESSAGE_BYTES, "{}", cut.len());
        assert!(cut.ends_with("é…"), "{cut}");

        let mut state = State::new(CacheLimits::DEFAULT);
        let long = MathError::Compile("x".repeat(100_000));
        let Err(MathError::Compile(stored)) = state.store("#bad", Err(long)) else {
            panic!("expected the compile error back");
        };
        assert_eq!(stored.len(), MAX_ERROR_MESSAGE_BYTES);
        assert_eq!(state.cached("#bad"), Some(Err(MathError::Compile(stored))));
    }

    #[test]
    fn the_first_outcome_stored_wins() {
        let mut state = State::new(CacheLimits::DEFAULT);
        let layout = MathEngine::new().layout("x").unwrap();
        let stored = state.store("x", Ok(Arc::clone(&layout)));
        assert_eq!(stored, Ok(Arc::clone(&layout)));
        assert_eq!(state.store("x", Err(MathError::Timeout)), Ok(layout));
        assert_eq!(state.cached_len(), 1);
    }
}

#[cfg(test)]
mod budget_tests {
    use kurbo::BezPath;

    use super::*;
    use crate::{MAX_LAYOUT_GLYPHS, MAX_LAYOUT_PATH_ELEMENTS, MathGlyph, MathItem};

    /// The layout the caps allow that takes the most memory: every glyph in a
    /// run of its own and every path empty, since an item is bigger than a
    /// glyph or a path element.
    fn biggest_allowed() -> MathLayout {
        let glyph = MathItem::Glyphs {
            font: 1,
            size: 20.0,
            fill: [0, 0, 0, 255],
            glyphs: vec![MathGlyph {
                id: 1,
                x: 0.0,
                y: 0.0,
            }],
        };
        let path = MathItem::Path {
            path: BezPath::new(),
            fill: None,
            stroke: None,
        };
        let mut items = vec![glyph; MAX_LAYOUT_GLYPHS];
        items.extend(vec![path; MAX_LAYOUT_PATH_ELEMENTS]);
        MathLayout {
            width: 1.0,
            height: 1.0,
            baseline: 1.0,
            ink: kurbo::Rect::ZERO,
            items,
        }
    }

    #[test]
    fn at_least_sixteen_of_the_biggest_layouts_fit_the_cache() {
        let layout = biggest_allowed();
        assert_eq!(crate::layout::check_limits(&layout), Ok(()));
        // Plus the longest source a board holds: 100,000 characters, at most
        // 400,000 bytes.
        let bytes = approx_bytes(&layout) + 400_000;
        println!("biggest allowed layout: about {bytes} bytes");
        assert!(bytes * 16 <= MAX_CACHED_LAYOUT_BYTES, "{bytes} bytes");
    }
}
