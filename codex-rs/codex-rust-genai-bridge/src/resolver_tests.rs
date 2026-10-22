use super::*;
use codex_api::AuthProvider;
use codex_api::RetryConfig;
use std::sync::Arc;
use std::time::Duration;

struct UnavailableAuth;

impl AuthProvider for UnavailableAuth {
    fn add_auth_headers(&self, headers: &mut HeaderMap) -> Result<(), AuthError> {
        // A provider may have populated some headers before discovering it
        // cannot complete auth. Neither caller may treat those as success.
        headers.insert("x-partial-auth", http::HeaderValue::from_static("fixture"));
        Err(AuthError::Transient(
            "fixture credentials unavailable".into(),
        ))
    }
}

fn provider() -> Provider {
    Provider {
        name: "auth failure fixture".into(),
        base_url: "http://127.0.0.1:9/v1".into(),
        query_params: None,
        headers: HeaderMap::new(),
        retry: RetryConfig {
            max_attempts: 1,
            base_delay: Duration::ZERO,
            retry_429: false,
            retry_5xx: false,
            retry_transport: false,
        },
        stream_idle_timeout: Duration::from_secs(1),
        max_output_tokens: None,
        hosted_results_replay: None,
    }
}

#[test]
fn auth_failure_is_not_an_unauthenticated_genai_client() {
    let auth: SharedAuthProvider = Arc::new(UnavailableAuth);
    let result = build_genai_client(&provider(), &auth, AdapterKind::OpenAI);
    assert!(matches!(
        result,
        Err(AuthError::Transient(message)) if message == "fixture credentials unavailable"
    ));
}

#[test]
fn auth_failure_is_not_a_partial_extra_header_map() {
    let auth: SharedAuthProvider = Arc::new(UnavailableAuth);
    let result = build_extra_headers(&provider(), &auth, &HeaderMap::new());
    assert!(matches!(
        result,
        Err(AuthError::Transient(message)) if message == "fixture credentials unavailable"
    ));
}
