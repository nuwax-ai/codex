//! Wire tests for hosted (server-side) tool translation on the Anthropic
//! wire: request injection of server-tool entries and response mapping of
//! streamed `server_tool_use` blocks (including GLM's gateway quirks).

use super::error_tests::provider;
use super::support;
use codex_api::ResponseEvent;
use codex_api::SharedAuthProvider;
use codex_protocol::models::ResponseItem;
use codex_rust_rig_bridge::RigProtocol;
use codex_rust_rig_bridge::stream_via_rig;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

fn frames_sse(frames: Vec<Value>) -> String {
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
fn glm_web_search_turn() -> String {
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

#[tokio::test]
async fn hosted_web_search_is_injected_as_an_anthropic_server_tool() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let server =
        tokio::spawn(
            async move { support::serve_payload(&listener, support::ANTHROPIC_SSE).await },
        );
    let mut request = support::request(vec![support::user()]);
    support::set_tools(
        &mut request,
        json!([
            {"type":"web_search"},
            {"type":"function","name":"lookup","parameters":{"type":"object","properties":{"query":{"type":"string"}}}},
        ]),
    );
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
    // The hosted tool is re-declared as the Anthropic server tool next to the
    // translated function tool; neither a schema-less function entry nor the
    // raw Responses shape may leak onto the wire.
    assert_eq!(
        wire["body"]["tools"],
        json!([
            {"name":"lookup","description":"","input_schema":{"type":"object","properties":{"query":{"type":"string"}}}},
            {"type":"web_search_20250305","name":"web_search"},
        ])
    );
}

#[tokio::test]
async fn anthropic_server_tool_use_maps_to_a_web_search_call_item() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let payload = glm_web_search_turn();
    let server = tokio::spawn(async move { support::serve_payload(&listener, &payload).await });
    let mut request = support::request(vec![support::user()]);
    support::set_tools(&mut request, json!([{"type":"web_search"}]));
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
    let mut done_items = Vec::new();
    let mut completed = 0;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            ResponseEvent::OutputItemDone(item) => done_items.push(item),
            ResponseEvent::Completed { .. } => completed += 1,
            _ => {}
        }
    }
    server.await.unwrap();
    assert_eq!(completed, 1, "turn must complete: {done_items:?}");
    // Exactly one web search call (completed, GLM query field) and one text
    // message; the non-standard assistant-side tool_result block must not
    // surface as an item or an empty message.
    let web_search_calls: Vec<&ResponseItem> = done_items
        .iter()
        .filter(|item| matches!(item, ResponseItem::WebSearchCall { .. }))
        .collect();
    assert_eq!(web_search_calls.len(), 1, "items: {done_items:?}");
    assert_eq!(
        serde_json::to_value(web_search_calls[0]).unwrap(),
        json!({
            "type":"web_search_call",
            "id":"srvu_glm",
            "status":"completed",
            "action":{"type":"search","query":"上海天气"},
        })
    );
    let messages: Vec<&ResponseItem> = done_items
        .iter()
        .filter(|item| matches!(item, ResponseItem::Message { .. }))
        .collect();
    assert_eq!(messages.len(), 1, "items: {done_items:?}");
    assert_eq!(
        serde_json::to_value(messages[0]).unwrap()["content"],
        json!([{"type":"output_text","text":"上海今天多云。"}])
    );
}

#[tokio::test]
async fn anthropic_server_tool_without_a_codex_item_is_ignored() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let payload = frames_sse(vec![
        json!({"type":"message_start","message":{"id":"msg-bash","type":"message","role":"assistant","content":[],"model":"m","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":2,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"srvu_bash","name":"bash_20250124","input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"ls\"}"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"done"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":3}}),
        json!({"type":"message_stop"}),
    ]);
    let server = tokio::spawn(async move { support::serve_payload(&listener, &payload).await });
    let request = support::request(vec![support::user()]);
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
    let mut done_items = Vec::new();
    let mut completed = 0;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            ResponseEvent::OutputItemDone(item) => done_items.push(item),
            ResponseEvent::Completed { .. } => completed += 1,
            _ => {}
        }
    }
    server.await.unwrap();
    assert_eq!(completed, 1);
    assert!(
        done_items
            .iter()
            .all(|item| !matches!(item, ResponseItem::WebSearchCall { .. })),
        "unmapped server tools must not become web search calls: {done_items:?}"
    );
    assert_eq!(done_items.len(), 1, "only the text message: {done_items:?}");
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
