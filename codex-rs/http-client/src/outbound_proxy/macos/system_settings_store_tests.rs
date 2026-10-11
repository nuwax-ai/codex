//! Unit coverage for the dedicated-thread macOS system settings loader.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;

use pretty_assertions::assert_eq;
use system_configuration::core_foundation::base::TCFType;
use system_configuration::core_foundation::dictionary::CFDictionary;
use system_configuration::core_foundation::string::CFString;

use super::LoaderTunings;
use super::ProxiesDictionary;
use super::SystemSettingsLoader;
use super::system_settings_snapshot;

fn fixture_dictionary(token: &str) -> ProxiesDictionary {
    CFDictionary::from_CFType_pairs(&[(
        CFString::new("fixture-token"),
        CFString::new(token).into_CFType(),
    )])
}

fn token_of(snapshot: &super::SettingsSnapshot) -> String {
    let value = snapshot
        .proxies()
        .find(CFString::new("fixture-token"))
        .expect("fixture key should exist");
    value
        .downcast::<CFString>()
        .expect("fixture value should be a string")
        .to_string()
}

/// Tight tunings so lifecycle transitions need no multi-second waits.
fn test_tunings() -> LoaderTunings {
    LoaderTunings {
        ttl: Duration::from_millis(200),
        failure_ttl: Duration::from_millis(100),
        wait_budget: Duration::from_secs(5),
        stale_grace: Duration::from_secs(30),
    }
}

fn counting_reader(
    reads: Arc<AtomicUsize>,
    delay: Duration,
) -> (
    impl FnMut() -> Option<ProxiesDictionary> + Send + 'static,
    Receiver<()>,
) {
    let (seen, seen_rx) = channel();
    let reader = move || {
        reads.fetch_add(1, AtomicOrdering::SeqCst);
        let _ = seen.send(());
        thread::sleep(delay);
        Some(fixture_dictionary(&format!(
            "read-{}",
            reads.load(AtomicOrdering::SeqCst)
        )))
    };
    (reader, seen_rx)
}

#[test]
fn production_snapshot_serves_the_real_system_settings() {
    let snapshot = system_settings_snapshot().expect("real system settings should load");
    assert!(
        !snapshot.proxies().is_empty(),
        "the dynamic-store proxies dictionary always carries per-scheme entries"
    );
}

#[test]
fn concurrent_requests_share_one_system_read() {
    let reads = Arc::new(AtomicUsize::new(0));
    let (reader, seen) = counting_reader(Arc::clone(&reads), Duration::from_millis(100));
    let loader = Arc::new(SystemSettingsLoader::with_reader_and_tunings(
        reader,
        test_tunings(),
    ));
    let mut threads = Vec::new();
    for _ in 0..4 {
        let loader = Arc::clone(&loader);
        threads.push(thread::spawn(move || {
            loader
                .snapshot()
                .expect("concurrent callers should share the snapshot")
        }));
    }
    let snapshots: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().expect("caller should not panic"))
        .collect();
    seen.recv().expect("reader should have run");
    for snapshot in &snapshots[1..] {
        assert!(
            Arc::ptr_eq(&snapshots[0], snapshot),
            "concurrent callers must share one read's result"
        );
    }
    assert_eq!(loader.system_reads(), 1);
}

#[test]
fn idle_loader_waits_instead_of_spinning_an_empty_run_loop() {
    let loader = SystemSettingsLoader::with_reader(|| Some(fixture_dictionary("idle")));
    let _snapshot = loader.snapshot().expect("initial read should succeed");
    let started = std::time::Instant::now();
    let before = loader.inner.loop_iterations.load(AtomicOrdering::SeqCst);
    thread::sleep(Duration::from_millis(200));
    let after = loader.inner.loop_iterations.load(AtomicOrdering::SeqCst);
    let iterations = after - before;
    let maximum_iterations = started.elapsed().as_nanos() / super::RUNLOOP_SLICE.as_nanos() + 3;
    assert!(
        iterations as u128 <= maximum_iterations,
        "an idle loader must block between polls, observed {iterations} iterations"
    );
    assert_eq!(loader.system_reads(), 1);
}

#[test]
fn fresh_snapshot_is_reused_and_then_reloaded() {
    let reads = Arc::new(AtomicUsize::new(0));
    let (reader, _seen) = counting_reader(Arc::clone(&reads), Duration::from_millis(0));
    let loader = SystemSettingsLoader::with_reader_and_tunings(reader, test_tunings());

    let first = loader.snapshot().expect("first read should succeed");
    let second = loader.snapshot().expect("fresh read should succeed");
    assert!(
        Arc::ptr_eq(&first, &second),
        "within the TTL the snapshot is shared"
    );
    assert_eq!(loader.system_reads(), 1);

    thread::sleep(test_tunings().ttl + Duration::from_millis(50));
    let third = loader.snapshot().expect("post-TTL read should succeed");
    assert!(
        !Arc::ptr_eq(&first, &third),
        "expiry must trigger a new system read"
    );
    assert_eq!(loader.system_reads(), 2);
}

#[test]
fn failed_read_returns_none_then_retries_after_the_failure_ttl() {
    let reads = Arc::new(AtomicUsize::new(0));
    let fail = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (reads2, fail2) = (Arc::clone(&reads), Arc::clone(&fail));
    let reader = move || {
        reads2.fetch_add(1, AtomicOrdering::SeqCst);
        if fail2.load(AtomicOrdering::SeqCst) {
            None
        } else {
            Some(fixture_dictionary("recovered"))
        }
    };
    let loader = SystemSettingsLoader::with_reader_and_tunings(reader, test_tunings());

    assert!(
        loader.snapshot().is_none(),
        "a failed system read reports unavailable"
    );
    assert!(
        loader.snapshot().is_none(),
        "within the failure TTL no retry is issued"
    );
    assert_eq!(loader.system_reads(), 1);

    fail.store(false, AtomicOrdering::SeqCst);
    thread::sleep(test_tunings().failure_ttl + Duration::from_millis(50));
    let snapshot = loader
        .snapshot()
        .expect("recovery after the failure TTL should serve settings");
    assert_eq!(token_of(&snapshot), "recovered");
    assert_eq!(loader.system_reads(), 2);
}

/// A panicking reader must not strand waiters or kill the loader thread.
#[test]
fn panicking_reader_publishes_failure_and_the_thread_survives() {
    let reads = Arc::new(AtomicUsize::new(0));
    let boom = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (reads2, boom2) = (Arc::clone(&reads), Arc::clone(&boom));
    let reader = move || {
        reads2.fetch_add(1, AtomicOrdering::SeqCst);
        if boom2.load(AtomicOrdering::SeqCst) {
            panic!("injected system read failure");
        }
        Some(fixture_dictionary("after-panic"))
    };
    let loader = SystemSettingsLoader::with_reader_and_tunings(reader, test_tunings());

    assert!(
        loader.snapshot().is_none(),
        "the panicked read reports unavailable"
    );
    boom.store(false, AtomicOrdering::SeqCst);
    thread::sleep(test_tunings().failure_ttl + Duration::from_millis(50));
    let snapshot = loader
        .snapshot()
        .expect("the loader thread should still serve reads");
    assert_eq!(token_of(&snapshot), "after-panic");
    assert_eq!(loader.system_reads(), 2);
}

/// A wedged read is bounded by the waiter budget; a bounded-stale snapshot
/// still answers instead of hanging or returning nothing.
#[test]
fn wedged_read_falls_back_to_a_stale_snapshot() {
    let reads = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (reads2, gate2) = (Arc::clone(&reads), Arc::clone(&gate));
    let reader = move || {
        reads2.fetch_add(1, AtomicOrdering::SeqCst);
        if gate2.load(AtomicOrdering::SeqCst) {
            // Model a store read that never completes.
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }
        Some(fixture_dictionary("fast"))
    };
    let loader = SystemSettingsLoader::with_reader_and_tunings(
        reader,
        LoaderTunings {
            ttl: Duration::from_millis(100),
            failure_ttl: Duration::from_millis(100),
            wait_budget: Duration::from_millis(300),
            stale_grace: Duration::from_secs(60),
        },
    );

    let fresh = loader.snapshot().expect("first read is fast");
    assert_eq!(token_of(&fresh), "fast");

    gate.store(true, AtomicOrdering::SeqCst);
    thread::sleep(Duration::from_millis(150));
    let started = std::time::Instant::now();
    let stale = loader
        .snapshot()
        .expect("a bounded-stale snapshot should answer a wedged read");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the waiter budget must bound the wait"
    );
    assert_eq!(token_of(&stale), "fast");
}

/// Losing the loader thread must rebuild with the same reader and keep
/// serving; the rebuild counter bounds the churn.
#[test]
fn dead_thread_is_rebuilt_with_the_same_reader() {
    let reads = Arc::new(AtomicUsize::new(0));
    let (reader, _seen) = counting_reader(Arc::clone(&reads), Duration::from_millis(0));
    let loader = SystemSettingsLoader::with_reader_and_tunings(reader, test_tunings());

    let _first = loader.snapshot().expect("initial read should succeed");
    assert_eq!(loader.system_reads(), 1);

    loader.simulate_thread_death();
    thread::sleep(test_tunings().ttl + Duration::from_millis(50));
    let rebuilt = loader
        .snapshot()
        .expect("the rebuilt thread should serve reads");
    assert_eq!(token_of(&rebuilt), "read-2");
    assert_eq!(loader.system_reads(), 2);
}

/// An OS-level spawn failure must publish the short failure TTL (reads
/// report unavailable) and recover by spawning again once the condition
/// clears; the constructor's own spawn failure is warned, not fatal.
#[test]
fn spawn_failure_publishes_unavailable_and_recovers() {
    let loader = SystemSettingsLoader::with_reader_and_tunings(
        || Some(fixture_dictionary("spawn-recovered")),
        test_tunings(),
    );
    loader.set_spawn_fails(true);
    loader.simulate_thread_death();
    assert!(
        loader.snapshot().is_none(),
        "a failed spawn reports the read unavailable"
    );
    assert!(
        loader.snapshot().is_none(),
        "within the failure TTL no respawn storm is issued"
    );

    loader.set_spawn_fails(false);
    thread::sleep(test_tunings().failure_ttl + Duration::from_millis(50));
    let snapshot = loader
        .snapshot()
        .expect("clearing the spawn failure should rebuild and serve");
    assert_eq!(token_of(&snapshot), "spawn-recovered");
}

/// Repeated thread death past the rebuild budget keeps the loader down:
/// reads stay unavailable instead of spawning unbounded threads.
#[test]
fn rebuild_budget_exhaustion_keeps_the_loader_down() {
    let loader = SystemSettingsLoader::with_reader_and_tunings(
        || Some(fixture_dictionary("budget")),
        test_tunings(),
    );
    let _ = loader.snapshot().expect("initial read should succeed");

    for _ in 0..super::MAX_THREAD_REBUILDS {
        loader.simulate_thread_death();
        thread::sleep(test_tunings().ttl + Duration::from_millis(50));
        let _ = loader
            .snapshot()
            .expect("within the rebuild budget a new thread serves");
    }
    // The budget is spent: further deaths must not respawn.
    loader.simulate_thread_death();
    thread::sleep(test_tunings().ttl + Duration::from_millis(50));
    assert!(
        loader.snapshot().is_none(),
        "past the rebuild budget the loader stays down"
    );
    // And it stays down after the failure TTL too — no unbounded churn.
    thread::sleep(test_tunings().failure_ttl + Duration::from_millis(50));
    assert!(loader.snapshot().is_none());
}

/// A wedged read whose previous snapshot is older than the stale grace must
/// answer none instead of serving arbitrarily old settings.
#[test]
fn wedged_read_with_expired_stale_grace_answers_none() {
    let reads = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (reads2, gate2) = (Arc::clone(&reads), Arc::clone(&gate));
    let reader = move || {
        reads2.fetch_add(1, AtomicOrdering::SeqCst);
        if gate2.load(AtomicOrdering::SeqCst) {
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }
        Some(fixture_dictionary("short-lived"))
    };
    let loader = SystemSettingsLoader::with_reader_and_tunings(
        reader,
        LoaderTunings {
            ttl: Duration::from_millis(100),
            failure_ttl: Duration::from_millis(100),
            wait_budget: Duration::from_millis(200),
            stale_grace: Duration::from_millis(250),
        },
    );

    let fresh = loader.snapshot().expect("first read is fast");
    assert_eq!(token_of(&fresh), "short-lived");

    gate.store(true, AtomicOrdering::SeqCst);
    // Outlive the stale grace so the wedged read has no admissible fallback.
    thread::sleep(Duration::from_millis(600));
    let started = std::time::Instant::now();
    assert!(
        loader.snapshot().is_none(),
        "an expired stale snapshot must not be served"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the waiter budget still bounds the wait"
    );
}
