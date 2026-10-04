//! Wire tests for hosted (server-side) tool translation on the Anthropic
//! wire: request injection of server-tool entries and response mapping of
//! streamed `server_tool_use` blocks (including GLM's gateway quirks).

pub(crate) use crate::error_tests::provider;
pub(crate) use crate::support;
pub(crate) use codex_api::ResponseEvent;
pub(crate) use codex_api::SharedAuthProvider;
pub(crate) use codex_protocol::models::ResponseItem;
pub(crate) use codex_rust_rig_bridge::RigProtocol;
pub(crate) use codex_rust_rig_bridge::stream_via_rig;
pub(crate) use futures::StreamExt;
pub(crate) use serde_json::Value;
pub(crate) use serde_json::json;
pub(crate) use std::sync::Arc;
pub(crate) use std::time::Duration;

pub(crate) fn frames_sse(frames: Vec<Value>) -> String {
    frames
        .into_iter()
        .map(|frame| {
            format!(
                "event: {}\ndata: {frame}\n\n",
                frame["type"].as_str().unwrap()
            )
        })
        .collect()
}

/// Mirrors GLM's live anthropic stream: a web-search server tool use (renamed
/// `web_search_prime`, `search_query` input field), its non-standard
/// assistant-side `tool_result` block, then the final text.
pub(crate) fn glm_web_search_turn() -> String {
    frames_sse(vec![
        json!({"type":"message_start","message":{"id":"msg-ws","type":"message","role":"assistant","content":[],"model":"glm-5.3-flash","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":9,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"srvu_glm","name":"web_search_prime","input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"search_query\":\"上海天气\",\"location\":\"cn\"}"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_result","tool_use_id":"srvu_glm","content":"[{'text': [{'title': 'weather', 'link': 'https://example.com'}]}]"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"content_block_start","index":2,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"上海今天多云。"}}),
        json!({"type":"content_block_stop","index":2}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":12}}),
        json!({"type":"message_stop"}),
    ])
}

pub(crate) enum SearchReplay {
    Enabled,
    Disabled,
}

pub(crate) fn search_pair(id: &str) -> Value {
    json!([
        {"type":"server_tool_use","id":id,"name":"web_search","input":{"query":id},"vendor_field":"preserve"},
        {"type":"web_search_tool_result","tool_use_id":id,"content":[{"type":"web_search_result","url":"https://example.com","title":id,"encrypted_content":"original-ciphertext"}]},
    ])
}

pub(crate) async fn capture_search_replay(raw_items: Vec<Value>, replay: SearchReplay) -> Value {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut provider = provider(listener.local_addr().unwrap());
    provider.hosted_results_replay = match replay {
        SearchReplay::Enabled => Some(true),
        SearchReplay::Disabled => Some(false),
    };
    let server =
        tokio::spawn(
            async move { support::serve_payload(&listener, support::ANTHROPIC_SSE).await },
        );
    // Fixtures write bare block arrays; wrap them into the versioned
    // envelope of the SAME source identity this request will use, exactly
    // like a captured turn would carry them.
    let source = codex_rust_rig_bridge::reasoning_source_with_auth_domain(
        &provider,
        RigProtocol::Anthropic,
        "review-model",
        Some("legacy-unscoped"),
    )
    .expect("source identity");
    let items = raw_items
        .into_iter()
        .map(|mut item| {
            if item["type"] == "web_search_call"
                && let Some(blocks) = item["wire_blocks"].as_array()
            {
                item["wire_blocks"] = json!({
                    "version": 1,
                    "source": source,
                    "blocks": blocks,
                });
            } else if item["wire_blocks"]["source"] == "test-current" {
                item["wire_blocks"]["source"] = json!(source);
            }
            item
        })
        .collect();
    let request = support::request(items);
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
    let wire = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    let messages = wire["body"]["messages"].clone();
    assert!(
        messages.as_array().unwrap().iter().all(|message| {
            message["role"] != "assistant" || !message["content"].as_array().unwrap().is_empty()
        }),
        "no empty assistant may reach the provider: {messages}"
    );
    messages
}
