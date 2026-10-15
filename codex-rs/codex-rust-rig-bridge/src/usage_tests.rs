use super::AnthropicUsage;
use super::ChatUsage;
use codex_protocol::protocol::ReportedResponseUsage;
use codex_protocol::protocol::ReportedUsageCounters;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn absent_thinking_counter_does_not_become_zero_or_reset_a_report() {
    let mut usage = AnthropicUsage::default();
    usage
        .observe(&json!({"type":"message_start","message":{"usage":{
            "input_tokens":0,"output_tokens_details":{}
        }}}))
        .expect("start usage");
    assert_eq!(
        usage.snapshot_report(),
        ReportedResponseUsage::from_counters(ReportedUsageCounters {
            input_tokens: Some(0),
            ..Default::default()
        })
    );
    usage
        .observe(&json!({"type":"message_delta","usage":{
            "output_tokens_details":{"thinking_tokens":7}
        }}))
        .expect("delta usage");
    usage
        .observe(&json!({"type":"message_delta","usage":{
            "output_tokens_details":{}
        }}))
        .expect("delta usage");
    assert_eq!(
        usage.snapshot_report(),
        ReportedResponseUsage::from_counters(ReportedUsageCounters {
            input_tokens: Some(0),
            reasoning_output_tokens: Some(7),
            ..Default::default()
        })
    );
    usage
        .observe(&json!({"type":"message_delta","usage":{
            "output_tokens_details":{"thinking_tokens":0}
        }}))
        .expect("delta usage");
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
    usage
        .observe(&json!({"type":"message_start","message":{"usage":{
            "input_tokens":u64::MAX,"output_tokens":0,
            "cache_read_input_tokens":u64::MAX,"cache_creation_input_tokens":u64::MAX,
            "output_tokens_details":{"thinking_tokens":u64::MAX}
        }}}))
        .expect("start usage");
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
    usage
        .observe(&json!({"choices":[{"index":0,"delta":{}}]}))
        .expect("frame without usage");
    assert_eq!(
        usage.snapshot_report(),
        None,
        "frames without a usage object leave the snapshot empty"
    );
    usage
        .observe(&json!({
            "choices":[{"index":0,"delta":{},"finish_reason":null}],
            "usage":{"prompt_tokens":4,"completion_tokens":10,"total_tokens":14}
        }))
        .expect("usage object");
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
    usage
        .observe(&json!({
            "choices":[],
            "usage":{
                "prompt_tokens":4,
                "prompt_tokens_details":{"cached_tokens":2},
                "completion_tokens":64,
                "completion_tokens_details":{"reasoning_tokens":0},
                "total_tokens":68
            }
        }))
        .expect("usage object");
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
    usage
        .observe(&json!({"choices":[],"usage":null}))
        .expect("null usage");
    assert!(usage.snapshot_report().is_some());
}

#[test]
fn chat_usage_explicit_zero_counters_stay_reported() {
    let mut usage = ChatUsage::default();
    usage
        .observe(&json!({
            "choices":[],
            "usage":{"prompt_tokens":0,"completion_tokens":0,"total_tokens":0}
        }))
        .expect("usage object");
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

#[test]
fn chat_negative_counters_fail_the_frame_instead_of_vanishing() {
    let mut usage = ChatUsage::default();
    let error = usage
        .observe(&json!({
            "choices":[],
            "usage":{"prompt_tokens":-5,"completion_tokens":64,"total_tokens":70}
        }))
        .expect_err("a negative counter is malformed, not absent");
    assert!(
        error.contains("invalid value: integer `-5`"),
        "message names the offending counter: {error}"
    );
    assert_eq!(
        usage.snapshot_report(),
        None,
        "a malformed frame must not partially update the snapshot"
    );
    // Detail counters fail the same way.
    let error = usage
        .observe(&json!({
            "choices":[],
            "usage":{
                "prompt_tokens":4,
                "prompt_tokens_details":{"cached_tokens":"many"},
                "completion_tokens":64,
                "total_tokens":70
            }
        }))
        .expect_err("a non-integer detail counter is malformed");
    assert!(
        error.contains("invalid value: integer `\"many\"`, expected u64 at `cached_tokens`"),
        "message names the offending counter: {error}"
    );
    // Overflow inside the wire's unsigned range stays a valid observation;
    // only its i64 report saturates out.
    usage
        .observe(&json!({
            "choices":[],
            "usage":{"prompt_tokens":u64::MAX,"completion_tokens":1,"total_tokens":u64::MAX}
        }))
        .expect("u64::MAX is a valid wire counter");
    assert_eq!(
        usage.snapshot_report(),
        ReportedResponseUsage::from_counters(ReportedUsageCounters {
            output_tokens: Some(1),
            ..Default::default()
        })
    );
}

#[test]
fn anthropic_missing_usage_object_fails_the_required_frames() {
    let mut usage = AnthropicUsage::default();
    let error = usage
        .observe(&json!({"type":"message_start","message":{
            "id":"m","role":"assistant","content":[]
        }}))
        .expect_err("message_start must carry usage");
    assert!(
        error.contains("missing field `usage`"),
        "message names the missing object: {error}"
    );
    usage
        .observe(&json!({"type":"message_start","message":{"usage":{
            "input_tokens":9,"output_tokens":11
        }}}))
        .expect("start usage");
    let error = usage
        .observe(&json!({"type":"message_delta","delta":{
            "stop_reason":"end_turn","stop_sequence":null
        }}))
        .expect_err("message_delta must carry usage");
    assert!(
        error.contains("missing field `usage`"),
        "message names the missing object: {error}"
    );
    assert_eq!(
        usage.snapshot_report(),
        ReportedResponseUsage::from_counters(ReportedUsageCounters {
            input_tokens: Some(9),
            output_tokens: Some(11),
            ..Default::default()
        }),
        "a rejected frame keeps the last good snapshot for diagnosis"
    );
}

#[test]
fn anthropic_negative_counters_fail_the_frame_instead_of_vanishing() {
    let mut usage = AnthropicUsage::default();
    usage
        .observe(&json!({"type":"message_start","message":{"usage":{
            "input_tokens":9,"output_tokens":11
        }}}))
        .expect("start usage");
    let error = usage
        .observe(
            &json!({"type":"message_delta","delta":{"stop_reason":null},"usage":{
                "input_tokens":-2,"output_tokens":13
            }}),
        )
        .expect_err("a negative counter is malformed, not absent");
    assert!(
        error.contains("invalid value: integer `-2`"),
        "message names the offending counter: {error}"
    );
    assert_eq!(
        usage.snapshot_report(),
        ReportedResponseUsage::from_counters(ReportedUsageCounters {
            input_tokens: Some(9),
            output_tokens: Some(11),
            ..Default::default()
        }),
        "a rejected frame keeps the last good values"
    );
}
