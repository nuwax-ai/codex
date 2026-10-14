import pathlib

# 1) Add a warm-only peek to the cache.
p = pathlib.Path("http-client/src/native_roots_cache.rs")
text = p.read_text()
old = """    pub(crate) fn load(
        &self,
        source_key: NativeRootsSourceKey,
        load_native_roots: impl FnOnce() -> rustls_native_certs::CertificateResult,
        now: impl Fn() -> Instant,
    ) -> Arc<RootCertStore> {"""
new = """    /// Returns the cached roots without triggering a platform load.
    ///
    /// Callers that must not pay (or wait for) the first platform load use
    /// this to reuse already-warm roots and otherwise keep their ordinary
    /// first-build behavior.
    pub(crate) fn peek(
        &self,
        source_key: NativeRootsSourceKey,
        now: impl Fn() -> Instant,
    ) -> Option<Arc<RootCertStore>> {
        let cache = self.entry.lock().unwrap_or_else(|poisoned| {
            warn!("native roots cache lock was poisoned; clearing the entry and reloading");
            let mut guard = poisoned.into_inner();
            *guard = None;
            self.entry.clear_poison();
            guard
        });
        cache
            .as_ref()
            .filter(|cached| cached.source_key == source_key && cached.expires_at > now())
            .map(|cached| Arc::clone(&cached.roots))
    }

    pub(crate) fn load(
        &self,
        source_key: NativeRootsSourceKey,
        load_native_roots: impl FnOnce() -> rustls_native_certs::CertificateResult,
        now: impl Fn() -> Instant,
    ) -> Arc<RootCertStore> {"""
assert old in text
p.write_text(text.replace(old, new))
print("cache peek added")

# 2) Rewrite the reqwest no-CA branch to use warm roots only.
p = pathlib.Path("http-client/src/custom_ca.rs")
text = p.read_text()
old = """    // Every reqwest client otherwise reloads the platform trust store at
    // build time (on macOS a keychain round trip that can cost hundreds of
    // milliseconds), which starves startup-deadline budgets when several
    // route pools build their first client. Route the no-override path
    // through the shared bounded-staleness native-roots cache instead: the
    // first build pays the platform load once and later builds reuse it.
    let config = build_rustls_client_config_with_env(env_source)?;
    builder = builder.use_rustls_tls().use_preconfigured_tls(config.as_ref().clone());

    match builder.build() {"""
new = """    // Every reqwest build otherwise reloads the platform trust store (on
    // macOS a keychain round trip that can cost hundreds of milliseconds),
    // which starves startup-deadline budgets when several route pools build
    // their first client. When the shared native-roots cache is already
    // warm, hand reqwest that configuration so this build skips the reload.
    // A cold cache keeps the ordinary first-build behavior: the first build
    // must not pay extra work on top of the platform load.
    if let Some(roots) = PRODUCTION_NATIVE_ROOTS_CACHE.peek(
        NativeRootsSourceKey::from_env_values(
            env_source.var_os(SSL_CERT_FILE_ENV),
            env_source.var_os(SSL_CERT_DIR_ENV),
        ),
        Instant::now,
    ) {
        ensure_rustls_crypto_provider();
        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        builder = builder.use_rustls_tls().use_preconfigured_tls(config);
    }

    match builder.build() {"""
assert old in text
p.write_text(text.replace(old, new))
print("warm-only preconfigured branch installed")
