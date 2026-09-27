//! Unit tests for the Responses passthrough: cross-protocol history rejection
//! and the strict terminal pump. Wire-level behavior (outbound JSON, headers,
//! SSE fixtures over real HTTP) lives in `tests/wire/`.

use std::time::Duration;

use bytes::Bytes;
use codex_api::ApiError;
use codex_api::ResponseEvent;
use codex_api::TransportError;
use codex_protocol::models::ResponseItem;
use futures::stream;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use crate::responses::reject_cross_protocol_history;

fn reasoning_item(encrypted: Option<String>) -> ResponseItem {
    ResponseItem::Reasoning {
        id: None,
        summary: Vec::new(),
        content: None,
        encrypted_content: encrypted,
        internal_chat_message_metadata_passthrough: None,
    }
}

#[test]
fn rejects_history_carrying_rig_replay_envelopes() {
    let input = vec![reasoning_item(Some(
        "codex-rig-reasoning-v1:0123abcd".to_string(),
    ))];
    let error = reject_cross_protocol_history(&input).expect_err("envelope must be rejected");
    assert!(
        matches!(error, ApiError::InvalidRequest { .. }),
        "expected InvalidRequest, got {error:?}"
    );
}

#[test]
fn accepts_native_and_absent_encrypted_reasoning() {
    let input = vec![
        reasoning_item(None),
        reasoning_item(Some("real-provider-ciphertext".to_string())),
        ResponseItem::Message {
            id: None,
            role: "user".into(),
            content: vec![],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
    ];
    assert!(reject_cross_protocol_history(&input).is_ok());
}

// ---------------------------------------------------------------------
// Strict pump
// ---------------------------------------------------------------------

fn sse_frame(data: &str) -> Bytes {
    Bytes::from(format!("event: message\ndata: {data}\n\n"))
}

async fn run_pump(frames: Vec<Bytes>) -> Vec<Result<ResponseEvent, ApiError>> {
    run_pump_with_timeout(frames, Duration::from_secs(5)).await
}

async fn run_pump_with_timeout(
    frames: Vec<Bytes>,
    idle_timeout: Duration,
) -> Vec<Result<ResponseEvent, ApiError>> {
    let byte_stream: codex_api::ByteStream = Box::pin(stream::iter(
        frames.into_iter().map(Ok::<_, TransportError>),
    ));
    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(crate::responses::strict_responses_pump(
        byte_stream,
        tx,
        idle_timeout,
    ));
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

fn created_frame() -> Bytes {
    sse_frame(r#"{"type":"response.created","response":{"id":"resp_1"}}"#)
}

fn message_done_frame() -> Bytes {
    sse_frame(
        r#"{"type":"response.output_item.done","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hi"}]}}"#,
    )
}

fn completed_frame() -> Bytes {
    sse_frame(
        r#"{"type":"response.completed","response":{"id":"resp_1","usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}}"#,
    )
}

#[tokio::test]
async fn happy_path_emits_events_and_stops_at_completed() {
    let events = run_pump(vec![
        created_frame(),
        message_done_frame(),
        completed_frame(),
    ])
    .await;
    assert_eq!(events.len(), 3, "{events:?}");
    assert!(
        matches!(&events[0], Ok(ResponseEvent::Created { response_id }) if response_id.as_deref() == Some("resp_1"))
    );
    assert!(matches!(&events[1], Ok(ResponseEvent::OutputItemDone(_))));
    assert!(matches!(&events[2], Ok(ResponseEvent::Completed { .. })));
}

#[tokio::test]
async fn done_sentinel_frames_are_tolerated() {
    let events = run_pump(vec![
        created_frame(),
        sse_frame("[DONE]"),
        completed_frame(),
    ])
    .await;
    assert_eq!(events.len(), 2, "{events:?}");
    assert!(matches!(&events[1], Ok(ResponseEvent::Completed { .. })));
}

#[tokio::test]
async fn malformed_json_frame_fails_the_turn() {
    let events = run_pump(vec![created_frame(), sse_frame("{not json")]).await;
    assert_eq!(events.len(), 2, "{events:?}");
    let error = events[1].as_ref().expect_err("malformed frame must error");
    assert!(
        matches!(error, ApiError::Stream(message) if message.contains("malformed")),
        "unexpected error: {error:?}"
    );
}

#[tokio::test]
async fn critical_event_with_missing_fields_fails_the_turn() {
    // response.created without the required `response` payload.
    let events = run_pump(vec![sse_frame(r#"{"type":"response.created"}"#)]).await;
    assert_eq!(events.len(), 1, "{events:?}");
    let error = events[0]
        .as_ref()
        .expect_err("corrupted critical event must error");
    assert!(
        matches!(error, ApiError::Stream(message) if message.contains("response.created")
            && message.contains("missing required fields")),
        "unexpected error: {error:?}"
    );
}

#[tokio::test]
async fn unknown_non_critical_events_are_ignored() {
    let events = run_pump(vec![
        sse_frame(r#"{"type":"response.in_progress","response":{"id":"resp_1"}}"#),
        sse_frame(r#"{"type":"response.output_text.done","text":"hi"}"#),
        completed_frame(),
    ])
    .await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(matches!(&events[0], Ok(ResponseEvent::Completed { .. })));
}

#[tokio::test]
async fn failed_response_surfaces_immediately_without_completed() {
    let events = run_pump(vec![sse_frame(
        r#"{"type":"response.failed","response":{"id":"resp_1","error":{"code":"context_length_exceeded","message":"too long"}}}"#,
    )])
    .await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(matches!(&events[0], Err(ApiError::ContextWindowExceeded)));
}

#[tokio::test]
async fn truncated_stream_reports_missing_completion() {
    let events = run_pump(vec![created_frame(), message_done_frame()]).await;
    assert_eq!(events.len(), 3, "{events:?}");
    let error = events[2].as_ref().expect_err("truncation must error");
    assert!(
        matches!(error, ApiError::Stream(message) if message.contains("stream closed before response.completed")),
        "unexpected error: {error:?}"
    );
}

#[tokio::test]
async fn idle_timeout_errors_instead_of_hanging() {
    let pending: codex_api::ByteStream = Box::pin(stream::pending());
    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(crate::responses::strict_responses_pump(
        pending,
        tx,
        Duration::from_millis(50),
    ));
    let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("pump must terminate")
        .expect("channel must yield the timeout error");
    let matches_timeout = matches!(
        &event,
        Err(ApiError::Stream(message)) if message.contains("idle timeout")
    );
    assert!(matches_timeout, "unexpected event: {event:?}");
}
