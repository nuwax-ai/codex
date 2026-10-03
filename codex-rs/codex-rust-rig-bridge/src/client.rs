//! rig client construction from Codex's provider/auth configuration.

use codex_api::Provider;
use http::HeaderMap;
// rig 0.42 embeds a reqwest 0.13 transport; the workspace pins 0.12 for the
// rest of codex. Depend on rig's major explicitly (renamed) so the two never
// share a type — features unify with rig-core's defaults (rustls TLS).
use reqwest_rig as reqwest13;
use rig_core::client::CompletionClient;

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
    let build = || {
        let mut builder = http_client_builder();
        if let Some(config) = custom_ca.as_ref() {
            builder = builder.tls_backend_preconfigured((**config).clone());
        }
        builder.build().map_err(|e| {
            codex_api::ApiError::Transport(codex_api::TransportError::Network(format!(
                "rig http client build failed: {e}"
            )))
        })
    };
    if custom_ca.is_some() {
        return build();
    }
    Ok((*pooled_http_client(protocol)?).clone())
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
    let client = std::sync::Arc::new(http_client_builder().build().map_err(|error| {
        codex_api::ApiError::Transport(codex_api::TransportError::Network(format!(
            "rig http client build failed: {error}"
        )))
    })?);
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

pub(crate) type RigChatModel =
    rig_core::providers::openai::completion::CompletionModel<crate::transport::RigHttpClient>;

pub(crate) type RigAnthropicModel =
    rig_core::providers::anthropic::completion::CompletionModel<crate::transport::RigHttpClient>;

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
    url.set_query(None);
    url.set_fragment(None);
    Ok((url.to_string().trim_end_matches('/').to_string(), query))
}

/// Compatibility entry point for callers without authentication-domain evidence.
pub fn reasoning_source(
    provider: &Provider,
    protocol: RigProtocol,
    model: &str,
) -> Result<String, codex_api::ApiError> {
    reasoning_source_with_auth_domain(provider, protocol, model, Some("legacy-unscoped"))
}

/// Binds opaque replay to a non-secret account or configuration selector.
/// Missing evidence gets a request-local nonce, so it can never authorize replay
/// captured by another request. The legacy wrapper explicitly opts into its old scope.
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
) -> Result<RigChatModel, codex_api::ApiError> {
    let (api_key, wire_auth) = crate::wire_auth::resolved_sdk_auth(headers, RigProtocol::Chat)?;
    http.wire_auth = Some(wire_auth);
    let client = rig_core::providers::openai::CompletionsClient::builder()
        .api_key(api_key)
        .base_url(base_url)
        .http_client(http)
        .build()
        .map_err(map_client_error)?;
    Ok(client.completion_model(model_name))
}

/// rig's OpenAI client speaking the Responses wire. The model capability is
/// unused by the passthrough path — it serializes Codex's
/// `ResponsesApiRequest` directly and drives `post_sse` + `send_streaming`
/// on the returned client — but constructing the client through rig keeps
/// URI joining, Bearer auth, and provider customization in rig's hands.
pub(crate) fn build_responses_client(
    base_url: &str,
    headers: &HeaderMap,
    mut http: crate::transport::RigHttpClient,
) -> Result<rig_core::providers::openai::Client<crate::transport::RigHttpClient>, codex_api::ApiError>
{
    let (api_key, wire_auth) =
        crate::wire_auth::resolved_sdk_auth(headers, RigProtocol::Responses)?;
    http.wire_auth = Some(wire_auth);
    rig_core::providers::openai::Client::builder()
        .api_key(api_key)
        .base_url(base_url)
        .http_client(http)
        .build()
        .map_err(map_client_error)
}

pub(crate) fn build_anthropic_model(
    model_name: &str,
    base_url: &str,
    headers: &HeaderMap,
    mut http: crate::transport::RigHttpClient,
) -> Result<RigAnthropicModel, codex_api::ApiError> {
    let (api_key, wire_auth) =
        crate::wire_auth::resolved_sdk_auth(headers, RigProtocol::Anthropic)?;
    http.wire_auth = Some(wire_auth);
    let mut builder = rig_core::providers::anthropic::Client::builder()
        .api_key(api_key)
        .base_url(base_url)
        .http_client(http);
    if let Some(version) = headers.get("anthropic-version") {
        let version = version
            .to_str()
            .map_err(|_| codex_api::ApiError::InvalidRequest {
                message: "Invalid anthropic-version header: expected an ASCII value".into(),
            })?;
        builder = builder.anthropic_version(version);
    }
    let client = builder.build().map_err(map_client_error)?;
    Ok(client.completion_model(model_name))
}

fn map_client_error(e: rig_core::http_client::Error) -> codex_api::ApiError {
    codex_api::ApiError::Transport(codex_api::TransportError::Network(format!(
        "rig client build failed: {e}"
    )))
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "client_retry_tests.rs"]
mod client_retry_tests;
