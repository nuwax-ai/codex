use codex_api::ResponseEvent;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;
use rig_core::completion::message::Reasoning;
use rig_core::completion::message::ToolCall;
use rig_core::completion::message::ToolFunction;
use rig_core::completion::request::FinishReason;
use rig_core::completion::request::Usage;
use rig_core::streaming::Item;
use rig_core::streaming::Part;
use rig_core::streaming::PartKind;
use rig_core::streaming::StreamEvent;
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;

/// Legacy-shaped test inputs; [`replay`] allocates 0.44 parts per id and
/// folds the terminal into a recording.
enum TestEvent {
    ToolArgs { id: String, args: String },
    ToolEnd { id: String, call: ToolCall },
    ReasoningDelta { id: String, text: String },
    ReasoningEnd { id: String, reasoning: Reasoning },
    Terminal(FinishReason),
}

fn part_of(index: u32) -> Part {
    serde_json::from_value(serde_json::Value::from(index)).expect("Part is transparent")
}

fn final_event() -> TestEvent {
    TestEvent::Terminal(FinishReason::ToolCalls)
}
fn tool(arguments: serde_json::Value) -> TestEvent {
    TestEvent::ToolEnd {
        id: "internal".into(),
        call: ToolCall::from_wire(
            "provider-call",
            ToolFunction {
                name: rig_core::completion::message::ToolName::new("apply_patch")
                    .expect("non-empty tool name"),
                arguments: arguments.as_object().cloned().unwrap_or_default(),
                invalid_arguments: None,
            },
        ),
    }
}
fn delta(text: &str) -> TestEvent {
    TestEvent::ToolArgs {
        id: "internal".into(),
        args: text.into(),
    }
}

/// Replays legacy-shaped events through the current conversion, allocating
/// one stream part per distinct id.
fn replay(
    events: Vec<TestEvent>,
    custom: HashSet<String>,
) -> Result<Vec<ResponseEvent>, codex_api::ApiError> {
    let mut recording = crate::cassette::RigRecording::default();
    let mut next_part = 0u32;
    let mut parts: HashMap<String, Part> = HashMap::new();
    for event in events {
        let item = match event {
            TestEvent::ToolArgs { id, args } => {
                let part = *parts.entry(id.clone()).or_insert_with(|| {
                    next_part += 1;
                    part_of(next_part)
                });
                Item::Event(StreamEvent::Arguments { part, json: args })
            }
            TestEvent::ToolEnd { id, call } => {
                let part = *parts.entry(id.clone()).or_insert_with(|| {
                    next_part += 1;
                    part_of(next_part)
                });
                Item::Event(StreamEvent::End {
                    part,
                    content: rig_core::completion::message::AssistantContent::ToolCall(call),
                })
            }
            TestEvent::ReasoningDelta { id, text } => {
                let part = *parts.entry(id.clone()).or_insert_with(|| {
                    next_part += 1;
                    part_of(next_part)
                });
                Item::Event(StreamEvent::Reasoning { part, text })
            }
            TestEvent::ReasoningEnd { id, reasoning } => {
                let part = *parts.entry(id.clone()).or_insert_with(|| {
                    next_part += 1;
                    part_of(next_part)
                });
                Item::Event(StreamEvent::End {
                    part,
                    content: rig_core::completion::message::AssistantContent::Reasoning(reasoning),
                })
            }
            TestEvent::Terminal(finish) => {
                recording.terminal = Some(crate::cassette::TerminalRecord {
                    finish_reason: Some(finish),
                    response_id: None,
                    model: None,
                    usage: Usage::new(),
                });
                continue;
            }
        };
        recording.events.push(item);
        let _ = PartKind::ToolCall;
    }
    crate::replay_rig_events(&recording, custom)
}

#[test]
fn custom_added_delta_done_have_consistent_type_call_id_and_unwrapped_input() {
    let events = replay(
        vec![
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
    let events = replay(
        vec![
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
        assert!(replay(events, HashSet::new()).is_err());
    }
}

#[test]
fn malformed_custom_wrapper_fails_before_executing_a_tool() {
    assert!(
        replay(
            vec![tool(json!({"other":"lost input"})), final_event()],
            HashSet::from(["apply_patch".into()])
        )
        .is_err()
    );
}

#[test]
fn reasoning_blocks_signatures_redactions_and_indices_survive_replay() {
    let signed = |text: &str, signature: &str| Reasoning {
        text: text.into(),
        redacted: false,
        native: Some(rig_core::completion::message::Native {
            item: serde_json::json!({ "signature": signature }),
            fingerprint: rig_core::completion::message::Fingerprint::of(&signature),
        }),
    };
    let first = signed("first chunk", "signed-first");
    let second = signed("second", "signed-second");
    let redacted = Reasoning {
        text: "opaque-redacted-data".into(),
        redacted: true,
        native: None,
    };
    let converted = replay(
        vec![
            TestEvent::ReasoningDelta {
                id: "r1".into(),
                text: "first ".into(),
            },
            TestEvent::ReasoningDelta {
                id: "r1".into(),
                text: "chunk".into(),
            },
            TestEvent::ReasoningEnd {
                id: "r1".into(),
                reasoning: first.clone(),
            },
            TestEvent::ReasoningEnd {
                id: "r2".into(),
                reasoning: second.clone(),
            },
            TestEvent::ReasoningEnd {
                id: "r3".into(),
                reasoning: redacted.clone(),
            },
            final_event(),
            final_event(),
        ],
        HashSet::new(),
    )
    .unwrap();
    let indices: Vec<_> = converted
        .iter()
        .filter_map(|event| match event {
            ResponseEvent::ReasoningContentDelta { content_index, .. } => Some(*content_index),
            _ => None,
        })
        .collect();
    assert_eq!(indices, vec![0, 0]);
    let item = converted
        .iter()
        .find_map(|event| match event {
            ResponseEvent::OutputItemDone(item @ ResponseItem::Reasoning { .. }) => Some(item),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        crate::reasoning::replay_reasoning(item, "cassette", crate::RigProtocol::Anthropic),
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
        converted
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
    // A serialized 0.42 Final record (untagged StreamFinal JSON shape).
    let legacy_final = json!({
        "kind":"final",
        "usage":{},
        "finish_reason":"tool_calls",
    });
    let fixture: crate::RigEventFixture =
        serde_json::from_value(json!({"vendor":"test","tag":"legacy","rig_events":[legacy_final]}))
            .unwrap();
    let events = crate::replay_fixture_events(&fixture).unwrap();
    assert!(matches!(
        events.last(),
        Some(ResponseEvent::Completed { .. })
    ));
    assert!(
        crate::replay_rig_events(&crate::cassette::RigRecording::default(), HashSet::new())
            .is_err()
    );
}

#[test]
fn redacted_blocks_do_not_shift_visible_reasoning_content_indices() {
    let events = replay(
        vec![
            TestEvent::ReasoningEnd {
                id: "r0".into(),
                reasoning: Reasoning {
                    text: "opaque".into(),
                    redacted: true,
                    native: None,
                },
            },
            TestEvent::ReasoningDelta {
                id: "r1".into(),
                text: "visible".into(),
            },
            TestEvent::Terminal(FinishReason::Stop),
        ],
        HashSet::new(),
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
