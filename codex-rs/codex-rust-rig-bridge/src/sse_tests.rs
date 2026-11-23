use super::*;
use futures::TryStreamExt;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

async fn checked_anthropic_frames(events: &[Value]) -> (Vec<Value>, crate::usage::AnthropicUsage) {
    let wire = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    let usage = Arc::new(Mutex::new(crate::usage::AnthropicUsage::default()));
    let body: BoxedStream = Box::pin(futures::stream::iter([Ok(Bytes::from(wire))]));
    let frames = with_terminal_check(
        body,
        RigProtocol::Anthropic,
        WireUsageObserver::Anthropic(Arc::clone(&usage)),
    )
    .eventsource()
    .map_ok(|event| serde_json::from_str::<Value>(&event.data).expect("decoded frame"))
    .try_collect::<Vec<_>>()
    .await
    .expect("complete Anthropic stream");
    let usage = Arc::try_unwrap(usage)
        .ok()
        .expect("stream releases its usage observer")
        .into_inner()
        .expect("usage state");
    (frames, usage)
}

#[tokio::test]
async fn safe_anthropic_counter_combinations_preserve_frames_and_terminal_order() {
    let safe = u64::MAX / 4;
    let events = vec![
        json!({"type":"message_start","message":{"usage":{
            "input_tokens":safe,"output_tokens":0
        }}}),
        json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{
            "input_tokens":safe,"output_tokens":safe,
            "cache_read_input_tokens":safe,"cache_creation_input_tokens":safe
        }}),
        json!({"type":"message_stop"}),
    ];
    let (frames, usage) = checked_anthropic_frames(&events).await;
    assert_eq!(frames, events);
    let mut normalized = rig_core::completion::Usage::new();
    usage.apply(&mut normalized);
    // 0.44 semantics: input already includes cache read+write.
    assert_eq!(normalized.input_tokens, Some(safe * 3));
    assert_eq!(normalized.output_tokens, Some(safe));
    assert_eq!(normalized.cached_input_tokens, Some(safe));
    assert_eq!(normalized.cache_creation_input_tokens, Some(safe));
    assert_eq!(normalized.total_tokens, Some(safe * 4));
    assert_eq!(normalized.reasoning_tokens, None);
}

#[tokio::test]
async fn unsafe_anthropic_counters_only_change_the_rig_decoder_copy() {
    let events = vec![
        json!({"type":"message_start","message":{"usage":{
            "input_tokens":u64::MAX,"output_tokens":0,"cache_read_input_tokens":7
        }}}),
        json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{
            "output_tokens":u64::MAX,"cache_creation_input_tokens":9
        }}),
        json!({"type":"message_stop"}),
    ];
    let (frames, usage) = checked_anthropic_frames(&events).await;
    let mut decoder_events = events;
    decoder_events[0]["message"]["usage"]["input_tokens"] = json!(0);
    decoder_events[1]["usage"]["output_tokens"] = json!(0);
    assert_eq!(frames, decoder_events);
    assert_eq!(
        usage.snapshot_report(),
        codex_protocol::protocol::ReportedResponseUsage::from_counters(
            codex_protocol::protocol::ReportedUsageCounters {
                cached_input_tokens: Some(7),
                cache_write_input_tokens: Some(9),
                ..Default::default()
            }
        )
    );
    let mut normalized = rig_core::completion::Usage::new();
    usage.apply(&mut normalized);
    // Saturating: u64::MAX input + cache parts still saturate, never wrap.
    assert_eq!(normalized.input_tokens, Some(u64::MAX));
    assert_eq!(normalized.output_tokens, Some(u64::MAX));
    assert_eq!(normalized.cached_input_tokens, Some(7));
    assert_eq!(normalized.cache_creation_input_tokens, Some(9));
    assert_eq!(normalized.total_tokens, Some(u64::MAX));
}

/// The Chat side of the same check: frames reach rig unchanged, and the
/// stream's end is not the bridge's to judge.
async fn checked_chat_frames(
    events: &[Value],
) -> (
    Vec<Value>,
    Option<codex_protocol::protocol::ReportedResponseUsage>,
) {
    let wire = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    let usage = Arc::new(Mutex::new(crate::usage::ChatUsage::default()));
    let body: BoxedStream = Box::pin(futures::stream::iter([Ok(Bytes::from(wire))]));
    let frames = with_terminal_check(
        body,
        RigProtocol::Chat,
        WireUsageObserver::Chat(Arc::clone(&usage)),
    )
    .eventsource()
    .map_ok(|event| serde_json::from_str::<Value>(&event.data).expect("decoded frame"))
    .try_collect::<Vec<_>>()
    .await
    .expect("the bridge must not pre-empt rig's Chat terminal decision");
    let report = usage
        .lock()
        .map(|usage| usage.snapshot_report())
        .ok()
        .flatten();
    (frames, report)
}

/// MiniMax-M3's dialect: the provider ends the turn with `finish_reason` and
/// closes without `data: [DONE]`. Every frame is forwarded and the terminal
/// decision — complete here, not truncated — stays rig's.
#[tokio::test]
async fn chat_eof_after_a_finish_reason_is_not_a_bridge_error() {
    let events = vec![
        json!({"id":"chatcmpl-sentinel-less","object":"chat.completion.chunk","created":1,"model":"server-model","choices":[{"index":0,"delta":{"role":"assistant","content":"ok"},"finish_reason":null}]}),
        json!({"id":"chatcmpl-sentinel-less","object":"chat.completion.chunk","created":1,"model":"server-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":1,"total_tokens":5}}),
    ];
    let (frames, report) = checked_chat_frames(&events).await;
    assert_eq!(frames, events);
    assert_eq!(
        report,
        codex_protocol::protocol::ReportedResponseUsage::from_counters(
            codex_protocol::protocol::ReportedUsageCounters {
                input_tokens: Some(4),
                output_tokens: Some(1),
                total_tokens: Some(5),
                ..Default::default()
            }
        ),
        "the terminal chunk's usage is observed before the body ends"
    );
}

/// A body cut before any `finish_reason` is rig's
/// `ProviderError::Truncated`; the bridge must let the frames run out rather
/// than report an error of its own, so one place owns the decision.
#[tokio::test]
async fn chat_eof_before_a_finish_reason_is_left_to_rig() {
    let events = vec![
        json!({"id":"chatcmpl-cut","object":"chat.completion.chunk","created":1,"model":"server-model","choices":[{"index":0,"delta":{"content":"partial"},"finish_reason":null}]}),
    ];
    let (frames, report) = checked_chat_frames(&events).await;
    assert_eq!(frames, events);
    assert_eq!(report, None, "no usage object reached the wire");
}

const ANTHROPIC_START: &str = "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n";

/// Drives the Anthropic wrapper on a scripted body: the frames it let through,
/// and the error it ended with, if it failed.
async fn anthropic_outcome(chunks: Vec<Result<Bytes, Error>>) -> (usize, Option<Error>) {
    let usage = Arc::new(Mutex::new(crate::usage::AnthropicUsage::default()));
    let body: BoxedStream = Box::pin(futures::stream::iter(chunks));
    let items: Vec<Result<Bytes, Error>> = with_terminal_check(
        body,
        RigProtocol::Anthropic,
        WireUsageObserver::Anthropic(Arc::clone(&usage)),
    )
    .collect()
    .await;
    let frames = items.iter().flatten().count();
    let error = items.into_iter().rev().find_map(Result::err);
    (frames, error)
}

/// A transport error of the inner body is already an `http_client::Error`:
/// rig's own verdict, or a preserved non-success status. It must surface
/// unchanged. Wrapping it as `Transient` made a `Permanent` failure and a
/// 4xx/5xx reply retryable, and hid the status codex-core's recovery reads.
#[tokio::test]
async fn an_inner_transport_error_keeps_its_own_verdict_and_status() {
    let preserved = |status| Error::InvalidStatusCodeWithDetails {
        status,
        body: "the provider's own reply".into(),
        headers: http::HeaderMap::new(),
    };
    let cases = [
        // A 5xx reply is retryable and a 401 is not: the status decides, and
        // codex-core's 401 recovery must still see it.
        (
            preserved(http::StatusCode::BAD_GATEWAY),
            true,
            Some(http::StatusCode::BAD_GATEWAY),
        ),
        (
            preserved(http::StatusCode::UNAUTHORIZED),
            false,
            Some(http::StatusCode::UNAUTHORIZED),
        ),
        (
            Error::permanent(std::io::Error::other("decode pipe closed")),
            false,
            None,
        ),
        (
            Error::transient(std::io::Error::other("connection reset")),
            true,
            None,
        ),
    ];
    for (inner, transient, status) in cases {
        let (frames, error) =
            anthropic_outcome(vec![Ok(Bytes::from(ANTHROPIC_START)), Err(inner)]).await;
        let error = error.unwrap_or_else(|| panic!("the transport error must end the stream"));
        assert_eq!(frames, 1, "the frame before the error still reaches rig");
        assert_eq!(error.is_transient(), transient, "{error}");
        assert_eq!(error.non_success_status(), status, "{error}");
    }
}

/// The Anthropic wrapper owns the truncation verdict, as FORK.md §4 records by
/// this very text: a body that ends before `message_stop` fails, and the next
/// attempt may complete.
#[tokio::test]
async fn a_body_ending_before_the_anthropic_terminal_is_a_retryable_named_failure() {
    let (frames, error) = anthropic_outcome(vec![Ok(Bytes::from(ANTHROPIC_START))]).await;
    let error = error.expect("a reply without message_stop must not end as a success");
    assert_eq!(frames, 1);
    assert!(
        error.is_transient(),
        "a reply cut short may end on the next attempt: {error}"
    );
    assert!(
        error
            .to_string()
            .contains("model stream ended before its SSE terminal"),
        "the failure FORK.md documents by this text: {error}"
    );
}

/// A malformed counter is a failure the fork reports rather than folding into
/// a default terminal, and — as before rig 0.47 — a retryable one.
#[tokio::test]
async fn a_malformed_usage_counter_fails_the_stream_as_retryable() {
    let frame = "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":\"many\",\"output_tokens\":0}}}\n\n";
    let (_frames, error) = anthropic_outcome(vec![Ok(Bytes::from(frame))]).await;
    let error = error.expect("a malformed counter must fail the stream");
    assert!(error.is_transient(), "{error}");
}
