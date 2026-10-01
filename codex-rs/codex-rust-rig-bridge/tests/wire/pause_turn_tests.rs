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
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

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

/// A loopback server that answers each request in sequence with its payload
/// and records every request body.
async fn sequence_server(
    payloads: Vec<String>,
) -> (
    std::net::SocketAddr,
    tokio::task::JoinHandle<Vec<serde_json::Value>>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut bodies = Vec::new();
        for payload in payloads {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let request = support::read_request(&mut socket).await;
            bodies.push(request["body"].clone());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nx-request-id: req\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            socket.write_all(response.as_bytes()).await.expect("write");
        }
        bodies
    });
    (address, server)
}

#[tokio::test]
async fn paused_turn_continues_with_verbatim_assistant_blocks() {
    let (address, server) =
        sequence_server(vec![paused_sse(), support::ANTHROPIC_SSE.to_string()]).await;
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
    let (address, server) = sequence_server(pauses).await;
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
