use super::*;
use rig_core::completion::request::FinishReason;
use rig_core::completion::request::Usage;
use rig_core::streaming::Item;
use rig_core::streaming::Part;
use rig_core::streaming::PartKind;
use rig_core::streaming::StreamEvent;

/// Legacy-shaped test inputs; `drive` allocates 0.44 parts per id.
enum TestEvent {
    Text(String),
    ReasoningDelta {
        id: String,
        text: String,
    },
    ToolName {
        id: String,
        name: String,
    },
    ToolArgs {
        id: String,
        args: String,
    },
    ToolEnd {
        id: String,
        call: rig_core::completion::message::ToolCall,
    },
    Terminal(StreamTerminal),
}

fn part_of(index: u32) -> Part {
    serde_json::from_value(serde_json::Value::from(index)).expect("Part is transparent")
}

#[cfg(test)]
mod mapping {
    use super::*;
    use pretty_assertions::assert_eq;
    fn rig_event_to_response_events(
        event: Item<StreamEvent>,
        pending: &mut PendingRigMessage,
    ) -> Vec<ResponseEvent> {
        super::rig_event_to_response_events(event, pending).expect("valid events")
    }
    use rig_core::completion::request::FinishReason;
    use rig_core::completion::request::Usage;

    fn text_delta(text: &str) -> TestEvent {
        TestEvent::Text(text.to_string())
    }

    fn reasoning_delta(text: &str, id: &str) -> TestEvent {
        TestEvent::ReasoningDelta {
            id: id.into(),
            text: text.into(),
        }
    }

    fn tool_name(id: &str, name: &str) -> TestEvent {
        TestEvent::ToolName {
            id: id.into(),
            name: name.into(),
        }
    }

    fn tool_args(id: &str, args: &str) -> TestEvent {
        TestEvent::ToolArgs {
            id: id.into(),
            args: args.into(),
        }
    }

    fn complete_tool(id: &str, wire_call_id: &str, name: &str, arguments: &str) -> TestEvent {
        TestEvent::ToolEnd {
            id: id.into(),
            call: rig_core::completion::message::ToolCall::from_wire(
                wire_call_id,
                rig_core::completion::message::ToolFunction {
                    name: rig_core::completion::message::ToolName::new(name)
                        .expect("non-empty tool name"),
                    arguments: serde_json::from_str(arguments).unwrap_or_default(),
                    invalid_arguments: None,
                },
            ),
        }
    }

    fn final_record(finish: Option<FinishReason>) -> TestEvent {
        TestEvent::Terminal(StreamTerminal {
            finish_reason: finish,
            response_id: None,
            model: None,
            usage: Usage::new(),
        })
    }

    fn drive(events: Vec<TestEvent>) -> Vec<ResponseEvent> {
        let mut pending =
            PendingRigMessage::new(std::sync::Arc::new(Default::default()), "test".into());
        let mut next_part = 0u32;
        let mut parts: std::collections::HashMap<String, Part> = std::collections::HashMap::new();
        fn part_for(
            key: Option<&str>,
            parts: &mut std::collections::HashMap<String, Part>,
            next_part: &mut u32,
        ) -> Part {
            match key {
                Some(key) => *parts.entry(key.to_string()).or_insert_with(|| {
                    *next_part += 1;
                    part_of(*next_part)
                }),
                None => {
                    *next_part += 1;
                    part_of(*next_part)
                }
            }
        }
        let mut out = Vec::new();
        for event in events {
            let item = match event {
                TestEvent::Text(text) => Item::Event(StreamEvent::Text {
                    // One text stream per drive: a fixed part keeps deltas
                    // joining the same part.
                    part: part_of(0),
                    text,
                }),
                TestEvent::ReasoningDelta { id, text } => {
                    let part = part_for(Some(&id), &mut parts, &mut next_part);
                    Item::Event(StreamEvent::Reasoning { part, text })
                }
                TestEvent::ToolName { id, name } => {
                    let part = part_for(Some(&id), &mut parts, &mut next_part);
                    Item::Event(StreamEvent::Start {
                        part,
                        kind: PartKind::ToolCall,
                        name: Some(
                            rig_core::completion::message::ToolName::new(name)
                                .expect("non-empty tool name"),
                        ),
                    })
                }
                TestEvent::ToolArgs { id, args } => {
                    let part = part_for(Some(&id), &mut parts, &mut next_part);
                    Item::Event(StreamEvent::Arguments { part, json: args })
                }
                TestEvent::ToolEnd { id, call } => {
                    let part = part_for(Some(&id), &mut parts, &mut next_part);
                    Item::Event(StreamEvent::End {
                        part,
                        content: rig_core::completion::message::AssistantContent::ToolCall(call),
                    })
                }
                TestEvent::Terminal(terminal) => {
                    out.extend(
                        super::super::handle_stream_terminal(terminal, &mut pending)
                            .expect("terminal"),
                    );
                    continue;
                }
            };
            out.extend(rig_event_to_response_events(item, &mut pending));
        }
        out
    }

    /// Regression (caught live on MiMo, 2026-09-24): the final
    /// `FunctionCall.arguments` must be byte-identical to the concatenation
    /// of the streamed `ToolCallInputDelta`s — no re-serialization that would
    /// normalize whitespace.
    #[test]
    fn final_arguments_match_streamed_deltas_byte_for_byte() {
        let events = drive(vec![
            tool_name("t1", "get_weather"),
            tool_args("t1", r#"{"city": "#),
            tool_args("t1", r#""北京"}"#),
            complete_tool("t1", "call_1", "get_weather", r#"{"city": "北京"}"#),
            final_record(Some(FinishReason::ToolCalls)),
        ]);
        let reassembled: String = events
            .iter()
            .filter_map(|e| match e {
                ResponseEvent::ToolCallInputDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        let final_args = events
            .iter()
            .find_map(|e| match e {
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { arguments, .. }) => {
                    Some(arguments.clone())
                }
                _ => None,
            })
            .expect("function call done");
        assert_eq!(
            reassembled, final_args,
            "final arguments must equal the raw delta concatenation"
        );
        assert!(
            final_args.contains(": "),
            "raw spacing preserved: {final_args}"
        );
    }

    /// When no deltas arrived, the complete ToolCall's serialized arguments
    /// are used instead.
    #[test]
    fn no_deltas_falls_back_to_serialized_arguments() {
        let events = drive(vec![
            complete_tool("t1", "call_1", "f", r#"{"a":1}"#),
            final_record(Some(FinishReason::ToolCalls)),
        ]);
        assert!(events.iter().any(|e| matches!(
            e,
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { arguments, .. })
                if arguments == r#"{"a":1}"#
        )));
    }

    #[test]
    fn item_added_precedes_first_delta() {
        let events = drive(vec![text_delta("h"), text_delta("i")]);
        let added = events
            .iter()
            .position(|e| {
                matches!(
                    e,
                    ResponseEvent::OutputItemAdded(ResponseItem::Message { .. })
                )
            })
            .expect("item added");
        let delta = events
            .iter()
            .position(|e| matches!(e, ResponseEvent::OutputTextDelta(_)))
            .expect("text delta");
        assert!(added < delta);
    }

    #[test]
    fn final_emits_reasoning_done_before_message_done() {
        let events = drive(vec![
            reasoning_delta("think", "r1"),
            text_delta("answer"),
            final_record(Some(FinishReason::Stop)),
        ]);
        let reasoning_done = events
            .iter()
            .position(|e| {
                matches!(
                    e,
                    ResponseEvent::OutputItemDone(ResponseItem::Reasoning { .. })
                )
            })
            .expect("reasoning done");
        let message_done = events
            .iter()
            .position(|e| {
                matches!(
                    e,
                    ResponseEvent::OutputItemDone(ResponseItem::Message { .. })
                )
            })
            .expect("message done");
        assert!(reasoning_done < message_done);
    }

    #[test]
    fn end_turn_reflects_finish_reason() {
        let tool_turn = drive(vec![final_record(Some(FinishReason::ToolCalls))]);
        assert_eq!(end_turn(&tool_turn), Some(false));
        let plain = drive(vec![final_record(Some(FinishReason::Stop))]);
        assert_eq!(end_turn(&plain), Some(true));
        let unknown = drive(vec![final_record(None)]);
        assert_eq!(end_turn(&unknown), None);
    }

    #[test]
    fn usage_maps_all_counters() {
        let mut usage = Usage::new();
        // 0.44 semantics: input_tokens already includes cache read+write.
        usage.input_tokens = Some(16);
        usage.cached_input_tokens = Some(4);
        usage.cache_creation_input_tokens = Some(2);
        usage.output_tokens = Some(6);
        usage.reasoning_tokens = Some(3);
        usage.total_tokens = Some(22);
        let events = drive(vec![TestEvent::Terminal(StreamTerminal {
            finish_reason: Some(FinishReason::Stop),
            response_id: None,
            model: None,
            usage,
        })]);
        let completed = events.iter().find_map(|e| match e {
            ResponseEvent::Completed { token_usage, .. } => token_usage.clone(),
            _ => None,
        });
        let usage = completed.expect("completed with usage");
        assert_eq!(usage.input_tokens, 16);
        assert_eq!(usage.cached_input_tokens, 4);
        assert_eq!(usage.cache_write_input_tokens, 2);
        assert_eq!(usage.output_tokens, 6);
        assert_eq!(usage.reasoning_output_tokens, 3);
        assert_eq!(usage.total_tokens, 22);
    }

    /// rig's contract: ending without a terminal record is truncation — the
    /// pump surfaces that, and `completed_emitted` stays false so callers can
    /// detect it.
    #[test]
    fn stream_without_final_never_completes() {
        let mut pending =
            PendingRigMessage::new(std::sync::Arc::new(Default::default()), "test".into());
        let events = rig_event_to_response_events(
            Item::Event(StreamEvent::Text {
                part: part_of(0),
                text: "hi".into(),
            }),
            &mut pending,
        );
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, ResponseEvent::Completed { .. }))
        );
        assert!(!pending.completed_emitted());
    }

    fn end_turn(events: &[ResponseEvent]) -> Option<bool> {
        events.iter().find_map(|e| match e {
            ResponseEvent::Completed { end_turn, .. } => *end_turn,
            _ => None,
        })
    }
}

#[cfg(test)]
mod custom_tool_response_tests {
    use super::*;
    use pretty_assertions::assert_eq;
    fn rig_event_to_response_events(
        event: Item<StreamEvent>,
        pending: &mut PendingRigMessage,
    ) -> Vec<ResponseEvent> {
        super::rig_event_to_response_events(event, pending).expect("valid events")
    }

    /// A completed call for a declared custom tool restores CustomToolCall
    /// with the unwrapped input string (codex dispatch expects Custom
    /// payloads for apply_patch-style handlers); function tools keep the
    /// FunctionCall item.
    #[test]
    fn custom_tool_done_restores_custom_tool_call() {
        let custom: std::collections::HashSet<String> =
            ["apply_patch".to_string()].into_iter().collect();
        let mut pending = PendingRigMessage::new(std::sync::Arc::new(custom), "test".into());

        let tool_event = Item::Event(StreamEvent::End {
            part: part_of(1),
            content: rig_core::completion::message::AssistantContent::ToolCall(
                rig_core::completion::message::ToolCall::from_wire(
                    "call_1",
                    rig_core::completion::message::ToolFunction {
                        name: rig_core::completion::message::ToolName::new("apply_patch")
                            .expect("non-empty tool name"),
                        arguments: serde_json::json!({"input": "*** Begin Patch\n+hello"})
                            .as_object()
                            .cloned()
                            .unwrap_or_default(),
                        invalid_arguments: None,
                    },
                ),
            ),
        });

        let mut all = Vec::new();
        all.extend(rig_event_to_response_events(tool_event, &mut pending));
        all.extend(
            super::super::handle_stream_terminal(
                StreamTerminal {
                    finish_reason: Some(FinishReason::Stop),
                    response_id: None,
                    model: None,
                    usage: Usage::new(),
                },
                &mut pending,
            )
            .expect("terminal"),
        );

        let done_item = all
            .iter()
            .find_map(|e| match e {
                ResponseEvent::OutputItemDone(item) => Some(item.clone()),
                _ => None,
            })
            .expect("one Done item");
        match done_item {
            ResponseItem::CustomToolCall { input, name, .. } => {
                assert_eq!(name, "apply_patch");
                assert!(input.contains("Begin Patch"), "input unwrapped: {input}");
            }
            other => panic!("expected CustomToolCall, got {other:?}"),
        }
    }

    /// Unconfirmed calls (deltas only, no complete ToolCall) must NOT be
    /// resurrected at Final — rig discarded them (e.g. truncated output).
    #[test]
    fn unconfirmed_tool_call_not_resurrected_at_final() {
        let mut pending =
            PendingRigMessage::new(std::sync::Arc::new(Default::default()), "test".into());
        // name arrives → establishes item; deltas accumulate; NO complete event
        let name_event = Item::Event(StreamEvent::Start {
            part: part_of(1),
            kind: PartKind::ToolCall,
            name: Some(
                rig_core::completion::message::ToolName::new("get_weather")
                    .expect("non-empty tool name"),
            ),
        });
        let delta_event = Item::Event(StreamEvent::Arguments {
            part: part_of(1),
            json: r#"{"city":"#.into(),
        });
        assert!(rig_event_to_response_events(name_event, &mut pending).is_empty());
        assert!(rig_event_to_response_events(delta_event, &mut pending).is_empty());
        assert!(
            super::super::handle_stream_terminal(
                StreamTerminal {
                    finish_reason: Some(FinishReason::Stop),
                    response_id: None,
                    model: None,
                    usage: Usage::new(),
                },
                &mut pending
            )
            .is_err()
        );
        assert!(!pending.completed_emitted());
    }
}
