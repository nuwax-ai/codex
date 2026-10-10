use super::AnthropicUsage;
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
