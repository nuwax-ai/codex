use crate::convert_response::PendingRigMessage;
use crate::convert_response::rig_event_to_response_events;
use codex_api::ResponseEvent;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;
use rig_core::completion::message::Reasoning;
use rig_core::completion::message::ToolCall;
use rig_core::completion::message::ToolCallId;
use rig_core::completion::message::ToolFunction;
use rig_core::completion::request::FinishReason;
use rig_core::completion::request::Usage;
use rig_core::streaming::StreamFinal;
use rig_core::streaming::StreamedAssistantContent as Event;
use rig_core::streaming::ToolCallDeltaContent;
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;

fn final_event() -> Event {
    Event::Final(
        StreamFinal::new("anthropic", Usage::new()).with_finish_reason(FinishReason::ToolCalls),
    )
}
fn tool(arguments: serde_json::Value) -> Event {
    Event::ToolCall {
        internal_call_id: "internal".into(),
        tool_call: ToolCall {
            id: ToolCallId::new_or_mint("provider-call"),
            provider: None,
            function: ToolFunction {
                name: "apply_patch".into(),
                arguments,
            },
            signature: None,
            additional_params: None,
        },
    }
}
fn delta(text: &str) -> Event {
    Event::ToolCallDelta {
        internal_call_id: "internal".into(),
        content: ToolCallDeltaContent::Delta(text.into()),
    }
}

#[test]
fn custom_added_delta_done_have_consistent_type_call_id_and_unwrapped_input() {
    let events = crate::replay_rig_events(
        &[
            delta(r#"{"input":"PATCH"}"#),
            tool(json!({"input":"PATCH"})),
            final_event(),
        ],
        HashSet::from(["apply_patch".into()]),
    )
    .unwrap();
    let mut added = None;
    let mut deltas = String::new();
    let mut done = None;
    for event in events {
        match event {
            ResponseEvent::OutputItemAdded(item) => added = Some(item),
            ResponseEvent::ToolCallInputDelta { call_id, delta, .. } => {
                assert!(added.is_some());
                assert_eq!(call_id.as_deref(), Some("provider-call"));
                deltas.push_str(&delta);
            }
            ResponseEvent::OutputItemDone(item) => done = Some(item),
            _ => {}
        }
    }
    let Some(ResponseItem::CustomToolCall { input, call_id, .. }) = &done else {
        panic!("custom Done")
    };
    assert_eq!(
        (input.as_str(), call_id.as_str()),
        ("PATCH", "provider-call")
    );
    assert_eq!(&deltas, input);
    if let Some(ResponseItem::CustomToolCall { input, .. }) = &mut added {
        *input = deltas;
    }
    assert_eq!(added, done);
}

#[test]
fn rig_repaired_arguments_are_emitted_once_and_match_done() {
    let events = crate::replay_rig_events(
        &[
            delta("null"),
            delta(r#"{"x":1}"#),
            tool(json!({"x":1})),
            final_event(),
        ],
        HashSet::new(),
    )
    .unwrap();
    let deltas: String = events
        .iter()
        .filter_map(|event| match event {
            ResponseEvent::ToolCallInputDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    let done = events
        .iter()
        .find_map(|event| match event {
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { arguments, .. }) => {
                Some(arguments)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(&deltas, done);
    assert_eq!(deltas, r#"{"x":1}"#);
}

#[test]
fn argument_limit_applies_to_fragments_and_authoritative_tool_values() {
    let value = json!({"input":"x".repeat(1_048_577)});
    for events in [
        vec![
            delta(&value.to_string()),
            tool(value.clone()),
            final_event(),
        ],
        vec![tool(value), final_event()],
    ] {
        assert!(crate::replay_rig_events(&events, HashSet::new()).is_err());
    }
}

#[test]
fn malformed_custom_wrapper_fails_before_executing_a_tool() {
    assert!(
        crate::replay_rig_events(
            &[tool(json!({"other":"lost input"})), final_event()],
            HashSet::from(["apply_patch".into()])
        )
        .is_err()
    );
}

#[test]
fn reasoning_blocks_signatures_redactions_and_indices_survive_replay() {
    let first = Reasoning::new_with_signature("first chunk", Some("signed-first".into()));
    let second = Reasoning::new_with_signature("second", Some("signed-second".into()));
    let redacted = Reasoning::redacted("opaque-redacted-data");
    let mut pending = PendingRigMessage::new(Arc::new(HashSet::new()), "original-source".into());
    let mut events = Vec::new();
    for event in [
        Event::ReasoningDelta {
            id: "r1".into(),
            provider_id: None,
            reasoning: "first ".into(),
        },
        Event::ReasoningDelta {
            id: "r1".into(),
            provider_id: None,
            reasoning: "chunk".into(),
        },
        Event::Reasoning {
            id: "r1".into(),
            reasoning: first.clone(),
        },
        Event::Reasoning {
            id: "r2".into(),
            reasoning: second.clone(),
        },
        Event::Reasoning {
            id: "r3".into(),
            reasoning: redacted.clone(),
        },
        final_event(),
        final_event(),
    ] {
        events.extend(rig_event_to_response_events(event, &mut pending).unwrap());
    }
    let indices: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ResponseEvent::ReasoningContentDelta { content_index, .. } => Some(*content_index),
            _ => None,
        })
        .collect();
    assert_eq!(indices, vec![0, 0]);
    let item = events
        .iter()
        .find_map(|event| match event {
            ResponseEvent::OutputItemDone(item @ ResponseItem::Reasoning { .. }) => Some(item),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        crate::reasoning::replay_reasoning(item, "original-source", crate::RigProtocol::Anthropic),
        vec![first, second, redacted]
    );
    assert!(
        crate::reasoning::replay_reasoning(item, "different-source", crate::RigProtocol::Anthropic)
            .is_empty()
    );
    if let ResponseItem::Reasoning { content, .. } = item {
        assert_eq!(
            content,
            &Some(vec![
                ReasoningItemContent::ReasoningText {
                    text: "first chunk".into()
                },
                ReasoningItemContent::ReasoningText {
                    text: "second".into()
                }
            ])
        );
    }
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ResponseEvent::Completed { .. }))
            .count(),
        1
    );
}

#[test]
fn opaque_responses_reasoning_is_never_replayed_as_plaintext() {
    let item = ResponseItem::Reasoning {
        id: None,
        summary: vec![],
        content: None,
        encrypted_content: Some("opaque-openai-ciphertext".into()),
        internal_chat_message_metadata_passthrough: None,
    };
    assert!(
        crate::reasoning::replay_reasoning(&item, "source", crate::RigProtocol::Chat).is_empty()
    );
}

#[test]
fn legacy_rig_cassette_requires_real_conversion_and_a_terminal_record() {
    let fixture: crate::RigEventFixture = serde_json::from_value(
        json!({"vendor":"test","tag":"legacy","rig_events":[final_event()]}),
    )
    .unwrap();
    let events = crate::replay_fixture_events(&fixture).unwrap();
    assert!(matches!(
        events.last(),
        Some(ResponseEvent::Completed { .. })
    ));
    assert!(crate::replay_rig_events(&[], HashSet::new()).is_err());
}

#[test]
fn redacted_blocks_do_not_shift_visible_reasoning_content_indices() {
    let mut pending = PendingRigMessage::new(Arc::new(HashSet::new()), "source".into());
    rig_event_to_response_events(
        Event::Reasoning {
            id: "r0".into(),
            reasoning: Reasoning::redacted("opaque"),
        },
        &mut pending,
    )
    .unwrap();
    let events = rig_event_to_response_events(
        Event::ReasoningDelta {
            id: "r1".into(),
            provider_id: None,
            reasoning: "visible".into(),
        },
        &mut pending,
    )
    .unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        ResponseEvent::ReasoningContentDelta {
            content_index: 0,
            ..
        }
    )));
}
