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
    let source =
        codex_rust_rig_bridge::reasoning_source(&provider, RigProtocol::Anthropic, "review-model")
            .expect("source identity");
    let message_id = done_items
        .iter()
        .find_map(|item| match item {
            ResponseItem::Message { id: Some(id), .. } => Some(id.as_str()),
            _ => None,
        })
        .expect("persisted text owner");
    let response_id = message_id
        .strip_prefix("rigseg_")
        .unwrap()
        .rsplit_once('_')
        .unwrap()
        .0;
    assert_eq!(
        serde_json::to_value(web_search_calls[0]).unwrap(),
        json!({
            "type":"web_search_call",
            "id":"srvu_glm",
            "status":"completed",
            "action":{"type":"search","query":"上海天气"},
            // D1+R2: the raw wire pair rides the item inside the versioned
            // same-source envelope — GLM's non-standard assistant-side
            // tool_result included, verbatim — plus the v2 block identity
            // (indices + layout) replay rebuilds positions from.
            "wire_blocks":{
                "version":3,
                "response_id":response_id,
                "source":source,
                "block_indices":[0,1],
                "blocks":[
                    {"type":"server_tool_use","id":"srvu_glm","name":"web_search_prime","input":{"search_query":"上海天气","location":"cn"}},
                    {"type":"tool_result","tool_use_id":"srvu_glm","content":"[{'text': [{'title': 'weather', 'link': 'https://example.com'}]}]"},
                ],
                "layout":[
                    {"kind":"pair","index":0},
                    {"kind":"pair","index":1},
                    {"kind":"text","index":2,"owner":message_id,"block":{"type":"text","text":"上海今天多云。"}},
                ],
            },
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
    // The persisted payload is the versioned envelope captured by the SAME
    // source identity this request will use (computed through the bridge's
    // own identity function), so the replay gate accepts it.
    let source =
        codex_rust_rig_bridge::reasoning_source(&provider, RigProtocol::Anthropic, "review-model")
            .expect("source identity");
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

pub(super) enum SearchReplay {
    Enabled,
    Disabled,
}

fn search_pair(id: &str) -> Value {
    json!([
        {"type":"server_tool_use","id":id,"name":"web_search","input":{"query":id},"vendor_field":"preserve"},
        {"type":"web_search_tool_result","tool_use_id":id,"content":[{"type":"web_search_result","url":"https://example.com","title":id,"encrypted_content":"original-ciphertext"}]},
    ])
}

pub(super) async fn capture_search_replay(raw_items: Vec<Value>, replay: SearchReplay) -> Value {
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
    let source =
        codex_rust_rig_bridge::reasoning_source(&provider, RigProtocol::Anthropic, "review-model")
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

#[tokio::test]
async fn anthropic_two_search_only_turns_replay_exact_raw_groups() {
    let first = search_pair("search-first");
    let second = search_pair("search-second");
    let messages = capture_search_replay(
        vec![
            support::user(),
            json!({"type":"web_search_call","wire_blocks":first}),
            support::user(),
            json!({"type":"web_search_call","wire_blocks":second}),
            support::user(),
        ],
        SearchReplay::Enabled,
    )
    .await;
    let user = json!({"role":"user","content":[{"type":"text","text":"hello"}]});
    assert_eq!(
        messages,
        json!([
            user, {"role":"assistant","content":first},
            user, {"role":"assistant","content":second}, user,
        ])
    );
}

#[tokio::test]
async fn anthropic_search_only_after_tool_result_replays_in_its_own_assistant() {
    let blocks = search_pair("search-after-tool");
    let messages = capture_search_replay(
        vec![
            support::user(),
            json!({"type":"function_call","name":"lookup","call_id":"call-1","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"call-1","output":"result"}),
            json!({"type":"web_search_call","wire_blocks":blocks}),
            support::user(),
        ],
        SearchReplay::Enabled,
    )
    .await;
    let user = json!({"role":"user","content":[{"type":"text","text":"hello"}]});
    assert_eq!(
        messages,
        json!([
            user,
            {"role":"assistant","content":[{"type":"tool_use","id":"call-1","name":"lookup","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"call-1","content":[{"type":"text","text":"result"}]}]},
            {"role":"assistant","content":blocks}, user,
        ])
    );
}

#[tokio::test]
async fn anthropic_search_only_opt_out_removes_sdk_anchors_and_empty_assistants() {
    let messages = capture_search_replay(vec![
        support::user(),
        json!({"type":"web_search_call","wire_blocks":search_pair("search-first")}),
        support::user(),
        json!({"type":"web_search_call","wire_blocks":search_pair("search-second")}),
        support::user(),
        json!({"type":"web_search_call","wire_blocks":[{"type":"unknown_legacy_block","opaque":"unused"}]}),
    ], SearchReplay::Disabled).await;
    let user = json!({"role":"user","content":[{"type":"text","text":"hello"}]});
    assert_eq!(messages, json!([user, user, user]));
}

#[tokio::test]
async fn anthropic_replay_group_cap_removes_dropped_search_only_anchors() {
    let mut items = vec![support::user()];
    for index in 0..65 {
        items.push(
            json!({"type":"web_search_call","wire_blocks":search_pair(&format!("search-{index}"))}),
        );
        items.push(support::user());
    }
    let messages = capture_search_replay(items, SearchReplay::Enabled).await;
    let assistants = messages
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "assistant")
        .map(|message| message["content"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        assistants,
        (1..65)
            .map(|index| search_pair(&format!("search-{index}")))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn anthropic_search_dedupe_keeps_first_valid_same_source_completed_item() {
    let first = search_pair("same-id");
    let mut later = first.clone();
    later[1]["content"][0]["encrypted_content"] = json!("LATER");
    let pending = json!([first[0]]);
    let foreign = json!({"version":1, "source":"another-endpoint", "blocks":later});
    let oversized = json!({"version":1, "source":"test-current", "blocks":later,
        "cited_text":[{"type":"text", "text":"x".repeat(40_960), "citations":[]}]});
    for (payloads, expected) in [
        (vec![first.clone(), later], first.clone()),
        (vec![foreign.clone(), first.clone()], first.clone()),
        (vec![pending.clone(), foreign], pending),
        (vec![oversized, first.clone()], first),
    ] {
        let mut items = vec![support::user()];
        items.extend(
            payloads
                .into_iter()
                .map(|payload| json!({"type":"web_search_call", "wire_blocks":payload})),
        );
        items.push(support::user());
        let messages = capture_search_replay(items, SearchReplay::Enabled).await;
        let assistants: Vec<Value> = messages
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "assistant")
            .map(|message| message["content"].clone())
            .collect();
        assert_eq!(assistants, vec![expected]);
    }
}

#[tokio::test]
async fn anthropic_malformed_loaded_search_payloads_drop_before_sdk_conversion() {
    let call = search_pair("loaded")[0].clone();
    for payload in [
        json!({"version":1, "source":"test-current", "blocks":[{"type":"server_tool_use", "id":"loaded", "input":{}}]}),
        json!({"version":1, "source":"test-current", "blocks":[{"type":"server_tool_use", "id":"loaded", "name":"web_search", "input":false}]}),
        json!({"version":1, "source":"test-current", "blocks":[call.clone(), {"type":"web_search_tool_result", "tool_use_id":"loaded", "content":false}]}),
        json!({"version":1, "source":"test-current", "blocks":[call.clone()], "cited_text":[{"type":"tool_use", "text":"x", "citations":[]}]}),
        json!({"version":1, "source":"test-current", "blocks":[call], "cited_text":[{"type":"text", "text":"x", "citations":{}}]}),
    ] {
        let messages = capture_search_replay(
            vec![
                support::user(),
                json!({"type":"web_search_call", "wire_blocks":payload}),
                support::user(),
            ],
            SearchReplay::Enabled,
        )
        .await;
        assert_eq!(
            messages,
            json!([
                {"role":"user", "content":[{"type":"text", "text":"hello"}]},
                {"role":"user", "content":[{"type":"text", "text":"hello"}]},
            ])
        );
    }
}

#[tokio::test]
async fn anthropic_late_search_results_preserve_response_positions_and_raw_fields() {
    for result_type in ["web_search_tool_result", "tool_result"] {
        for plain_message in [false, true] {
            for identity in [false, true] {
                let mut pair = search_pair("late");
                pair[1]["type"] = json!(result_type);
                if result_type == "tool_result" {
                    pair[1]["content"] = json!(
                        "[{'text': [{'title': 'GLM result', 'link': 'https://example.com'}]}]"
                    );
                }
                pair[1]["vendor_result"] =
                    json!({"opaque":"terminal", "number":9007199254740993u64});
                let cited = json!({"type":"text", "text":"late answer", "citations":[], "vendor_cite":"preserve"});
                let mut items = vec![
                    support::user(),
                    json!({"type":"function_call", "name":"lookup", "call_id":"client", "arguments":"{}"}),
                    json!({"type":"web_search_call", "wire_blocks":[pair[0].clone()]}),
                    json!({"type":"function_call_output", "call_id":"client", "output":"client result"}),
                ];
                if plain_message {
                    items.push(json!({"type":"message", "id":"rigseg_late_0", "role":"assistant", "content":[{"type":"output_text", "text":"late answer"}]}));
                }
                // v2 payloads carry block identity: the result streamed at
                // wire index 0 of the late response (the cloned call is
                // foreign), and the cited block at index 1, raw fields and
                // all. v1 payloads carry only the pair plus unattributable
                // cited text.
                let winner = if identity {
                    json!({
                        "version":3, "source":"test-current", "response_id":"late",
                        "blocks":pair,
                        "block_indices":[u64::MAX, 0],
                        "layout":[
                            {"kind":"pair","index":0},
                            {"kind":"cited","index":1,"owner":"rigseg_late_0","block":cited},
                        ],
                    })
                } else {
                    json!({"version":1, "source":"test-current", "blocks":pair, "cited_text":[cited.clone()]})
                };
                items.extend([
                    json!({"type":"web_search_call", "wire_blocks":winner}),
                    support::user(),
                ]);
                let late_content = if identity && plain_message {
                    // Identity rebuild: the result at its response position
                    // and the raw cited block replacing (or supplying) the
                    // merged text — vendor fields verbatim either way.
                    vec![pair[1].clone(), cited]
                } else if plain_message {
                    // No identity: the plain text stays untouched and the
                    // unattributable cited text is not injected.
                    vec![json!({"type":"text","text":"late answer"}), pair[1].clone()]
                } else {
                    vec![pair[1].clone()]
                };
                let user = json!({"role":"user", "content":[{"type":"text", "text":"hello"}]});
                assert_eq!(
                    capture_search_replay(items, SearchReplay::Enabled).await,
                    json!([
                        user.clone(),
                        {"role":"assistant", "content":[{"type":"tool_use", "id":"client", "name":"lookup", "input":{}}, pair[0].clone()]},
                        {"role":"user", "content":[{"type":"tool_result", "tool_use_id":"client", "content":[{"type":"text", "text":"client result"}]}]},
                        {"role":"assistant", "content":late_content},
                        user,
                    ]),
                    "{result_type}, plain_message={plain_message}, identity={identity}"
                );
            }
        }
    }
}

#[tokio::test]
async fn anthropic_late_search_result_opt_out_removes_result_only_anchors() {
    for result_type in ["web_search_tool_result", "tool_result"] {
        let mut pair = search_pair("disabled-late");
        pair[1]["type"] = json!(result_type);
        let items = vec![
            support::user(),
            json!({"type":"web_search_call", "wire_blocks":[pair[0].clone()]}),
            support::user(),
            json!({"type":"web_search_call", "wire_blocks":pair}),
            support::user(),
        ];
        let user = json!({"role":"user", "content":[{"type":"text", "text":"hello"}]});
        assert_eq!(
            capture_search_replay(items, SearchReplay::Disabled).await,
            json!([user, user, user])
        );
    }
}

#[tokio::test]
async fn anthropic_search_dedupe_across_user_messages_keeps_one_call_and_result() {
    let pair = search_pair("across-users");
    let mut duplicate = pair.clone();
    duplicate[1]["vendor_result"] = json!("must not replay");
    let messages = capture_search_replay(
        vec![
            support::user(),
            json!({"type":"web_search_call", "wire_blocks":[pair[0].clone()]}),
            support::user(),
            json!({"type":"web_search_call", "wire_blocks":pair}),
            support::user(),
            json!({"type":"web_search_call", "wire_blocks":duplicate}),
            support::user(),
            json!({"type":"web_search_call", "wire_blocks":[pair[0].clone()]}),
            support::user(),
        ],
        SearchReplay::Enabled,
    )
    .await;
    let user = json!({"role":"user", "content":[{"type":"text", "text":"hello"}]});
    assert_eq!(
        messages,
        json!([
            user, {"role":"assistant", "content":[pair[0].clone()]},
            user, {"role":"assistant", "content":[pair[1].clone()]}, user, user, user,
        ])
    );
}

#[tokio::test]
async fn anthropic_split_search_pair_cap_drops_both_wire_positions() {
    let mut items = vec![support::user()];
    for index in 0..65 {
        let pair = search_pair(&format!("split-{index}"));
        items.extend([
            json!({"type":"web_search_call", "wire_blocks":[pair[0].clone()]}),
            support::user(),
            json!({"type":"web_search_call", "wire_blocks":pair}),
            support::user(),
        ]);
    }
    let messages = capture_search_replay(items, SearchReplay::Enabled).await;
    let assistants: Vec<Value> = messages
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "assistant")
        .map(|message| message["content"].clone())
        .collect();
    let expected: Vec<Value> = (1..65)
        .flat_map(|index| {
            let pair = search_pair(&format!("split-{index}"));
            [json!([pair[0].clone()]), json!([pair[1].clone()])]
        })
        .collect();
    assert_eq!(assistants, expected);
}

/// R3: a mixed server/client turn — the server tool call arrives in
/// response 1 (pending), the client tool returns, and ONLY the search
/// result arrives in response 2. The result must close the pending call
/// from history as a NEW completed item, and the following request must
/// replay the pair exactly once (no duplicate call block).
#[tokio::test]
async fn mixed_turn_result_arriving_in_the_next_response_closes_the_pending_call() {
    let sse_with_pending_call = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvu_mixed\",\"name\":\"web_search\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\":\\\"mixed\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"lookup\",\"input\":{}}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":6}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    )
    .to_string();
    let sse_with_late_result = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m2\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"web_search_tool_result\",\"tool_use_id\":\"srvu_mixed\",\"content\":[{\"type\":\"web_search_result\",\"url\":\"https://example.com\",\"encrypted_content\":\"ENC_LATE\"}]}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":2}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    )
    .to_string();
    let (address, server) = support::sequence_server(vec![
        sse_with_pending_call,
        sse_with_late_result.clone(),
        support::ANTHROPIC_SSE.to_string(),
    ])
    .await;
    let provider = provider(address);
    let source =
        codex_rust_rig_bridge::reasoning_source(&provider, RigProtocol::Anthropic, "review-model")
            .expect("source identity");
    let pending_call = json!([
        {"type":"server_tool_use","id":"srvu_mixed","name":"web_search","input":{"query":"mixed"}}
    ]);
    let mut search_events: Vec<serde_json::Value> = Vec::new();

    // Request 1: the pending call is emitted in_progress.
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let mut request = support::request(vec![support::user()]);
    support::set_tools(
        &mut request,
        json!([{"type":"web_search"}, {"type":"function","name":"lookup"}]),
    );
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
        if let Ok(codex_api::ResponseEvent::OutputItemDone(item)) = event
            && let ResponseItem::WebSearchCall { status, .. } = &item
        {
            search_events.push(json!({"phase": 1, "status": status}));
        }
    }

    // Request 2: the history carries the client tool result plus the pending
    // search item; the response delivers ONLY the late result.
    request.input = vec![
        serde_json::from_value(support::user()).unwrap(),
        serde_json::from_value(json!({
            "type":"function_call","name":"lookup","call_id":"tool_1","arguments":"{}"
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "type":"function_call_output","call_id":"tool_1","output":"client result"
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "type":"web_search_call","id":"srvu_mixed","status":"in_progress",
            "action":{"type":"search","query":"mixed"},
            "wire_blocks":{"version":1,"source":source,"blocks":pending_call}
        }))
        .unwrap(),
    ];
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
    let mut completed_pair = None;
    while let Some(event) = stream.next().await {
        if let Ok(codex_api::ResponseEvent::OutputItemDone(item)) = event
            && let ResponseItem::WebSearchCall {
                wire_blocks,
                status,
                ..
            } = item
        {
            search_events.push(json!({"phase": 2, "status": status}));
            if status.as_deref() == Some("completed") {
                completed_pair = wire_blocks;
            }
        }
    }
    let completed_pair = completed_pair.expect("the late result closes the pending call");
    assert_eq!(
        completed_pair["blocks"],
        json!([
            {"type":"server_tool_use","id":"srvu_mixed","name":"web_search","input":{"query":"mixed"}},
            {"type":"web_search_tool_result","tool_use_id":"srvu_mixed","content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":"ENC_LATE"}]},
        ]),
        "the appended item carries call+result"
    );
    assert_eq!(completed_pair["source"], json!(source));

    // Request 3: history carries BOTH items (in_progress + appended
    // completed); the replay must contain the call exactly once.
    request.input = vec![
        serde_json::from_value(support::user()).unwrap(),
        serde_json::from_value(json!({
            "type":"web_search_call","id":"srvu_mixed","status":"in_progress",
            "action":{"type":"search","query":"mixed"},
            "wire_blocks":{"version":1,"source":source,"blocks":pending_call}
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "type":"web_search_call","id":"srvu_mixed","status":"completed",
            "action":{"type":"search","query":"mixed"},
            "wire_blocks":completed_pair
        }))
        .unwrap(),
    ];
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
    let bodies = server.await.unwrap();
    let encoded = bodies[2]["messages"].to_string();
    let call_occurrences = encoded.matches("srvu_mixed").count();
    assert_eq!(
        call_occurrences, 2,
        "call id once in the use block + once in tool_use_id: {encoded}"
    );
    assert_eq!(encoded.matches("ENC_LATE").count(), 1, "{encoded}");
}

/// R3: cited text blocks persist inside the capture envelope and replay
/// after their pair inside the same assistant group.
#[tokio::test]
async fn cited_text_blocks_persist_and_replay_after_their_pair() {
    let cited_sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvu_cited\",\"name\":\"web_search\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\":\\\"cited\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"web_search_tool_result\",\"tool_use_id\":\"srvu_cited\",\"content\":[{\"type\":\"web_search_result\",\"url\":\"https://example.com\",\"encrypted_content\":\"ENC_CIT\"}]}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"text\",\"text\":\"\",\"citations\":[{\"type\":\"search_result_location\",\"cited_text\":\"finding\",\"source\":\"https://example.com\",\"title\":\"Example\",\"search_result_index\":0,\"start_block_index\":1,\"end_block_index\":2}]}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"text_delta\",\"text\":\"answer citing \"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"text_delta\",\"text\":\"the result\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":2}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":5}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    )
    .to_string();
    let (address, server) =
        support::sequence_server(vec![cited_sse, support::ANTHROPIC_SSE.to_string()]).await;
    let provider = provider(address);
    let source =
        codex_rust_rig_bridge::reasoning_source(&provider, RigProtocol::Anthropic, "review-model")
            .expect("source identity");
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);

    // Turn 1: the emitted search item carries the cited text in its envelope.
    let mut request = support::request(vec![support::user()]);
    support::set_tools(&mut request, json!([{"type":"web_search"}]));
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
    let mut captured_envelope = None;
    let mut captured_message = None;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            ResponseEvent::OutputItemDone(item @ ResponseItem::Message { .. }) => {
                captured_message = Some(item)
            }
            ResponseEvent::OutputItemDone(ResponseItem::WebSearchCall { wire_blocks, .. }) => {
                captured_envelope = wire_blocks
            }
            _ => {}
        }
    }
    let envelope = captured_envelope.expect("search item emitted");
    let captured_message = captured_message.expect("saved message");
    let owner = match &captured_message {
        ResponseItem::Message { id: Some(id), .. } => id.as_str(),
        _ => panic!("message id"),
    };
    assert_eq!(envelope["source"], json!(source));
    assert_eq!(
        envelope["block_indices"],
        json!([0, 1]),
        "the pair blocks carry their wire indices"
    );
    assert_eq!(
        envelope["layout"],
        json!([
            {"kind":"pair","index":0},
            {"kind":"pair","index":1},
            {"kind":"cited","index":2,"owner":owner,"block":{
                "type":"text",
                "text":"answer citing the result",
                "citations":[{"type":"search_result_location","cited_text":"finding","source":"https://example.com","title":"Example","search_result_index":0,"start_block_index":1,"end_block_index":2}]
            }},
        ]),
        "the cited text block persists verbatim in the identity layout"
    );

    // Turn 2: the cited block replays after the pair, inside the assistant.
    request.input = vec![
        serde_json::from_value(support::user()).unwrap(),
        captured_message.clone(),
        serde_json::from_value(json!({
            "type":"web_search_call","id":"srvu_cited","status":"completed",
            "action":{"type":"search","query":"cited"},
            "wire_blocks":envelope
        }))
        .unwrap(),
    ];
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
    let bodies = server.await.unwrap();
    let messages = bodies[1]["messages"].as_array().expect("messages");
    let assistant = messages
        .iter()
        .find(|message| message["role"] == "assistant")
        .expect("assistant with replay");
    assert_eq!(
        assistant["content"],
        json!([
            {"type":"server_tool_use","id":"srvu_cited","name":"web_search","input":{"query":"cited"}},
            {"type":"web_search_tool_result","tool_use_id":"srvu_cited","content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":"ENC_CIT"}]},
            {"type":"text","text":"answer citing the result","citations":[{"type":"search_result_location","cited_text":"finding","source":"https://example.com","title":"Example","search_result_index":0,"start_block_index":1,"end_block_index":2}]},
        ]),
        "pair first, cited text after, verbatim"
    );
}

/// N2: the assistant's plain answer and the envelope's cited text describe
/// the SAME streamed text; the projection must carry it exactly once, with
/// the citations attached at the answer's position.
#[tokio::test]
async fn cited_text_replaces_the_plain_answer_projection_in_place() {
    let cited_sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvu_once\",\"name\":\"web_search\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\":\\\"once\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"web_search_tool_result\",\"tool_use_id\":\"srvu_once\",\"content\":[{\"type\":\"web_search_result\",\"url\":\"https://example.com\",\"encrypted_content\":\"ENC_ONCE\"}]}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"text\",\"text\":\"\",\"citations\":[{\"type\":\"search_result_location\",\"cited_text\":\"finding\",\"source\":\"https://example.com\",\"title\":\"Example\",\"search_result_index\":0,\"start_block_index\":1,\"end_block_index\":2}]}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"text_delta\",\"text\":\"the single answer\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":2}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":5}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    )
    .to_string();
    let (address, server) =
        support::sequence_server(vec![cited_sse, support::ANTHROPIC_SSE.to_string()]).await;
    let provider = provider(address);
    let _source =
        codex_rust_rig_bridge::reasoning_source(&provider, RigProtocol::Anthropic, "review-model")
            .expect("source identity");
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);

    // Turn 1 captures the envelope (pair + cited text).
    let mut request = support::request(vec![support::user()]);
    support::set_tools(&mut request, json!([{"type":"web_search"}]));
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
    let mut captured_envelope = None;
    let mut captured_message = None;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            ResponseEvent::OutputItemDone(item @ ResponseItem::Message { .. }) => {
                captured_message = Some(item)
            }
            ResponseEvent::OutputItemDone(ResponseItem::WebSearchCall { wire_blocks, .. }) => {
                captured_envelope = wire_blocks
            }
            _ => {}
        }
    }
    let envelope = captured_envelope.expect("search item emitted");
    let captured_message = captured_message.expect("saved message");

    // Turn 2 replays the REAL history shape: the saved assistant Message
    // with the plain answer AND the search item with its envelope.
    request.input = vec![
        serde_json::from_value(support::user()).unwrap(),
        captured_message,
        serde_json::from_value(json!({
            "type":"web_search_call","id":"srvu_once","status":"completed",
            "action":{"type":"search","query":"once"},
            "wire_blocks":envelope
        }))
        .unwrap(),
    ];
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
    let bodies = server.await.unwrap();
    let messages = bodies[1]["messages"].as_array().expect("messages");
    let assistant = messages
        .iter()
        .find(|message| message["role"] == "assistant")
        .expect("assistant with replay");
    assert_eq!(
        assistant["content"],
        json!([
            {"type":"server_tool_use","id":"srvu_once","name":"web_search","input":{"query":"once"}},
            {"type":"web_search_tool_result","tool_use_id":"srvu_once","content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":"ENC_ONCE"}]},
            {"type":"text","text":"the single answer","citations":[{"type":"search_result_location","cited_text":"finding","source":"https://example.com","title":"Example","search_result_index":0,"start_block_index":1,"end_block_index":2}]},
        ]),
        "the cited terminal state replaces the plain answer at its position"
    );
    assert_eq!(
        assistant["content"]
            .to_string()
            .matches("the single answer")
            .count(),
        1,
        "the answer must appear exactly once"
    );
}

/// N3: in the production history order — client call, pending server call,
/// client output, late completed item — the request prefix sent while the
/// search was pending stays byte-identical after the result arrives; the
/// late result joins only its own new response position.
#[tokio::test]
async fn mixed_turn_late_result_preserves_the_sent_request_prefix() {
    let sse_pending = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvu_prefix\",\"name\":\"web_search\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\":\\\"prefix\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_p\",\"name\":\"lookup\",\"input\":{}}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":6}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    )
    .to_string();
    let sse_late_result = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m2\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"web_search_tool_result\",\"tool_use_id\":\"srvu_prefix\",\"content\":[{\"type\":\"web_search_result\",\"url\":\"https://example.com\",\"encrypted_content\":\"ENC_PREFIX\"}]}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":2}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    )
    .to_string();
    let (address, server) = support::sequence_server(vec![
        sse_pending,
        sse_late_result,
        support::ANTHROPIC_SSE.to_string(),
    ])
    .await;
    let provider = provider(address);
    let source =
        codex_rust_rig_bridge::reasoning_source(&provider, RigProtocol::Anthropic, "review-model")
            .expect("source identity");
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let pending_call = json!([
        {"type":"server_tool_use","id":"srvu_prefix","name":"web_search","input":{"query":"prefix"}}
    ]);

    // Request 1 emits the pending call in_progress (nothing asserted here
    // beyond draining the stream).
    let mut request = support::request(vec![support::user()]);
    support::set_tools(
        &mut request,
        json!([{"type":"web_search"}, {"type":"function","name":"lookup"}]),
    );
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

    // Request 2 carries the REAL production order: client call, client
    // output, then the pending server call. This is the prefix the model
    // saw while the search was pending.
    request.input = vec![
        serde_json::from_value(support::user()).unwrap(),
        serde_json::from_value(json!({
            "type":"function_call","name":"lookup","call_id":"tool_p","arguments":"{}"
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "type":"function_call_output","call_id":"tool_p","output":"client result"
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "type":"web_search_call","id":"srvu_prefix","status":"in_progress",
            "action":{"type":"search","query":"prefix"},
            "wire_blocks":{"version":1,"source":source,"blocks":pending_call}
        }))
        .unwrap(),
    ];
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
    let mut completed_pair = None;
    while let Some(event) = stream.next().await {
        if let Ok(codex_api::ResponseEvent::OutputItemDone(item)) = event
            && let ResponseItem::WebSearchCall {
                wire_blocks,
                status,
                ..
            } = item
            && status.as_deref() == Some("completed")
        {
            completed_pair = wire_blocks;
        }
    }
    let completed_pair = completed_pair.expect("the late result closes the pending call");

    // Request 3: same history plus the appended completed item.
    request.input.push(
        serde_json::from_value(json!({
            "type":"web_search_call","id":"srvu_prefix","status":"completed",
            "action":{"type":"search","query":"prefix"},
            "wire_blocks":completed_pair
        }))
        .unwrap(),
    );
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
    let bodies = server.await.unwrap();
    let prefix = bodies[1]["messages"]
        .as_array()
        .expect("request 2 messages");
    let after = bodies[2]["messages"]
        .as_array()
        .expect("request 3 messages");
    // The late result joins the pending call's own response position: every
    // message BEFORE that assistant stays byte-identical, and the assistant
    // keeps its already-sent content as a prefix (use first, result after).
    assert_eq!(
        after.len(),
        prefix.len(),
        "the late result extends its assistant, not the message list"
    );
    assert_eq!(
        after[..prefix.len() - 1],
        prefix[..prefix.len() - 1],
        "messages before the pending call's assistant stay byte-identical: {} vs {}",
        serde_json::to_string(&after).unwrap(),
        serde_json::to_string(prefix).unwrap(),
    );
    let pending_assistant = prefix
        .last()
        .expect("the pending call's assistant was already sent");
    let extended_assistant = after
        .last()
        .expect("the same assistant carries the late result");
    assert_eq!(
        pending_assistant["role"],
        json!("assistant"),
        "the pending call projected into an assistant message"
    );
    let pending_content = pending_assistant["content"]
        .as_array()
        .expect("pending assistant content");
    let extended_content = extended_assistant["content"]
        .as_array()
        .expect("extended assistant content");
    assert!(
        extended_content.starts_with(pending_content),
        "the already-sent assistant content stays a stable prefix: {} then {}",
        serde_json::to_string(pending_content).unwrap(),
        serde_json::to_string(extended_content).unwrap(),
    );
    // The client call/output never left the projection.
    let encoded = serde_json::to_string(&after).unwrap();
    assert!(encoded.contains("\"tool_p\""), "{encoded}");
    assert!(encoded.contains("client result"), "{encoded}");
    // The use block stays in the pending assistant; the result joins that
    // same response position after it.
    let completed_assistant = after
        .last()
        .expect("the pending call's assistant carries the late result");
    assert_eq!(completed_assistant["role"], json!("assistant"));
    let completed_content = completed_assistant["content"].to_string();
    assert!(
        completed_content.contains("ENC_PREFIX"),
        "{completed_content}"
    );
    let use_index = completed_content.find("server_tool_use").expect("use kept");
    let result_index = completed_content
        .find("ENC_PREFIX")
        .expect("result present");
    assert!(
        use_index < result_index,
        "the use stays at its position ahead of the late result"
    );
}
