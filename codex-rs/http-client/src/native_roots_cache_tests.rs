//! Unit coverage for the native-roots cache lifecycle and source-key identity.

use std::ffi::OsString;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use pretty_assertions::assert_eq;
use rustls_pki_types::CertificateDer;

use super::NativeRootsCache;
use super::NativeRootsSourceKey;
use super::NATIVE_ROOTS_CACHE_TTL;
use super::NATIVE_ROOTS_PARTIAL_FAILURE_TTL;

/// A controllable clock so cache validity can be exercised without real waits.
#[derive(Clone)]
struct TestClock {
    state: Arc<Mutex<(std::time::Instant, Duration)>>,
}

impl TestClock {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new((std::time::Instant::now(), Duration::ZERO))),
        }
    }

    fn advance(&self, duration: Duration) {
        self.state.lock().unwrap().1 += duration;
    }

    fn now(&self) -> std::time::Instant {
        let (start, offset) = *self.state.lock().unwrap();
        start + offset
    }
}

fn platform_key() -> NativeRootsSourceKey {
    NativeRootsSourceKey::from_env_values(/*cert_file*/ None, /*cert_dir*/ None)
}

fn file_key(value: &str) -> NativeRootsSourceKey {
    NativeRootsSourceKey::from_env_values(Some(OsString::from(value)), /*cert_dir*/ None)
}

fn dir_key(value: &str) -> NativeRootsSourceKey {
    NativeRootsSourceKey::from_env_values(/*cert_file*/ None, Some(OsString::from(value)))
}

fn fixture_native_root() -> CertificateDer<'static> {
    let cert = rcgen::generate_simple_self_signed(vec!["cache-fixture-root.test".to_string()])
        .expect("fixture root")
        .cert;
    cert.der().clone()
}

fn fixture_load(certs: Vec<CertificateDer<'static>>) -> rustls_native_certs::CertificateResult {
    // CertificateResult is non-exhaustive, so build it from Default.
    let mut result = rustls_native_certs::CertificateResult::default();
    result.certs = certs;
    result
}

/// A loader that counts how many times the platform was actually read.
struct CountingLoads {
    loads: Arc<Mutex<usize>>,
}

impl CountingLoads {
    fn new() -> Self {
        Self {
            loads: Arc::new(Mutex::new(0)),
        }
    }

    fn count(&self) -> usize {
        *self.loads.lock().unwrap()
    }

    fn loader(
        &self,
        make_result: impl Fn() -> rustls_native_certs::CertificateResult + 'static,
    ) -> impl FnOnce() -> rustls_native_certs::CertificateResult + '_ {
        let loads = Arc::clone(&self.loads);
        move || {
            *loads.lock().unwrap() += 1;
            make_result()
        }
    }
}

#[test]
fn reuses_within_ttl_and_reloads_after_expiry() {
    let cache = NativeRootsCache::new();
    let clock = TestClock::new();
    let loads = CountingLoads::new();

    let first = cache.load(platform_key(), loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    let second = cache.load(platform_key(), loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert!(
        Arc::ptr_eq(&first, &second),
        "within TTL the store is shared"
    );
    assert_eq!(loads.count(), 1);

    clock.advance(NATIVE_ROOTS_CACHE_TTL + Duration::from_secs(/*secs*/ 1));
    let third = cache.load(platform_key(), loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert!(
        !Arc::ptr_eq(&first, &third),
        "expiry must reload the platform"
    );
    assert_eq!(loads.count(), 2);
}

/// Missing and empty environment values load differently in the locked
/// loader (`SSL_CERT_FILE=""` still redirects away from the platform store),
/// so the key must keep them distinct.
#[test]
fn reloads_when_the_source_key_changes() {
    let cache = NativeRootsCache::new();
    let clock = TestClock::new();
    let loads = CountingLoads::new();
    let keys = [
        platform_key(),
        file_key("/tmp/roots-a.pem"),
        file_key("/tmp/roots-b.pem"),
        NativeRootsSourceKey::from_env_values(Some(OsString::new()), /*cert_dir*/ None),
        dir_key("/tmp/roots-dir-a"),
        dir_key("/tmp/roots-dir-b"),
        NativeRootsSourceKey::from_env_values(
            /*cert_file*/ None,
            Some(OsString::new()),
        ),
    ];
    for key in &keys {
        let _ = cache.load(key.clone(), loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    }
    assert_eq!(
        loads.count(),
        keys.len(),
        "every distinct source (including empty-vs-missing and FILE-vs-DIR) must reload"
    );
}

/// The key is a structured pair, so values containing the byte patterns that
/// a joined-string key would have used as separators cannot collide.
#[test]
fn structured_keys_do_not_collide_across_field_values() {
    let cache = NativeRootsCache::new();
    let clock = TestClock::new();
    let loads = CountingLoads::new();
    let joined = NativeRootsSourceKey::from_env_values(
        Some(OsString::from("a\u{1}b")),
        /*cert_dir*/ None,
    );
    let split = NativeRootsSourceKey::from_env_values(
        Some(OsString::from("a")),
        Some(OsString::from("b")),
    );
    let _ = cache.load(joined, loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    let _ = cache.load(split, loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert_eq!(loads.count(), 2, "distinct structured keys must not collide");
}

#[cfg(unix)]
#[test]
fn non_unicode_source_keys_keep_their_identity() {
    use std::os::unix::ffi::OsStringExt;
    let cache = NativeRootsCache::new();
    let clock = TestClock::new();
    let loads = CountingLoads::new();
    let first = NativeRootsSourceKey::from_env_values(
        Some(OsString::from_vec(b"/tmp/roots/\xff".to_vec())),
        /*cert_dir*/ None,
    );
    let second = NativeRootsSourceKey::from_env_values(
        Some(OsString::from_vec(b"/tmp/roots/\xfe".to_vec())),
        /*cert_dir*/ None,
    );
    let _ = cache.load(first.clone(), loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    let _ = cache.load(first, loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert_eq!(
        loads.count(),
        1,
        "the same non-UTF-8 bytes are the same source"
    );
    let _ = cache.load(second, loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert_eq!(loads.count(), 2, "different non-UTF-8 values are different sources");
}

#[cfg(windows)]
#[test]
fn non_unicode_source_keys_keep_their_identity() {
    use std::os::windows::ffi::OsStringExt;
    let cache = NativeRootsCache::new();
    let clock = TestClock::new();
    let loads = CountingLoads::new();
    let first = NativeRootsSourceKey::from_env_values(
        Some(OsString::from_wide(&[0x0052, 0x006F, 0xD800, 0x0074])),
        /*cert_dir*/ None,
    );
    let second = NativeRootsSourceKey::from_env_values(
        Some(OsString::from_wide(&[0x0052, 0x006F, 0xDFFF, 0x0054])),
        /*cert_dir*/ None,
    );
    let _ = cache.load(first.clone(), loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    let _ = cache.load(first, loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert_eq!(
        loads.count(),
        1,
        "the same non-UTF-16 bytes are the same source"
    );
    let _ = cache.load(second, loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert_eq!(loads.count(), 2, "different non-UTF-8 values are different sources");
}

#[test]
fn never_caches_an_empty_load() {
    let cache = NativeRootsCache::new();
    let clock = TestClock::new();
    let loads = CountingLoads::new();

    let first = cache.load(platform_key(), loads.loader(rustls_native_certs::CertificateResult::default), || clock.now());
    assert!(
        first.is_empty(),
        "a failed load must fail closed for this connector"
    );
    let second = cache.load(platform_key(), loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert_eq!(second.len(), 1, "the next connector must retry the load");
    assert_eq!(loads.count(), 2);
}

#[test]
fn uses_the_short_ttl_for_partial_failures() {
    let cache = NativeRootsCache::new();
    let clock = TestClock::new();
    let loads = CountingLoads::new();
    let make_partial = || {
        // CertificateResult is non-exhaustive, so build it from Default.
        let mut result = rustls_native_certs::CertificateResult::default();
        result.certs = vec![fixture_native_root()];
        result.errors = vec![rustls_native_certs::Error {
            context: "test partial failure",
            kind: rustls_native_certs::ErrorKind::Io {
                inner: std::io::Error::other("injected"),
                path: std::path::PathBuf::from("/tmp/missing-root"),
            },
        }];
        result
    };

    let first = cache.load(platform_key(), loads.loader(make_partial), || clock.now());
    let second = cache.load(platform_key(), loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert!(
        Arc::ptr_eq(&first, &second),
        "partial results are shared briefly"
    );
    assert_eq!(loads.count(), 1);
    clock.advance(NATIVE_ROOTS_PARTIAL_FAILURE_TTL + Duration::from_secs(/*secs*/ 1));
    let _ = cache.load(platform_key(), loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert_eq!(
        loads.count(),
        2,
        "partial failures must refresh on the short TTL"
    );
}

/// Certificates the root store refuses to parse are a partial answer even
/// when the loader itself reported no errors.
#[test]
fn uses_the_short_ttl_when_platform_certs_are_rejected() {
    let cache = NativeRootsCache::new();
    let clock = TestClock::new();
    let loads = CountingLoads::new();
    let make_rejected = || {
        // CertificateResult is non-exhaustive, so build it from Default.
        let mut result = rustls_native_certs::CertificateResult::default();
        result.certs = vec![
            fixture_native_root(),
            CertificateDer::from(vec![0x30, 0x03, 0x02, 0x01, 0xff]),
        ];
        result
    };

    let first = cache.load(platform_key(), loads.loader(make_rejected), || clock.now());
    let second = cache.load(platform_key(), loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert!(
        Arc::ptr_eq(&first, &second),
        "rejected-cert loads are shared briefly"
    );
    assert_eq!(loads.count(), 1);
    assert_eq!(
        first.len(),
        1,
        "only the parseable platform certificate is trusted"
    );
    clock.advance(NATIVE_ROOTS_PARTIAL_FAILURE_TTL + Duration::from_secs(/*secs*/ 1));
    let _ = cache.load(platform_key(), loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert_eq!(
        loads.count(),
        2,
        "loads with rejected certificates must refresh on the short TTL"
    );
}

#[test]
fn is_single_flight_across_threads() {
    let cache = Arc::new(NativeRootsCache::new());
    let loads = Arc::new(Mutex::new(0_usize));
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let cache = Arc::clone(&cache);
        let loads = Arc::clone(&loads);
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            cache.load(
                platform_key(),
                move || {
                    std::thread::sleep(Duration::from_millis(/*millis*/ 50));
                    *loads.lock().unwrap() += 1;
                    fixture_load(vec![fixture_native_root()])
                },
                std::time::Instant::now,
            )
        }));
    }
    let stores: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().expect("cache thread should not panic"))
        .collect();
    assert!(Arc::ptr_eq(&stores[0], &stores[1]));
    assert_eq!(
        *loads.lock().unwrap(),
        1,
        "concurrent callers must share one system load"
    );
}

/// Expiry is measured from load completion, not from when the load started:
/// a load that itself outlives the TTL must still produce a usable entry.
#[test]
fn computes_expiry_after_the_load_completes() {
    let cache = NativeRootsCache::new();
    let clock = TestClock::new();
    let loads = CountingLoads::new();
    let slow_clock = clock.clone();
    let loader = move || {
        // The platform load takes longer than the whole TTL.
        slow_clock.advance(NATIVE_ROOTS_CACHE_TTL + Duration::from_secs(/*secs*/ 1));
        fixture_load(vec![fixture_native_root()])
    };

    let first = cache.load(platform_key(), loader, || clock.now());
    let second = cache.load(platform_key(), loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert!(
        Arc::ptr_eq(&first, &second),
        "an entry completed after a slow load is still fresh at load-completion time"
    );
    assert_eq!(loads.count(), 0);

    clock.advance(Duration::from_secs(/*secs*/ 1));
    let _ = cache.load(platform_key(), loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert_eq!(
        loads.count(),
        0,
        "an entry whose load outlived the TTL is still fresh just after completion"
    );

    clock.advance(NATIVE_ROOTS_CACHE_TTL + Duration::from_secs(/*secs*/ 1));
    let _ = cache.load(platform_key(), loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert_eq!(
        loads.count(),
        1,
        "expiry runs from load completion, not from load start"
    );
}

/// A loader that panics poisons the cache lock; the next caller must clear
/// the entry and reload instead of panicking in a user process.
#[test]
fn a_poisoned_cache_lock_clears_and_reloads() {
    let cache = NativeRootsCache::new();
    let clock = TestClock::new();
    let loads = CountingLoads::new();

    let _ = cache.load(platform_key(), loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert_eq!(loads.count(), 1);

    // Expire the primed entry so the panicking loader actually runs and
    // poisons the lock mid-load.
    clock.advance(NATIVE_ROOTS_CACHE_TTL + Duration::from_secs(/*secs*/ 1));
    let panicking_loader = loads.loader(|| panic!("injected loader failure"));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cache.load(platform_key(), panicking_loader, || clock.now())
    }));
    assert!(
        outcome.is_err(),
        "the loader's own panic must propagate to its caller"
    );
    assert_eq!(loads.count(), 2);

    let recovered = cache.load(platform_key(), loads.loader(|| fixture_load(vec![fixture_native_root()])), || clock.now());
    assert_eq!(
        (recovered.len(), loads.count()),
        (1, 3),
        "after poison recovery the cache reloads instead of panicking"
    );
}

/// A caller that waits on the cache lock behind another load must judge
/// validity with the clock it reads after acquiring the lock, never with a
/// timestamp from before the wait.
#[test]
fn lock_wait_does_not_extend_entry_validity() {
    let cache = Arc::new(NativeRootsCache::new());
    let clock = Arc::new(TestClock::new());
    let parked = std::sync::mpsc::channel::<()>();
    let released = std::sync::mpsc::channel::<()>();

    let primed = cache.load(
        platform_key(),
        || fixture_load(vec![fixture_native_root()]),
        || clock.now(),
    );

    // Thread A misses on a different source key and parks inside its loader,
    // holding the cache lock.
    let cache_a = Arc::clone(&cache);
    let clock_a = Arc::clone(&clock);
    let (parked_tx, parked_rx) = (parked.0, parked.1);
    let (released_tx, released_rx) = (released.0, released.1);
    let thread_a = std::thread::spawn(move || {
        let loader = move || {
            parked_tx.send(()).expect("park signal");
            released_rx
                .recv()
                .expect("release signal");
            fixture_load(vec![fixture_native_root()])
        };
        cache_a.load(file_key("/tmp/other-source.pem"), loader, || clock_a.now())
    });
    parked_rx.recv().expect("thread A parks inside its loader");

    // Thread B starts while the primed entry is still valid, then blocks on
    // the lock held by A while the entry expires.
    let cache_b = Arc::clone(&cache);
    let clock_b = Arc::clone(&clock);
    let thread_b = std::thread::spawn(move || {
        cache_b.load(platform_key(), || fixture_load(vec![fixture_native_root()]), || clock_b.now())
    });

    clock.advance(NATIVE_ROOTS_CACHE_TTL + Duration::from_secs(/*secs*/ 1));
    released_tx.send(()).expect("release thread A");
    let from_b = thread_b.join().expect("thread B should not panic");
    let _ = thread_a.join().expect("thread A should not panic");

    assert!(
        !Arc::ptr_eq(&primed, &from_b),
        "an entry that expired while the caller waited on the lock must not be reused"
    );
}
