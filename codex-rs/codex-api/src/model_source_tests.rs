use super::*;
use pretty_assertions::assert_eq;

fn provider(base_url: &str) -> Provider {
    Provider {
        name: "test".into(),
        base_url: base_url.into(),
        query_params: None,
        headers: http::HeaderMap::new(),
        retry: crate::RetryConfig {
            max_attempts: 1,
            base_delay: std::time::Duration::ZERO,
            retry_429: false,
            retry_5xx: false,
            retry_transport: false,
        },
        stream_idle_timeout: std::time::Duration::from_secs(1),
        max_output_tokens: None,
        hosted_results_replay: None,
    }
}

#[test]
fn endpoint_identity_normalizes_origin_but_preserves_routing() {
    let a = provider("https://HOST.test:443/v1/?tenant=A");
    let b = provider("https://host.test/v1?tenant=A#ignored");
    assert_eq!(model_endpoint_identity(&a), model_endpoint_identity(&b));
    assert_ne!(
        model_endpoint_identity(&a),
        model_endpoint_identity(&provider("https://host.test/v1?tenant=B"))
    );
    assert_ne!(
        model_endpoint_identity(&a),
        model_endpoint_identity(&provider("https://host.test/v2?tenant=A"))
    );
}

#[test]
fn actual_query_credentials_never_enter_endpoint_hashes() {
    for name in [
        "token",
        "api-key",
        "authorization",
        "client_secret",
        "password",
        "access_key",
        "credentials",
        "cookie",
        "Cookie",
        "x-api-key",
        "X-Amz-Signature",
    ] {
        let a = provider(&format!(
            "https://host.test/v1?tenant=A&{name}=first-secret"
        ));
        let b = provider(&format!(
            "https://host.test/v1?tenant=A&{name}=rotated-secret"
        ));
        assert_eq!(
            model_endpoint_identity(&a),
            model_endpoint_identity(&b),
            "{name}"
        );
    }
    assert_eq!(
        model_endpoint_identity(&provider("https://user:secret@host.test/v1")),
        None
    );
}

#[test]
fn configured_cookie_credentials_are_excluded_without_hiding_routing_changes() {
    let mut first = provider("https://host.test/v1");
    first.query_params = Some(
        [
            ("cookie".into(), "first-private-cookie".into()),
            ("tenant".into(), "route-a".into()),
        ]
        .into(),
    );
    let mut rotated = first.clone();
    rotated
        .query_params
        .as_mut()
        .unwrap()
        .insert("cookie".into(), "different-private-cookie".into());
    assert_eq!(
        model_endpoint_identity(&first),
        model_endpoint_identity(&rotated)
    );
    rotated
        .query_params
        .as_mut()
        .unwrap()
        .insert("tenant".into(), "route-b".into());
    assert_ne!(
        model_endpoint_identity(&first),
        model_endpoint_identity(&rotated)
    );
}
