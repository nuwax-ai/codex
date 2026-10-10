use super::AnthropicUsage;
use super::ChatUsage;
use codex_protocol::protocol::ReportedResponseUsage;
use codex_protocol::protocol::ReportedUsageCounters;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn absent_thinking_counter_does_not_become_zero_or_reset_a_report() {
    let mut usage = AnthropicUsage::default();
    usage.observe(&json!({"type":"message_start","message":{"usage":{
        "input_tokens":0,"output_tokens_details":{}
    }}}));
    assert_eq!(
        usage.snapshot_report(),
        ReportedResponseUsage::from_counters(ReportedUsageCounters {
            input_tokens: Some(0),
            ..Default::default()
        })
    );
    usage.observe(&json!({"type":"message_delta","usage":{
        "output_tokens_details":{"thinking_tokens":7}
    }}));
    usage.observe(&json!({"type":"message_delta","usage":{
        "output_tokens_details":{}
    }}));
    assert_eq!(
        usage.snapshot_report(),
        ReportedResponseUsage::from_counters(ReportedUsageCounters {
            input_tokens: Some(0),
            reasoning_output_tokens: Some(7),
            ..Default::default()
        })
    );
    usage.observe(&json!({"type":"message_delta","usage":{
        "output_tokens_details":{"thinking_tokens":0}
    }}));
    assert_eq!(
        usage.snapshot_report(),
        ReportedResponseUsage::from_counters(ReportedUsageCounters {
            input_tokens: Some(0),
            reasoning_output_tokens: Some(0),
            ..Default::default()
        })
    );
}

#[test]
fn out_of_range_wire_counters_are_unreported_instead_of_negative() {
    let mut usage = AnthropicUsage::default();
    usage.observe(&json!({"type":"message_start","message":{"usage":{
        "input_tokens":u64::MAX,"output_tokens":0,
        "cache_read_input_tokens":u64::MAX,"cache_creation_input_tokens":u64::MAX,
        "output_tokens_details":{"thinking_tokens":u64::MAX}
    }}}));
    assert_eq!(
        usage.snapshot_report(),
        ReportedResponseUsage::from_counters(ReportedUsageCounters {
            output_tokens: Some(0),
            ..Default::default()
        })
    );
}

#[test]
fn chat_usage_object_replaces_its_snapshot_with_wire_presence() {
    let mut usage = ChatUsage::default();
    assert_eq!(usage.snapshot_report(), None, "nothing observed yet");
    usage.observe(&json!({"choices":[{"index":0,"delta":{}}]}));
    assert_eq!(
        usage.snapshot_report(),
        None,
        "frames without a usage object leave the snapshot empty"
    );
    usage.observe(&json!({
        "choices":[{"index":0,"delta":{},"finish_reason":null}],
        "usage":{"prompt_tokens":4,"completion_tokens":10,"total_tokens":14}
    }));
    assert_eq!(
        usage.snapshot_report(),
        ReportedResponseUsage::from_counters(ReportedUsageCounters {
            input_tokens: Some(4),
            output_tokens: Some(10),
            total_tokens: Some(14),
            ..Default::default()
        })
    );
    // A later complete usage object replaces the snapshot; partial details
    // counters keep their own presence.
    usage.observe(&json!({
        "choices":[],
        "usage":{
            "prompt_tokens":4,
            "prompt_tokens_details":{"cached_tokens":2},
            "completion_tokens":64,
            "completion_tokens_details":{"reasoning_tokens":0},
            "total_tokens":68
        }
    }));
    assert_eq!(
        usage.snapshot_report(),
        ReportedResponseUsage::from_counters(ReportedUsageCounters {
            input_tokens: Some(4),
            cached_input_tokens: Some(2),
            output_tokens: Some(64),
            reasoning_output_tokens: Some(0),
            total_tokens: Some(68),
            ..Default::default()
        })
    );
    // Null usage is not an object: the previous snapshot survives.
    usage.observe(&json!({"choices":[],"usage":null}));
    assert!(usage.snapshot_report().is_some());
}

#[test]
fn chat_usage_explicit_zero_counters_stay_reported() {
    let mut usage = ChatUsage::default();
    usage.observe(&json!({
        "choices":[],
        "usage":{"prompt_tokens":0,"completion_tokens":0,"total_tokens":0}
    }));
    let report = usage
        .snapshot_report()
        .expect("an all-zero but fully reported usage object is a report");
    // Complete requires EVERY counter reported directly; the Chat wire has
    // no cache-write counter, so even a full Chat usage object stays
    // formally Incomplete and claims no normalized snapshot.
    assert_eq!(
        report.completeness,
        codex_protocol::protocol::UsageCompleteness::Incomplete
    );
    assert!(report.normalized.is_none());
}
