//! macOS system proxy settings served from a dedicated CFRunLoop thread.
//!
//! Creating an `SCDynamicStore` and reading the proxies dictionary blocks on
//! a synchronous IPC to `configd`; on threads without a CoreFoundation run
//! loop that read has been observed to stall for many seconds on some hosts
//! while the same read takes milliseconds on the main thread. Every resolver
//! call used to pay that cost inline on whichever thread resolved the route.
//!
//! This loader owns one dedicated thread that services a real run loop and
//! performs the store reads. Callers request a snapshot and wait on a
//! condvar — never performing CoreFoundation IPC themselves. Published
//! snapshots are owned immutable dictionary copies, so publishing across the
//! thread boundary is sound (see the `Send` impl below).
//!
//! Lifecycle contract:
//!
//! - concurrent requests coalesce into a single system read (single flight)
//! - a successful snapshot stays valid for [`SNAPSHOT_TTL`]; a failed read
//!   is retried after [`FAILURE_TTL`] and never publishes stale settings
//! - waiters bound their wait by [`WAIT_BUDGET`] (sized above the observed
//!   worst-case stall so a slow first read still completes); on timeout they
//!   fall back to a bounded-staleness snapshot when one exists, otherwise
//!   report the read as unavailable
//! - if the loader thread dies, it is rebuilt with the same reader up to
//!   [`MAX_THREAD_REBUILDS`] times per loader; beyond that reads report
//!   unavailable rather than falling back to an inline store read that would
//!   reintroduce the stall
//!
//! This bounds staleness and moves the blocking CoreFoundation work off
//! request threads; it does not make the underlying system call itself
//! faster. Whether the run loop (rather than some other thread property) is
//! what makes the main-thread read fast remains unproven and is tracked as
//! an open experiment, not a claim of this module.

use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::SyncSender;
use std::sync::mpsc::TrySendError;
use std::sync::mpsc::sync_channel;
use std::time::Duration;
use std::time::Instant;

use system_configuration::core_foundation::base::CFType;
use system_configuration::core_foundation::dictionary::CFDictionary;
use system_configuration::core_foundation::runloop::CFRunLoop;
use system_configuration::core_foundation::runloop::kCFRunLoopDefaultMode;
use system_configuration::core_foundation::string::CFString;

/// How long a published snapshot stays fresh.
const SNAPSHOT_TTL: Duration = Duration::from_secs(60);
/// How long a failed read suppresses retries.
const FAILURE_TTL: Duration = Duration::from_secs(5);
/// Bound on how long a caller waits for an in-flight or new read.
///
/// Sized above the observed worst-case stall (12.28s in the R7 evidence) so a
/// slow-but-progressing read still completes instead of timing out.
const WAIT_BUDGET: Duration = Duration::from_secs(20);
/// A timed-out caller may use an older snapshot only within this staleness.
const STALE_FALLBACK_GRACE: Duration = Duration::from_secs(600);
/// How many times a dead loader thread is replaced per loader instance.
const MAX_THREAD_REBUILDS: usize = 8;
/// Run-loop slice serviced between channel polls; bounds request wake-up.
const RUNLOOP_SLICE: Duration = Duration::from_millis(50);

type ProxiesDictionary = CFDictionary<CFString, CFType>;
type SharedReader = Arc<Mutex<Box<dyn FnMut() -> Option<ProxiesDictionary> + Send>>>;

/// Timing knobs; production uses the documented defaults, tests tighten them.
#[derive(Clone, Copy)]
pub(super) struct LoaderTunings {
    ttl: Duration,
    failure_ttl: Duration,
    wait_budget: Duration,
    stale_grace: Duration,
}

impl Default for LoaderTunings {
    fn default() -> Self {
        Self {
            ttl: SNAPSHOT_TTL,
            failure_ttl: FAILURE_TTL,
            wait_budget: WAIT_BUDGET,
            stale_grace: STALE_FALLBACK_GRACE,
        }
    }
}

/// The shared production loader for macOS system proxy settings.
pub(super) fn system_settings_snapshot() -> Option<Arc<SettingsSnapshot>> {
    static PRODUCTION: OnceLock<SystemSettingsLoader> = OnceLock::new();
    PRODUCTION.get_or_init(SystemSettingsLoader::production).snapshot()
}

/// An owned, immutable copy of the dynamic-store proxies dictionary.
pub(super) struct SettingsSnapshot {
    proxies: ProxiesDictionary,
    loaded_at: Instant,
}

impl SettingsSnapshot {
    /// The proxies dictionary as `CFNetworkCopyProxiesForURL` expects it.
    pub(super) fn proxies(&self) -> &ProxiesDictionary {
        &self.proxies
    }
}

// SAFETY: the dictionary is an owned copy produced under the create rule on
// the loader thread and is never mutated after publication. CoreFoundation
// immutable collections are safe to read concurrently from any thread
// ("CFDictionary is immutable, and therefore thread-safe"), so sharing the
// owned copy across the thread boundary cannot race a writer.
unsafe impl Send for SettingsSnapshot {}
unsafe impl Sync for SettingsSnapshot {}

enum LoadState {
    /// A snapshot exists and is fresh until the deadline.
    Fresh(Arc<SettingsSnapshot>),
    /// A failed read suppresses retries until the deadline.
    Failed(Instant),
    /// A read is in flight on the loader thread, retaining the previously
    /// published snapshot (if any) for bounded-stale fallback if the read
    /// outlives the waiter budget.
    Loading(Option<Arc<SettingsSnapshot>>),
    /// Nothing loaded yet.
    Empty,
}

struct LoaderInner {
    /// The live loader thread's request slot with its generation, so a dead
    /// thread's exit guard can only clear the slot it owns.
    request: Mutex<Option<(SyncSender<()>, usize)>>,
    state: Mutex<LoadState>,
    published: Condvar,
    thread_rebuilds: AtomicUsize,
    thread_generation: AtomicUsize,
    system_reads: AtomicUsize,
    reader: SharedReader,
    tunings: LoaderTunings,
}

/// Serves macOS system proxy settings from a dedicated run-loop thread.
///
/// Production shares one loader (see [`system_settings_snapshot`]); tests
/// construct private loaders with injected readers so fixture behavior never
/// touches process-global state.
pub(super) struct SystemSettingsLoader {
    inner: Arc<LoaderInner>,
}

impl SystemSettingsLoader {
    /// Builds a production loader that reads the real dynamic store.
    pub(super) fn production() -> Self {
        Self::with_reader(production_reader())
    }

    /// Builds a loader whose system reads are produced by `reader`.
    ///
    /// `reader` runs on the loader thread and may block exactly like the real
    /// store read; returning `None` models a failed system read.
    pub(super) fn with_reader(
        reader: impl FnMut() -> Option<ProxiesDictionary> + Send + 'static,
    ) -> Self {
        Self::with_reader_and_tunings(reader, LoaderTunings::default())
    }

    /// Builds a loader with explicit timing tunings (tests).
    pub(super) fn with_reader_and_tunings(
        reader: impl FnMut() -> Option<ProxiesDictionary> + Send + 'static,
        tunings: LoaderTunings,
    ) -> Self {
        let inner = Arc::new(LoaderInner {
            tunings,
            request: Mutex::new(None),
            state: Mutex::new(LoadState::Empty),
            published: Condvar::new(),
            thread_rebuilds: AtomicUsize::new(0),
            thread_generation: AtomicUsize::new(0),
            system_reads: AtomicUsize::new(0),
            reader: Arc::new(Mutex::new(Box::new(reader))),
        });
        let loader = Self {
            inner: Arc::clone(&inner),
        };
        loader.spawn_thread();
        loader
    }

    /// Returns a fresh snapshot, driving a system read when needed.
    pub(super) fn snapshot(&self) -> Option<Arc<SettingsSnapshot>> {
        let deadline = Instant::now() + self.inner.tunings.wait_budget;
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            let now = Instant::now();
            match &*state {
                LoadState::Fresh(snapshot) if snapshot.loaded_at + self.inner.tunings.ttl > now => {
                    return Some(Arc::clone(snapshot));
                }
                LoadState::Failed(retry_at) if *retry_at > now => return None,
                LoadState::Loading(_) => {}
                _ => {
                    self.request_read(&mut state);
                    continue;
                }
            }
            let (guard, wait_result) = self
                .inner
                .published
                .wait_timeout(state, deadline.saturating_duration_since(now))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = guard;
            if wait_result.timed_out() {
                // The read is slower than the waiter budget. A bounded-stale
                // snapshot still answers; otherwise the read is unavailable.
                return match &*state {
                    LoadState::Fresh(snapshot)
                        if snapshot.loaded_at + self.inner.tunings.stale_grace
                            > Instant::now() =>
                    {
                        Some(Arc::clone(snapshot))
                    }
                    LoadState::Loading(Some(previous))
                        if previous.loaded_at + self.inner.tunings.stale_grace
                            > Instant::now() =>
                    {
                        Some(Arc::clone(previous))
                    }
                    _ => None,
                };
            }
        }
    }

    /// Marks the state as loading and pokes the loader thread.
    fn request_read(&self, state: &mut LoadState) {
        let mut request = self
            .inner
            .request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = match std::mem::replace(&mut *state, LoadState::Empty) {
            LoadState::Fresh(previous) => Some(previous),
            LoadState::Loading(previous) => previous,
            _ => None,
        };
        *state = LoadState::Loading(previous);
        match request.as_ref() {
            Some((sender, _)) => match sender.try_send(()) {
                Ok(()) | Err(TrySendError::Full(())) => {}
                Err(TrySendError::Disconnected(())) => self.rebuild_thread(&mut request, state),
            },
            None => self.rebuild_thread(&mut request, state),
        }
    }

    fn rebuild_thread(
        &self,
        request: &mut MutexGuard<'_, Option<(SyncSender<()>, usize)>>,
        state: &mut LoadState,
    ) {
        let rebuilds = self.inner.thread_rebuilds.fetch_add(1, Ordering::SeqCst);
        if rebuilds >= MAX_THREAD_REBUILDS {
            // Repeated thread death means the loader stays down; report the
            // read as unavailable instead of spawning unbounded threads or
            // falling back to an inline store read.
            tracing::warn!(
                rebuilds,
                "system settings loader thread rebuild budget exhausted"
            );
            *state = LoadState::Failed(Instant::now() + self.inner.tunings.failure_ttl);
            return;
        }
        tracing::warn!(rebuilds, "rebuilding the system settings loader thread");
        self.spawn_thread_with_sender(request);
        let previous = match std::mem::replace(&mut *state, LoadState::Empty) {
            LoadState::Fresh(previous) => Some(previous),
            LoadState::Loading(previous) => previous,
            _ => None,
        };
        *state = LoadState::Loading(previous);
        if let Some((sender, _)) = request.as_ref() {
            let _ = sender.try_send(());
        }
    }

    fn spawn_thread(&self) {
        let mut request = self
            .inner
            .request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.spawn_thread_with_sender(&mut request);
    }

    fn spawn_thread_with_sender(
        &self,
        request: &mut MutexGuard<'_, Option<(SyncSender<()>, usize)>>,
    ) {
        let (sender, receiver) = sync_channel::<()>(/*bound*/ 1);
        let generation = self.inner.thread_generation.fetch_add(1, Ordering::SeqCst);
        **request = Some((sender, generation));
        let inner = Arc::clone(&self.inner);
        std::thread::Builder::new()
            .name("codex-system-proxy-settings".to_string())
            .spawn(move || loader_thread(inner, receiver, generation))
            .expect("system settings loader thread should spawn");
    }

    /// How many system reads this loader has performed (tests).
    pub(super) fn system_reads(&self) -> usize {
        self.inner.system_reads.load(Ordering::SeqCst)
    }

    /// Simulates loader-thread death by clearing the request slot (tests);
    /// the next snapshot call must rebuild a thread with the same reader.
    #[cfg(test)]
    pub(super) fn simulate_thread_death(&self) {
        let mut request = self
            .inner
            .request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *request = None;
    }
}

fn production_reader() -> impl FnMut() -> Option<ProxiesDictionary> + Send + 'static {
    move || {
        let store = system_configuration::dynamic_store::SCDynamicStoreBuilder::new(
            "CodexProxySettings",
        )
        .build()?;
        store.get_proxies()
    }
}

fn loader_thread(inner: Arc<LoaderInner>, receiver: Receiver<()>, generation: usize) {
    // If the thread dies (reader panic), clear the request slot — but only
    // while it still belongs to this generation — so the next caller
    // rebuilds instead of sending into a void.
    let _alive = AliveGuard {
        inner: Arc::clone(&inner),
        generation,
    };
    loop {
        // Service this thread's run loop in short slices so CoreFoundation
        // machinery on it sees a live loop, then poll for work; requests
        // wake within one slice.
        CFRunLoop::run_in_mode(
            unsafe { kCFRunLoopDefaultMode },
            RUNLOOP_SLICE,
            /*return_after_source_handled*/ true,
        );
        match receiver.recv_timeout(Duration::from_millis(0)) {
            Ok(()) => perform_read(&inner),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn perform_read(inner: &LoaderInner) {
    inner.system_reads.fetch_add(1, Ordering::SeqCst);
    // A panicking reader must not strand waiters until their budget expires
    // or kill the loader thread; treat it as a failed system read.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut reader = inner
            .reader
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        reader()
    }))
    .unwrap_or_default();
    let mut state = inner
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *state = match result {
        Some(proxies) => LoadState::Fresh(Arc::new(SettingsSnapshot {
            proxies,
            loaded_at: Instant::now(),
        })),
        None => LoadState::Failed(Instant::now() + inner.tunings.failure_ttl),
    };
    drop(state);
    inner.published.notify_all();
}

/// Clears the request channel slot when the thread exits so senders observe
/// the disconnect and rebuild.
struct AliveGuard {
    inner: Arc<LoaderInner>,
    generation: usize,
}

impl Drop for AliveGuard {
    fn drop(&mut self) {
        let mut request = self
            .inner
            .request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if request
            .as_ref()
            .is_some_and(|(_, generation)| *generation == self.generation)
        {
            *request = None;
        }
    }
}

#[cfg(test)]
#[path = "system_settings_store_tests.rs"]
mod tests;
