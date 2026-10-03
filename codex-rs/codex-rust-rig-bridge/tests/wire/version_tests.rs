use super::support;
use codex_api::AuthProvider;
use codex_rust_rig_bridge::RigProtocol;
use futures::StreamExt;
use http::HeaderMap;
use http::HeaderValue;
use pretty_assertions::assert_eq;
use std::sync::Arc;

struct VersionedAuth;

impl AuthProvider for VersionedAuth {
    fn add_auth_headers(&self, headers: &mut HeaderMap) {
        support::DummyAuth.add_auth_headers(headers);
        headers.insert("anthropic-version", HeaderValue::from_static("2025-01-01"));
    }
}

#[tokio::test]
async fn configured_anthropic_version_reaches_the_final_http_request() {
    let (wire, _, _) = support::capture_with_auth(
        &support::request(vec![support::user()]),
        RigProtocol::Anthropic,
        Arc::new(VersionedAuth),
    )
    .await;
    let versions: Vec<_> = wire["headers"]
        .as_str()
        .expect("captured HTTP headers")
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("anthropic-version")
                .then_some(value.trim())
        })
        .collect();
    assert_eq!(versions, vec!["2025-01-01"]);
}

// D2: the explicit final-request capture records the wire shape exactly as
// sent — post-rewrite body included — while masking query credentials and
// never recording headers.
#[tokio::test]
async fn final_request_capture_records_the_sanitized_wire_shape() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut provider = super::error_tests::provider(listener.local_addr().unwrap());
    provider.base_url = format!(
        "http://{}/v1?api-key=query-secret",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        super::support::serve_payload(&listener, super::support::ANTHROPIC_SSE).await
    });
    let request = super::support::request(vec![super::support::user()]);
    let auth: codex_api::SharedAuthProvider = std::sync::Arc::new(super::support::DummyAuth);
    let recorders = codex_rust_rig_bridge::RigTurnRecorders {
        events: None,
        final_request: Some(std::sync::Arc::new(std::sync::Mutex::new(
            codex_rust_rig_bridge::FinalRequestCapture::default(),
        ))),
    };
    let (mut stream, recorders) = codex_rust_rig_bridge::stream_via_rig_with_recorders(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        codex_rust_rig_bridge::RigProtocol::Anthropic,
        std::time::Duration::from_secs(5),
        recorders,
    )
    .await
    .unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let wire = server.await.unwrap();
    let captured = recorders
        .final_request
        .expect("capture attached")
        .lock()
        .unwrap()
        .clone();
    assert_eq!(captured.requests.len(), 1);
    let captured = serde_json::to_value(&captured.requests[0]).unwrap();
    assert_eq!(captured["method"], serde_json::json!("POST"));
    let url = captured["url"].as_str().expect("url");
    assert!(
        url.contains("api-key=REDACTED"),
        "query credentials are masked: {url}"
    );
    assert!(!url.contains("query-secret"), "{url}");
    let encoded = captured.to_string();
    assert!(!encoded.contains("query-secret"), "no credential anywhere");
    assert!(
        captured["headers"].is_null(),
        "headers are never recorded: {captured}"
    );
    // The captured body is the exact final wire body.
    assert_eq!(captured["body"], wire["body"], "parsed convenience body");
    assert_eq!(
        captured["body_raw"], wire["body_raw"],
        "exact UTF-8 wire bytes"
    );
}

#[tokio::test]
async fn responses_capture_keeps_exact_raw_tool_schema_after_projection() {
    use codex_protocol::models::ResponseItem;
    use codex_rust_rig_bridge::FinalRequestCapture;
    use codex_rust_rig_bridge::stream_responses_via_rig_with_capture;
    use std::sync::Mutex;
    use std::time::Duration;
    let raw_tools = r#"[ { "type" : "function", "name" : "lookup", "parameters" : { "type" : "object",
        "properties" : { "z" : { "const" : 18446744073709551617 }, "a" : { "const" : 1e+02 } } } } ]"#;
    for encrypted_content in [
        "native-ciphertext",
        "codex-rig-reasoning-v1:{\"source\":\"chat-wire\",\"blocks\":[]}",
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let provider = super::error_tests::provider(listener.local_addr().expect("address"));
        let server =
            tokio::spawn(
                async move { support::serve_payload(&listener, support::RESPONSES_SSE).await },
            );
        let mut request = support::request(vec![
            support::user(),
            serde_json::json!({
                "type":"reasoning", "id":"rsn_saved", "summary":[],
                "content":[{"type":"reasoning_text","text":"prior reasoning"}],
                "encrypted_content":encrypted_content,
            }),
        ]);
        let raw: Arc<serde_json::value::RawValue> = Arc::from(
            serde_json::value::RawValue::from_string(raw_tools.to_string()).expect("raw schema"),
        );
        request.tools = Some(raw.into());
        let original = serde_json::to_vec(&request).expect("original request bytes");
        let mut projected = request.clone();
        if encrypted_content.starts_with(codex_rust_rig_bridge::REPLAY_PREFIX)
            && let ResponseItem::Reasoning {
                encrypted_content, ..
            } = &mut projected.input[1]
        {
            *encrypted_content = None;
        }
        let expected = serde_json::to_vec(&projected).expect("projected final bytes");
        let capture = Arc::new(Mutex::new(FinalRequestCapture::default()));
        let auth: codex_api::SharedAuthProvider = Arc::new(support::DummyAuth);
        let mut stream = stream_responses_via_rig_with_capture(
            &request,
            &provider,
            &auth,
            HeaderMap::new(),
            Duration::from_secs(5),
            /*sse_recorder*/ None,
            /*turn_state*/ None,
            Some(capture.clone()),
        )
        .await
        .expect("Responses stream");
        let mut completed = 0;
        while let Some(event) = stream.next().await {
            if matches!(
                event.expect("stream event"),
                codex_api::ResponseEvent::Completed { .. }
            ) {
                completed += 1;
            }
        }
        let wire = server.await.expect("loopback server");
        let captured = capture.lock().expect("final capture").clone();
        assert_eq!(completed, 1);
        assert_eq!(captured.requests.len(), 1);
        assert_eq!(
            captured.requests[0].body_raw.as_bytes(),
            expected.as_slice(),
            "exact post-projection request bytes"
        );
        assert_eq!(
            captured.requests[0].body_raw.as_bytes(),
            wire["body_raw"]
                .as_str()
                .expect("wire raw bytes")
                .as_bytes()
        );
        assert_eq!(captured.requests[0].body, wire["body"]);
        assert_eq!(
            serde_json::to_vec(&request).expect("caller request unchanged"),
            original
        );
    }
}

#[tokio::test]
async fn pause_continuation_capture_keeps_every_http_attempt_in_wire_order() {
    use codex_rust_rig_bridge::FinalRequestCapture;
    use codex_rust_rig_bridge::RigTurnRecorders;
    use codex_rust_rig_bridge::stream_via_rig_with_recorders;
    use std::sync::Mutex;
    use std::time::Duration;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let provider = super::error_tests::provider(listener.local_addr().expect("address"));
    let server = tokio::spawn(async move {
        let mut wires = Vec::new();
        for payload in [
            super::pause_turn_tests::paused_sse(),
            support::ANTHROPIC_SSE.to_string(),
        ] {
            wires.push(support::serve_payload(&listener, &payload).await);
        }
        wires
    });
    let mut request = support::request(vec![support::user()]);
    support::set_tools(&mut request, serde_json::json!([{"type":"web_search"}]));
    let capture = Arc::new(Mutex::new(FinalRequestCapture::default()));
    let auth: codex_api::SharedAuthProvider = Arc::new(support::DummyAuth);
    let (mut stream, _) = stream_via_rig_with_recorders(
        &request,
        &provider,
        &auth,
        HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(5),
        RigTurnRecorders {
            events: None,
            final_request: Some(capture.clone()),
        },
    )
    .await
    .expect("pause-capable stream");
    let mut completed = 0;
    while let Some(event) = stream.next().await {
        if matches!(
            event.expect("stream event"),
            codex_api::ResponseEvent::Completed { .. }
        ) {
            completed += 1;
        }
    }
    let wires = server.await.expect("two-attempt loopback server");
    let captured = capture.lock().expect("final capture").clone();
    assert_eq!((completed, captured.requests.len(), wires.len()), (1, 2, 2));
    let expected: Vec<_> = wires
        .iter()
        .map(|wire| {
            serde_json::json!({
                "method":"POST", "url":format!("{}/messages", provider.base_url),
                "body_raw":wire["body_raw"], "body":wire["body"],
            })
        })
        .collect();
    assert_eq!(
        serde_json::to_value(&captured).expect("captured vector"),
        serde_json::json!({"requests":expected})
    );
    let continuation = &captured.requests[1].body["messages"];
    let last_assistant = continuation
        .as_array()
        .expect("messages")
        .iter()
        .rev()
        .find(|message| message["role"] == "assistant")
        .expect("paused assistant");
    assert_eq!(
        last_assistant["content"],
        serde_json::json!([
            {"type":"server_tool_use","id":"srvu_p1","name":"web_search","input":{"query":"pause q"}},
            {"type":"web_search_tool_result","tool_use_id":"srvu_p1","content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":"ENC_P1"}]},
            {"type":"text","text":"partial so far"},
        ])
    );
}
