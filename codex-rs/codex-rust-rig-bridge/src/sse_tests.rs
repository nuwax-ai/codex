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
    assert_eq!(
        normalized,
        rig_core::completion::Usage {
            input_tokens: safe,
            output_tokens: safe,
            cached_input_tokens: safe,
            cache_creation_input_tokens: safe,
            total_tokens: safe * 4,
            ..Default::default()
        }
    );
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
    assert_eq!(
        normalized,
        rig_core::completion::Usage {
            input_tokens: u64::MAX,
            output_tokens: u64::MAX,
            cached_input_tokens: 7,
            cache_creation_input_tokens: 9,
            total_tokens: u64::MAX,
            ..Default::default()
        }
    );
}
