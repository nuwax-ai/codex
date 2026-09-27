use super::error_tests::provider;
use super::support;
use codex_api::ResponseEvent;
use codex_api::SharedAuthProvider;
use codex_rust_rig_bridge::RigProtocol;
use codex_rust_rig_bridge::stream_via_rig;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn provider_correlation_headers_preserve_priority_and_response_identity() {
    let cases = [
        (vec![("X-Trace-Id", "step-trace")], Some("step-trace")),
        (vec![("X-LOG-ID", "glm-log")], Some("glm-log")),
        (vec![("request-id", "anthropic-id")], Some("anthropic-id")),
        (
            vec![
                ("X-Request-ID", "primary-id"),
                ("request-id", "anthropic-id"),
                ("X-Trace-Id", "step-trace"),
                ("X-LOG-ID", "glm-log"),
            ],
            Some("primary-id"),
        ),
        (
            vec![("request-id", "anthropic-id"), ("X-Trace-Id", "step-trace")],
            Some("anthropic-id"),
        ),
        (
            vec![("X-Trace-Id", "step-trace"), ("X-LOG-ID", "glm-log")],
            Some("step-trace"),
        ),
        (
            vec![("x-request-id", ""), ("X-Trace-Id", "step-trace")],
            Some("step-trace"),
        ),
        (vec![], None),
    ];
    for protocol in [RigProtocol::Chat, RigProtocol::Anthropic] {
        for (headers, expected) in &cases {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let provider = provider(listener.local_addr().unwrap());
            let response_headers = headers.clone();
            let (payload, response_id) = match protocol {
                RigProtocol::Chat => (support::CHAT_SSE, "chatcmpl-test"),
                RigProtocol::Anthropic => (support::ANTHROPIC_SSE, "msg-test"),
            };
            let server = tokio::spawn(async move {
                support::serve_payload_with_headers(&listener, payload, &response_headers).await
            });
            let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
            let mut stream = stream_via_rig(
                &support::request(vec![support::user()]),
                &provider,
                &auth,
                http::HeaderMap::new(),
                protocol,
                Duration::from_secs(3),
            )
            .await
            .unwrap();
            let mut completed_ids = Vec::new();
            while let Some(event) = stream.next().await {
                if let ResponseEvent::Completed { response_id, .. } = event.unwrap() {
                    completed_ids.push(response_id);
                }
            }
            tokio::time::timeout(Duration::from_secs(3), server)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                (stream.upstream_request_id.as_deref(), completed_ids),
                (*expected, vec![response_id.to_string()]),
                "protocol={protocol:?} headers={headers:?}"
            );
        }
    }
}
