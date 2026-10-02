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
pub(super) fn paused_sse() -> String {
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
    // The paused assistant re-sends VERBATIM (deep equality, not substring
    // probes): the streamed search pair and the partial text in original
    // block order.
    let messages = bodies[1]["messages"].as_array().expect("messages");
    let last_assistant = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "assistant")
        .expect("paused assistant message");
    assert_eq!(
        last_assistant["content"],
        serde_json::Value::Array(vec![
            json!({"type":"server_tool_use","id":"srvu_p1","name":"web_search","input":{"query":"pause q"}}),
            json!({"type":"web_search_tool_result","tool_use_id":"srvu_p1","content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":"ENC_P1"}]}),
            json!({"type":"text","text":"partial so far"}),
        ])
    );
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
    assert!(first["tools"].is_array());
    assert_eq!(second["tools"], first["tools"]);
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
    let (address, server) = support::sequence_server(vec![
        paused.clone(),
        paused
            .replace("only thinking", "next thinking")
            .replace("SIG_T", "SIG_NEXT"),
        support::ANTHROPIC_SSE.to_string(),
    ])
    .await;
    let provider = provider(address);
    let request = support::request(vec![
        support::user(),
        json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"previous answer"}]}),
        support::user(),
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
    let bodies = server.await.unwrap();
    let user = json!({"role":"user","content":[{"type":"text","text":"hello"}]});
    let previous = json!({"role":"assistant","content":[{"type":"text","text":"previous answer"}]});
    let first_pause = json!({"role":"assistant","content":[{"type":"thinking","thinking":"only thinking","signature":"SIG_T"}]});
    let next_pause = json!({"role":"assistant","content":[{"type":"thinking","thinking":"next thinking","signature":"SIG_NEXT"}]});
    assert_eq!(
        bodies[1]["messages"],
        json!([user, previous, user, first_pause])
    );
    assert_eq!(
        bodies[2]["messages"],
        json!([user, previous, user, first_pause, next_pause])
    );
}

#[tokio::test]
async fn paused_content_budget_fails_before_another_http_request() {
    use tokio::io::AsyncWriteExt;
    for oversized_calls in [false, true] {
        let payload = if oversized_calls {
            let extra = (3..67).map(|index| format!(
                "event: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":{index},\"content_block\":{{\"type\":\"server_tool_use\",\"id\":\"extra-{index}\",\"name\":\"web_search\",\"input\":{{}}}}}}\n\nevent: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":{index}}}\n\n"
            )).collect::<String>();
            paused_sse().replace(
                "event: message_delta\n",
                &format!("{extra}event: message_delta\n"),
            )
        } else {
            paused_sse().replace("partial so far", &"x".repeat(41_000))
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            support::read_request(&mut socket).await;
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}", payload.len()).as_bytes()).await.unwrap();
            drop(socket);
            tokio::time::timeout(Duration::from_secs(1), listener.accept())
                .await
                .is_err()
        });
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
        while let Some(event) = stream.next().await {
            match event {
                Err(failure) => {
                    error = Some(failure);
                    break;
                }
                Ok(ResponseEvent::Completed { .. }) => panic!("over-budget pause cannot complete"),
                Ok(_) => {}
            }
        }
        assert!(
            error
                .expect("budget failure")
                .to_string()
                .contains("budget")
        );
        assert!(
            server.await.unwrap(),
            "no over-budget continuation may be sent"
        );
    }
}

#[tokio::test]
async fn pending_search_closes_when_the_result_arrives_on_another_pause() {
    let without_index = |excluded| {
        paused_sse()
            .split("\n\n")
            .filter(|frame| {
                frame
                    .lines()
                    .find_map(|line| line.strip_prefix("data: "))
                    .and_then(|data| serde_json::from_str::<serde_json::Value>(data).ok())
                    .is_none_or(|event| event["index"].as_u64() != Some(excluded))
            })
            .map(|frame| format!("{frame}\n\n"))
            .collect::<String>()
    };
    let (address, server) = support::sequence_server(vec![
        without_index(1),
        without_index(0),
        support::ANTHROPIC_SSE.into(),
    ])
    .await;
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
    let mut statuses = Vec::new();
    while let Some(event) = stream.next().await {
        if let ResponseEvent::OutputItemDone(
            codex_protocol::models::ResponseItem::WebSearchCall { status, .. },
        ) = event.unwrap()
        {
            statuses.push(status);
        }
    }
    assert_eq!(
        statuses,
        vec![Some("in_progress".into()), Some("completed".into())]
    );
    assert_eq!(server.await.unwrap().len(), 3);
}

#[tokio::test]
async fn paused_current_and_late_search_results_have_one_citation_owner() {
    let first = paused_sse()
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str::<serde_json::Value>(data).unwrap())
        .filter(|frame| frame["index"].as_u64() != Some(1))
        .map(|frame| {
            format!(
                "event: {}\ndata: {frame}\n\n",
                frame["type"].as_str().unwrap()
            )
        })
        .collect();
    let mut second_frames = paused_rich_sse()
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str::<serde_json::Value>(data).unwrap())
        .collect::<Vec<_>>();
    let terminal = second_frames
        .iter()
        .position(|frame| frame["type"] == "message_delta")
        .unwrap();
    second_frames.splice(terminal..terminal, [
        json!({"type":"content_block_start","index":4,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvu_p1","content":[{"type":"web_search_result","url":"https://example.com/late","encrypted_content":"ENC_LATE"}]}}),
        json!({"type":"content_block_stop","index":4}),
    ]);
    let second = second_frames
        .into_iter()
        .map(|frame| {
            format!(
                "event: {}\ndata: {frame}\n\n",
                frame["type"].as_str().unwrap()
            )
        })
        .collect();
    let (address, server) =
        support::sequence_server(vec![first, second, support::ANTHROPIC_SSE.into()]).await;
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
    let mut searches = Vec::new();
    while let Some(event) = stream.next().await {
        if let ResponseEvent::OutputItemDone(
            codex_protocol::models::ResponseItem::WebSearchCall {
                status,
                wire_blocks,
                ..
            },
        ) = event.unwrap()
        {
            let payload = wire_blocks.unwrap();
            searches.push(json!({"call_id":payload["blocks"][0]["id"], "status":status, "cited_text":payload["cited_text"]}));
        }
    }
    assert_eq!(server.await.unwrap().len(), 3);
    assert_eq!(
        searches,
        vec![
            json!({"call_id":"srvu_p1","status":"in_progress","cited_text":null}),
            json!({"call_id":"srvu_r","status":"completed","cited_text":[{"type":"text","text":"answer with a citation","citations":[{"type":"search_result_location","cited_text":"finding","source":"https://example.com","title":"Example","search_result_index":0,"start_block_index":2,"end_block_index":3}]}]}),
            json!({"call_id":"srvu_p1","status":"completed","cited_text":null}),
        ]
    );
}

/// C3/N5: a paused turn's usage is the FINAL request's Completed counters —
/// the context the last request actually consumed — never a sum across the
/// continuation attempts.
#[tokio::test]
async fn paused_turn_usage_reports_the_final_request_counters() {
    let sse_pause = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg-u1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":40,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"paused usage\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"pause_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":7}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    )
    .to_string();
    // The continuation's Completed frame is the only usage the turn reports.
    let sse_final = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg-u2\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":120,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"done\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":5}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    )
    .to_string();
    let (address, server) = support::sequence_server(vec![sse_pause, sse_final]).await;
    let provider = provider(address);
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let request = support::request(vec![support::user()]);
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
    let mut final_usage = None;
    while let Some(event) = stream.next().await {
        if let codex_api::ResponseEvent::Completed { token_usage, .. } = event.unwrap() {
            final_usage = token_usage;
        }
    }
    server.await.unwrap();
    let usage = final_usage.expect("the turn completes with usage");
    // NOT 40+120=160: the final request's context occupancy is what counts.
    assert_eq!(usage.input_tokens, 120, "{usage:?}");
    assert_eq!(usage.output_tokens, 5, "{usage:?}");
}
