//! Compiles formulas off the calling thread with a time limit, and caches
//! every outcome, errors included.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use typst_layout::PagedDocument;

use crate::world::SandboxWorld;
use crate::{MathError, MathLayout, layout};

/// How long [`MathEngine::layout`] waits for one compile by default.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

/// Worker stack size. Typst recurses through nested content and math; a big
/// stack keeps deep but legitimate formulas from overflowing.
const WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Most compile threads one engine keeps alive. Typst can't be cancelled, so
/// a timed-out compile keeps running detached until it finishes on its own;
/// past this many, new compiles are refused instead of piling up threads.
pub const MAX_WORKERS: usize = 4;

/// comemo cache entries unused for this many compiles are evicted, so memory
/// doesn't grow with every formula ever typed.
const COMEMO_MAX_AGE: usize = 10;

type Outcome = Result<Arc<MathLayout>, MathError>;

/// Compiles Typst math to [`MathLayout`]s, sandboxed, time-limited and cached
/// by source text.
///
/// Every outcome is cached, errors and timeouts included, so a hostile
/// formula costs one compile, not one per frame. The only result that isn't
/// cached is the refusal returned while [`MAX_WORKERS`] timed-out compiles are
/// still running: the same source is tried again once a worker frees up.
#[derive(Debug)]
pub struct MathEngine {
    timeout: Duration,
    cache: Mutex<HashMap<String, Outcome>>,
    /// Compile threads alive, shared with the threads so they can count
    /// themselves out even after the engine is dropped.
    workers: Arc<AtomicUsize>,
}

impl Default for MathEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MathEngine {
    /// An engine with the [`DEFAULT_TIMEOUT`] and an empty cache.
    pub fn new() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            cache: Mutex::new(HashMap::new()),
            workers: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Sets how long one compile may take before [`MathError::Timeout`].
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Lays out `src` (Typst math, without `$`), compiling it on first use.
    ///
    /// Blocks for at most the timeout on a cache miss and returns at once on
    /// a hit; the same source always returns the same `Arc`.
    pub fn layout(&self, src: &str) -> Result<Arc<MathLayout>, MathError> {
        if let Some(hit) = self.cached(src) {
            return hit;
        }

        let started = Instant::now();
        let outcome = self.compile_on_worker(src)?;
        let elapsed = started.elapsed();
        tracing::debug!(
            elapsed_ms = elapsed.as_secs_f64() * 1000.0,
            src_len = src.len(),
            outcome = outcome_name(&outcome),
            "math compiled"
        );
        if matches!(outcome, Err(MathError::Timeout | MathError::Panicked)) {
            tracing::warn!(outcome = outcome_name(&outcome), "math compile failed");
        }

        self.lock_cache()
            .entry(src.to_owned())
            // Another thread may have compiled the same source meanwhile;
            // keep the first result so every caller gets the same `Arc`.
            .or_insert(outcome)
            .clone()
    }

    fn cached(&self, src: &str) -> Option<Outcome> {
        self.lock_cache().get(src).cloned()
    }

    fn lock_cache(&self) -> std::sync::MutexGuard<'_, HashMap<String, Outcome>> {
        // A panic can't leave the map half-written, so a poisoned lock is safe.
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs one compile on a fresh thread and waits up to the timeout.
    /// `Err` means the compile never started and must not be cached.
    fn compile_on_worker(&self, src: &str) -> Result<Outcome, MathError> {
        let Some(slot) = WorkerSlot::claim(&self.workers) else {
            tracing::warn!(
                workers = MAX_WORKERS,
                "math compile refused: too many timed-out compiles still running"
            );
            return Err(MathError::Timeout);
        };
        let (tx, rx) = mpsc::sync_channel(1);
        let owned = src.to_owned();
        let spawned = thread::Builder::new()
            .name("lawh-math".to_owned())
            .stack_size(WORKER_STACK_BYTES)
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

        Ok(match rx.recv_timeout(self.timeout) {
            Ok(outcome) => outcome,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(MathError::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(MathError::Panicked),
        })
    }
}

/// One counted compile thread; dropping it counts the thread out.
struct WorkerSlot(Arc<AtomicUsize>);

impl WorkerSlot {
    fn claim(workers: &Arc<AtomicUsize>) -> Option<Self> {
        let alive_before = workers.fetch_add(1, Ordering::AcqRel);
        // Over the cap, the slot is dropped here and counts itself back out.
        let slot = Self(Arc::clone(workers));
        (alive_before < MAX_WORKERS).then_some(slot)
    }
}

impl Drop for WorkerSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Compiles and converts one formula. Runs on a worker thread.
fn compile(src: &str) -> Outcome {
    let world = SandboxWorld::new(src);
    let compiled = typst::compile::<PagedDocument>(&world).output;
    typst::comemo::evict(COMEMO_MAX_AGE);
    let document = compiled.map_err(|diagnostics| {
        let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
        MathError::Compile(messages.join("; "))
    })?;
    let page = document
        .pages()
        .first()
        .ok_or_else(|| MathError::Compile("the formula produced no page".to_owned()))?;
    layout::convert(&page.frame).map(Arc::new)
}

fn outcome_name(outcome: &Outcome) -> &'static str {
    match outcome {
        Ok(_) => "ok",
        Err(MathError::Compile(_)) => "compile-error",
        Err(MathError::Timeout) => "timeout",
        Err(MathError::Panicked) => "panicked",
    }
}
