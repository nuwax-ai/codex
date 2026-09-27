use super::*;
use pretty_assertions::assert_eq;
use rig_core::completion::message::ToolCall;
use rig_core::completion::request::FinishReason;
use rig_core::completion::request::Usage;
use rig_core::streaming::StreamFinal;

fn terminal() -> StreamedAssistantContent {
    StreamedAssistantContent::Final(
        StreamFinal::new("test", Usage::new()).with_finish_reason(FinishReason::Stop),
    )
}

#[test]
fn replay_includes_created_and_stops_at_the_first_completion() {
    let events = replay_rig_events(&[terminal(), terminal()], HashSet::new()).unwrap();
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
    let error = replay_rig_events(&[], HashSet::new()).unwrap_err();
    assert!(
        matches!(error, codex_api::ApiError::Stream(message) if message.contains("without a terminal record"))
    );
}

#[test]
fn replay_does_not_recover_from_a_conversion_error_at_a_later_final_record() {
    let invalid: ToolCall = serde_json::from_value(serde_json::json!({
        "id": "call-invalid",
        "function": {"name": "custom", "arguments": {"missing_input": true}}
    }))
    .unwrap();
    let error = replay_rig_events(
        &[
            StreamedAssistantContent::ToolCall {
                internal_call_id: "internal".into(),
                tool_call: invalid,
            },
            terminal(),
        ],
        HashSet::from(["custom".into()]),
    )
    .unwrap_err();
    assert!(matches!(error, codex_api::ApiError::Stream(_)));
}
