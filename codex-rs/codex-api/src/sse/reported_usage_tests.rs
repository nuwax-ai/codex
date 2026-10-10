use super::parse_reported_response_usage;
use codex_protocol::protocol::ReportedResponseUsage;
use codex_protocol::protocol::ReportedUsageCounters;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn invalid_counters_do_not_become_negative_usage() {
    let usage = json!({
        "input_tokens":-1,"output_tokens":0,"total_tokens":u64::MAX,
        "input_tokens_details":{"cached_tokens":-2,"cache_write_tokens":0},
        "output_tokens_details":{"reasoning_tokens":-3}
    });
    assert_eq!(
        parse_reported_response_usage(&usage),
        ReportedResponseUsage::from_counters(ReportedUsageCounters {
            output_tokens: Some(0),
            cache_write_input_tokens: Some(0),
            ..Default::default()
        })
    );
}

#[test]
fn absent_null_and_invalid_usage_remain_unknown() {
    for usage in [
        json!(null),
        json!({}),
        json!({"input_tokens":null}),
        json!({"input_tokens":-1}),
        json!({"output_tokens":u64::MAX}),
        json!({"output_tokens":"0"}),
    ] {
        assert_eq!(parse_reported_response_usage(&usage), None, "{usage}");
    }
}
