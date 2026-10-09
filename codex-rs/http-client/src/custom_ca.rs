//! Custom CA handling shared by Codex outbound HTTP and websocket clients.
//!
//! Codex constructs outbound reqwest clients and secure websocket connections in a few crates, but
//! they all need the same trust-store policy when enterprise proxies or gateways intercept TLS.
//! This module centralizes that policy so callers can start from an ordinary
//! `reqwest::ClientBuilder` or rustls client config, layer in custom CA support, and either get
//! back a configured transport or a user-facing error that explains how to fix a misconfigured CA
//! bundle.
//!
//! The module intentionally has a narrow responsibility:
//!
//! - read CA material from `CODEX_CA_CERTIFICATE`, falling back to `SSL_CERT_FILE`
//! - normalize PEM variants that show up in real deployments, including OpenSSL-style
//!   `TRUSTED CERTIFICATE` labels and bundles that also contain CRLs
//! - return user-facing errors that explain how to fix misconfigured CA files
//!
//! Its production contract is narrow: produce a transport configuration whose root store contains
//! every parseable certificate block from the configured PEM bundle, or fail early with a precise
//! error before the caller starts network traffic.
//!
//! In this module's test setup, a hermetic test is one whose result depends only on the CA file
//! and environment variables that the test chose for itself. That matters here because the normal
//! reqwest client-construction path is not hermetic enough for environment-sensitive tests:
//!
//! - on macOS seatbelt runs, `reqwest::Client::builder().build()` can panic inside
//!   `system-configuration` while probing platform proxy settings, which means the process can die
//!   before the custom-CA code reports success or a structured error. That matters in practice
//!   because Codex itself commonly runs spawned test processes under seatbelt, so this is not just
//!   a hypothetical CI edge case.
//! - child processes inherit CA-related environment variables by default, which lets developer
//!   shell state or CI configuration affect a test unless the test scrubs those variables first
//!
//! The tests in this crate therefore stay split across two layers:
//!
//! - unit tests in this module cover env-selection logic without constructing a real client
//! - subprocess integration tests under `tests/` cover real client construction through
//!   [`build_reqwest_client_for_subprocess_tests`], which disables reqwest proxy autodetection so
//!   the tests can observe custom-CA success and failure directly, including one TLS handshake
//!   through a local HTTPS server
//! - those subprocess tests also scrub inherited CA environment variables before launch so their
//!   result depends only on the test fixtures and env vars set by the test itself

use std::env;
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use codex_utils_rustls_provider::ensure_rustls_crypto_provider;
use rustls::ClientConfig;
use rustls::RootCertStore;
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::pem::SectionKind;
use rustls_pki_types::pem::{self};
use thiserror::Error;
use tracing::info;
use tracing::warn;

pub const CODEX_CA_CERT_ENV: &str = "CODEX_CA_CERTIFICATE";
pub const SSL_CERT_FILE_ENV: &str = "SSL_CERT_FILE";
const CA_CERT_HINT: &str = "If you set CODEX_CA_CERTIFICATE or SSL_CERT_FILE, ensure it points to a PEM file containing one or more CERTIFICATE blocks, or unset it to use system roots.";
type PemSection = (SectionKind, Vec<u8>);

/// Describes why a transport using shared custom CA support could not be constructed.
///
/// These failure modes apply to both reqwest client construction and websocket TLS
/// configuration. A build can fail because the configured CA file could not be read, could not be
/// parsed as certificates, contained certs that the target TLS stack refused to register, or
/// because the final reqwest client builder failed. Callers that do not care about the
/// distinction can rely on the `From<BuildCustomCaTransportError> for io::Error` conversion.
#[derive(Debug, Error)]
pub enum BuildCustomCaTransportError {
    /// Reading the selected CA file from disk failed before any PEM parsing could happen.
    #[error(
        "Failed to read CA certificate file {} selected by {}: {source}. {hint}",
        path.display(),
        source_env,
        hint = CA_CERT_HINT
    )]
    ReadCaFile {
        source_env: &'static str,
        path: PathBuf,
        source: io::Error,
    },

    /// The selected CA file was readable, but did not produce usable certificate material.
    #[error(
        "Failed to load CA certificates from {} selected by {}: {detail}. {hint}",
        path.display(),
        source_env,
        hint = CA_CERT_HINT
    )]
    InvalidCaFile {
        source_env: &'static str,
        path: PathBuf,
        detail: String,
    },

    /// One parsed certificate block could not be registered with the reqwest client builder.
    #[error(
        "Failed to parse certificate #{certificate_index} from {} selected by {}: {source}. {hint}",
        path.display(),
        source_env,
        hint = CA_CERT_HINT
    )]
    RegisterCertificate {
        source_env: &'static str,
        path: PathBuf,
        certificate_index: usize,
        source: reqwest::Error,
    },

    /// Reqwest rejected the final client configuration after a custom CA bundle was loaded.
    #[error(
        "Failed to build HTTP client while using CA bundle from {} ({}): {source}",
        source_env,
        path.display()
    )]
    BuildClientWithCustomCa {
        source_env: &'static str,
        path: PathBuf,
        #[source]
        source: reqwest::Error,
    },

    /// Reqwest rejected the final client configuration while using only system roots.
    #[error("Failed to build HTTP client while using system root certificates: {0}")]
    BuildClientWithSystemRoots(#[source] reqwest::Error),

    /// One parsed certificate block could not be registered with the websocket TLS root store.
    #[error(
        "Failed to register certificate #{certificate_index} from {} selected by {} in rustls root store: {source}. {hint}",
        path.display(),
        source_env,
        hint = CA_CERT_HINT
    )]
    RegisterRustlsCertificate {
        source_env: &'static str,
        path: PathBuf,
        certificate_index: usize,
        source: rustls::Error,
    },
}

impl From<BuildCustomCaTransportError> for io::Error {
    fn from(error: BuildCustomCaTransportError) -> Self {
        match error {
            BuildCustomCaTransportError::ReadCaFile { ref source, .. } => {
                io::Error::new(source.kind(), error)
            }
            BuildCustomCaTransportError::InvalidCaFile { .. }
            | BuildCustomCaTransportError::RegisterCertificate { .. }
            | BuildCustomCaTransportError::RegisterRustlsCertificate { .. } => {
                io::Error::new(io::ErrorKind::InvalidData, error)
            }
            BuildCustomCaTransportError::BuildClientWithCustomCa { .. }
            | BuildCustomCaTransportError::BuildClientWithSystemRoots(_) => io::Error::other(error),
        }
    }
}

/// Builds a reqwest client that honors Codex custom CA environment variables.
///
/// Callers supply the baseline builder configuration they need, and this helper layers in custom
/// CA handling before finally constructing the client. `CODEX_CA_CERTIFICATE` takes precedence
/// over `SSL_CERT_FILE`, and empty values for either are treated as unset so callers do not
/// accidentally turn `VAR=""` into a bogus path lookup.
///
/// Callers that build a raw `reqwest::Client` directly bypass this policy entirely. That is an
/// easy mistake to make when adding a new outbound Codex HTTP path, and the resulting bug only
/// shows up in environments where a proxy or gateway requires a custom root CA.
///
/// # Errors
///
/// Returns a [`BuildCustomCaTransportError`] when the configured CA file is unreadable,
/// malformed, or contains a certificate block that `reqwest` cannot register as a root.
pub fn build_reqwest_client_with_custom_ca(
    builder: reqwest::ClientBuilder,
) -> Result<reqwest::Client, BuildCustomCaTransportError> {
    build_reqwest_client_with_env(&ProcessEnv, builder)
}

/// Builds a rustls client config when a Codex custom CA bundle is configured.
///
/// This is the websocket-facing sibling of [`build_reqwest_client_with_custom_ca`]. When
/// `CODEX_CA_CERTIFICATE` or `SSL_CERT_FILE` selects a CA bundle, the returned config starts from
/// the platform native roots and then adds the configured custom CA certificates. When no custom
/// CA env var is set, this returns `Ok(None)` so websocket callers can keep using their ordinary
/// default connector path.
///
/// Callers that let tungstenite build its default TLS connector directly bypass this policy
/// entirely. That bug only shows up in environments where secure websocket traffic needs the same
/// enterprise root CA bundle as HTTPS traffic.
pub fn maybe_build_rustls_client_config_with_custom_ca()
-> Result<Option<Arc<ClientConfig>>, BuildCustomCaTransportError> {
    maybe_build_rustls_client_config_with_env(&ProcessEnv)
}

/// Builds a rustls client config using native roots and any configured Codex custom CA bundle.
///
/// Unlike [`maybe_build_rustls_client_config_with_custom_ca`], this always returns a config. Use
/// this when the caller must perform TLS itself instead of delegating default configuration to a
/// transport library.
pub fn build_rustls_client_config_with_custom_ca()
-> Result<Arc<ClientConfig>, BuildCustomCaTransportError> {
    build_rustls_client_config_with_env(&ProcessEnv)
}

/// Builds a reqwest client for spawned subprocess tests that exercise CA behavior.
///
/// This is the test-only client-construction path used by the subprocess coverage in `tests/`.
/// The module-level docs explain the hermeticity problem in full; this helper only addresses the
/// reqwest proxy-discovery panic side of that problem by disabling proxy autodetection. The tests
/// still scrub inherited CA environment variables themselves. Normal production callers should use
/// [`build_reqwest_client_with_custom_ca`] so test-only proxy behavior does not leak into
/// ordinary client construction.
pub fn build_reqwest_client_for_subprocess_tests(
    builder: reqwest::ClientBuilder,
) -> Result<reqwest::Client, BuildCustomCaTransportError> {
    build_reqwest_client_with_env(&ProcessEnv, builder.no_proxy())
}

fn maybe_build_rustls_client_config_with_env(
    env_source: &dyn EnvSource,
) -> Result<Option<Arc<ClientConfig>>, BuildCustomCaTransportError> {
    let Some(bundle) = env_source.configured_ca_bundle() else {
        return Ok(None);
    };

    build_rustls_client_config(Some(&bundle)).map(Some)
}

fn build_rustls_client_config_with_env(
    env_source: &dyn EnvSource,
) -> Result<Arc<ClientConfig>, BuildCustomCaTransportError> {
    let bundle = env_source.configured_ca_bundle();
    build_rustls_client_config(bundle.as_ref())
}

/// How long a fully successful platform-root load stays valid for new connectors.
const NATIVE_ROOTS_CACHE_TTL: Duration = Duration::from_secs(60);
/// How long a partially failed load (some errors, some certificates) stays valid.
///
/// A partial load is the platform's current answer, but the failure means it may
/// also be incomplete, so it refreshes sooner than a clean load.
const NATIVE_ROOTS_PARTIAL_FAILURE_TTL: Duration = Duration::from_secs(5);

/// Platform root certificates with a source-keyed, bounded-staleness cache.
///
/// Loading macOS trust settings (`rustls_native_certs` iterates the keychain
/// `TrustSettings`) can take seconds on some hosts, and the load is identical
/// for every caller, so finished loads are shared. Unlike a process-wide
/// snapshot, the cache is keyed by the environment sources the loader reads
/// (on Unix, `SSL_CERT_FILE`/`SSL_CERT_DIR`) and expires, so connectors built
/// later observe revoked roots or replaced files; loads that produce no
/// certificates are never cached, and partially failed loads are retried on
/// the short TTL. This bounds staleness and amortizes the load; it does not
/// remove the underlying synchronous system call, which the first load per
/// source still pays on the calling thread.
static NATIVE_ROOTS_CACHE: Mutex<Option<CachedNativeRoots>> = Mutex::new(None);

struct CachedNativeRoots {
    source_key: String,
    expires_at: Instant,
    roots: Arc<RootCertStore>,
}

/// Serializes tests that observe or mutate the global [`NATIVE_ROOTS_CACHE`].
#[cfg(test)]
static NATIVE_ROOTS_CACHE_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Runs one hermetic observation against the global cache, restoring the
/// previous entry afterwards so concurrent tests never inherit fixture roots.
#[cfg(test)]
fn with_native_roots_cache<T>(observed: impl FnOnce() -> T) -> T {
    let _guard = NATIVE_ROOTS_CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| panic!("test lock should not be poisoned: {error}"));
    let previous = NATIVE_ROOTS_CACHE
        .lock()
        .unwrap_or_else(|error| panic!("cache lock should not be poisoned: {error}"))
        .take();
    let outcome = observed();
    let mut cache = NATIVE_ROOTS_CACHE
        .lock()
        .unwrap_or_else(|error| panic!("cache lock should not be poisoned: {error}"));
    *cache = previous;
    outcome
}

/// The environment identity of the platform-root source for one load.
///
/// On Unix the loader honors `SSL_CERT_FILE`/`SSL_CERT_DIR` (via
/// `openssl-probe`), so a value change must invalidate immediately. The macOS
/// keychain and Windows system stores cannot be redirected through those
/// variables, so there the source is the platform identity and only TTL
/// expiry picks up trust-setting changes.
fn native_root_source_key(env_source: &dyn EnvSource) -> String {
    #[cfg(unix)]
    {
        let cert_file = env_source.var("SSL_CERT_FILE").unwrap_or_default();
        let cert_dir = env_source.var("SSL_CERT_DIR").unwrap_or_default();
        format!("unix\u{1}{cert_file}\u{1}{cert_dir}")
    }
    #[cfg(not(unix))]
    {
        let _ = env_source;
        "platform".to_string()
    }
}

/// Loads the platform roots through the bounded-staleness cache.
///
/// The loader runs while the cache lock is held, so concurrent callers are
/// single-flight: one system load, everyone shares its result.
fn cached_native_roots(
    env_source: &dyn EnvSource,
    load_native_roots: impl FnOnce() -> rustls_native_certs::CertificateResult,
    now: Instant,
) -> Arc<RootCertStore> {
    let source_key = native_root_source_key(env_source);
    let mut cache = NATIVE_ROOTS_CACHE
        .lock()
        .unwrap_or_else(|error| panic!("native roots cache lock should not be poisoned: {error}"));
    if let Some(cached) = cache.as_ref()
        && cached.source_key == source_key
        && cached.expires_at > now
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
        // while the next connector retries the load, so a transient keychain
        // or bundle failure recovers instead of poisoning the process.
        warn!("native root load produced no certificates; retrying on the next connector");
        return Arc::new(root_store);
    }
    let ttl = if errors.is_empty() {
        NATIVE_ROOTS_CACHE_TTL
    } else {
        NATIVE_ROOTS_PARTIAL_FAILURE_TTL
    };
    let roots = Arc::new(root_store);
    *cache = Some(CachedNativeRoots {
        source_key,
        expires_at: now + ttl,
        roots: Arc::clone(&roots),
    });
    roots
}

fn build_rustls_client_config(
    bundle: Option<&ConfiguredCaBundle>,
) -> Result<Arc<ClientConfig>, BuildCustomCaTransportError> {
    build_rustls_client_config_with_native_roots(
        &ProcessEnv,
        rustls_native_certs::load_native_certs,
        Instant::now(),
        bundle,
    )
}

/// Builds a rustls client config from cached platform roots, injecting the
/// environment source, root loader, and cache clock for hermetic tests.
fn build_rustls_client_config_with_native_roots(
    env_source: &dyn EnvSource,
    load_native_roots: impl FnOnce() -> rustls_native_certs::CertificateResult,
    now: Instant,
    bundle: Option<&ConfiguredCaBundle>,
) -> Result<Arc<ClientConfig>, BuildCustomCaTransportError> {
    ensure_rustls_crypto_provider();

    // Start from the platform roots so websocket callers keep the same baseline trust behavior
    // they would get from tungstenite's default rustls connector, then layer in the Codex custom
    // CA bundle on top when configured. The bundle extends a private clone, never the cache.
    let native_roots = cached_native_roots(env_source, load_native_roots, now);
    let root_store = match bundle {
        None => native_roots,
        Some(bundle) => {
            let mut root_store = native_roots.as_ref().clone();
            let certificates = bundle.load_certificates()?;
            for (idx, cert) in certificates.into_iter().enumerate() {
                if let Err(source) = root_store.add(cert) {
                    warn!(
                        source_env = bundle.source_env,
                        ca_path = %bundle.path.display(),
                        certificate_index = idx + 1,
                        error = %source,
                        "failed to register CA certificate in rustls root store"
                    );
                    return Err(BuildCustomCaTransportError::RegisterRustlsCertificate {
                        source_env: bundle.source_env,
                        path: bundle.path.clone(),
                        certificate_index: idx + 1,
                        source,
                    });
                }
            }
            Arc::new(root_store)
        }
    };

    Ok(Arc::new(
        ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    ))
}

/// Builds a reqwest client using an injected environment source and reqwest builder.
///
/// This exists so tests can exercise precedence behavior deterministically without mutating the
/// real process environment. It selects the CA bundle, delegates file parsing to
/// [`ConfiguredCaBundle::load_certificates`], preserves the caller's chosen `reqwest` builder
/// configuration, forces rustls when a custom CA is configured, and finally registers each parsed
/// certificate with that builder.
fn build_reqwest_client_with_env(
    env_source: &dyn EnvSource,
    mut builder: reqwest::ClientBuilder,
) -> Result<reqwest::Client, BuildCustomCaTransportError> {
    if let Some(bundle) = env_source.configured_ca_bundle() {
        ensure_rustls_crypto_provider();
        info!(
            source_env = bundle.source_env,
            ca_path = %bundle.path.display(),
            "building HTTP client with rustls backend for custom CA bundle"
        );
        builder = builder.use_rustls_tls();

        let certificates = bundle.load_certificates()?;

        for (idx, cert) in certificates.iter().enumerate() {
            let certificate = match reqwest::Certificate::from_der(cert.as_ref()) {
                Ok(certificate) => certificate,
                Err(source) => {
                    warn!(
                        source_env = bundle.source_env,
                        ca_path = %bundle.path.display(),
                        certificate_index = idx + 1,
                        error = %source,
                        "failed to register CA certificate"
                    );
                    return Err(BuildCustomCaTransportError::RegisterCertificate {
                        source_env: bundle.source_env,
                        path: bundle.path.clone(),
                        certificate_index: idx + 1,
                        source,
                    });
                }
            };
            builder = builder.add_root_certificate(certificate);
        }
        return match builder.build() {
            Ok(client) => Ok(client),
            Err(source) => {
                warn!(
                    source_env = bundle.source_env,
                    ca_path = %bundle.path.display(),
                    error = %source,
                    "failed to build client after loading custom CA bundle"
                );
                Err(BuildCustomCaTransportError::BuildClientWithCustomCa {
                    source_env: bundle.source_env,
                    path: bundle.path.clone(),
                    source,
                })
            }
        };
    }

    info!(
        codex_ca_certificate_configured = false,
        ssl_cert_file_configured = false,
        "using system root certificates because no CA override environment variable was selected"
    );

    match builder.build() {
        Ok(client) => Ok(client),
        Err(source) => {
            warn!(
                error = %source,
                "failed to build client while using system root certificates"
            );
            Err(BuildCustomCaTransportError::BuildClientWithSystemRoots(
                source,
            ))
        }
    }
}

/// Abstracts environment access so tests can cover precedence rules without mutating process-wide
/// variables.
trait EnvSource {
    /// Returns the environment variable value for `key`, if this source considers it set.
    ///
    /// Implementations should return `None` for absent values and may also collapse unreadable
    /// process-environment states into `None`, because the custom CA logic treats both cases as
    /// "no override configured". Callers build precedence and empty-string handling on top of this
    /// method, so implementations should not trim or normalize the returned string.
    fn var(&self, key: &str) -> Option<String>;

    /// Returns a non-empty environment variable value interpreted as a filesystem path.
    ///
    /// Empty strings are treated as unset because presence here acts as a boolean "custom CA
    /// override requested" signal. This keeps the precedence logic from treating `VAR=""` as an
    /// attempt to open the current working directory or some other platform-specific oddity once
    /// it is converted into a path.
    fn non_empty_path(&self, key: &str) -> Option<PathBuf> {
        self.var(key)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }

    /// Returns the configured CA bundle and which environment variable selected it.
    ///
    /// `CODEX_CA_CERTIFICATE` wins over `SSL_CERT_FILE` because it is the Codex-specific override.
    /// Keeping the winning variable name with the path lets later logging explain not only which
    /// file was used but also why that file was chosen.
    fn configured_ca_bundle(&self) -> Option<ConfiguredCaBundle> {
        self.non_empty_path(CODEX_CA_CERT_ENV)
            .map(|path| ConfiguredCaBundle {
                source_env: CODEX_CA_CERT_ENV,
                path,
            })
            .or_else(|| {
                self.non_empty_path(SSL_CERT_FILE_ENV)
                    .map(|path| ConfiguredCaBundle {
                        source_env: SSL_CERT_FILE_ENV,
                        path,
                    })
            })
    }
}

/// Reads CA configuration from the real process environment.
///
/// This is the production `EnvSource` implementation used by
/// [`build_reqwest_client_with_custom_ca`]. Tests substitute in-memory env maps so they can
/// exercise precedence and empty-value behavior without mutating process-global variables.
struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn var(&self, key: &str) -> Option<String> {
        env::var(key).ok()
    }
}

/// Identifies the CA bundle selected for a client and the policy decision that selected it.
///
/// This is the concrete output of the environment-precedence logic. Callers use `source_env` for
/// logging and diagnostics, while `path` is the bundle that will actually be loaded.
struct ConfiguredCaBundle {
    /// The environment variable that won the precedence check for this bundle.
    source_env: &'static str,
    /// The filesystem path that should be read as PEM certificate input.
    path: PathBuf,
}

impl ConfiguredCaBundle {
    /// Loads certificates from this selected CA bundle.
    ///
    /// The bundle already represents the output of environment-precedence selection, so this is
    /// the natural point where the file-loading phase begins. The method owns the high-level
    /// success/failure logs for that phase and keeps the source env and path together for lower-
    /// level parsing and error shaping.
    fn load_certificates(
        &self,
    ) -> Result<Vec<CertificateDer<'static>>, BuildCustomCaTransportError> {
        match self.parse_certificates() {
            Ok(certificates) => {
                info!(
                    source_env = self.source_env,
                    ca_path = %self.path.display(),
                    certificate_count = certificates.len(),
                    "loaded certificates from custom CA bundle"
                );
                Ok(certificates)
            }
            Err(error) => {
                warn!(
                    source_env = self.source_env,
                    ca_path = %self.path.display(),
                    error = %error,
                    "failed to load custom CA bundle"
                );
                Err(error)
            }
        }
    }

    /// Loads every certificate block from a PEM file intended for Codex CA overrides.
    ///
    /// This accepts a few common real-world variants so Codex behaves like other CA-aware tooling:
    /// leading comments are preserved, `TRUSTED CERTIFICATE` labels are normalized to standard
    /// certificate labels, and embedded CRLs are ignored when they are well-formed enough for the
    /// section iterator to classify them.
    fn parse_certificates(
        &self,
    ) -> Result<Vec<CertificateDer<'static>>, BuildCustomCaTransportError> {
        let pem_data = self.read_pem_data()?;
        let normalized_pem = NormalizedPem::from_pem_data(self.source_env, &self.path, &pem_data);

        let mut certificates = Vec::new();
        let mut logged_crl_presence = false;
        for section_result in normalized_pem.sections() {
            // Known limitation: if `rustls-pki-types` fails while parsing a malformed CRL section,
            // that error is reported here before we can classify the block as ignorable. A bundle
            // containing valid certificates plus a malformed `X509 CRL` therefore still fails to
            // load today, even though well-formed CRLs are ignored.
            let (section_kind, der) = match section_result {
                Ok(section) => section,
                Err(error) => return Err(self.pem_parse_error(&error)),
            };
            match section_kind {
                SectionKind::Certificate => {
                    // Standard CERTIFICATE blocks already decode to the exact DER bytes reqwest
                    // wants. Only OpenSSL TRUSTED CERTIFICATE blocks need trimming to drop any
                    // trailing X509_AUX trust metadata before registration.
                    let cert_der = normalized_pem.certificate_der(&der).ok_or_else(|| {
                        self.invalid_ca_file(
                            "failed to extract certificate data from TRUSTED CERTIFICATE: invalid DER length",
                        )
                    })?;
                    certificates.push(CertificateDer::from(cert_der.to_vec()));
                }
                SectionKind::Crl if !logged_crl_presence => {
                    info!(
                        source_env = self.source_env,
                        ca_path = %self.path.display(),
                        "ignoring X509 CRL entries found in custom CA bundle"
                    );
                    logged_crl_presence = true;
                }
                _ => {}
            }
        }

        if certificates.is_empty() {
            return Err(self.pem_parse_error(&pem::Error::NoItemsFound));
        }

        Ok(certificates)
    }

    /// Reads the CA bundle bytes while preserving the original filesystem error kind.
    ///
    /// The caller wants a user-facing error that includes the bundle path and remediation hint, but
    /// higher-level surfaces still benefit from distinguishing "not found" from other I/O
    /// failures. This helper keeps both pieces together.
    fn read_pem_data(&self) -> Result<Vec<u8>, BuildCustomCaTransportError> {
        fs::read(&self.path).map_err(|source| BuildCustomCaTransportError::ReadCaFile {
            source_env: self.source_env,
            path: self.path.clone(),
            source,
        })
    }

    /// Rewrites PEM parsing failures into user-facing configuration errors.
    ///
    /// The underlying parser knows whether the file was empty, malformed, or contained unsupported
    /// PEM content, but callers need a message that also points them back to the relevant
    /// environment variables and the expected remediation.
    fn pem_parse_error(&self, error: &pem::Error) -> BuildCustomCaTransportError {
        let detail = match error {
            pem::Error::NoItemsFound => "no certificates found in PEM file".to_string(),
            _ => format!("failed to parse PEM file: {error}"),
        };

        self.invalid_ca_file(detail)
    }

    /// Creates an invalid-CA error tied to this file path.
    ///
    /// Most parse-time failures in this module eventually collapse to "the configured CA bundle is
    /// not usable", but the detailed reason still matters for operator debugging. Centralizing that
    /// formatting keeps the path and hint text consistent across the different parser branches.
    fn invalid_ca_file(&self, detail: impl std::fmt::Display) -> BuildCustomCaTransportError {
        BuildCustomCaTransportError::InvalidCaFile {
            source_env: self.source_env,
            path: self.path.clone(),
            detail: detail.to_string(),
        }
    }
}

/// The PEM text shape after OpenSSL compatibility normalization.
///
/// `Standard` means the input already used ordinary PEM certificate labels. `TrustedCertificate`
/// means the input used OpenSSL's `TRUSTED CERTIFICATE` labels, so callers must also be prepared
/// to trim trailing `X509_AUX` bytes from decoded certificate sections.
enum NormalizedPem {
    /// PEM contents that already used ordinary `CERTIFICATE` labels.
    Standard(String),
    /// PEM contents rewritten from OpenSSL `TRUSTED CERTIFICATE` labels to `CERTIFICATE`.
    TrustedCertificate(String),
}

impl NormalizedPem {
    /// Normalizes PEM text from a CA bundle into the label shape this module expects.
    ///
    /// Codex only needs certificate DER bytes to seed `reqwest`'s root store, but operators may
    /// point it at CA files that came from OpenSSL tooling rather than from a minimal certificate
    /// bundle. OpenSSL's `TRUSTED CERTIFICATE` form is one such variant: it is still certificate
    /// material, but it uses a different PEM label and may carry auxiliary trust metadata that
    /// this crate does not consume. This constructor rewrites only the PEM labels so the mixed-
    /// section parser can keep treating the file as certificate input. The rustls ecosystem does
    /// not currently accept `TRUSTED CERTIFICATE` as a standard certificate label upstream, so
    /// this remains a local compatibility shim rather than behavior delegated to
    /// `rustls-pki-types`.
    ///
    /// See also:
    /// - rustls/pemfile issue #52, closed as not planned, documenting that
    ///   `BEGIN TRUSTED CERTIFICATE` blocks are ignored upstream:
    ///   <https://github.com/rustls/pemfile/issues/52>
    /// - OpenSSL `x509 -trustout`, which emits `TRUSTED CERTIFICATE` PEM blocks:
    ///   <https://docs.openssl.org/master/man1/openssl-x509/>
    /// - OpenSSL PEM readers, which document that plain `PEM_read_bio_X509()` discards auxiliary
    ///   trust settings:
    ///   <https://docs.openssl.org/master/man3/PEM_read_bio_PrivateKey/>
    /// - `openssl s_server`, a real OpenSSL-based server/test tool that operates in this
    ///   ecosystem:
    ///   <https://docs.openssl.org/master/man1/openssl-s_server/>
    fn from_pem_data(source_env: &'static str, path: &Path, pem_data: &[u8]) -> Self {
        let pem = String::from_utf8_lossy(pem_data);
        if pem.contains("TRUSTED CERTIFICATE") {
            info!(
                source_env,
                ca_path = %path.display(),
                "normalizing OpenSSL TRUSTED CERTIFICATE labels in custom CA bundle"
            );
            Self::TrustedCertificate(
                pem.replace("BEGIN TRUSTED CERTIFICATE", "BEGIN CERTIFICATE")
                    .replace("END TRUSTED CERTIFICATE", "END CERTIFICATE"),
            )
        } else {
            Self::Standard(pem.into_owned())
        }
    }

    /// Returns the normalized PEM contents regardless of the label shape that produced them.
    fn contents(&self) -> &str {
        match self {
            Self::Standard(contents) | Self::TrustedCertificate(contents) => contents,
        }
    }

    /// Iterates over every recognized PEM section in this normalized PEM text.
    ///
    /// `rustls-pki-types` exposes mixed-section parsing through a `PemObject` implementation on the
    /// `(SectionKind, Vec<u8>)` tuple. Keeping that type-directed API here lets callers iterate in
    /// terms of normalized sections rather than trait plumbing.
    fn sections(&self) -> impl Iterator<Item = Result<PemSection, pem::Error>> + '_ {
        PemSection::pem_slice_iter(self.contents().as_bytes())
    }

    /// Returns the certificate DER bytes for one parsed PEM certificate section.
    ///
    /// Standard PEM certificates already decode to the exact DER bytes `reqwest` wants. OpenSSL
    /// `TRUSTED CERTIFICATE` sections may append `X509_AUX` bytes after the certificate, so those
    /// sections need to be trimmed down to their first DER object before registration.
    fn certificate_der<'a>(&self, der: &'a [u8]) -> Option<&'a [u8]> {
        match self {
            Self::Standard(_) => Some(der),
            Self::TrustedCertificate(_) => first_der_item(der),
        }
    }
}

/// Returns the first DER-encoded ASN.1 object in `der`, ignoring any trailing OpenSSL metadata.
///
/// A PEM `CERTIFICATE` block usually decodes to exactly one DER blob: the certificate itself.
/// OpenSSL's `TRUSTED CERTIFICATE` variant is different. It starts with that same certificate
/// blob, but may append extra `X509_AUX` bytes after it to describe OpenSSL-specific trust
/// settings. `reqwest::Certificate::from_der` only understands the certificate object, not those
/// trailing OpenSSL extensions.
///
/// This helper therefore asks a narrower question than "is this a valid certificate?": where does
/// the first top-level DER object end? If that boundary can be found, the caller keeps only that
/// prefix and discards the trailing trust metadata. If it cannot be found, the input is treated as
/// malformed CA data.
fn first_der_item(der: &[u8]) -> Option<&[u8]> {
    der_item_length(der).map(|length| &der[..length])
}

/// Returns the byte length of the first DER item in `der`.
///
/// DER is a binary encoding for ASN.1 objects. Each object begins with:
///
/// - a tag byte describing what kind of object follows
/// - one or more length bytes describing how many content bytes belong to that object
/// - the content bytes themselves
///
/// For this module, the important fact is that a certificate is stored as one complete top-level
/// DER object. Once we know that object's declared length, we know exactly where the certificate
/// ends and where any trailing OpenSSL `X509_AUX` data begins.
///
/// This helper intentionally parses only that outer length field. It does not validate the inner
/// certificate structure, the meaning of the tag, or every nested ASN.1 value. That narrower scope
/// is deliberate: the caller only needs a safe slice boundary for the leading certificate object
/// before handing those bytes to `reqwest`, which performs the real certificate parsing.
///
/// The implementation supports the DER length forms needed here:
///
/// - short form, where the length is stored directly in the second byte
/// - long form, where the second byte says how many following bytes make up the length value
///
/// Indefinite lengths are rejected because DER does not permit them, and any declared length that
/// would run past the end of the input is treated as malformed.
fn der_item_length(der: &[u8]) -> Option<usize> {
    let &length_octet = der.get(1)?;
    if length_octet & 0x80 == 0 {
        return Some(2 + usize::from(length_octet)).filter(|length| *length <= der.len());
    }

    let length_octets = usize::from(length_octet & 0x7f);
    if length_octets == 0 {
        return None;
    }

    let length_start = 2usize;
    let length_end = length_start.checked_add(length_octets)?;
    let length_bytes = der.get(length_start..length_end)?;
    let mut content_length = 0usize;
    for &byte in length_bytes {
        content_length = content_length
            .checked_mul(256)?
            .checked_add(usize::from(byte))?;
    }

    length_end
        .checked_add(content_length)
        .filter(|length| *length <= der.len())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;
    use std::time::Instant;

    use pretty_assertions::assert_eq;
    use rustls_pki_types::CertificateDer;
    use tempfile::TempDir;

    use super::BuildCustomCaTransportError;
    use super::CODEX_CA_CERT_ENV;
    use super::EnvSource;
    use super::NATIVE_ROOTS_CACHE_TTL;
    use super::NATIVE_ROOTS_PARTIAL_FAILURE_TTL;
    use super::SSL_CERT_FILE_ENV;
    use super::cached_native_roots;
    use super::maybe_build_rustls_client_config_with_env;

    const TEST_CERT: &str = include_str!("../tests/fixtures/test-ca.pem");

    struct MapEnv {
        values: HashMap<String, String>,
    }

    impl EnvSource for MapEnv {
        fn var(&self, key: &str) -> Option<String> {
            self.values.get(key).cloned()
        }
    }

    fn map_env(pairs: &[(&str, &str)]) -> MapEnv {
        MapEnv {
            values: pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        }
    }

    fn write_cert_file(temp_dir: &TempDir, name: &str, contents: &str) -> PathBuf {
        let path = temp_dir.path().join(name);
        fs::write(&path, contents).unwrap_or_else(|error| {
            panic!("write cert fixture failed for {}: {error}", path.display())
        });
        path
    }

    #[test]
    fn ca_path_prefers_codex_env() {
        let env = map_env(&[
            (CODEX_CA_CERT_ENV, "/tmp/codex.pem"),
            (SSL_CERT_FILE_ENV, "/tmp/fallback.pem"),
        ]);

        assert_eq!(
            env.configured_ca_bundle().map(|bundle| bundle.path),
            Some(PathBuf::from("/tmp/codex.pem"))
        );
    }

    #[test]
    fn ca_path_falls_back_to_ssl_cert_file() {
        let env = map_env(&[(SSL_CERT_FILE_ENV, "/tmp/fallback.pem")]);

        assert_eq!(
            env.configured_ca_bundle().map(|bundle| bundle.path),
            Some(PathBuf::from("/tmp/fallback.pem"))
        );
    }

    #[test]
    fn ca_path_ignores_empty_values() {
        let env = map_env(&[
            (CODEX_CA_CERT_ENV, ""),
            (SSL_CERT_FILE_ENV, "/tmp/fallback.pem"),
        ]);

        assert_eq!(
            env.configured_ca_bundle().map(|bundle| bundle.path),
            Some(PathBuf::from("/tmp/fallback.pem"))
        );
    }

    #[test]
    fn rustls_config_uses_custom_ca_bundle_when_configured() {
        let temp_dir = TempDir::new().expect("tempdir");
        let cert_path = write_cert_file(&temp_dir, "ca.pem", TEST_CERT);
        let env = map_env(&[(CODEX_CA_CERT_ENV, cert_path.to_string_lossy().as_ref())]);

        let config = maybe_build_rustls_client_config_with_env(&env)
            .expect("rustls config")
            .expect("custom CA config should be present");

        assert!(config.enable_sni);
    }

    #[test]
    fn rustls_config_reports_invalid_ca_file() {
        let temp_dir = TempDir::new().expect("tempdir");
        let cert_path = write_cert_file(&temp_dir, "empty.pem", "");
        let env = map_env(&[(CODEX_CA_CERT_ENV, cert_path.to_string_lossy().as_ref())]);

        let error = maybe_build_rustls_client_config_with_env(&env).expect_err("invalid CA");

        assert!(matches!(
            error,
            BuildCustomCaTransportError::InvalidCaFile { .. }
        ));
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

    #[test]
    fn native_roots_cache_reuses_within_ttl_and_reloads_after_expiry() {
        super::with_native_roots_cache(|| {
            let env = map_env(&[]);
            let loads = Arc::new(Mutex::new(0_usize));
            let counter = Arc::clone(&loads);
            let loader = move || {
                *counter.lock().unwrap() += 1;
                fixture_load(vec![fixture_native_root()])
            };
            let now = Instant::now();

            let first = cached_native_roots(&env, &loader, now);
            let second = cached_native_roots(&env, &loader, now);
            assert!(
                Arc::ptr_eq(&first, &second),
                "within TTL the store is shared"
            );
            assert_eq!(*loads.lock().unwrap(), 1);

            let third = cached_native_roots(
                &env,
                &loader,
                now + NATIVE_ROOTS_CACHE_TTL + Duration::from_secs(/*secs*/ 1),
            );
            assert!(
                !Arc::ptr_eq(&first, &third),
                "expiry must reload the platform"
            );
            assert_eq!(*loads.lock().unwrap(), 2);
        });
    }

    #[cfg(unix)]
    #[test]
    fn native_roots_cache_reloads_when_the_source_environment_changes() {
        super::with_native_roots_cache(|| {
            let env_a = map_env(&[("SSL_CERT_FILE", "/tmp/roots-a.pem")]);
            let env_b = map_env(&[("SSL_CERT_FILE", "/tmp/roots-b.pem")]);
            let loads = Arc::new(Mutex::new(0_usize));
            let counter = Arc::clone(&loads);
            let loader = move || {
                *counter.lock().unwrap() += 1;
                fixture_load(vec![fixture_native_root()])
            };
            let now = Instant::now();

            let _ = cached_native_roots(&env_a, &loader, now);
            let _ = cached_native_roots(&env_b, &loader, now);
            assert_eq!(
                *loads.lock().unwrap(),
                2,
                "a different SSL_CERT_FILE must invalidate immediately"
            );
        });
    }

    #[test]
    fn native_roots_cache_never_caches_an_empty_load() {
        super::with_native_roots_cache(|| {
            let env = map_env(&[]);
            let loads = Arc::new(Mutex::new(0_usize));
            let counter = Arc::clone(&loads);
            let loader = move || {
                let load = *counter.lock().unwrap();
                *counter.lock().unwrap() += 1;
                if load == 0 {
                    rustls_native_certs::CertificateResult::default()
                } else {
                    fixture_load(vec![fixture_native_root()])
                }
            };
            let now = Instant::now();

            let first = cached_native_roots(&env, &loader, now);
            assert!(
                first.is_empty(),
                "a failed load must fail closed for this connector"
            );
            let second = cached_native_roots(&env, &loader, now);
            assert_eq!(second.len(), 1, "the next connector must retry the load");
            assert_eq!(*loads.lock().unwrap(), 2);
        });
    }

    #[test]
    fn native_roots_cache_uses_the_short_ttl_for_partial_failures() {
        super::with_native_roots_cache(|| {
            let env = map_env(&[]);
            let loads = Arc::new(Mutex::new(0_usize));
            let counter = Arc::clone(&loads);
            let loader = move || {
                *counter.lock().unwrap() += 1;
                {
                    // CertificateResult is non-exhaustive, so build it from Default.
                    let mut result = rustls_native_certs::CertificateResult::default();
                    result.certs = vec![fixture_native_root()];
                    result.errors = vec![rustls_native_certs::Error {
                        context: "test partial failure",
                        kind: rustls_native_certs::ErrorKind::Io {
                            inner: std::io::Error::other("injected"),
                            path: PathBuf::from("/tmp/missing-root"),
                        },
                    }];
                    result
                }
            };
            let now = Instant::now();

            let first = cached_native_roots(&env, &loader, now);
            let second = cached_native_roots(&env, &loader, now);
            assert!(
                Arc::ptr_eq(&first, &second),
                "partial results are shared briefly"
            );
            assert_eq!(*loads.lock().unwrap(), 1);
            let _ = cached_native_roots(
                &env,
                &loader,
                now + NATIVE_ROOTS_PARTIAL_FAILURE_TTL + Duration::from_secs(/*secs*/ 1),
            );
            assert_eq!(
                *loads.lock().unwrap(),
                2,
                "partial failures must refresh on the short TTL"
            );
        });
    }

    #[test]
    fn native_roots_cache_is_single_flight_across_threads() {
        super::with_native_roots_cache(|| {
            let env = map_env(&[]);
            let loads = Arc::new(Mutex::new(0_usize));
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let mut threads = Vec::new();
            for _ in 0..2 {
                let env_map = MapEnv {
                    values: env.values.clone(),
                };
                let loads = Arc::clone(&loads);
                let barrier = Arc::clone(&barrier);
                threads.push(std::thread::spawn(move || {
                    barrier.wait();
                    cached_native_roots(
                        &env_map,
                        move || {
                            std::thread::sleep(Duration::from_millis(/*millis*/ 50));
                            *loads.lock().unwrap() += 1;
                            fixture_load(vec![fixture_native_root()])
                        },
                        Instant::now(),
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
        });
    }
}

#[cfg(test)]
#[path = "custom_ca_tls_tests.rs"]
mod tls_tests;
