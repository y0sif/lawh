//! [`MathEngine`]: compiles formulas with a time limit, either in this process
//! (trusted input) or in a worker process that can be killed (anything from a
//! board), and caches outcomes, errors included, within fixed memory bounds.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;
use std::hash::{BuildHasher, RandomState};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use crate::compile::{Outcome, compile, outcome_name};
use crate::layout::approx_bytes;
use crate::process::{ProcessBackend, WorkerCommand};
use crate::{MathError, MathLayout, MathSummary};

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
/// sources and error messages. Well above the largest `.lawh` file (1 MiB,
/// `lawh_core::format::MAX_FILE_BYTES`) plus a full message for each of a
/// board's objects, so every failing formula on one board stays cached.
pub const MAX_CACHED_FAILURE_BYTES: usize = 16 * 1024 * 1024;

/// Most layouts pushed out of the cache one engine compiles again at once;
/// past that, one per [`RECOMPILE_INTERVAL`]. So in any `t` seconds it
/// compiles at most `MAX_RECOMPILE_BURST + t / RECOMPILE_INTERVAL` layouts
/// again (rounded up), 8 + 2t, however many formulas want it, each followed by
/// one call of the waker.
///
/// Layouts are only pushed out when the formulas a board shows take more than
/// [`MAX_CACHED_LAYOUT_BYTES`] together. Without a limit, every formula pushed
/// out was asked for again on the next frame and compiled again, pushing out
/// another: 40 formulas at the complexity caps, all on screen, made 2,099
/// compiles in 55 seconds and never settled. Real formulas take a few
/// kilobytes, so real boards never get here.
pub const MAX_RECOMPILE_BURST: u32 = 8;

/// How often one engine compiles a layout pushed out of the cache again, past
/// [`MAX_RECOMPILE_BURST`]: two a second.
pub const RECOMPILE_INTERVAL: Duration = Duration::from_millis(500);

/// How long after a formula's layout was pushed out of the cache the engine
/// waits before compiling it again, at the least. Long enough that formulas
/// pushing each other out don't make the window redraw continuously; short
/// enough that one panned back to is drawn again soon.
pub const RECOMPILE_COOLDOWN: Duration = Duration::from_secs(5);

/// How long a formula whose layout was pushed out of the cache stays wanted
/// after [`MathEngine::request`] last asked for it: the engine compiles it
/// again only while it is wanted. Well above how long one waits for its turn
/// without the window drawing a frame (the cooldown, or the interval, and a
/// compile; every compile then calls the waker, and the frame it draws asks
/// again), so a formula on screen stays wanted, and one no longer on screen
/// stops being compiled.
pub const WANTED_FOR: Duration = Duration::from_secs(30);

/// Most formulas one engine remembers the layout of once it is pushed out of
/// the cache: when, and a [`MathSummary`] of it. Kept by a hash of the source,
/// about 100 bytes each; the oldest are forgotten first. Well above the number
/// of objects a board may hold (500).
pub const MAX_FORMULA_RECORDS: usize = 4096;

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
    /// Laid out before, but the layout was pushed out of the cache by others
    /// and isn't cached again yet. Holds what the layout measured, so the
    /// formula can be drawn as an outline. Nothing is queued: the engine
    /// compiles it again on its own, [`RECOMPILE_COOLDOWN`] after it was
    /// pushed out at the soonest and within [`MAX_RECOMPILE_BURST`] and
    /// [`RECOMPILE_INTERVAL`], the formula pushed out longest ago first, as
    /// long as it is asked for at least every [`WANTED_FOR`]; then the waker
    /// is called.
    Evicted(MathSummary),
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
/// used first out. A failure pushed out is compiled again when next asked
/// for. A layout pushed out isn't queued by [`MathEngine::request`], which
/// answers [`MathStatus::Evicted`]: the engine compiles it again itself, after
/// [`RECOMPILE_COOLDOWN`] and within an engine-wide limit
/// ([`MAX_RECOMPILE_BURST`], [`RECOMPILE_INTERVAL`]), the one pushed out
/// longest ago first, and calls the waker. So formulas that don't fit in the
/// cache together are each drawn again in turn, but can't keep the compiler
/// busy. [`MathEngine::layout_blocking`] always compiles again: exports need
/// every formula.
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

    /// How many compiles this engine has finished and cached, layouts and
    /// failures alike, in either mode. For diagnostics and tests.
    pub fn compiles(&self) -> u64 {
        self.shared.lock().compiles
    }

    /// Sets how long one compile may take before [`MathError::Timeout`].
    /// Set it before the first compile.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Calls `waker` whenever a queued compile finishes, so a UI that got
    /// [`MathStatus::Pending`] or [`MathStatus::Evicted`] knows to ask again.
    /// Replaces any earlier waker.
    pub fn set_waker(&self, waker: MathWaker) {
        *self
            .shared
            .waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(waker);
    }

    /// Where `src` stands, without blocking: the cached layout or error,
    /// [`MathStatus::Evicted`] for a layout pushed out of the cache, which the
    /// engine compiles again itself (see [`MathEngine`]), or
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
        if let Some(summary) = state.evicted(src) {
            if state.want(src, Instant::now()) {
                self.shared.changed.notify_all();
            }
            return MathStatus::Evicted(summary);
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
    /// Signalled when a compile finishes, a formula is queued or wanted again,
    /// or the engine shuts down.
    changed: Condvar,
    waker: Mutex<Option<MathWaker>>,
}

/// How much each of an engine's caches may hold, and when a layout pushed out
/// is compiled again.
#[derive(Debug, Clone, Copy)]
struct CacheLimits {
    layout_bytes: usize,
    failures: usize,
    failure_bytes: usize,
    records: usize,
    cooldown: Duration,
    burst: u32,
    interval: Duration,
    wanted_for: Duration,
}

impl CacheLimits {
    const DEFAULT: Self = Self {
        layout_bytes: MAX_CACHED_LAYOUT_BYTES,
        failures: MAX_CACHED_FAILURES,
        failure_bytes: MAX_CACHED_FAILURE_BYTES,
        records: MAX_FORMULA_RECORDS,
        cooldown: RECOMPILE_COOLDOWN,
        burst: MAX_RECOMPILE_BURST,
        interval: RECOMPILE_INTERVAL,
        wanted_for: WANTED_FOR,
    };
}

struct State {
    /// Laid-out formulas, bounded by bytes.
    ready: Lru<Arc<MathLayout>>,
    /// Failed formulas, bounded by count and bytes.
    failed: Lru<MathError>,
    /// What was pushed out of `ready`, and when.
    records: Records,
    /// Formulas pushed out of `ready` and asked for since, by when they were
    /// last asked for: the queue thread compiles them again
    /// ([`State::recompile`]). At most as many as `records`.
    wanted: HashMap<String, Instant>,
    /// How many of them the queue thread may compile again, and when.
    recompiles: Limiter,
    cooldown: Duration,
    wanted_for: Duration,
    /// Compiles finished and cached, layouts and failures alike.
    compiles: u64,
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
            records: Records::new(limits.records),
            wanted: HashMap::new(),
            recompiles: Limiter::new(limits.burst, limits.interval, Instant::now()),
            cooldown: limits.cooldown,
            wanted_for: limits.wanted_for,
            compiles: 0,
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

    /// The summary of `src`'s layout if it was pushed out of the cache and
    /// isn't cached again yet.
    fn evicted(&self, src: &str) -> Option<MathSummary> {
        self.records.get(src)?.evicted.map(|(_, summary)| summary)
    }

    /// Marks `src`, whose layout was pushed out of the cache, as asked for at
    /// `now`, unless it is being compiled already. `true` when it wasn't
    /// wanted before, so the queue thread has something new to look at.
    fn want(&mut self, src: &str, now: Instant) -> bool {
        if self.is_waiting(src) {
            return false;
        }
        if let Some(asked) = self.wanted.get_mut(src) {
            *asked = now;
            return false;
        }
        if self.wanted.len() >= self.records.max {
            let stalest = self
                .wanted
                .iter()
                .min_by_key(|(_, asked)| **asked)
                .map(|(src, _)| src.clone());
            if let Some(stalest) = stalest {
                self.wanted.remove(&stalest);
            }
        }
        self.wanted.insert(src.to_owned(), now);
        true
    }

    /// A wanted formula to compile again now: of those pushed out at least
    /// the cooldown before `now`, the one pushed out longest ago, if the
    /// limiter lets one through (it is counted, and no longer wanted). `Err`
    /// holds when to look again, and `None` means nothing is wanted. Forgets
    /// formulas cached again, or not asked for within `wanted_for`.
    fn recompile(&mut self, now: Instant) -> Option<Result<String, Instant>> {
        let (records, wanted_for) = (&self.records, self.wanted_for);
        self.wanted.retain(|src, asked| {
            now.saturating_duration_since(*asked) <= wanted_for
                && records
                    .get(src)
                    .is_some_and(|record| record.evicted.is_some())
        });
        // The longest pushed out of those cooled down, and when the next of the others is.
        let mut oldest: Option<(Instant, &str)> = None;
        let mut cooled_at: Option<Instant> = None;
        for src in self.wanted.keys() {
            let Some((evicted_at, _)) = self.records.get(src).and_then(|record| record.evicted)
            else {
                continue;
            };
            if self.is_waiting(src) {
                continue;
            }
            let at = evicted_at.checked_add(self.cooldown).unwrap_or(now);
            if at <= now {
                if oldest.is_none_or(|(oldest, _)| evicted_at < oldest) {
                    oldest = Some((evicted_at, src));
                }
            } else if cooled_at.is_none_or(|soonest| at < soonest) {
                cooled_at = Some(at);
            }
        }
        let allowed_at = self.recompiles.next();
        match (oldest, cooled_at) {
            (Some((_, src)), _) if allowed_at <= now => {
                let src = src.to_owned();
                self.wanted.remove(&src);
                self.recompiles.take(now);
                Some(Ok(src))
            }
            (Some(_), _) => Some(Err(allowed_at)),
            (None, Some(at)) => Some(Err(at.max(allowed_at))),
            (None, None) => None,
        }
    }

    /// Caches `outcome` unless `src` is already cached (another compile got
    /// there first), and returns what is cached. Compile error messages are
    /// cut to [`MAX_ERROR_MESSAGE_BYTES`] first. Layouts the new one pushes
    /// out of the cache are recorded as evicted.
    fn store(&mut self, src: &str, outcome: Outcome) -> Outcome {
        if let Some(hit) = self.cached(src) {
            return hit;
        }
        self.compiles = self.compiles.saturating_add(1);
        match outcome {
            Ok(layout) => {
                if let Some(record) = self.records.get_mut(src) {
                    record.evicted = None;
                }
                let bytes = src.len().saturating_add(approx_bytes(&layout));
                let (layout, evicted) = self.ready.insert(src, layout, bytes);
                let now = Instant::now();
                for (gone, gone_layout) in evicted {
                    tracing::debug!(src_len = gone.len(), "math layout pushed out of the cache");
                    self.records.entry(&gone).evicted = Some((now, MathSummary::of(&gone_layout)));
                }
                Ok(layout)
            }
            Err(err) => {
                let err = match err {
                    MathError::Compile(message) => MathError::Compile(shorten(message)),
                    other => other,
                };
                let bytes = src.len().saturating_add(error_bytes(&err));
                Err(self.failed.insert(src, err, bytes).0)
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

    /// The next formula to compile, or `None` once the engine is dropped:
    /// one queued, or else a wanted one whose layout was pushed out of the
    /// cache, once its cooldown and the limiter allow.
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
            let now = Instant::now();
            let wait = match state.recompile(now) {
                Some(Ok(src)) => {
                    tracing::debug!(
                        src_len = src.len(),
                        "compiling a math layout pushed out of the cache again"
                    );
                    state.current = Some((src.clone(), now));
                    return Some(src);
                }
                Some(Err(at)) => Some(at.saturating_duration_since(now)),
                None => None,
            };
            state = match wait {
                Some(wait) => {
                    self.changed
                        .wait_timeout(state, wait)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                None => self
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
            };
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

/// What the engine remembers of a formula whose layout it pushed out of the
/// cache.
#[derive(Debug, Clone, Copy, Default)]
struct Record {
    /// When its layout was last pushed out of the cache, and what it
    /// measured; `None` once it is cached again.
    evicted: Option<(Instant, MathSummary)>,
}

/// [`Record`]s by a hash of the formula's source, at most a fixed number of
/// them, the oldest forgotten first. The hash is keyed per engine, so a board
/// can't make two formulas share a record on purpose; an accidental clash
/// would only draw one formula as an outline.
struct Records {
    hasher: RandomState,
    by_hash: HashMap<u64, Record>,
    /// Every hash in `by_hash`, oldest first.
    order: VecDeque<u64>,
    max: usize,
}

impl Records {
    fn new(max: usize) -> Self {
        Self {
            hasher: RandomState::new(),
            by_hash: HashMap::new(),
            order: VecDeque::new(),
            max: max.max(1),
        }
    }

    fn get(&self, src: &str) -> Option<&Record> {
        self.by_hash.get(&self.hasher.hash_one(src))
    }

    fn get_mut(&mut self, src: &str) -> Option<&mut Record> {
        self.by_hash.get_mut(&self.hasher.hash_one(src))
    }

    /// The record of `src`, new if there is none, forgetting the oldest if
    /// that makes too many.
    fn entry(&mut self, src: &str) -> &mut Record {
        let hash = self.hasher.hash_one(src);
        if !self.by_hash.contains_key(&hash) {
            while self.by_hash.len() >= self.max {
                let Some(oldest) = self.order.pop_front() else {
                    break;
                };
                self.by_hash.remove(&oldest);
            }
            self.order.push_back(hash);
        }
        self.by_hash.entry(hash).or_default()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.by_hash.len()
    }
}

/// Lets `burst` events through at once, then one per `interval`, evenly (a
/// generic cell rate algorithm): in any span of time `t`, at most
/// `burst + t / interval` of them, rounded up.
#[derive(Debug, Clone, Copy)]
struct Limiter {
    burst: u32,
    interval: Duration,
    epoch: Instant,
    /// When the next event would be due, after `epoch`, were they spaced
    /// `interval` apart from the first.
    due: Duration,
}

impl Limiter {
    fn new(burst: u32, interval: Duration, now: Instant) -> Self {
        Self {
            burst,
            interval,
            epoch: now,
            due: Duration::ZERO,
        }
    }

    /// The soonest the next event may happen.
    fn next(&self) -> Instant {
        let slack = self.interval.saturating_mul(self.burst.saturating_sub(1));
        self.epoch
            .checked_add(self.due.saturating_sub(slack))
            .unwrap_or(self.epoch)
    }

    /// Counts an event at `now`, no sooner than [`Self::next`].
    fn take(&mut self, now: Instant) {
        let now = now.saturating_duration_since(self.epoch);
        self.due = self.due.max(now).saturating_add(self.interval);
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
    /// within its limits. Returns `value` and the entries dropped.
    fn insert(&mut self, key: &str, value: V, bytes: usize) -> (V, Vec<(Arc<str>, V)>) {
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
        let mut dropped = Vec::new();
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
                dropped.push((key, entry.value));
            }
        }
        (value, dropped)
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
        let y = engine.layout("y").unwrap();
        assert_eq!(engine.shared.lock().cached_len(), 1);
        // `x` was pushed out: compiled again, the same layout in a new `Arc`.
        let again = engine.layout("x").unwrap();
        assert!(!Arc::ptr_eq(&first, &again));
        assert_eq!(first, again);
        assert_eq!(engine.compiles(), 3);

        // `request` doesn't compile `y`, pushed out just now: it answers with what `y`
        // measured, without queueing it.
        assert_eq!(
            engine.request("y"),
            MathStatus::Evicted(MathSummary::of(&y))
        );
        let state = engine.shared.lock();
        assert!(state.queue.is_empty() && state.current.is_none());
    }

    /// Waits for the next call of the waker `rx` stands for.
    fn woken(rx: &mpsc::Receiver<()>) {
        rx.recv_timeout(Duration::from_secs(30)).unwrap();
    }

    fn with_waker(engine: &MathEngine) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel();
        engine.set_waker(Arc::new(move || {
            let _ = tx.send(());
        }));
        rx
    }

    #[test]
    fn a_pushed_out_layout_alone_on_screen_is_drawn_again_after_the_cooldown() {
        let one = one_glyph_bytes();
        let cooldown = Duration::from_millis(300);
        let engine = MathEngine::new().with_cache_limits(CacheLimits {
            layout_bytes: one + one / 2,
            cooldown,
            ..CacheLimits::DEFAULT
        });
        let rx = with_waker(&engine);
        let ready = |src: &str| {
            assert_eq!(engine.request(src), MathStatus::Pending, "{src}");
            woken(&rx);
            let MathStatus::Ready(layout) = engine.request(src) else {
                panic!("{src} did not lay out");
            };
            layout
        };
        let x = ready("x");
        let y = ready("y");
        // `y` pushed `x` out just now. A window showing one formula at a time, as the app
        // does: it asks for the formula once, draws an outline, and draws the next frame only
        // when the waker calls. The two take turns, each pushing the other out, as often as
        // they like: no formula is held back for good.
        for (src, layout) in [("x", &x), ("y", &y), ("x", &x), ("y", &y)] {
            // Not asked for since, so not compiled again yet.
            let (pushed_out, _) = engine
                .shared
                .lock()
                .records
                .get(src)
                .unwrap()
                .evicted
                .unwrap();
            assert_eq!(
                engine.request(src),
                MathStatus::Evicted(MathSummary::of(layout)),
                "{src}"
            );
            woken(&rx);
            let waited = pushed_out.elapsed();
            assert!(
                matches!(engine.request(src), MathStatus::Ready(_)),
                "{src} was not compiled again"
            );
            println!(
                "{src} drawn again {:.0} ms after it was pushed out",
                waited.as_secs_f64() * 1e3
            );
            // Not before its cooldown, and within it and a frame or two (the compile, and a
            // busy machine).
            assert!(
                waited >= cooldown && waited <= cooldown + Duration::from_millis(250),
                "{src}: {waited:?}"
            );
        }
        assert_eq!(engine.compiles(), 6);
        // `x`, pushed out last, isn't asked for: it isn't compiled again.
        assert!(rx.recv_timeout(cooldown * 3).is_err(), "nothing was wanted");
        assert_eq!(engine.compiles(), 6);

        // Waiting for a layout always compiles it at once: exports need every formula.
        assert_eq!(engine.layout_blocking("x").unwrap(), x);
        assert_eq!(engine.compiles(), 7);
    }

    #[test]
    fn formulas_that_do_not_fit_in_the_cache_are_compiled_again_at_a_bounded_rate() {
        // Twenty formulas, only a quarter of which fit in the cache together, as forty formulas
        // at the complexity caps don't fit in `MAX_CACHED_LAYOUT_BYTES`. Asked for over and
        // over, as a window that draws frames all the time asks for every formula on screen.
        const FORMULAS: usize = 20;
        let sources: Vec<String> = (0..FORMULAS).map(|i| format!("x_{}", 100 + i)).collect();
        let bytes =
            sources[0].len() + approx_bytes(&MathEngine::new().layout(&sources[0]).unwrap());
        let limits = CacheLimits {
            layout_bytes: bytes * 5 + bytes / 2,
            cooldown: Duration::from_millis(20),
            burst: 2,
            interval: Duration::from_millis(50),
            ..CacheLimits::DEFAULT
        };
        // Before the engine, so `allowed` below counts no less than its limiter does.
        let created = Instant::now();
        let engine = MathEngine::new().with_cache_limits(limits);
        let mut drawn = [false; FORMULAS];
        let mut thrashing = None;
        loop {
            let statuses: Vec<MathStatus> = sources.iter().map(|src| engine.request(src)).collect();
            assert!(
                !statuses
                    .iter()
                    .any(|status| matches!(status, MathStatus::Failed(_))),
                "{statuses:?}"
            );
            let compiles = engine.compiles();
            if thrashing.is_none() && compiles >= FORMULAS as u64 {
                thrashing = Some(Instant::now());
            }
            if let Some(since) = thrashing {
                for (drawn, status) in drawn.iter_mut().zip(&statuses) {
                    *drawn |= matches!(status, MathStatus::Ready(_));
                }
                if since.elapsed() > Duration::from_secs(2) {
                    break;
                }
            }
            assert!(
                created.elapsed() < Duration::from_secs(60),
                "{compiles} compiles"
            );
            thread::sleep(Duration::from_millis(2));
        }
        // Every formula is compiled once, then the ones pushed out again by turns: at most
        // `burst` at once and one per interval since the engine was made.
        // Compiles counted before the time they are checked against.
        let again = engine.compiles() - FORMULAS as u64;
        let elapsed = created.elapsed();
        let allowed = u64::from(limits.burst)
            + u64::try_from(elapsed.as_nanos().div_ceil(limits.interval.as_nanos())).unwrap();
        println!(
            "{again} compiles again in {:.2} s, at most {allowed} allowed",
            elapsed.as_secs_f64()
        );
        assert!(again <= allowed, "{again} > {allowed}");
        // And every formula pushed out had its turn on screen.
        assert!(again >= (FORMULAS - 5) as u64, "{again}");
        assert!(drawn.iter().all(|drawn| *drawn), "{drawn:?}");
    }

    #[test]
    fn the_limiter_lets_a_burst_through_then_one_per_interval() {
        let start = Instant::now();
        let interval = Duration::from_millis(100);
        let mut limiter = Limiter::new(3, interval, start);
        for _ in 0..3 {
            assert!(limiter.next() <= start);
            limiter.take(start);
        }
        assert_eq!(limiter.next(), start + interval);
        // Time without events builds up to a burst again, no more.
        let later = start + interval * 10;
        for _ in 0..3 {
            assert!(limiter.next() <= later);
            limiter.take(later);
        }
        assert_eq!(limiter.next(), later + interval);
    }

    #[test]
    fn records_forget_the_oldest_formulas_first() {
        let summary = MathSummary::of(&MathEngine::new().layout("x").unwrap());
        let at = |glyphs: usize| Some((Instant::now(), MathSummary { glyphs, ..summary }));
        let glyphs = |records: &Records, src| records.get(src)?.evicted.map(|(_, s)| s.glyphs);
        let mut records = Records::new(3);
        for (i, src) in (1..).zip(["a", "b", "c", "d", "e"]) {
            records.entry(src).evicted = at(i);
        }
        assert_eq!(records.len(), 3);
        assert!(records.get("a").is_none() && records.get("b").is_none());
        assert_eq!(glyphs(&records, "e"), Some(5));
        // Using a remembered formula doesn't add a record.
        records.entry("c").evicted = at(13);
        assert_eq!(records.len(), 3);
        assert_eq!(glyphs(&records, "c"), Some(13));
        records.entry("f");
        assert!(records.get("c").is_none(), "the oldest goes first");
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
        // Plus a long source: 400,000 bytes, far past the longest a board
        // holds (5,000 characters).
        let bytes = approx_bytes(&layout) + 400_000;
        println!("biggest allowed layout: about {bytes} bytes");
        assert!(bytes * 16 <= MAX_CACHED_LAYOUT_BYTES, "{bytes} bytes");
    }
}
