//! R4: dropping the stream cancels the turn — the pump stops waiting on the
//! model, the in-flight connection is released promptly, and no second
//! request (continuation, retry or tool re-run) follows.

use super::error_tests::provider;
use super::support;
use codex_api::ResponseEvent;
use codex_rust_rig_bridge::RigProtocol;
use codex_rust_rig_bridge::stream_via_rig;
use futures::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

/// One stalled SSE response: headers claim a body larger than what is sent,
/// then the server holds the connection without another model event.
const PARTIAL_SSE: &str = concat!(
    "event: message_start\n",
    "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
    "event: content_block_start\n",
    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n",
    "event: content_block_stop\n",
    "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
);

#[tokio::test]
async fn dropping_the_stream_cancels_the_in_flight_request() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("first request");
        let request = support::read_request(&mut socket).await;
        let body = PARTIAL_SSE;
        // Chunked with the terminating zero-chunk never sent: the client
        // receives complete SSE frames and then the stream stalls.
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nx-request-id: req\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.expect("write");
        // Hold the connection open with no further model events; observe
        // when the client aborts.
        let mut probe = [0; 64];
        let aborted = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match socket.read(&mut probe).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        })
        .await;
        // No second request may arrive after the cancel.
        let second = tokio::time::timeout(Duration::from_millis(750), listener.accept()).await;
        (request, aborted.is_ok(), second.is_err())
    });

    let provider = provider(address);
    let mut request = support::request(vec![support::user()]);
    support::set_tools(&mut request, serde_json::json!([{"type":"web_search"}]));
    let auth: codex_api::SharedAuthProvider = Arc::new(support::DummyAuth);
    let mut stream = stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(30),
    )
    .await
    .unwrap();
    // Wait for the turn to start, then cancel it.
    while let Some(event) = stream.next().await {
        if matches!(event.unwrap(), ResponseEvent::Created { .. }) {
            break;
        }
    }
    drop(stream);

    let (request, aborted, no_second) = server.await.unwrap();
    assert_eq!(request["body"]["messages"][0]["role"], "user");
    assert!(
        aborted,
        "the connection must be released promptly after the drop"
    );
    assert!(
        no_second,
        "no continuation or retry request may follow a cancel"
    );
}
