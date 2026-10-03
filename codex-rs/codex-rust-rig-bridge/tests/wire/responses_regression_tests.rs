use super::error_tests::provider;
use super::support;
use codex_api::ApiError;
use codex_api::AuthProvider;
use codex_api::ResponseEvent;
use codex_api::SharedAuthProvider;
use codex_api::TransportError;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::ModelVerification;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::RateLimitWindow;
use codex_protocol::protocol::TurnModerationMetadataEvent;
use codex_rust_rig_bridge::RigProtocol;
use codex_rust_rig_bridge::stream_via_rig;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

struct HeaderAuth(http::HeaderMap);

impl AuthProvider for HeaderAuth {
    fn add_auth_headers(&self, headers: &mut http::HeaderMap) {
        headers.extend(self.0.clone());
    }
}

#[tokio::test]
async fn responses_preserve_absent_and_non_bearer_authorization() {
    for auth_headers in [
        Vec::new(),
        vec![("api-key", "local-api-key")],
        vec![
            ("authorization", "Basic bG9jYWw6dGVzdA=="),
            ("api-key", "local-api-key"),
        ],
    ] {
        let mut headers = http::HeaderMap::new();
        for &(name, value) in &auth_headers {
            headers.insert(name, http::HeaderValue::from_static(value));
        }
        let (wire, _, _) = support::capture_with_auth(
            &support::request(vec![support::user()]),
            RigProtocol::Responses,
            Arc::new(HeaderAuth(headers)),
        )
        .await;
        let actual: BTreeMap<_, _> = wire["headers"]
            .as_str()
            .expect("request headers")
            .lines()
            .filter_map(|line| line.split_once(':'))
            .filter(|(name, _)| {
                name.eq_ignore_ascii_case("authorization") || name.eq_ignore_ascii_case("api-key")
            })
            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        let expected: BTreeMap<_, _> = auth_headers
            .into_iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            actual, expected,
            "the SDK must not invent Bearer credentials"
        );
    }
}

#[tokio::test]
async fn responses_preserve_raw_tool_schema_bytes_with_and_without_projection() {
    // Neither Value's key ordering nor its number representation can preserve
    // this schema. Compare the complete HTTP body before parsing it as JSON.
    let tools = r#"[ { "type" : "function", "name" : "lookup", "parameters" : {
        "type" : "object", "properties" : { "z" : { "const" : 18446744073709551617 }, "a" : { "type" : "string" } }
    } } ]"#;
    for encrypted_content in [
        "native-ciphertext",
        "codex-rig-reasoning-v1:{\"source\":\"chat-wire\",\"blocks\":[]}",
    ] {
        let mut request = support::request(vec![
            support::user(),
            json!({
                "type":"reasoning", "id":"rsn_saved", "summary":[],
                "content":[{"type":"reasoning_text","text":"prior reasoning"}],
                "encrypted_content":encrypted_content
            }),
        ]);
        let raw: Arc<serde_json::value::RawValue> = Arc::from(
            serde_json::value::RawValue::from_string(tools.to_string()).expect("raw tool schema"),
        );
        request.tools = Some(raw.into());
        request.max_output_tokens = Some(4096);
        let original = serde_json::to_vec(&request).expect("original request");
        let mut projected = request.clone();
        if encrypted_content.starts_with(codex_rust_rig_bridge::REPLAY_PREFIX)
            && let ResponseItem::Reasoning {
                encrypted_content, ..
            } = &mut projected.input[1]
        {
            *encrypted_content = None;
        }
        let expected = serde_json::to_vec(&projected).expect("projected request");
        let (wire, _, _) = support::capture(&request, RigProtocol::Responses).await;
        assert_eq!(
            wire["body_raw"]
                .as_str()
                .expect("raw request body")
                .as_bytes(),
            expected.as_slice(),
            "the complete request must retain schema bytes and field order"
        );
        assert_eq!(
            serde_json::to_vec(&request).expect("unchanged request"),
            original,
            "projection must leave the caller's request intact"
        );
    }
}

async fn response_events(
    payload: String,
    headers: Vec<(&'static str, &'static str)>,
) -> Vec<Result<ResponseEvent, ApiError>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let provider = provider(listener.local_addr().expect("address"));
    let server = tokio::spawn(async move {
        support::serve_payload_with_headers(&listener, &payload, &headers).await
    });
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let mut stream = stream_via_rig(
        &support::request(vec![support::user()]),
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Responses,
        Duration::from_secs(1),
    )
    .await
    .expect("start responses stream");
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    server.await.expect("server task");
    events
}

#[tokio::test]
async fn responses_preserve_header_and_sse_metadata_events() {
    let metadata = json!({
        "type":"response.metadata",
        "headers":{"openai-model":"metadata-model"},
        "metadata":{
            "openai_verification_recommendation":["trusted_access_for_cyber"],
            "openai_chatgpt_moderation_metadata":{"presentation":"inline"}
        }
    });
    let payload = format!("data: {metadata}\n\n{}", support::RESPONSES_SSE);
    let events = response_events(
        payload,
        vec![
            ("openai-model", "header-model"),
            ("x-codex-primary-used-percent", "25"),
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-primary-reset-at", "1800000000"),
            ("x-models-etag", "catalog-revision"),
            ("x-reasoning-included", "true"),
        ],
    )
    .await;
    let metadata_events: Vec<_> = events
        .into_iter()
        .map(|event| event.expect("metadata stream event"))
        .filter(|event| {
            matches!(
                event,
                ResponseEvent::ServerModel(_)
                    | ResponseEvent::RateLimits(_)
                    | ResponseEvent::ModelsEtag(_)
                    | ResponseEvent::ServerReasoningIncluded(_)
                    | ResponseEvent::ModelVerifications(_)
                    | ResponseEvent::TurnModerationMetadata(_)
            )
        })
        .collect();
    let expected = vec![
        ResponseEvent::ServerModel("header-model".into()),
        ResponseEvent::RateLimits(RateLimitSnapshot {
            limit_id: Some("codex".into()),
            limit_name: None,
            normal_model_slug: None,
            primary: Some(RateLimitWindow {
                used_percent: 25.0,
                window_minutes: Some(300),
                resets_at: Some(1_800_000_000),
            }),
            secondary: None,
            credits: None,
            individual_limit: None,
            spend_control_reached: None,
            plan_type: None,
            rate_limit_reached_type: None,
        }),
        ResponseEvent::ModelsEtag("catalog-revision".into()),
        ResponseEvent::ServerReasoningIncluded(true),
        ResponseEvent::ServerModel("metadata-model".into()),
        ResponseEvent::ModelVerifications(vec![ModelVerification::TrustedAccessForCyber]),
        ResponseEvent::TurnModerationMetadata(TurnModerationMetadataEvent {
            metadata: json!({"presentation":"inline"}),
        }),
        ResponseEvent::ServerModel("server-model".into()),
    ];
    assert_eq!(
        serde_json::to_value(metadata_events).expect("actual metadata"),
        serde_json::to_value(expected).expect("expected metadata")
    );

    // A named quota without numeric windows is still server metadata.
    let named_quota = response_events(
        support::RESPONSES_SSE.to_string(),
        vec![("x-codex-limit-name", "Team quota")],
    )
    .await
    .into_iter()
    .filter_map(|event| match event.expect("named quota event") {
        ResponseEvent::RateLimits(snapshot) => Some(snapshot),
        _ => None,
    })
    .collect::<Vec<_>>();
    assert_eq!(
        named_quota,
        vec![RateLimitSnapshot {
            limit_id: Some("codex".into()),
            limit_name: Some("Team quota".into()),
            normal_model_slug: None,
            primary: None,
            secondary: None,
            credits: None,
            individual_limit: None,
            spend_control_reached: None,
            plan_type: None,
            rate_limit_reached_type: None,
        }]
    );
}

#[tokio::test]
async fn responses_time_out_when_server_never_sends_headers() {
    // Client initialization is outside the HTTP header deadline under test.
    // Warm the shared client before putting a guard around the stalled request.
    support::capture(
        &support::request(vec![support::user()]),
        RigProtocol::Responses,
    )
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let provider = provider(listener.local_addr().expect("address"));
    let (release, hold_connection) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut hold_connection = hold_connection;
        let accepted = tokio::select! {
            result = listener.accept() => result,
            _ = &mut hold_connection => return,
        };
        let (socket, _) = accepted.expect("accept");
        // A header timeout can cancel the upload before the body is complete.
        // Hold the accepted connection without assuming a complete request.
        hold_connection.await.expect("release stalled server");
        drop(socket);
    });
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        stream_via_rig(
            &support::request(vec![support::user()]),
            &provider,
            &auth,
            http::HeaderMap::new(),
            RigProtocol::Responses,
            Duration::from_millis(100),
        ),
    )
    .await;
    release.send(()).expect("release server");
    server.await.expect("server held the accepted connection");
    assert!(matches!(
        result.expect("stream start must honor the shorter idle timeout"),
        Err(ApiError::Transport(TransportError::Timeout))
    ));
}

#[tokio::test]
async fn responses_terminal_failures_reject_later_completion() {
    for failure in [
        json!({"type":"response.failed","response":{"id":"resp-test","error":{"code":"context_length_exceeded","message":"too long"}}}),
        json!({"type":"response.incomplete","response":{"id":"resp-test","incomplete_details":{"reason":"max_output_tokens"}}}),
        json!({"type":"error","code":"server_error","message":"gateway failure"}),
    ] {
        let payload = format!("data: {failure}\n\n{}", support::RESPONSES_SSE);
        let events = response_events(payload, Vec::new()).await;
        assert!(
            events
                .iter()
                .all(|event| !matches!(event, Ok(ResponseEvent::Completed { .. }))),
            "terminal failure must not be followed by success: {events:?}"
        );
        let errors: Vec<_> = events.into_iter().filter_map(Result::err).collect();
        match failure["type"].as_str().expect("failure type") {
            "response.failed" => {
                assert!(matches!(
                    errors.as_slice(),
                    [ApiError::ContextWindowExceeded]
                ));
            }
            "response.incomplete" => {
                assert!(
                    matches!(errors.as_slice(), [ApiError::InvalidRequest { message }] if message.contains("max_output_tokens"))
                );
            }
            "error" => assert_eq!(
                errors.len(),
                1,
                "top-level errors must terminate the stream"
            ),
            other => panic!("unexpected failure fixture: {other}"),
        }
    }
}
