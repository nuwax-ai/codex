use super::*;
#[cfg(test)]
mod mapping {
    use super::*;
    use pretty_assertions::assert_eq;
    fn rig_event_to_response_events(
        event: StreamedAssistantContent,
        pending: &mut PendingRigMessage,
    ) -> Vec<ResponseEvent> {
        super::rig_event_to_response_events(event, pending).expect("valid events")
    }
    use rig_core::completion::message::ToolCallId;
    use rig_core::completion::message::ToolFunction;
    use rig_core::completion::request::FinishReason;
    use rig_core::completion::request::Usage;
    use rig_core::streaming::StreamedAssistantContent;
    use rig_core::streaming::ToolCallDeltaContent;

    fn text_delta(text: &str) -> StreamedAssistantContent {
        StreamedAssistantContent::Text(rig_core::completion::message::Text {
            text: text.to_string(),
            additional_params: None,
        })
    }

    fn reasoning_delta(text: &str, id: &str) -> StreamedAssistantContent {
        StreamedAssistantContent::ReasoningDelta {
            id: id.into(),
            provider_id: None,
            reasoning: text.into(),
        }
    }

    fn tool_name(id: &str, name: &str) -> StreamedAssistantContent {
        StreamedAssistantContent::ToolCallDelta {
            internal_call_id: id.into(),
            content: ToolCallDeltaContent::Name(name.into()),
        }
    }

    fn tool_args(id: &str, args: &str) -> StreamedAssistantContent {
        StreamedAssistantContent::ToolCallDelta {
            internal_call_id: id.into(),
            content: ToolCallDeltaContent::Delta(args.into()),
        }
    }

    fn complete_tool(
        id: &str,
        wire_call_id: &str,
        name: &str,
        arguments: &str,
    ) -> StreamedAssistantContent {
        StreamedAssistantContent::ToolCall {
            internal_call_id: id.into(),
            tool_call: rig_core::completion::message::ToolCall {
                id: ToolCallId::new_or_mint(wire_call_id),
                provider: None,
                function: ToolFunction {
                    name: name.into(),
                    arguments: serde_json::from_str(arguments).unwrap_or_default(),
                },
                signature: None,
                additional_params: None,
            },
        }
    }

    fn final_record(finish: Option<FinishReason>) -> StreamedAssistantContent {
        let mut record = StreamFinal::new("test", Usage::new());
        if let Some(finish) = finish {
            record = record.with_finish_reason(finish);
        }
        StreamedAssistantContent::Final(record)
    }

    fn drive(events: Vec<StreamedAssistantContent>) -> Vec<ResponseEvent> {
        let mut pending =
            PendingRigMessage::new(std::sync::Arc::new(Default::default()), "test".into());
        events
            .into_iter()
            .flat_map(|e| rig_event_to_response_events(e, &mut pending))
            .collect()
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
        usage.input_tokens = 10;
        usage.cached_input_tokens = 4;
        usage.cache_creation_input_tokens = 2;
        usage.output_tokens = 6;
        usage.reasoning_tokens = 3;
        usage.total_tokens = 16;
        let record = StreamFinal::new("test", usage);
        let events = drive(vec![StreamedAssistantContent::Final(record)]);
        let completed = events.iter().find_map(|e| match e {
            ResponseEvent::Completed { token_usage, .. } => token_usage.clone(),
            _ => None,
        });
        let usage = completed.expect("completed with usage");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.cached_input_tokens, 4);
        assert_eq!(usage.cache_write_input_tokens, 2);
        assert_eq!(usage.output_tokens, 6);
        assert_eq!(usage.reasoning_output_tokens, 3);
        assert_eq!(usage.total_tokens, 16);
    }

    /// rig's contract: ending without a terminal record is truncation — the
    /// pump surfaces that, and `completed_emitted` stays false so callers can
    /// detect it.
    #[test]
    fn stream_without_final_never_completes() {
        let mut pending =
            PendingRigMessage::new(std::sync::Arc::new(Default::default()), "test".into());
        let events = rig_event_to_response_events(text_delta("hi"), &mut pending);
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
    use rig_core::streaming::ToolCallDeltaContent;
    fn rig_event_to_response_events(
        event: StreamedAssistantContent,
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

        let tool_event = StreamedAssistantContent::ToolCall {
            internal_call_id: "t1".into(),
            tool_call: rig_core::completion::message::ToolCall {
                id: rig_core::completion::message::ToolCallId::new_or_mint("call_1"),
                provider: None,
                function: rig_core::completion::message::ToolFunction {
                    name: "apply_patch".into(),
                    arguments: serde_json::json!({"input": "*** Begin Patch\n+hello"}),
                },
                signature: None,
                additional_params: None,
            },
        };
        let final_event = StreamedAssistantContent::Final(StreamFinal::new(
            "test",
            rig_core::completion::request::Usage::new(),
        ));

        let mut all = Vec::new();
        all.extend(rig_event_to_response_events(tool_event, &mut pending));
        all.extend(rig_event_to_response_events(final_event, &mut pending));

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
        let name_event = StreamedAssistantContent::ToolCallDelta {
            internal_call_id: "t1".into(),
            content: ToolCallDeltaContent::Name("get_weather".into()),
        };
        let delta_event = StreamedAssistantContent::ToolCallDelta {
            internal_call_id: "t1".into(),
            content: ToolCallDeltaContent::Delta(r#"{"city":"#.into()),
        };
        let final_event = StreamedAssistantContent::Final(StreamFinal::new(
            "test",
            rig_core::completion::request::Usage::new(),
        ));
        let mut all = Vec::new();
        for ev in [name_event, delta_event, final_event] {
            all.extend(rig_event_to_response_events(ev, &mut pending));
        }
        let has_function_call_done = all.iter().any(|e| {
            matches!(
                e,
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { .. })
            )
        });
        assert!(
            !has_function_call_done,
            "unconfirmed call must not be emitted as Done"
        );
    }
}
