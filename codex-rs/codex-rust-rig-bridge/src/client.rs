//! rig client construction from Codex's provider/auth configuration.

use codex_api::Provider;
use http::HeaderMap;
use std::time::Duration;

/// Fork (2026-11-18): identify this client the way the first-party transport
/// does. Gateways that whitelist coding agents (Kimi Coding Plan returns
/// "Coding Plan is currently only available for Coding Agents") read the
/// request's identity, and the rig wires previously sent **no** `User-Agent`
/// at all, so every such gateway rejected us regardless of the protocol.
///
/// Headers the caller already set win: an operator-configured `http_headers`
/// entry is an explicit choice, not something to overwrite.
pub(crate) fn with_identity_headers(mut headers: HeaderMap) -> HeaderMap {
    for (name, value) in codex_login::default_client::default_headers() {
        if let Some(name) = name {
            headers.entry(name).or_insert(value);
        }
    }
    headers
}
// rig 0.42 embeds a reqwest 0.13 transport; the workspace pins 0.12 for the
// rest of codex. Depend on rig's major explicitly (renamed) so the two never
// share a type — features unify with rig-core's defaults (rustls TLS).
use reqwest_rig as reqwest13;

/// Which rig provider implementation serves a given base URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum RigProtocol {
    /// OpenAI Responses (`POST {base}/responses`) — same-protocol passthrough
    /// of Codex's `ResponsesApiRequest`.
    Responses,
    /// OpenAI-compatible Chat Completions.
    #[default]
    Chat,
    /// Anthropic Messages protocol (`/anthropic` gateways).
    Anthropic,
}

impl RigProtocol {
    /// URL heuristic (`/anthropic` gateways) — a fallback for callers that
    /// do not know the wire; dispatch must derive the protocol from the
    /// provider's explicit `wire_api` instead (see `stream_via_rig`).
    /// Only the URL path may select the protocol; authority, credentials,
    /// query, and fragment are ignored.
    pub fn from_base_url(base_url: &str) -> Self {
        if let Ok(url) = reqwest13::Url::parse(base_url)
            && url.path().contains("/anthropic")
        {
            return Self::Anthropic;
        }
        Self::Chat
    }
}

/// Back-compat alias for [`RigProtocol::from_base_url`].
pub fn protocol_for_base_url(base_url: &str) -> RigProtocol {
    RigProtocol::from_base_url(base_url)
}

pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(http::header::AUTHORIZATION)?.to_str().ok()?;
    value
        .split_once(' ')
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| token)
}

/// Preserves the effective request headers except the primary auth header
/// rebuilt by Rig. In Anthropic mode, a bearer token is translated to x-api-key;
/// an explicitly configured x-api-key may coexist with gateway Authorization.
/// Responses and Chat both use Rig's Bearer scheme, so they rebuild the same
/// Authorization header.
pub(crate) fn request_headers(headers: &HeaderMap, protocol: RigProtocol) -> HeaderMap {
    let mut merged = reqwest13::header::HeaderMap::new();
    for (key, value) in headers {
        let rebuilt = match protocol {
            RigProtocol::Responses | RigProtocol::Chat => key == http::header::AUTHORIZATION,
            RigProtocol::Anthropic => {
                key == "x-api-key"
                    || (key == http::header::AUTHORIZATION
                        && !headers.contains_key("x-api-key")
                        && bearer_token(headers).is_some())
            }
        };
        if !rebuilt {
            merged.insert(key.clone(), value.clone());
        }
    }
    merged
}

// One policy owner: protocol-level resends inside reqwest would bypass the
// provider budget and final-request recorder, even with zero configured retries.
fn http_client_builder() -> reqwest13::ClientBuilder {
    reqwest13::Client::builder().retry(reqwest13::retry::never())
}

/// A client whose TLS roots are the Mozilla bundle compiled into this binary.
///
/// reqwest 0.13 resolves trust roots through `rustls-platform-verifier` while
/// the client is being *built*, so a host without a platform trust store (a
/// minimal container, a trimmed distribution) fails `build()` with
/// "builder error" for every target — plain `http://` included, because the
/// client exists before a URL is known. Falling back to the compiled-in roots
/// keeps HTTPS verifying real certificates; disabling verification would hand a
/// man-in-the-middle every API key and prompt this process sends.
fn bundled_roots_builder() -> Result<reqwest13::ClientBuilder, reqwest13::Error> {
    let mut builder = http_client_builder().use_rustls_tls();
    for certificate in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        builder =
            builder.add_root_certificate(reqwest13::Certificate::from_der(certificate.as_ref())?);
    }
    Ok(builder)
}

/// A build failure is deterministic — retrying cannot conjure a trust store — so
/// it is reported as an invalid request with the fix spelled out, instead of a
/// retryable transport error that core resamples and then ends silently.
fn client_build_failure(error: impl std::fmt::Display, tried_bundled: bool) -> codex_api::ApiError {
    let detail = if tried_bundled {
        "the TLS roots compiled into this binary could not be used either"
    } else {
        "the platform TLS trust store could not be loaded"
    };
    codex_api::ApiError::InvalidRequest {
        message: format!(
            "HTTP client could not be built: {detail} ({error}). Install a CA bundle              (Debian/Ubuntu: `apt install ca-certificates`, Alpine: `apk add ca-certificates`,              RHEL: `dnf install ca-certificates`) or point SSL_CERT_FILE at a valid PEM bundle."
        ),
    }
}

pub(crate) fn http_client(protocol: RigProtocol) -> Result<reqwest13::Client, codex_api::ApiError> {
    // Headers belong to the request decorator, including gateway credentials
    // and per-turn trace IDs. Keeping them in client defaults would create a
    // permanent pool for every turn and retain old credentials. Only three
    // protocol clients are cached. Custom trust roots stay per-request.
    let custom_ca =
        codex_http_client::maybe_build_rustls_client_config_with_custom_ca().map_err(|error| {
            codex_api::ApiError::InvalidRequest {
                message: format!("Rig TLS configuration: {error}"),
            }
        })?;
    match custom_ca.as_ref() {
        // An explicit bundle is the operator's choice: never second-guess it,
        // and keep it off the shared pool because it differs per request.
        Some(config) => http_client_builder()
            .tls_backend_preconfigured((**config).clone())
            .build()
            .map_err(|error| client_build_failure(error, false)),
        None => Ok((*pooled_http_client(protocol)?).clone()),
    }
}

fn pooled_http_client(
    protocol: RigProtocol,
) -> Result<std::sync::Arc<reqwest13::Client>, codex_api::ApiError> {
    let mut cache = SHARED_HTTP_CLIENTS
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
        .lock()
        .map_err(|_| {
            codex_api::ApiError::Stream("shared rig client cache is unavailable".into())
        })?;
    if let Some(client) = cache.get(&protocol) {
        return Ok(std::sync::Arc::clone(client));
    }
    let client = std::sync::Arc::new(match http_client_builder().build() {
        Ok(client) => client,
        Err(_) => bundled_roots_builder()
            .map_err(|error| client_build_failure(error, true))?
            .build()
            .map_err(|error| client_build_failure(error, true))?,
    });
    cache.insert(protocol, std::sync::Arc::clone(&client));
    Ok(client)
}

/// At most one header-free pool for each protocol.
static SHARED_HTTP_CLIENTS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<RigProtocol, std::sync::Arc<reqwest13::Client>>>,
> = std::sync::OnceLock::new();

/// Anthropic's wire requires `max_tokens`; codex does not model an output
/// cap, so default generously (documented in the plan: revisit per model).
pub const DEFAULT_ANTHROPIC_MAX_TOKENS: u64 = 16384;

pub(crate) type RigChatModel = rig_core::driver::Model<
    rig_core::providers::openai::wire::Chat,
    crate::transport::RigHttpClient,
>;

pub(crate) type RigAnthropicModel = rig_core::driver::Model<
    rig_core::providers::anthropic::wire::Messages,
    crate::transport::RigHttpClient,
>;

/// `wire` sent through the bridge's own transport decorator, keeping the
/// decorator's budget gate, capture, and auth rewrite on every attempt.
///
/// `rig_idle_timeout` is the stall timer of rig's own driver, already adjusted
/// by [`rig_stream_idle_timeout`].
/// How long rig's own frame-idle backstop waits for a streamed reply.
///
/// rig drops a reply that sends no *frame* for this long (`ProviderError::Stalled`)
/// and defaults to five minutes, regardless of the provider's configured
/// `stream_idle_timeout`. The bridge keeps its own event-idle timer, which
/// yields the codex-native timeout. rig's stays the backstop it was: never
/// shorter than the configured wait — otherwise one configured above five
/// minutes was cut short at rig's default — and never shorter than rig's own
/// default either, so a wait configured below it still expires on the bridge's
/// timer and reports the same timeout as before.
fn rig_stream_idle_timeout(idle_timeout: Duration) -> Duration {
    idle_timeout.max(rig_core::driver::STREAM_IDLE_TIMEOUT)
}

fn model_via_bridge<W>(
    wire: W,
    transport: crate::transport::RigHttpClient,
    rig_idle_timeout: Duration,
) -> rig_core::driver::Model<W, crate::transport::RigHttpClient> {
    // 0.45 made `prepared` private state; the constructor starts it fresh,
    // which is what the direct struct literal expressed before.
    rig_core::driver::Model::new(wire, transport).with_idle_timeout(Some(rig_idle_timeout))
}

/// The SDK appends an endpoint by string concatenation. Remove query/fragment
/// first and let the transport append percent-encoded pairs to the FINAL URI.
pub(crate) fn endpoint(
    base_url: &str,
    api_provider: &Provider,
) -> Result<(String, Vec<(String, String)>), codex_api::ApiError> {
    let mut url =
        reqwest13::Url::parse(base_url).map_err(|error| codex_api::ApiError::InvalidRequest {
            message: format!("Invalid model provider URL: {error}"),
        })?;
    let mut query: Vec<_> = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if let Some(params) = &api_provider.query_params {
        let mut configured: Vec<_> = params
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        configured.sort();
        query.extend(configured);
    }
    // A `ws://`/`wss://` base URL describes the WebSocket transport, not an
    // HTTP endpoint; the bridge speaks HTTP(S) (the native transport owns the
    // ws handshake). Translate instead of handing reqwest an unspeakable
    // scheme, which would only surface as an opaque builder error later.
    match url.scheme() {
        "ws" => {
            let _ = url.set_scheme("http");
        }
        "wss" => {
            let _ = url.set_scheme("https");
        }
        _ => {}
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok((url.to_string().trim_end_matches('/').to_string(), query))
}

/// Compatibility entry point for callers without authentication-domain
/// evidence. It deliberately binds to a per-call unbound identity: without
/// domain evidence it can never prove two requests share credentials, so it
/// must not authorize replay captured by any other call. Callers that own
/// real evidence pass it via [`reasoning_source_with_auth_domain`].
pub fn reasoning_source(
    provider: &Provider,
    protocol: RigProtocol,
    model: &str,
) -> Result<String, codex_api::ApiError> {
    reasoning_source_with_auth_domain(provider, protocol, model, /*auth_domain*/ None)
}

/// Binds opaque replay to a non-secret account or configuration selector.
/// Missing evidence gets a request-local nonce, so it can never authorize replay
/// captured by another request.
pub fn reasoning_source_with_auth_domain(
    provider: &Provider,
    protocol: RigProtocol,
    model: &str,
    auth_domain: Option<&str>,
) -> Result<String, codex_api::ApiError> {
    use sha2::Digest;
    let endpoint = codex_api::model_endpoint_identity(provider);
    let auth_domain = auth_domain.filter(|_| endpoint.is_some());
    static UNKNOWN_DOMAIN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unbound;
    let auth_domain = match auth_domain {
        Some(domain) => domain,
        None => {
            let nonce = UNKNOWN_DOMAIN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos());
            unbound = format!("unbound:{}:{timestamp}:{nonce}", std::process::id());
            &unbound
        }
    };
    let endpoint = endpoint.unwrap_or_else(|| "unbound-endpoint".into());
    let encoded = serde_json::to_vec(&(endpoint, model, auth_domain)).map_err(|error| {
        codex_api::ApiError::InvalidRequest {
            message: format!("Invalid reasoning source: {error}"),
        }
    })?;
    Ok(format!("{protocol:?}:{:x}", sha2::Sha256::digest(encoded)))
}

pub(crate) fn build_chat_model(
    model_name: &str,
    base_url: &str,
    headers: &HeaderMap,
    mut http: crate::transport::RigHttpClient,
    stream_idle_timeout: Duration,
) -> Result<RigChatModel, codex_api::ApiError> {
    let (api_key, wire_auth) = crate::wire_auth::resolved_sdk_auth(headers, RigProtocol::Chat)?;
    http.wire_auth = Some(wire_auth);
    let provider = rig_core::providers::openai::OpenAIConfig::new(api_key).with_base_url(base_url);
    let wire = rig_core::providers::openai::wire::Chat {
        provider,
        model: model_name.to_string(),
        strict_tools: false,
        tool_result_array_content: false,
        // Unbound facts: rig answers model questions from its builtin
        // catalog, exactly as its own default construction does. The bridge
        // sets max_tokens itself, so no fact-driven default kicks in.
        //
        // Binding a spec here is not harmless. rig's
        // `Rewrite::OutputCapRename` renames `max_tokens` to
        // `max_completion_tokens` when `reasons(facts, model)` answers
        // `Some(true)`, and a bound spec makes it answer that for every model
        // — an OpenAI-only spelling a compatible gateway ignores, so the
        // configured output cap would silently stop applying. rig scopes the
        // rename to the model precisely because this wire reaches compatible
        // servers; leaving the facts unbound is what keeps it scoped.
        //
        // So the declared tiers do not reach rig on this wire: the caller
        // checks them itself (`reasoning_effort::check_declared`) and keeps the
        // effort as a raw body key. Same rule, same refusal point, same
        // vocabulary — FORK.md §10.
        //
        // The facts stay unbound but answer from the lenient vendored catalog:
        // `ModelFacts::default()` falls back to rig's builtin catalog, which
        // panics under this workspace's arbitrary_precision feature (191
        // unreadable vendor rows — FORK.md §7.x). `bound` staying `None`
        // preserves the per-model output-cap scoping described above.
        facts: rig_core::catalog::ModelFacts::default()
            .with_catalog(crate::reasoning_effort::nuwax_catalog()),
    };
    Ok(model_via_bridge(
        wire,
        http,
        rig_stream_idle_timeout(stream_idle_timeout),
    ))
}

/// rig's OpenAI client speaking the Responses wire. The model capability is
/// unused by the passthrough path — it serializes Codex's
/// `ResponsesApiRequest` directly and drives `post_sse` + `send_streaming`
/// on the returned client — but constructing the client through rig keeps
/// URI joining, Bearer auth, and provider customization in rig's hands.
pub(crate) fn build_responses_client(
    _base_url: &str,
    headers: &HeaderMap,
    mut http: crate::transport::RigHttpClient,
) -> Result<crate::transport::RigHttpClient, codex_api::ApiError> {
    let (api_key, wire_auth) =
        crate::wire_auth::resolved_sdk_auth(headers, RigProtocol::Responses)?;
    http.wire_auth = Some(wire_auth);
    // The passthrough no longer routes through a rig provider client: the
    // Responses wire serializes Codex's own request and drives
    // `send_streaming` on this decorator directly, so auth (bearer key),
    // query joining, and per-attempt headers stay in the bridge's hands.
    let _ = api_key;
    Ok(http)
}

/// The facts the Anthropic wire answers model questions from.
///
/// Always bound, never `ModelFacts::default()`: an unbound wire answers from
/// rig's catalog, and for a model it does not list that is the *newest*
/// models' shape — `thinking: {"type": "adaptive"}` beside the effort, which a
/// GLM or MiMo endpoint does not take. Binding a spec that starts from the
/// catalog row keeps a listed model's own facts and gives a gateway's model
/// its own shape, so the request does not change form depending on whether the
/// operator declared tiers.
///
/// `declared` is the parsed tier list; empty means the provider declares
/// nothing, and the spec stays permissive, as rig is for a model it does not
/// list. The Chat wire deliberately does not come here — see
/// [`build_chat_model`].
fn rig_facts(
    protocol: RigProtocol,
    model_name: &str,
    declared: &[rig_core::completion::Effort],
) -> rig_core::catalog::ModelFacts {
    let vendor = match protocol {
        RigProtocol::Anthropic => rig_core::providers::anthropic::PROVIDER_NAME,
        RigProtocol::Chat | RigProtocol::Responses => {
            return rig_core::catalog::ModelFacts::default();
        }
    };
    // A refused vendor name leaves the wire unbound rather than panicking; a
    // literal vendor name cannot be refused.
    crate::reasoning_effort::effort_spec(vendor, model_name, declared)
        .map(rig_core::catalog::ModelFacts::new)
        .unwrap_or_default()
}

pub(crate) fn build_anthropic_model(
    model_name: &str,
    base_url: &str,
    headers: &HeaderMap,
    mut http: crate::transport::RigHttpClient,
    prompt_caching: bool,
    declared_levels: &[rig_core::completion::Effort],
    stream_idle_timeout: Duration,
) -> Result<RigAnthropicModel, codex_api::ApiError> {
    let (api_key, wire_auth) =
        crate::wire_auth::resolved_sdk_auth(headers, RigProtocol::Anthropic)?;
    http.wire_auth = Some(wire_auth);
    let mut config =
        rig_core::providers::anthropic::wire::AnthropicConfig::new(api_key).with_base_url(base_url);
    if let Some(version) = headers.get("anthropic-version") {
        let version = version
            .to_str()
            .map_err(|_| codex_api::ApiError::InvalidRequest {
                message: "Invalid anthropic-version header: expected an ASCII value".into(),
            })?;
        config.version = version.to_string();
    }
    // The tiers this provider declares are bound to the model, so rig refuses
    // one the gateway would silently remap. They arrive parsed — the caller
    // refuses a word rig does not spell before either model is built.
    let facts = rig_facts(RigProtocol::Anthropic, model_name, declared_levels);
    let wire = rig_core::providers::anthropic::wire::Messages {
        provider: config,
        model: model_name.to_string(),
        default_max_tokens: None,
        // Provider-level opt-in (`model_providers.*.anthropic_prompt_caching`):
        // rig places the `cache_control` breakpoints (system tail, final
        // non-deferred tool, conversation tail) with the 5-minute TTL.
        prompt_caching,
        static_prefix_cache_ttl: None,
        strict_tools: false,
        // 0.42 wire parity: no per-tool `eager_input_streaming` flag. The
        // provider streams tool input as it chooses (rig's `Off`).
        tool_input_streaming: rig_core::providers::anthropic::wire::ToolInputStreaming::Off,
        facts,
    };
    Ok(model_via_bridge(
        wire,
        http,
        rig_stream_idle_timeout(stream_idle_timeout),
    ))
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "client_retry_tests.rs"]
mod client_retry_tests;

#[cfg(test)]
#[path = "client_idle_tests.rs"]
mod client_idle_tests;

#[cfg(test)]
mod tls_fallback_tests {
    use super::*;

    #[test]
    fn bundled_roots_build_a_usable_client() {
        // A host with no platform trust store depends on this path, so it must
        // produce a real client from the Mozilla roots compiled into the binary.
        let client = bundled_roots_builder()
            .expect("bundled roots configure the builder")
            .build()
            .expect("a client built from compiled-in roots");
        drop(client);
    }

    #[test]
    fn a_missing_trust_store_is_an_invalid_request_not_a_retryable_transport_error() {
        // The original defect surfaced this as a transport error, so core retried
        // five times and then ended the turn with no reply at all.
        let error = client_build_failure("builder error", true);
        match error {
            codex_api::ApiError::InvalidRequest { message } => {
                assert!(
                    message.contains("ca-certificates"),
                    "the message must name the fix: {message}"
                );
                assert!(
                    message.contains("SSL_CERT_FILE"),
                    "the message must mention the env override: {message}"
                );
                assert!(
                    message.contains("compiled into this binary"),
                    "it must say the bundled roots were tried too: {message}"
                );
            }
            other => panic!("a build failure must not be retryable, got {other:?}"),
        }
    }

    /// Opt-in: proves the compiled-in roots validate real certificates, which is
    /// the whole point of the fallback. Ignored by default to keep the suite
    /// hermetic; run with `cargo nextest run -p codex-rust-rig-bridge --run-ignored tls_fallback`.
    #[test]
    #[ignore = "network: verifies the bundled roots against a live HTTPS endpoint"]
    fn bundled_roots_validate_a_real_certificate_chain() {
        let client = bundled_roots_builder()
            .expect("builder")
            .build()
            .expect("client");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            // Any HTTP status is fine: a TLS failure would surface as an error
            // before the response is read.
            let response = client
                .get("https://token-plan-cn.xiaomimimo.com/v1/models")
                .header("Authorization", "Bearer invalid-probe")
                .send()
                .await;
            match response {
                Ok(response) => assert!(
                    response.status().is_client_error() || response.status().is_success(),
                    "unexpected status {}",
                    response.status()
                ),
                Err(error) => panic!("bundled roots failed a real handshake: {error}"),
            }
        });
    }

    #[test]
    fn the_pooled_client_builds_on_this_host() {
        // Whatever the host's trust store looks like, resolution must yield a
        // client (falling back to bundled roots) rather than an error.
        for protocol in [RigProtocol::Responses, RigProtocol::Chat] {
            let client = http_client(protocol).expect("a usable rig client");
            drop(client);
        }
    }
}
