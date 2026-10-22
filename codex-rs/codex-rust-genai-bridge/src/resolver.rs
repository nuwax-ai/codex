use codex_api::AuthError;
use codex_api::Provider;
use codex_api::SharedAuthProvider;
use genai::ServiceTarget;
use genai::adapter::AdapterKind;
use genai::resolver::AuthData;
use genai::resolver::Endpoint;
use http::HeaderMap;

/// Builds a rust-genai `Client` with Codex's auth and endpoint routing.
///
/// - Auth: extracts Bearer token from Codex's `SharedAuthProvider`.
/// - Endpoint: overrides the adapter default with Codex's configured `base_url`.
/// - Custom headers are passed per-request via `ChatOptions.extra_headers`.
pub fn build_genai_client(
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    adapter_kind: AdapterKind,
) -> Result<genai::Client, AuthError> {
    let mut base_url = reqwest::Url::parse(&api_provider.base_url)
        .map_err(|error| AuthError::Build(format!("invalid genai endpoint URL: {error}")))?;
    // Normalize the path before appending query parameters. Appending '/' to
    // the serialized URL would alter the last query value and leave /v1
    // vulnerable to replacement by OpenAI's Url::join("chat/completions").
    if !base_url.path().ends_with('/') {
        base_url.set_path(&format!("{}/", base_url.path()));
    }
    if let Some(params) = &api_provider.query_params
        && !params.is_empty()
    {
        base_url.query_pairs_mut().extend_pairs(params);
    }
    let has_endpoint_query = base_url.query().is_some();
    if adapter_kind == AdapterKind::Anthropic && has_endpoint_query {
        // genai's Anthropic adapter concatenates "messages" to the endpoint
        // string rather than joining a URL. Reject unsupported routing instead
        // of silently absorbing the suffix into a query value.
        return Err(AuthError::Build(
            "the genai Anthropic bridge does not support endpoint query parameters; use the rig bridge"
                .into(),
        ));
    }
    let base_url = base_url.to_string();
    let auth_headers = api_auth.to_auth_headers()?;

    let has_auth = auth_headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some();
    tracing::info!(
        has_endpoint_query,
        adapter = ?adapter_kind,
        has_authorization_header = has_auth,
        "Building genai client"
    );

    // The builder chain already yields `Result<Client, genai::Error>` —
    // map the error so this function returns `Result<Client, AuthError>`.
    genai::Client::builder()
        .with_adapter_kind(adapter_kind)
        .with_auth_resolver_fn(move |_model_iden| {
            let token = auth_headers
                .get(http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .map(|v| {
                    // Strip "Bearer " prefix — the adapter adds it back.
                    // `api_auth.to_auth_headers()` returns the full header
                    // value (e.g. "Bearer sk-xxx"), but genai adapters
                    // format!("Bearer {api_key}") when constructing the
                    // Authorization header. Without stripping we'd send
                    // "Bearer Bearer sk-xxx".
                    v.strip_prefix("Bearer ").unwrap_or(v).to_string()
                })
                .or_else(|| {
                    auth_headers
                        .get("api-key")
                        .and_then(|v| v.to_str().ok())
                        .map(|v| v.to_string())
                });

            Ok(token.map(AuthData::Key))
        })
        .with_service_target_resolver_fn(move |mut target: ServiceTarget| {
            target.endpoint = Endpoint::from_owned(base_url.clone());
            Ok(target)
        })
        // genai 0.7: `ClientBuilder::build` is fallible (zero-panic HTTP
        // client init) — surface the failure instead of `unwrap`ing.
        .build()
        .map_err(|e| {
            AuthError::Build(format!(
                "failed to build genai client for adapter {adapter_kind:?}: {e}"
            ))
        })
}

/// Builds genai `Headers` from Codex provider, auth, and extra headers.
pub fn build_extra_headers(
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: &HeaderMap,
) -> Result<genai::Headers, AuthError> {
    let mut headers: Vec<(String, String)> = Vec::new();

    // The PRIMARY Authorization header is intentionally NOT forwarded:
    // the genai adapter rebuilds it via the auth resolver, and duplicating
    // it triggers 400 rejections from reverse proxies. SECONDARY auth
    // headers (gateway OAuth cookies, api-key variants, …) MUST survive —
    // the adapter only manages the primary header.
    let auth_headers = api_auth.to_auth_headers()?;
    // genai stores one String per header name. Reject unrepresentable values
    // and repeated names within one layer instead of silently dropping data.
    // Repeated names across layers remain intentional overrides, in order:
    // provider -> auth -> extra headers.
    for (layer, skip_primary_auth) in [
        (&api_provider.headers, false),
        (&auth_headers, true),
        (extra_headers, false),
    ] {
        for (key, value) in layer.iter() {
            if layer.get_all(key).iter().nth(1).is_some() {
                return Err(AuthError::Build(
                    "the genai bridge cannot represent multiple values for one header name".into(),
                ));
            }
            let value = value.to_str().map_err(|_| {
                AuthError::Build("the genai bridge cannot represent a non-text header value".into())
            })?;
            if skip_primary_auth && key == http::header::AUTHORIZATION {
                continue;
            }
            headers.push((key.as_str().to_string(), value.to_string()));
        }
    }

    Ok(headers.into())
}

#[cfg(test)]
#[path = "resolver_tests.rs"]
mod tests;
