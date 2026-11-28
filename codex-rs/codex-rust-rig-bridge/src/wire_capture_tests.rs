//! Wire-capture contract tests: a slot records what it is given, in order,
//! and never reopens for a chunk that arrives after an error.

use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use futures::executor::block_on;

use super::Capture;
use super::Slot;
use super::SlotState;

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
    let run_dir = capture.dir.clone();
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
    let request_body =
        std::fs::read(run_dir.join("0001-anthropic.request.body")).expect("body file");
    assert_eq!(request_body, body);
    let summary =
        std::fs::read_to_string(run_dir.join("0001-anthropic.request.json")).expect("json");
    assert!(summary.contains("\"method\": \"POST\""));
    assert!(
        summary.contains("secret"),
        "local debugging keeps headers verbatim"
    );
    let response = std::fs::read(run_dir.join("0001-anthropic.response.sse")).expect("sse file");
    assert_eq!(
        String::from_utf8_lossy(&response),
        "event: message_start\ndata: {}\n\nevent: content_block_delta\ndata: {\"type\":\"thinking_delta\"}\n\n"
    );
}

#[test]
fn numbers_each_call_separately() {
    let dir = temp_dir("sequence");
    let capture = Arc::new(Capture::at(dir.clone()).expect("capture"));
    let run_dir = capture.dir.clone();
    capture.begin(
        "chat",
        "POST",
        "https://gateway.example/v1/chat/completions",
        &[],
        b"{}",
    );
    capture.begin(
        "chat",
        "POST",
        "https://gateway.example/v1/chat/completions",
        &[],
        b"{}",
    );

    assert!(run_dir.join("0001-chat.request.body").exists());
    assert!(run_dir.join("0002-chat.request.body").exists());
}

#[test]
fn records_a_transport_error_and_stops_appending() {
    let dir = temp_dir("error");
    let capture = Arc::new(Capture::at(dir.clone()).expect("capture"));
    let run_dir = capture.dir.clone();
    let slot = capture
        .begin(
            "anthropic",
            "POST",
            "https://gateway.example/anthropic/messages",
            &[],
            b"{}",
        )
        .expect("slot");

    slot.record_error("connection reset");
    slot.record_error("late chunk");

    let response =
        std::fs::read_to_string(run_dir.join("0001-anthropic.response.sse")).expect("sse");
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
    let run_dir = capture.dir.clone();
    let slot = capture
        .begin(
            "chat",
            "POST",
            "https://gateway.example/v1/chat/completions",
            &[],
            b"{}",
        )
        .expect("slot");

    let body: rig_core::http_client::LazyBody =
        Box::pin(async { Ok(Bytes::from_static(b"{\"ok\":true}")) });
    let bytes = block_on(slot.tee_lazy(body)).expect("body");
    assert_eq!(&bytes[..], b"{\"ok\":true}");
    let response = std::fs::read_to_string(run_dir.join("0001-chat.response.sse")).expect("sse");
    assert_eq!(response, "{\"ok\":true}");
}

#[test]
fn stops_at_the_response_cap_and_closes_the_slot() {
    let dir = temp_dir("cap");
    let path = dir.join("capped.sse");
    let file = std::fs::File::create(&path).expect("response file");
    let slot = Arc::new(Slot {
        label: "cap".to_string(),
        cap_bytes: 8,
        state: std::sync::Mutex::new(SlotState {
            file: Some(file),
            written: 0,
        }),
    });

    slot.append(b"12345");
    slot.append(b"6789"); // would cross 8 bytes: dropped whole, never truncated mid-chunk
    slot.append(b"x"); // the slot stays closed
    slot.finish();

    assert_eq!(std::fs::read_to_string(&path).expect("sse"), "12345");
}

#[test]
fn two_instances_sharing_a_root_do_not_overwrite_each_other() {
    let dir = temp_dir("shared-root");
    let first = Arc::new(Capture::at(dir.clone()).expect("first"));
    let second = Arc::new(Capture::at(dir.clone()).expect("second"));

    assert_ne!(
        first.dir, second.dir,
        "each process needs its own run directory, or 0001 collides"
    );
    for capture in [&first, &second] {
        capture.begin("anthropic", "POST", "https://gw/messages", &[], b"{}");
        assert!(capture.dir.join("0001-anthropic.request.body").exists());
    }
}

#[cfg(unix)]
#[test]
fn run_directories_and_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("perms");
    let capture = Arc::new(Capture::at(dir.clone()).expect("capture"));
    capture.begin("anthropic", "POST", "https://gw/messages", &[], b"{}");

    let dir_mode = std::fs::metadata(&capture.dir)
        .expect("dir")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        dir_mode, 0o700,
        "the run directory holds prompts and auth headers"
    );
    let file_mode = std::fs::metadata(capture.dir.join("0001-anthropic.request.json"))
        .expect("json")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o600, "captures must not be world readable");
}
