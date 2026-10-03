use super::*;
use pretty_assertions::assert_eq;
use std::time::Duration;

fn provider(base_url: &str) -> Provider {
    Provider {
        name: "test".into(),
        base_url: base_url.into(),
        query_params: None,
        headers: HeaderMap::new(),
        retry: codex_api::RetryConfig {
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
fn legacy_protocol_detection_uses_only_the_url_path() {
    for (url, expected) in [
        ("https://api.example/anthropic/v1", RigProtocol::Anthropic),
        (
            "https://api.example/v1?next=/anthropic/v1",
            RigProtocol::Chat,
        ),
        ("https://api.example/v1#docs/anthropic", RigProtocol::Chat),
        ("https://anthropic:token@api.example/v1", RigProtocol::Chat),
    ] {
        assert_eq!(RigProtocol::from_base_url(url), expected, "{url}");
        assert_eq!(protocol_for_base_url(url), expected, "{url}");
    }
}

#[test]
fn reasoning_source_includes_query_routing_without_persisting_credentials() {
    let a = provider("https://host.test/v1?tenant=A&tenant=B");
    let b = provider("https://host.test/v1?tenant=B&tenant=A");
    let a_key = reasoning_source(&a, RigProtocol::Anthropic, "model").unwrap();
    assert_ne!(
        a_key,
        reasoning_source(&b, RigProtocol::Anthropic, "model").unwrap()
    );
    let mut keyed = a;
    keyed.query_params = Some(
        [("token".into(), "secret-should-not-persist".into())]
            .into_iter()
            .collect(),
    );
    let key = reasoning_source(&keyed, RigProtocol::Anthropic, "model").unwrap();
    assert_eq!(key, a_key);
    // Credentials never enter source identity, including as a hash. An auth
    // selector/account domain must separately partition authenticated replay.
    keyed
        .query_params
        .as_mut()
        .unwrap()
        .insert("token".into(), "rotated-secret".into());
    assert_eq!(
        key,
        reasoning_source(&keyed, RigProtocol::Anthropic, "model").unwrap()
    );
    assert!(!key.contains("secret-should-not-persist"));
    assert_eq!(
        key,
        reasoning_source(&keyed, RigProtocol::Anthropic, "model").unwrap()
    );
}

/// N4: the provenance identity participates in replay decisions per
/// dimension — an envelope captured under one identity never replays onto a
/// request with another, whatever the crossing axis. Same endpoint + model
/// + protocol stays replayable.
#[test]
fn provenance_identity_partitions_replay_by_endpoint_model_and_protocol() {
    use crate::hosted_replay::envelope;
    use crate::hosted_replay::parse_envelope;
    use crate::hosted_replay::replayable;

    let capture = provider("https://capture.test/v1");
    let captured_source = reasoning_source(&capture, RigProtocol::Anthropic, "model-a").unwrap();
    let payload = envelope(
        &captured_source,
        vec![serde_json::json!({
            "type": "server_tool_use", "id": "s1",
            "name": "web_search", "input": {"query": "q"},
        })],
        vec![0],
        "response",
        None,
    );
    let parsed = parse_envelope(&payload).unwrap();

    // The identical identity replays.
    let same = reasoning_source(
        &provider("https://capture.test/v1"),
        RigProtocol::Anthropic,
        "model-a",
    )
    .unwrap();
    assert!(replayable(&parsed, &same));

    // Model switch on the same gateway: not replayable.
    let other_model = reasoning_source(&capture, RigProtocol::Anthropic, "model-b").unwrap();
    assert_ne!(same, other_model);
    assert!(!replayable(&parsed, &other_model));

    // Endpoint switch with the same model: not replayable.
    let other_endpoint = reasoning_source(
        &provider("https://other.test/v1"),
        RigProtocol::Anthropic,
        "model-a",
    )
    .unwrap();
    assert_ne!(same, other_endpoint);
    assert!(!replayable(&parsed, &other_endpoint));

    // Protocol switch over the same endpoint+model: not replayable.
    let other_protocol = reasoning_source(&capture, RigProtocol::Chat, "model-a").unwrap();
    assert_ne!(same, other_protocol);
    assert!(!replayable(&parsed, &other_protocol));
}

#[tokio::test]
async fn query_credentials_are_redacted_from_transport_errors() {
    use rig_core::http_client::HttpClientExt;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let client = crate::transport::RigHttpClient {
        inner: reqwest_rig::Client::builder().no_proxy().build().unwrap(),
        query: vec![("api-key".into(), "dummy-query-secret".into())],
        ..Default::default()
    };
    let request = http::Request::post(format!("http://{address}/v1/chat/completions"))
        .body(bytes::Bytes::new())
        .unwrap();
    let error = match client.send_streaming(request).await {
        Ok(_) => panic!("expected refused connection"),
        Err(error) => error,
    };
    assert!(!format!("{error:?} {error}").contains("dummy-query-secret"));
}

// The production pool stores no per-turn headers or credentials.
#[test]
fn shared_http_client_pool_reuses_production_clients() {
    for protocol in [
        RigProtocol::Chat,
        RigProtocol::Anthropic,
        RigProtocol::Responses,
    ] {
        let first = pooled_http_client(protocol).unwrap();
        let second = pooled_http_client(protocol).unwrap();
        assert!(std::sync::Arc::ptr_eq(&first, &second));
    }
}

#[test]
fn provenance_auth_domains_partition_replay_without_hashing_keys() {
    let provider = provider("https://capture.test/v1");
    let original = reasoning_source_with_auth_domain(
        &provider,
        RigProtocol::Anthropic,
        "model",
        Some("env:KEY_A"),
    )
    .unwrap();
    let other = reasoning_source_with_auth_domain(
        &provider,
        RigProtocol::Anthropic,
        "model",
        Some("env:KEY_B"),
    )
    .unwrap();
    let unknown = reasoning_source(&provider, RigProtocol::Anthropic, "model").unwrap();
    assert_ne!(original, other);
    assert_ne!(original, unknown);
    assert_eq!(
        original,
        reasoning_source_with_auth_domain(
            &provider,
            RigProtocol::Anthropic,
            "model",
            Some("env:KEY_A")
        )
        .unwrap()
    );
}

#[test]
fn missing_actual_auth_domain_cannot_authorize_another_request_replay() {
    let provider = provider("https://capture.test/v1");
    let a = reasoning_source_with_auth_domain(&provider, RigProtocol::Anthropic, "model", None)
        .unwrap();
    let b = reasoning_source_with_auth_domain(&provider, RigProtocol::Anthropic, "model", None)
        .unwrap();
    assert_ne!(a, b);
    assert_eq!(
        reasoning_source(&provider, RigProtocol::Anthropic, "model").unwrap(),
        reasoning_source(&provider, RigProtocol::Anthropic, "model").unwrap()
    );
}

#[test]
fn resolved_sdk_auth_validates_selected_keys_and_preserves_explicit_empty_priority() {
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_static("Bearer real-bearer"),
    );
    headers.insert("api-key", http::HeaderValue::from_static("alternate-key"));
    headers.insert("x-api-key", http::HeaderValue::from_static(""));
    let (key, auth) = crate::wire_auth::resolved_sdk_auth(&headers, RigProtocol::Anthropic)
        .expect("explicit empty key");
    assert_eq!(key, "");
    assert_eq!(
        auth.headers,
        [
            (
                http::header::AUTHORIZATION,
                http::HeaderValue::from_static("Bearer real-bearer")
            ),
            (
                http::HeaderName::from_static("x-api-key"),
                http::HeaderValue::from_static("")
            )
        ]
        .into_iter()
        .collect::<HeaderMap>()
    );
    headers.insert(
        "x-api-key",
        http::HeaderValue::from_bytes(b"\xffprivate-secret").expect("opaque key"),
    );
    let error = match crate::wire_auth::resolved_sdk_auth(&headers, RigProtocol::Anthropic) {
        Ok(_) => panic!("an invalid explicit key cannot select the bearer fallback"),
        Err(error) => error,
    };
    assert!(
        matches!(error, codex_api::ApiError::InvalidRequest { message }
        if message == "Invalid x-api-key header: the Rig SDK requires an ASCII credential value" && !message.contains("private-secret"))
    );
}

#[test]
fn anonymous_wire_auth_is_empty_and_bearer_translation_remains_protocol_specific() {
    for protocol in [
        RigProtocol::Chat,
        RigProtocol::Anthropic,
        RigProtocol::Responses,
    ] {
        let (key, auth) = crate::wire_auth::resolved_sdk_auth(&HeaderMap::new(), protocol)
            .expect("anonymous credentials");
        assert_eq!((key, auth.headers), (String::new(), HeaderMap::new()));
    }
    let headers = [(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_static("bearer model-key"),
    )]
    .into_iter()
    .collect();
    for (protocol, name, value) in [
        (RigProtocol::Chat, "authorization", "Bearer model-key"),
        (RigProtocol::Anthropic, "x-api-key", "model-key"),
        (RigProtocol::Responses, "authorization", "bearer model-key"),
    ] {
        let (key, auth) =
            crate::wire_auth::resolved_sdk_auth(&headers, protocol).expect("bearer credentials");
        assert_eq!(
            (key, auth.headers),
            (
                "model-key".to_string(),
                [(
                    http::HeaderName::from_static(name),
                    http::HeaderValue::from_static(value)
                )]
                .into_iter()
                .collect::<HeaderMap>()
            )
        );
    }
}

#[test]
fn opaque_authorization_remains_passthrough_without_an_invented_model_key() {
    let opaque = http::HeaderValue::from_bytes(b"Legacy \x80private-credential")
        .expect("obs-text authorization");
    let headers = [(http::header::AUTHORIZATION, opaque.clone())]
        .into_iter()
        .collect();
    for protocol in [
        RigProtocol::Chat,
        RigProtocol::Anthropic,
        RigProtocol::Responses,
    ] {
        let (key, auth) = crate::wire_auth::resolved_sdk_auth(&headers, protocol)
            .expect("opaque authorization passthrough");
        assert_eq!(
            (key, auth.headers),
            (
                String::new(),
                [(http::header::AUTHORIZATION, opaque.clone())]
                    .into_iter()
                    .collect::<HeaderMap>()
            )
        );
    }
}
