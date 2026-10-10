//! Process-wide bounded-staleness cache for platform root certificate loads.
//!
//! Loading platform trust settings (`rustls_native_certs` iterates the macOS
//! keychain `TrustSettings`, reads `SSL_CERT_FILE`/`SSL_CERT_DIR`, or walks the
//! Windows system store) can take seconds on some hosts, and the load is
//! identical for every caller in the process, so finished loads are shared.
//!
//! The cache is keyed by the loader's real inputs, not by a platform label:
//! the locked `rustls-native-certs` reads `SSL_CERT_FILE` and `SSL_CERT_DIR`
//! through `var_os` on every platform before it ever touches a platform store,
//! so the key is the raw `Option<OsString>` pair. Missing and empty values
//! stay distinct (an empty `SSL_CERT_FILE` redirects the loader away from the
//! platform store), and non-UTF-8 values keep their exact bytes.
//!
//! Lifecycle contract:
//!
//! - a source-key change invalidates immediately, before any TTL
//! - a clean load stays valid for [`NATIVE_ROOTS_CACHE_TTL`]
//! - a partial load (loader errors, or platform certificates that failed to
//!   parse) stays valid only for [`NATIVE_ROOTS_PARTIAL_FAILURE_TTL`]
//! - a load that produced no certificates is never cached, so the current
//!   connector fails closed while the next connector retries the platform
//! - validity is checked with a clock read *after* the cache lock is
//!   acquired, and `expires_at` is computed from a clock read *after* the
//!   load completes, so waiting on the lock or behind a slow load can never
//!   extend an entry's usable lifetime
//! - the loader runs while the lock is held, making concurrent callers
//!   single-flight; if a loader panics and poisons the lock, the next caller
//!   clears the entry and reloads instead of panicking
//!
//! This bounds staleness and amortizes the load. It does not remove the
//! underlying synchronous system call, which the first load per source still
//! pays on the calling thread.
//!
//! Production uses the shared [`PRODUCTION_NATIVE_ROOTS_CACHE`]; tests create
//! independent [`NativeRootsCache`] instances so fixture roots never leak
//! into, or inherit from, the process-wide cache.

use std::ffi::OsString;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use rustls::RootCertStore;
use tracing::warn;

/// Environment variable the locked loader reads for a single certificate
/// bundle file, honored on every platform before the platform store.
pub(crate) const SSL_CERT_DIR_ENV: &str = "SSL_CERT_DIR";

/// How long a fully successful platform-root load stays valid for new connectors.
pub(crate) const NATIVE_ROOTS_CACHE_TTL: Duration = Duration::from_secs(60);
/// How long a partially failed load stays valid.
///
/// A partial load (loader errors reported, or some certificates rejected by
/// the root store) is the platform's current answer, but the failure means it
/// may also be incomplete, so it refreshes sooner than a clean load.
pub(crate) const NATIVE_ROOTS_PARTIAL_FAILURE_TTL: Duration = Duration::from_secs(5);

/// The shared cache instance used by production connector construction.
pub(super) static PRODUCTION_NATIVE_ROOTS_CACHE: NativeRootsCache = NativeRootsCache::new();

/// The environment identity of one platform-root load, mirroring the locked
/// loader's inputs without loss.
///
/// `cert_file` and `cert_dir` are the raw `SSL_CERT_FILE`/`SSL_CERT_DIR`
/// values: `None` means the variable is absent, `Some` keeps the exact bytes
/// (including empty and non-UTF-8 values, which the loader treats
/// differently from absence). Because the pair is structured, two different
/// sources can never collide the way a joined string key could.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NativeRootsSourceKey {
    cert_file: Option<OsString>,
    cert_dir: Option<OsString>,
}

impl NativeRootsSourceKey {
    /// Builds the key from the raw environment values the loader will read.
    pub(crate) fn from_env_values(cert_file: Option<OsString>, cert_dir: Option<OsString>) -> Self {
        Self {
            cert_file,
            cert_dir,
        }
    }
}

struct CachedNativeRoots {
    source_key: NativeRootsSourceKey,
    expires_at: Instant,
    roots: Arc<RootCertStore>,
}

/// A bounded-staleness cache for platform root stores.
///
/// See the [module documentation](self) for the lifecycle contract. Instances
/// are cheap value types; production shares [`PRODUCTION_NATIVE_ROOTS_CACHE`]
/// while tests build private ones.
pub(crate) struct NativeRootsCache {
    entry: Mutex<Option<CachedNativeRoots>>,
}

impl NativeRootsCache {
    /// Creates an empty cache.
    pub(crate) const fn new() -> Self {
        Self {
            entry: Mutex::new(None),
        }
    }

    /// Returns the platform roots for `source_key`, loading them through
    /// `load_native_roots` when no still-valid entry exists.
    ///
    /// `now` is a clock callback rather than a timestamp so validity can be
    /// judged after the lock is acquired and expiry computed after the load
    /// finishes; a caller that waits behind another load never extends an
    /// entry's lifetime past real expiry.
    pub(crate) fn load(
        &self,
        source_key: NativeRootsSourceKey,
        load_native_roots: impl FnOnce() -> rustls_native_certs::CertificateResult,
        now: impl Fn() -> Instant,
    ) -> Arc<RootCertStore> {
        let mut cache = self.entry.lock().unwrap_or_else(|poisoned| {
            // A loader panicked while holding the lock. The entry may hold a
            // half-finished generation's view, so the recovery policy is to
            // clear it and reload rather than panic in a user process.
            warn!("native roots cache lock was poisoned; clearing the entry and reloading");
            let mut guard = poisoned.into_inner();
            *guard = None;
            guard
        });
        if let Some(cached) = cache.as_ref()
            && cached.source_key == source_key
            && cached.expires_at > now()
        {
            return Arc::clone(&cached.roots);
        }

        let rustls_native_certs::CertificateResult { certs, errors, .. } = load_native_roots();
        if !errors.is_empty() {
            warn!(
                native_root_error_count = errors.len(),
                "encountered errors while loading native root certificates"
            );
        }
        let mut root_store = RootCertStore::empty();
        let (added, ignored) = root_store.add_parsable_certificates(certs);
        if ignored > 0 {
            warn!(
                ignored_certificate_count = ignored,
                "native root certificates could not be parsed and were skipped"
            );
        }
        if added == 0 {
            // Cache nothing: an empty store must fail closed for this connector
            // while the next connector retries the load, so a transient
            // keychain or bundle failure recovers instead of poisoning the
            // process.
            warn!("native root load produced no certificates; retrying on the next connector");
            return Arc::new(root_store);
        }
        let ttl = if errors.is_empty() && ignored == 0 {
            NATIVE_ROOTS_CACHE_TTL
        } else {
            NATIVE_ROOTS_PARTIAL_FAILURE_TTL
        };
        let roots = Arc::new(root_store);
        *cache = Some(CachedNativeRoots {
            source_key,
            expires_at: now() + ttl,
            roots: Arc::clone(&roots),
        });
        roots
    }
}

#[cfg(test)]
#[path = "native_roots_cache_tests.rs"]
mod tests;
