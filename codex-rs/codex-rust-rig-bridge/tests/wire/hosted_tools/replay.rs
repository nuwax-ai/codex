use super::common::*;
use pretty_assertions::assert_eq;

// Fork (nuwax-codex) D2: a persisted wire pair replays verbatim into the
// assistant message it followed — server_tool_use, the result block and its
// encrypted content all reach the outbound body unchanged.
#[tokio::test]
async fn anthropic_replay_restores_persisted_web_search_blocks() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let server =
        tokio::spawn(
            async move { support::serve_payload(&listener, support::ANTHROPIC_SSE).await },
        );
    // The persisted payload is the versioned envelope captured by the SAME
    // source identity this request will use (computed through the bridge's
    // own identity function), so the replay gate accepts it.
    let source = support::source_for_dummy_auth(&provider, RigProtocol::Anthropic);
    let request = support::request(vec![
        support::user(),
        serde_json::json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"prior answer"}]}),
        serde_json::json!({
            "type":"web_search_call","id":"call_prev","status":"completed",
            "action":{"type":"search","query":"last turn query"},
            "wire_blocks":{
                "version":1,
                "source":source,
                "blocks":[
                    {"type":"server_tool_use","id":"srvtoolu_prev","name":"web_search","input":{"query":"last turn query"}},
                    {"type":"web_search_tool_result","tool_use_id":"srvtoolu_prev","content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":"ENCRYPTED_PAYLOAD"}]},
                ],
            },
        }),
        serde_json::json!({"type":"message","role":"user","content":[{"type":"input_text","text":"next question"}]}),
    ]);
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let mut stream = stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let wire = server.await.unwrap();
    let messages = &wire["body"]["messages"];
    let encoded = messages.to_string();
    let assistant = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "assistant")
        .expect("assistant message");
    let content = assistant["content"].as_array().expect("content array");
    // Text first (history order), then the replayed pair, verbatim.
    assert_eq!(content[0]["type"], json!("text"));
    assert_eq!(content[0]["text"], json!("prior answer"));
    assert_eq!(content[1]["type"], json!("server_tool_use"));
    assert_eq!(content[1]["id"], json!("srvtoolu_prev"));
    assert_eq!(content[2]["type"], json!("web_search_tool_result"));
    assert_eq!(
        content[2]["content"][0]["encrypted_content"],
        json!("ENCRYPTED_PAYLOAD"),
        "encrypted content must round-trip unmodified"
    );
    assert!(encoded.contains("next question"));
}

// R2: payloads captured by a different endpoint/model identity never ride a
// request to this endpoint — the gate drops them before the wire.
#[tokio::test]
async fn anthropic_replay_drops_payloads_from_another_source() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let server =
        tokio::spawn(
            async move { support::serve_payload(&listener, support::ANTHROPIC_SSE).await },
        );
    let foreign = json!({
        "version": 1,
        "source": "Anthropic:0000000000000000000000000000000000000000000000000000000000000000",
        "blocks": [
            {"type":"server_tool_use","id":"srvtoolu_foreign","name":"web_search","input":{"query":"q"}},
            {"type":"web_search_tool_result","tool_use_id":"srvtoolu_foreign","content":[{"type":"web_search_result","encrypted_content":"FOREIGN_CIPHERTEXT"}]},
        ],
    });
    let request = support::request(vec![
        support::user(),
        serde_json::json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"prior answer"}]}),
        serde_json::json!({
            "type":"web_search_call","id":"call_prev","status":"completed",
            "action":{"type":"search","query":"q"},
            "wire_blocks":foreign,
        }),
        serde_json::json!({"type":"message","role":"user","content":[{"type":"input_text","text":"next question"}]}),
    ]);
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let mut stream = stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let wire = server.await.unwrap();
    let encoded = wire["body"]["messages"].to_string();
    assert!(
        !encoded.contains("server_tool_use") && !encoded.contains("FOREIGN_CIPHERTEXT"),
        "cross-source ciphertext must not ride the request: {encoded}"
    );
    assert!(encoded.contains("prior answer") && encoded.contains("next question"));
}

// D2: opting out via the provider knob drops the pairs (legacy behavior).
#[tokio::test]
async fn anthropic_replay_opt_out_drops_persisted_blocks() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut provider = provider(listener.local_addr().unwrap());
    provider.hosted_results_replay = Some(false);
    let server =
        tokio::spawn(
            async move { support::serve_payload(&listener, support::ANTHROPIC_SSE).await },
        );
    let request = support::request(vec![
        support::user(),
        serde_json::json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"prior answer"}]}),
        serde_json::json!({
            "type":"web_search_call","id":"call_prev","status":"completed",
            "wire_blocks":[
                {"type":"server_tool_use","id":"srvtoolu_prev","name":"web_search","input":{"query":"q"}},
                {"type":"web_search_tool_result","tool_use_id":"srvtoolu_prev","content":[]},
            ],
        }),
        serde_json::json!({"type":"message","role":"user","content":[{"type":"input_text","text":"next question"}]}),
    ]);
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let mut stream = stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let wire = server.await.unwrap();
    let encoded = wire["body"]["messages"].to_string();
    assert!(
        !encoded.contains("server_tool_use") && !encoded.contains("ENCRYPTED"),
        "opted-out replay must drop the pair: {encoded}"
    );
    assert!(encoded.contains("prior answer") && encoded.contains("next question"));
}

// Fork: cross-turn web_search replay on the Anthropic wire. `WebSearchCall`
// items carry no result payload (protocol/src/models.rs), so the only sound
// replay is to drop them — this pins that behavior as the regression
// baseline for the phase-3 faithful-replay enhancement (paired
// server_tool_use / web_search_tool_result blocks).
#[tokio::test]
async fn anthropic_replay_drops_web_search_call_history() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let server =
        tokio::spawn(
            async move { support::serve_payload(&listener, support::ANTHROPIC_SSE).await },
        );
    let web_search_call: Value = serde_json::from_str(
        r#"{"type":"web_search_call","id":"call_prev","status":"completed",
            "action":{"type":"search","query":"last turn query"}}"#,
    )
    .unwrap();
    let request = support::request(vec![
        support::user(),
        web_search_call,
        serde_json::json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"prior answer"}]}),
        serde_json::json!({"type":"message","role":"user","content":[{"type":"input_text","text":"next question"}]}),
    ]);
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let mut stream = stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let wire = server.await.unwrap();
    let encoded = wire["body"]["messages"].to_string();
    assert!(
        !encoded.contains("server_tool_use"),
        "replay must not carry server_tool_use blocks: {encoded}"
    );
    assert!(
        !encoded.contains("web_search_tool_result") && !encoded.contains("\"tool_result\""),
        "replay must not carry search result blocks: {encoded}"
    );
    assert!(
        !encoded.contains("last turn query"),
        "dropped items must not leak their payloads: {encoded}"
    );
    // The retained assistant text and the new user turn survive.
    assert!(encoded.contains("prior answer"));
    assert!(encoded.contains("next question"));
}
