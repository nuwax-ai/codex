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
fn endpoint_identity_normalizes_origin_and_partitions_by_query_shape() {
    let a = provider("https://HOST.test:443/v1/?tenant=A");
    let b = provider("https://host.test/v1?tenant=A#ignored");
    assert_eq!(model_endpoint_identity(&a), model_endpoint_identity(&b));
    assert_ne!(
        model_endpoint_identity(&a),
        model_endpoint_identity(&provider("https://host.test/v2?tenant=A"))
    );
    // v2: only the SHAPE of the routing surface (names) partitions. Value
    // differences route through the private credential scope instead.
    assert_eq!(
        model_endpoint_identity(&a),
        model_endpoint_identity(&provider("https://host.test/v1?tenant=B"))
    );
    assert_ne!(
        model_endpoint_identity(&a),
        model_endpoint_identity(&provider("https://host.test/v1?tenant=A&api-version=2"))
    );
}

#[test]
fn no_query_value_ever_enters_the_endpoint_hash() {
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
        // Unrecognized credential names must not leak either: the persisted
        // digest simply never contains values.
        "sessionkey",
        "sig",
        "hmac",
        "jwt",
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
    // Empty and repeated values stay out of the digest exactly like any
    // other value.
    assert_eq!(
        model_endpoint_identity(&provider("https://host.test/v1?sessionkey=")),
        model_endpoint_identity(&provider("https://host.test/v1?sessionkey=full-secret"))
    );
    assert_eq!(
        model_endpoint_identity(&provider("https://host.test/v1?a=1&a=2")),
        model_endpoint_identity(&provider("https://host.test/v1?a=9&a=8"))
    );
    assert_eq!(
        model_endpoint_identity(&provider("https://user:secret@host.test/v1")),
        None
    );
}

#[test]
fn configured_query_values_stay_out_of_the_endpoint_hash() {
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
    rotated
        .query_params
        .as_mut()
        .unwrap()
        .insert("tenant".into(), "route-b".into());
    assert_eq!(
        model_endpoint_identity(&first),
        model_endpoint_identity(&rotated)
    );
    // A new routing NAME still partitions the persisted identity.
    rotated
        .query_params
        .as_mut()
        .unwrap()
        .insert("shard".into(), "any".into());
    assert_ne!(
        model_endpoint_identity(&first),
        model_endpoint_identity(&rotated)
    );
}

#[test]
fn any_private_query_presence_marks_the_provider_private_scoped() {
    assert!(!provider_carries_private_query(&provider(
        "https://host.test/v1"
    )));
    for base_url in [
        "https://host.test/v1?sessionkey=x",
        "https://host.test/v1?api-version=2",
        "https://host.test/v1?empty=",
        "https://user:secret@host.test/v1",
    ] {
        assert!(
            provider_carries_private_query(&provider(base_url)),
            "{base_url}"
        );
    }
    let mut configured = provider("https://host.test/v1");
    configured.query_params = Some([("tenant".into(), "route".into())].into());
    assert!(provider_carries_private_query(&configured));
}

#[test]
fn benign_header_policy_covers_static_telemetry_only() {
    for name in [
        "accept",
        "Content-Type",
        "User-Agent",
        "anthropic-version",
        "anthropic-beta",
        "openai-beta",
        "originator",
        "x-originator",
        "x-openai-subagent",
        "x-openai-memgen-request",
        "x-openai-internal-codex-responses-lite",
        "x-oai-attestation",
        "traceparent",
        "tracestate",
        "b3",
        "x-b3-traceid",
        "x-codex-window-id",
        "x-codex-turn-metadata",
        "x-codex-turn-state",
        "x-codex-inference-call-id",
    ] {
        assert!(is_benign_request_header(name), "{name}");
    }
    for name in [
        "authorization",
        "x-api-key",
        "cookie",
        "sessionkey",
        "x-vendor-session",
        "proxy-authorization",
    ] {
        assert!(!is_benign_request_header(name), "{name}");
    }
}
