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

// F01: user-selected search constraints (domain allowlist, approximate
// location) must survive the Responses→Messages translation.
#[tokio::test]
async fn hosted_web_search_constraints_reach_the_anthropic_wire() {
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
            {"type":"web_search","external_web_access":true,
             "filters":{"allowed_domains":["docs.rs","example.com/blog"]},
             "user_location":{"type":"approximate","city":"Shanghai","country":"CN","timezone":"Asia/Shanghai"},
             "search_context_size":"medium"},
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
    let tools = wire["body"]["tools"].as_array().unwrap().clone();
    let server_entry = tools
        .iter()
        .find(|tool| tool["type"] == "web_search_20250305")
        .expect("translated server tool on the wire");
    assert_eq!(
        server_entry["allowed_domains"],
        json!(["docs.rs", "example.com/blog"])
    );
    assert_eq!(
        server_entry["user_location"],
        json!({"type":"approximate","city":"Shanghai","country":"CN","timezone":"Asia/Shanghai"})
    );
    assert!(
        server_entry.get("search_context_size").is_none(),
        "tuning knobs without a Messages equivalent must not leak: {server_entry}"
    );
}

// F06: hosted-only requests carry no function tools, so rig's streaming path
// drops tool_choice — the transport must restore the requested choice (and
// the parallel-tool-use gate that piggybacks on it) alongside the injected
// server tools.
#[tokio::test]
async fn hosted_only_request_restores_tool_choice_and_parallel_control() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let server =
        tokio::spawn(
            async move { support::serve_payload(&listener, support::ANTHROPIC_SSE).await },
        );
    let mut request = support::request(vec![support::user()]);
    request.parallel_tool_calls = false;
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
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let wire = server.await.unwrap();
    assert_eq!(
        wire["body"]["tool_choice"],
        json!({"type":"auto","disable_parallel_tool_use":true})
    );
    assert_eq!(
        wire["body"]["tools"],
        json!([{"type":"web_search_20250305","name":"web_search"}])
    );
}

#[tokio::test]
async fn tool_choice_none_is_restored_on_the_wire() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let server =
        tokio::spawn(
            async move { support::serve_payload(&listener, support::ANTHROPIC_SSE).await },
        );
    let mut request = support::request(vec![support::user()]);
    request.tool_choice = "none".into();
    request.parallel_tool_calls = false;
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
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let wire = server.await.unwrap();
    assert_eq!(wire["body"]["tool_choice"], json!({"type":"none"}));
}

// Cached/indexed search modes cannot be expressed on the Messages wire; the
// request must fail before any bytes reach the provider instead of silently
// widening to live search.
#[tokio::test]
async fn cached_web_search_mode_fails_before_the_request() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let mut request = support::request(vec![support::user()]);
    support::set_tools(
        &mut request,
        json!([{"type":"web_search","external_web_access":false}]),
    );
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let error = match stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(5),
    )
    .await
    {
        Err(error) => error,
        Ok(_) => panic!("cached mode must fail the request, not silently widen to live search"),
    };
    let message = format!("{error:#}");
    assert!(
        message.contains("cached mode") && message.contains("live search mode"),
        "error must name the mode and the supported alternative: {message}"
    );
}

#[tokio::test]
async fn required_tool_choice_with_hosted_only_tools_fails_before_the_request() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let mut request = support::request(vec![support::user()]);
    request.tool_choice = "required".into();
    support::set_tools(&mut request, json!([{"type":"web_search"}]));
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let error = match stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(5),
    )
    .await
    {
        Err(error) => error,
        Ok(_) => panic!("forced tool choice without function tools must fail the request"),
    };
    let message = format!("{error:#}");
    assert!(
        message.contains("required") && message.contains("function tool"),
        "error must name the choice and why it cannot be honored: {message}"
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
            // D1: the raw wire pair rides the item for faithful replay —
            // GLM's non-standard assistant-side tool_result included.
            "wire_blocks":[
                {"type":"server_tool_use","id":"srvu_glm","name":"web_search_prime","input":{"search_query":"上海天气","location":"cn"}},
                {"type":"tool_result","tool_use_id":"srvu_glm","content":"[{'text': [{'title': 'weather', 'link': 'https://example.com'}]}]"},
            ],
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
    let request = support::request(vec![
        support::user(),
        serde_json::json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"prior answer"}]}),
        serde_json::json!({
            "type":"web_search_call","id":"call_prev","status":"completed",
            "action":{"type":"search","query":"last turn query"},
            "wire_blocks":[
                {"type":"server_tool_use","id":"srvtoolu_prev","name":"web_search","input":{"query":"last turn query"}},
                {"type":"web_search_tool_result","tool_use_id":"srvtoolu_prev","content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":"ENCRYPTED_PAYLOAD"}]},
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
