//! Unit tests for the Responses passthrough: cross-protocol history
//! projection and the strict terminal pump. Wire-level behavior (outbound
//! JSON, headers, SSE fixtures over real HTTP) lives in `tests/wire/`.

use std::time::Duration;

use bytes::Bytes;
use codex_api::ApiError;
use codex_api::ResponseEvent;
use codex_api::TransportError;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
use futures::StreamExt;
use futures::stream;
use pretty_assertions::assert_eq;

use crate::responses::project_cross_protocol_history;

fn reasoning_item(
    encrypted: Option<String>,
    summary: Vec<ReasoningItemReasoningSummary>,
    content: Option<Vec<ReasoningItemContent>>,
) -> ResponseItem {
    ResponseItem::Reasoning {
        id: None,
        summary,
        content,
        encrypted_content: encrypted,
        internal_chat_message_metadata_passthrough: None,
    }
}

#[test]
fn projection_clears_envelopes_but_keeps_the_visible_item() {
    let summary = vec![ReasoningItemReasoningSummary::SummaryText {
        text: "thought".into(),
    }];
    let content = Some(vec![ReasoningItemContent::ReasoningText {
        text: "visible thinking".into(),
    }]);
    let mut input = vec![
        ResponseItem::Message {
            id: None,
            role: "user".into(),
            content: vec![],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        reasoning_item(
            Some("codex-rig-reasoning-v1:source-payload".to_string()),
            summary.clone(),
            content.clone(),
        ),
    ];
    let projected = project_cross_protocol_history(&mut input);
    assert_eq!(projected, 1);
    // The reasoning item survives with its visible payload intact; only the
    // envelope payload is cleared (it serializes as `encrypted_content: null`,
    // the same shape as any ciphertext-less reasoning item).
    assert_eq!(input[1], reasoning_item(None, summary, content));
}

#[test]
fn projection_passes_non_envelope_ciphertext_and_plain_items_through() {
    let mut input = vec![
        reasoning_item(
            Some("gAAAA-same-gateway-ciphertext".to_string()),
            vec![],
            None,
        ),
        reasoning_item(None, vec![], None),
        ResponseItem::FunctionCall {
            id: None,
            name: "lookup".into(),
            namespace: None,
            arguments: "{}".into(),
            encrypted_function_args: None,
            call_id: "c1".into(),
            internal_chat_message_metadata_passthrough: None,
        },
    ];
    let before = input.clone();
    let projected = project_cross_protocol_history(&mut input);
    assert_eq!(projected, 0, "nothing to project");
    assert_eq!(input, before, "verbatim passthrough");
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
    let mut stream = codex_api::spawn_strict_response_stream(
        codex_api::StreamResponse {
            status: http::StatusCode::OK,
            headers: http::HeaderMap::new(),
            bytes: byte_stream,
        },
        idle_timeout,
        /*turn_state*/ None,
    );
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
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
    let mut stream = codex_api::spawn_strict_response_stream(
        codex_api::StreamResponse {
            status: http::StatusCode::OK,
            headers: http::HeaderMap::new(),
            bytes: pending,
        },
        Duration::from_millis(50),
        /*turn_state*/ None,
    );
    let event = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("pump must terminate")
        .expect("channel must yield the timeout error");
    let matches_timeout = matches!(
        &event,
        Err(ApiError::Stream(message)) if message.contains("idle timeout")
    );
    assert!(matches_timeout, "unexpected event: {event:?}");
}
