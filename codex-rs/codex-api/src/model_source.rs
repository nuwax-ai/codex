//! Credential-free normalized endpoint ownership and the shared request
//! scope policy for opaque model output.

use crate::Provider;
use sha2::Digest;

/// Identifies the normalized origin/path and the SHAPE of the routing query
/// (sorted, deduplicated names). No query VALUE ever enters this digest, so
/// an unrecognized credential name such as `sessionkey` cannot leak secret
/// bytes into persisted metadata. Value-level isolation comes from the
/// private per-request credential-instance comparison, never from this
/// persisted identity. The `endpoint-v2` tag makes every source recorded
/// under the v1 value-bearing digest fail closed after upgrade: opaque
/// payloads degrade conservatively while visible history is preserved.
pub fn model_endpoint_identity(provider: &Provider) -> Option<String> {
    let mut url = url::Url::parse(&provider.base_url).ok()?;
    if !url.username().is_empty() || url.password().is_some() {
        // Userinfo is credentials riding the URL; it must not become an
        // identity input, and providers carrying it get no endpoint identity.
        return None;
    }
    let mut names: Vec<String> = url
        .query_pairs()
        .map(|(name, _)| name.into_owned())
        .collect();
    if let Some(params) = &provider.query_params {
        names.extend(params.keys().cloned());
    }
    names.sort();
    names.dedup();
    url.set_query(None);
    url.set_fragment(None);
    let encoded = serde_json::to_vec(&(url.as_str().trim_end_matches('/'), names)).ok()?;
    Some(format!("endpoint-v2:{:x}", sha2::Sha256::digest(encoded)))
}

/// True when the provider carries any query parameter (URL-embedded or
/// configured) or URL userinfo. Every such value — recognized or not — makes
/// the request credential-scoped: only an immutable credential snapshot may
/// authorize opaque replay, and the full query is compared in the private
/// bounded cache.
pub fn provider_carries_private_query(provider: &Provider) -> bool {
    let url_info = url::Url::parse(&provider.base_url).ok();
    url_info.as_ref().is_some_and(|url| {
        !url.username().is_empty() || url.password().is_some() || url.query_pairs().next().is_some()
    }) || provider
        .query_params
        .as_ref()
        .is_some_and(|params| !params.is_empty())
}

/// Shared policy for headers that never join a request's credential scope.
/// These are static protocol labels or per-attempt telemetry metadata; their
/// variation must not rotate a credential identity. Any other header name —
/// including future gateway auth headers — is treated as credential-carrying
/// and compared privately.
pub fn is_benign_request_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "accept"
            | "content-type"
            | "user-agent"
            | "version"
            | "anthropic-version"
            | "anthropic-beta"
            | "openai-beta"
            | "originator"
            | "x-originator"
            | "x-openai-subagent"
            | "x-openai-memgen-request"
            | "x-oai-attestation"
            | "traceparent"
            | "tracestate"
            | "b3"
    ) || name.starts_with("x-b3-")
        || name.starts_with("x-codex-")
        || name.starts_with("x-openai-internal-")
}

#[cfg(test)]
#[path = "model_source_tests.rs"]
mod tests;
