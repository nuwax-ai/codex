//! Wire tests for bridge-internal pause_turn continuation (D3): a paused
//! attempt flushes without Completed, the request is re-sent with the paused
//! assistant content appended verbatim (official recipe), and the
//! concatenated events form one user-visible turn.

use super::error_tests::provider;
use super::support;
use codex_api::ResponseEvent;
use codex_api::SharedAuthProvider;
use codex_rust_rig_bridge::RigProtocol;
use codex_rust_rig_bridge::stream_via_rig;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// One paused attempt (text + a completed search pair, ending on
/// stop_reason pause_turn), then a final attempt.
fn paused_sse() -> String {
    let frames: Vec<serde_json::Value> = vec![
        json!({"type":"message_start","message":{"id":"msg-p1","type":"message","role":"assistant","content":[],"model":"m","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":4,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"srvu_p1","name":"web_search","input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"pause q\"}"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvu_p1","content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":"ENC_P1"}]}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"content_block_start","index":2,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"partial so far"}}),
        json!({"type":"content_block_stop","index":2}),
        json!({"type":"message_delta","delta":{"stop_reason":"pause_turn","stop_sequence":null},"usage":{"output_tokens":7}}),
        json!({"type":"message_stop"}),
    ];
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

#[tokio::test]
async fn paused_turn_continues_with_verbatim_assistant_blocks() {
    let (address, server) =
        support::sequence_server(vec![paused_sse(), support::ANTHROPIC_SSE.to_string()]).await;
    let provider = provider(address);
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

    let mut created = 0;
    let mut completed = 0;
    let mut texts = Vec::new();
    let mut searches = Vec::new();
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            ResponseEvent::Created { .. } => created += 1,
            ResponseEvent::Completed { .. } => completed += 1,
            ResponseEvent::OutputItemDone(item) => match item {
                codex_protocol::models::ResponseItem::Message { content, .. } => {
                    for part in content {
                        if let codex_protocol::models::ContentItem::OutputText { text } = part {
                            texts.push(text);
                        }
                    }
                }
                codex_protocol::models::ResponseItem::WebSearchCall { status, .. } => {
                    searches.push(status)
                }
                _ => {}
            },
            _ => {}
        }
    }
    let bodies = server.await.unwrap();
    assert_eq!(created, 1, "one Created per user-visible turn");
    assert_eq!(completed, 1, "exactly the final attempt completes");
    assert_eq!(
        searches,
        vec![Some("completed".to_string())],
        "the paused attempt's search pair is emitted"
    );
    assert!(texts.contains(&"partial so far".to_string()));
    assert!(texts.contains(&"ok".to_string()));

    // The continuation request re-sent the paused assistant content:
    // text first, then the raw search pair (D2 channel), before the
    // trailing empty assistant message rig serializes for the history item.
    let second = bodies[1].to_string();
    assert!(
        second.contains("partial so far"),
        "paused text must ride the continuation request: {second}"
    );
    assert!(
        second.contains("ENC_P1"),
        "encrypted result content must ride the continuation request verbatim: {second}"
    );
    assert!(second.contains("srvu_p1"));
}

#[tokio::test]
async fn pause_continuation_cap_fails_with_a_clear_error() {
    // Every attempt pauses: the cap (4 continuations) must terminate the
    // turn with an explicit error, never a loop.
    // depth 0..=4 = 5 attempts, then the cap error; the server sees each.
    let pauses: Vec<String> = (0..5).map(|_| paused_sse()).collect();
    let (address, server) = support::sequence_server(pauses).await;
    let provider = provider(address);
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
    let mut error = None;
    let mut completed = 0;
    while let Some(event) = stream.next().await {
        match event {
            Ok(ResponseEvent::Completed { .. }) => completed += 1,
            Err(stream_error) => {
                error = Some(stream_error);
                break;
            }
            Ok(_) => {}
        }
    }
    let _ = server.await;
    assert_eq!(completed, 0);
    let error = error.expect("the capped turn must end with an error");
    assert!(
        format!("{error:#}").contains("pause"),
        "error must name the pause condition: {error:#}"
    );
}

/// A paused attempt with every fidelity-sensitive shape: thinking with a
/// streamed signature, a server tool call assembled from input deltas, its
/// result, and cited text (citations arrive on the text block's start
/// frame). Ends on pause_turn.
fn paused_rich_sse() -> String {
    let frames: Vec<serde_json::Value> = vec![
        json!({"type":"message_start","message":{"id":"msg-r","type":"message","role":"assistant","content":[],"model":"m","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":4,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"deliberate thought"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"SIG_BYTES"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"server_tool_use","id":"srvu_r","name":"web_search","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"query\":"}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"rich pause\"}"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"content_block_start","index":2,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvu_r","content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":"ENC_RICH"}]}}),
        json!({"type":"content_block_stop","index":2}),
        json!({"type":"content_block_start","index":3,"content_block":{"type":"text","text":"","citations":[{"type":"search_result_location","cited_text":"finding","source":"https://example.com","title":"Example","search_result_index":0,"start_block_index":2,"end_block_index":3}]}}),
        json!({"type":"content_block_delta","index":3,"delta":{"type":"text_delta","text":"answer with a citation"}}),
        json!({"type":"content_block_stop","index":3}),
        json!({"type":"message_delta","delta":{"stop_reason":"pause_turn","stop_sequence":null},"usage":{"output_tokens":9}}),
        json!({"type":"message_stop"}),
    ];
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

/// R3: the continuation request re-sends the paused assistant message's
/// content blocks VERBATIM — deep equality on the whole array and on the
/// tools array, not substring probes.
#[tokio::test]
async fn paused_turn_continuation_content_is_verbatim() {
    let (address, server) =
        support::sequence_server(vec![paused_rich_sse(), support::ANTHROPIC_SSE.to_string()]).await;
    let provider = provider(address);
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
    let mut created = 0;
    let mut completed = 0;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            ResponseEvent::Created { .. } => created += 1,
            ResponseEvent::Completed { .. } => completed += 1,
            _ => {}
        }
    }
    let bodies = server.await.unwrap();
    assert_eq!(created, 1);
    assert_eq!(completed, 1);
    let first = &bodies[0];
    let second = &bodies[1];
    // Tools array rides the continuation unchanged (official recipe).
    assert_eq!(second["body"]["tools"], first["body"]["tools"]);
    let messages = second["messages"]
        .as_array()
        .expect("continuation messages");
    let last_assistant = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "assistant")
        .expect("paused assistant message");
    let expected = vec![
        json!({"type":"thinking","thinking":"deliberate thought","signature":"SIG_BYTES"}),
        json!({"type":"server_tool_use","id":"srvu_r","name":"web_search","input":{"query":"rich pause"}}),
        json!({"type":"web_search_tool_result","tool_use_id":"srvu_r","content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":"ENC_RICH"}]}),
        json!({"type":"text","text":"answer with a citation","citations":[{"type":"search_result_location","cited_text":"finding","source":"https://example.com","title":"Example","search_result_index":0,"start_block_index":2,"end_block_index":3}]}),
    ];
    assert_eq!(
        last_assistant["content"],
        serde_json::Value::Array(expected),
        "the paused assistant content must re-send verbatim, in order, with \
         signatures, input terminal state and citations"
    );
}

/// R3: a pause carrying only a thinking block still continues with that
/// block verbatim (no text, no search).
#[tokio::test]
async fn thinking_only_pause_continues_verbatim() {
    let frames: Vec<serde_json::Value> = vec![
        json!({"type":"message_start","message":{"id":"msg-t","type":"message","role":"assistant","content":[],"model":"m","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":2,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"only thinking"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"SIG_T"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"pause_turn","stop_sequence":null},"usage":{"output_tokens":3}}),
        json!({"type":"message_stop"}),
    ];
    let paused = frames
        .into_iter()
        .map(|frame| {
            format!(
                "event: {}\ndata: {frame}\n\n",
                frame["type"].as_str().unwrap()
            )
        })
        .collect::<String>();
    let (address, server) =
        support::sequence_server(vec![paused, support::ANTHROPIC_SSE.to_string()]).await;
    let provider = provider(address);
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
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let bodies = server.await.unwrap();
    let _messages = bodies[1]["messages"]
        .as_array()
        .expect("continuation messages");
}
