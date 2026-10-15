use super::*;
use pretty_assertions::assert_eq;
use rig_core::completion::message::ToolCall;
use rig_core::completion::request::FinishReason;
use rig_core::completion::request::Usage;
use rig_core::streaming::Item;
use rig_core::streaming::Part;
use rig_core::streaming::StreamEvent;

fn part_of(index: u32) -> Part {
    serde_json::from_value(serde_json::Value::from(index)).expect("Part is transparent")
}

fn terminal() -> TerminalRecord {
    TerminalRecord {
        finish_reason: Some(FinishReason::Stop),
        response_id: None,
        model: None,
        usage: Usage::new(),
    }
}

#[test]
fn replay_includes_created_and_stops_at_the_first_completion() {
    let recording = RigRecording {
        terminal: Some(terminal()),
        ..Default::default()
    };
    let events = replay_rig_events(&recording, HashSet::new()).unwrap();
    let expected = vec![
        ResponseEvent::Created { response_id: None },
        ResponseEvent::Completed {
            response_id: String::new(),
            token_usage: None,
            usage_metadata: None,
            end_turn: Some(true),
        },
    ];
    assert_eq!(
        serde_json::to_value(events).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

#[test]
fn replay_rejects_missing_terminal_records() {
    let error = replay_rig_events(&RigRecording::default(), HashSet::new()).unwrap_err();
    assert!(
        matches!(error, codex_api::ApiError::Stream(message) if message.contains("without a terminal record"))
    );
}

#[test]
fn replay_does_not_recover_from_a_conversion_error_at_the_terminal() {
    // 0.44's `CallId` no longer deserializes from a bare wire string, so the
    // invalid call (custom tool whose arguments lack `input`) is built in
    // code; the replay contract under test is unchanged.
    let invalid = ToolCall::from_wire(
        "call-invalid",
        rig_core::completion::message::ToolFunction {
            name: rig_core::completion::message::ToolName::new("custom")
                .expect("non-empty tool name"),
            arguments: serde_json::json!({"missing_input": true})
                .as_object()
                .cloned()
                .unwrap_or_default(),
            invalid_arguments: None,
        },
    );
    let recording = RigRecording {
        events: vec![Item::Event(StreamEvent::End {
            part: part_of(1),
            content: rig_core::completion::message::AssistantContent::ToolCall(invalid),
        })],
        terminal: Some(terminal()),
    };
    let error = replay_rig_events(&recording, HashSet::from(["custom".into()])).unwrap_err();
    assert!(matches!(error, codex_api::ApiError::Stream(_)));
}

#[test]
fn legacy_usage_upgrades_to_inclusive_counters_and_rederives_the_total() {
    // A recorded 0.42 Final usage: `input_tokens` excluded cache and the
    // recorded `total_tokens` excluded it too. 0.44 replay upgrades both
    // exactly as the live pump's wire-counter correction would have
    // recorded them; the bridge_live fixture replays pin this end-to-end.
    let usage: LegacyUsage = serde_json::from_value(serde_json::json!({
        "input_tokens": 131,
        "output_tokens": 51,
        "total_tokens": 182,
        "cached_input_tokens": 128,
        "cache_creation_input_tokens": 0,
        "reasoning_tokens": 32,
    }))
    .expect("legacy usage");
    let upgraded = usage.into_usage();
    assert_eq!(upgraded.input_tokens, Some(259));
    assert_eq!(upgraded.cached_input_tokens, Some(128));
    assert_eq!(upgraded.cache_creation_input_tokens, None);
    assert_eq!(upgraded.output_tokens, Some(51));
    assert_eq!(upgraded.reasoning_tokens, Some(32));
    assert_eq!(upgraded.total_tokens, Some(310));
}
