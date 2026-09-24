//! rig client construction from Codex's provider/auth configuration.

use codex_api::Provider;
use codex_api::SharedAuthProvider;
use http::HeaderMap;
// rig 0.42 embeds a reqwest 0.13 transport; the workspace pins 0.12 for the
// rest of codex. Depend on rig's major explicitly (renamed) so the two never
// share a type — features unify with rig-core's defaults (rustls TLS).
use reqwest_rig as reqwest13;
use rig_core::client::CompletionClient;

/// Which rig provider implementation serves a given base URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RigProtocol {
    /// OpenAI-compatible Chat Completions.
    #[default]
    Chat,
    /// Anthropic Messages protocol (`/anthropic` gateways).
    Anthropic,
}

impl RigProtocol {
    /// URL heuristic (`/anthropic` gateways) — a fallback for callers that
    /// do not know the wire; prefer passing the protocol explicitly from
    /// `wire_api` (see `stream_via_rig`).
    pub fn from_base_url(base_url: &str) -> Self {
        let path = base_url.split_once("://").map_or(base_url, |(_, rest)| rest);
        if path.contains("/anthropic") {
            Self::Anthropic
        } else {
            Self::Chat
        }
    }
}

/// Back-compat alias for [`RigProtocol::from_base_url`].
pub fn protocol_for_base_url(base_url: &str) -> RigProtocol {
    RigProtocol::from_base_url(base_url)
}

/// Extracts the bearer token from Codex's auth headers (stripping the
/// `Bearer ` prefix — both rig providers re-add their own scheme).
fn api_key_from_auth(api_auth: &SharedAuthProvider) -> String {
    let headers = api_auth.to_auth_headers();
    headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.strip_prefix("Bearer ").unwrap_or(v).to_string())
        .or_else(|| {
            headers
                .get("api-key")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_string())
        })
        .unwrap_or_default()
}

/// Merges provider-level headers with per-request extras onto the HTTP
/// client's defaults. Auth headers are deliberately excluded — rig adds the
/// provider-appropriate scheme itself, and duplicating Authorization trips
/// reverse proxies that reject duplicate headers.
fn default_headers(
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: &HeaderMap,
) -> reqwest13::header::HeaderMap {
    let mut merged = reqwest13::header::HeaderMap::new();
    let mut insert = |name: &http::header::HeaderName, value: &http::HeaderValue| {
        if let Ok(v) = reqwest13::header::HeaderValue::from_bytes(value.as_bytes()) {
            merged.insert(name.clone(), v);
        }
    };
    // Gateway OAuth and other secondary auth headers MUST survive: the
    // adapter only rebuilds the PRIMARY auth header (Authorization/api-key),
    // so everything else from the auth provider is merged here. The primary
    // header itself is skipped to avoid duplicate-Authorization rejections.
    let primary = api_auth.to_auth_headers();
    let primary_authz = primary.get(http::header::AUTHORIZATION).cloned();
    for (key, value) in primary.iter() {
        if Some(value) == primary_authz.as_ref() {
            continue; // adapter rebuilds this one
        }
        insert(&key, &value);
    }
    for (key, value) in api_provider.headers.iter() {
        insert(&key, &value);
    }
    for (key, value) in extra_headers.iter() {
        insert(&key, &value);
    }
    merged
}

fn http_client(
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: &HeaderMap,
) -> Result<reqwest13::Client, codex_api::ApiError> {
    reqwest13::Client::builder()
        .default_headers(default_headers(api_provider, api_auth, extra_headers))
        .build()
        .map_err(|e| {
            codex_api::ApiError::Transport(codex_api::TransportError::Network(format!(
                "rig http client build failed: {e}"
            )))
        })
}

/// Anthropic's wire requires `max_tokens`; codex does not model an output
/// cap, so default generously (documented in the plan: revisit per model).
pub(crate) const DEFAULT_ANTHROPIC_MAX_TOKENS: u64 = 16384;

pub(crate) type RigChatModel =
    rig_core::providers::openai::completion::CompletionModel<reqwest13::Client>;

pub(crate) type RigAnthropicModel =
    rig_core::providers::anthropic::completion::CompletionModel<reqwest13::Client>;

/// Appends the provider's configured query params (api-version, tenant
/// routing, …) to the base URL so bridge-path requests keep the same
/// targeting as the native transport.
fn base_url_with_params(api_provider: &Provider, base_url: &str) -> String {
    let Some(params) = &api_provider.query_params else {
        return base_url.to_string();
    };
    if params.is_empty() {
        return base_url.to_string();
    }
    let qs = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let sep = if base_url.contains('?') { '&' } else { '?' };
    format!("{base_url}{sep}{qs}")
}

pub(crate) fn build_chat_model(
    model_name: &str,
    base_url: &str,
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: &HeaderMap,
) -> Result<RigChatModel, codex_api::ApiError> {
    let client = rig_core::providers::openai::CompletionsClient::builder()
        .api_key(api_key_from_auth(api_auth))
        .base_url(base_url_with_params(api_provider, base_url))
        .http_client(http_client(api_provider, api_auth, extra_headers)?)
        .build()
        .map_err(map_client_error)?;
    Ok(client.completion_model(model_name))
}

pub(crate) fn build_anthropic_model(
    model_name: &str,
    base_url: &str,
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: &HeaderMap,
) -> Result<RigAnthropicModel, codex_api::ApiError> {
    let client = rig_core::providers::anthropic::Client::builder()
        .api_key(api_key_from_auth(api_auth))
        .base_url(base_url_with_params(api_provider, base_url))
        .http_client(http_client(api_provider, api_auth, extra_headers)?)
        .build()
        .map_err(map_client_error)?;
    Ok(client.completion_model(model_name))
}

fn map_client_error(e: rig_core::http_client::Error) -> codex_api::ApiError {
    codex_api::ApiError::Transport(codex_api::TransportError::Network(format!(
        "rig client build failed: {e}"
    )))
}
