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

/// Only actual bearer tokens are API keys. Basic/Token gateway credentials
/// remain Authorization headers and must never be embedded in a new scheme.
fn api_key_from_headers(headers: &HeaderMap, protocol: RigProtocol) -> String {
    if protocol == RigProtocol::Anthropic
        && let Some(key) = headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
    {
        return key.to_string();
    }
    bearer_token(headers)
        .map(str::to_string)
        .or_else(|| {
            headers
                .get("api-key")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        })
        .unwrap_or_default()
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
        let mut builder = reqwest13::Client::builder();
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
    let client = std::sync::Arc::new(reqwest13::Client::builder().build().map_err(|error| {
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

pub(crate) fn reasoning_source(
    provider: &Provider,
    protocol: RigProtocol,
    model: &str,
) -> Result<String, codex_api::ApiError> {
    use sha2::Digest;
    use sha2::Sha256;
    let (base_url, query) = endpoint(&provider.base_url, provider)?;
    // Include routing query values without persisting credentials or tenant IDs.
    let encoded = serde_json::to_vec(&(base_url, query, model)).map_err(|error| {
        codex_api::ApiError::InvalidRequest {
            message: format!("Invalid reasoning source: {error}"),
        }
    })?;
    Ok(format!("{protocol:?}:{:x}", Sha256::digest(encoded)))
}

pub(crate) fn build_chat_model(
    model_name: &str,
    base_url: &str,
    headers: &HeaderMap,
    http: crate::transport::RigHttpClient,
) -> Result<RigChatModel, codex_api::ApiError> {
    let client = rig_core::providers::openai::CompletionsClient::builder()
        .api_key(api_key_from_headers(headers, RigProtocol::Chat))
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
    http: crate::transport::RigHttpClient,
) -> Result<rig_core::providers::openai::Client<crate::transport::RigHttpClient>, codex_api::ApiError>
{
    rig_core::providers::openai::Client::builder()
        .api_key(api_key_from_headers(headers, RigProtocol::Responses))
        .base_url(base_url)
        .http_client(http)
        .build()
        .map_err(map_client_error)
}

pub(crate) fn build_anthropic_model(
    model_name: &str,
    base_url: &str,
    headers: &HeaderMap,
    http: crate::transport::RigHttpClient,
) -> Result<RigAnthropicModel, codex_api::ApiError> {
    let mut builder = rig_core::providers::anthropic::Client::builder()
        .api_key(api_key_from_headers(headers, RigProtocol::Anthropic))
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
