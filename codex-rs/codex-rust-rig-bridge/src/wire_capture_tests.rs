//! Wire-capture contract tests: a slot records what it is given, in order,
//! and never reopens for a chunk that arrives after an error.

use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use futures::executor::block_on;
use futures::StreamExt;

use super::Capture;

fn temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "wire-capture-{label}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn stream_of(chunks: Vec<&'static [u8]>) -> rig_core::http_client::BoxedStream {
    let items: Vec<Result<Bytes, rig_core::http_client::Error>> = chunks
        .into_iter()
        .map(|chunk| Ok(Bytes::from_static(chunk)))
        .collect();
    Box::pin(futures::stream::iter(items)) as rig_core::http_client::BoxedStream
}

#[test]
fn writes_the_request_and_the_streamed_response_in_order() {
    let dir = temp_dir("stream");
    let capture = Arc::new(Capture::at(dir.clone()).expect("capture"));
    let body = br#"{"model":"mimo-v2.6-flash","stream":true}"#;
    let slot = capture
        .begin(
            "anthropic",
            "POST",
            "https://gateway.example/anthropic/messages",
            &[("x-api-key".to_string(), "secret".to_string())],
            body,
        )
        .expect("slot");

    let teed = slot.tee_stream(stream_of(vec![
        b"event: message_start\ndata: {}\n\n",
        b"event: content_block_delta\ndata: {\"type\":\"thinking_delta\"}\n\n",
    ]));
    let drained: Vec<_> = block_on(teed.collect());

    assert_eq!(drained.len(), 2, "the tee must pass every chunk through");
    let request_body = std::fs::read(dir.join("0001-anthropic.request.body")).expect("body file");
    assert_eq!(request_body, body);
    let summary = std::fs::read_to_string(dir.join("0001-anthropic.request.json")).expect("json");
    assert!(summary.contains("\"method\": \"POST\""));
    assert!(summary.contains("secret"), "local debugging keeps headers verbatim");
    let response = std::fs::read(dir.join("0001-anthropic.response.sse")).expect("sse file");
    assert_eq!(
        String::from_utf8_lossy(&response),
        "event: message_start\ndata: {}\n\nevent: content_block_delta\ndata: {\"type\":\"thinking_delta\"}\n\n"
    );
}

#[test]
fn numbers_each_call_separately() {
    let dir = temp_dir("sequence");
    let capture = Arc::new(Capture::at(dir.clone()).expect("capture"));
    capture.begin("chat", "POST", "https://gateway.example/v1/chat/completions", &[], b"{}");
    capture.begin("chat", "POST", "https://gateway.example/v1/chat/completions", &[], b"{}");

    assert!(dir.join("0001-chat.request.body").exists());
    assert!(dir.join("0002-chat.request.body").exists());
}

#[test]
fn records_a_transport_error_and_stops_appending() {
    let dir = temp_dir("error");
    let capture = Arc::new(Capture::at(dir.clone()).expect("capture"));
    let slot = capture
        .begin("anthropic", "POST", "https://gateway.example/anthropic/messages", &[], b"{}")
        .expect("slot");

    slot.record_error("connection reset");
    slot.record_error("late chunk");

    let response = std::fs::read_to_string(dir.join("0001-anthropic.response.sse")).expect("sse");
    assert!(response.contains("connection reset"));
    assert!(
        !response.contains("late chunk"),
        "a closed slot must not accept later writes"
    );
}

#[test]
fn records_a_lazily_read_body() {
    let dir = temp_dir("lazy");
    let capture = Arc::new(Capture::at(dir.clone()).expect("capture"));
    let slot = capture
        .begin("chat", "POST", "https://gateway.example/v1/chat/completions", &[], b"{}")
        .expect("slot");

    let body: rig_core::http_client::LazyBody = Box::pin(async { Ok(Bytes::from_static(b"{\"ok\":true}")) });
    let bytes = block_on(slot.tee_lazy(body)).expect("body");
    assert_eq!(&bytes[..], b"{\"ok\":true}");
    let response = std::fs::read_to_string(dir.join("0001-chat.response.sse")).expect("sse");
    assert_eq!(response, "{\"ok\":true}");
}
