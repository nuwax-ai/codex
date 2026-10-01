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
    assert_ne!(key, a_key);
    assert!(!key.contains("secret-should-not-persist"));
    assert_eq!(
        key,
        reasoning_source(&keyed, RigProtocol::Anthropic, "model").unwrap()
    );
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

// Fork: the shared connection pool must hand the same client (and therefore
// reqwest connection pool) to every turn with identical static headers,
// while a different header set gets its own client.
#[test]
fn shared_http_client_pool_reuses_clients_per_header_fingerprint() {
    use crate::client::RigProtocol;
    use http::HeaderMap;

    let mut headers = HeaderMap::new();
    headers.insert("anthropic-version", "2023-06-01".parse().unwrap());
    let first = crate::client::shared_client_for_test(&headers, RigProtocol::Anthropic)
        .expect("first client");
    let second = crate::client::shared_client_for_test(&headers, RigProtocol::Anthropic)
        .expect("second client");
    assert!(
        std::sync::Arc::ptr_eq(&first, &second),
        "identical headers must reuse the pooled client"
    );

    let mut other = headers.clone();
    other.insert("x-gateway", "other".parse().unwrap());
    let third = crate::client::shared_client_for_test(&other, RigProtocol::Anthropic)
        .expect("third client");
    assert!(
        !std::sync::Arc::ptr_eq(&first, &third),
        "different headers must get a distinct client"
    );
}
