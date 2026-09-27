use crate::convert_response::PendingRigMessage;
use crate::convert_response::rig_event_to_response_events;
use codex_api::ApiError;
use codex_api::ResponseEvent;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;
use rig_core::completion::message::ProviderCallId;
use rig_core::completion::message::Reasoning;
use rig_core::completion::message::ToolCall;
use rig_core::completion::message::ToolCallId;
use rig_core::completion::message::ToolFunction;
use rig_core::completion::request::FinishReason;
use rig_core::completion::request::Usage;
use rig_core::streaming::StreamFinal;
use rig_core::streaming::StreamedAssistantContent;
use rig_core::streaming::ToolCallDeltaContent;
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;

fn pending() -> PendingRigMessage {
    PendingRigMessage::new(Arc::new(HashSet::from(["custom".into()])), "source".into())
}

fn reasoning_delta(id: &str, text: &str) -> StreamedAssistantContent {
    StreamedAssistantContent::ReasoningDelta {
        id: id.into(),
        provider_id: None,
        reasoning: text.into(),
    }
}

fn signed_reasoning(id: &str, text: &str) -> StreamedAssistantContent {
    StreamedAssistantContent::Reasoning {
        id: id.into(),
        reasoning: Reasoning::new_with_signature(text, Some(format!("signature-{id}"))),
    }
}

fn tool_delta(id: &str, content: ToolCallDeltaContent) -> StreamedAssistantContent {
    StreamedAssistantContent::ToolCallDelta {
        internal_call_id: id.into(),
        content,
    }
}

fn tool(id: &str, name: &str, arguments: serde_json::Value) -> StreamedAssistantContent {
    StreamedAssistantContent::ToolCall {
        internal_call_id: id.into(),
        tool_call: ToolCall {
            id: ToolCallId::new_or_mint(format!("local-{id}")),
            provider: ProviderCallId::new(format!("wire-{id}")),
            function: ToolFunction {
                name: name.into(),
                arguments,
            },
            signature: None,
            additional_params: None,
        },
    }
}

fn terminal(reason: FinishReason) -> StreamedAssistantContent {
    StreamedAssistantContent::Final(
        StreamFinal::new("test", Usage::new()).with_finish_reason(reason),
    )
}

fn drive(input: Vec<StreamedAssistantContent>) -> Vec<ResponseEvent> {
    let mut state = pending();
    input
        .into_iter()
        .flat_map(|event| rig_event_to_response_events(event, &mut state).expect("valid stream"))
        .collect()
}

/// Model core's single active item and reconstruct each Done from its deltas.
fn assert_lifecycle(events: &[ResponseEvent]) -> Vec<ResponseItem> {
    let mut active: Option<ResponseItem> = None;
    let mut done = Vec::new();
    let mut completed = 0;
    for event in events {
        match event {
            ResponseEvent::OutputItemAdded(item) => {
                assert_eq!(active, None, "overlapping Added events");
                active = Some(item.clone());
            }
            ResponseEvent::OutputTextDelta(delta) => {
                let Some(ResponseItem::Message { content, .. }) = active.as_mut() else {
                    panic!("text delta has no active message");
                };
                if content.is_empty() {
                    content.push(ContentItem::OutputText {
                        text: String::new(),
                    });
                }
                let ContentItem::OutputText { text } = &mut content[0] else {
                    panic!("unexpected message content");
                };
                text.push_str(delta);
            }
            ResponseEvent::ReasoningContentDelta {
                delta,
                content_index,
            } => {
                let Some(ResponseItem::Reasoning { content, .. }) = active.as_mut() else {
                    panic!("reasoning delta has no active reasoning item");
                };
                let parts = content.get_or_insert_with(Vec::new);
                let index = usize::try_from(*content_index).expect("nonnegative content index");
                assert!(
                    index <= parts.len(),
                    "reasoning content index skipped a block"
                );
                if index == parts.len() {
                    parts.push(ReasoningItemContent::ReasoningText {
                        text: String::new(),
                    });
                }
                let ReasoningItemContent::ReasoningText { text } = &mut parts[index] else {
                    panic!("unexpected reasoning content");
                };
                text.push_str(delta);
            }
            ResponseEvent::ToolCallInputDelta {
                item_id,
                call_id,
                delta,
            } => {
                let item = active.as_mut().expect("tool delta needs an active item");
                assert_eq!(
                    item.id().map(codex_protocol::ResponseItemId::as_str),
                    Some(item_id.as_str())
                );
                let (expected_call_id, input) = match item {
                    ResponseItem::FunctionCall {
                        call_id, arguments, ..
                    } => (call_id, arguments),
                    ResponseItem::CustomToolCall { call_id, input, .. } => (call_id, input),
                    _ => panic!("tool delta has the wrong active item"),
                };
                assert_eq!(call_id.as_deref(), Some(expected_call_id.as_str()));
                input.push_str(delta);
            }
            ResponseEvent::OutputItemDone(item) => {
                let mut reconstructed = active.take().expect("Done needs a matching Added");
                if let (
                    ResponseItem::Reasoning {
                        encrypted_content: expected,
                        ..
                    },
                    ResponseItem::Reasoning {
                        encrypted_content, ..
                    },
                ) = (&mut reconstructed, item)
                {
                    *expected = encrypted_content.clone();
                }
                assert_eq!(&reconstructed, item);
                done.push(item.clone());
            }
            ResponseEvent::Completed { .. } => {
                assert_eq!(active, None, "response completed with an active item");
                completed += 1;
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
    assert_eq!(active, None);
    assert_eq!(completed, 1);
    done
}

#[test]
fn text_and_reasoning_switches_close_each_item_before_the_next_delta() {
    let events = drive(vec![
        reasoning_delta("r1", "first"),
        signed_reasoning("r1", "first"),
        StreamedAssistantContent::text("answer one"),
        reasoning_delta("r2", "second"),
        signed_reasoning("r2", "second"),
        StreamedAssistantContent::text("answer two"),
        terminal(FinishReason::Stop),
    ]);
    let done = assert_lifecycle(&events);
    assert_eq!(done.len(), 4);
    for (index, id, text) in [(0, "r1", "first"), (2, "r2", "second")] {
        assert_eq!(
            crate::reasoning::replay_reasoning(
                &done[index],
                "source",
                crate::RigProtocol::Anthropic
            ),
            vec![Reasoning::new_with_signature(
                text,
                Some(format!("signature-{id}"))
            )]
        );
    }
}

#[test]
fn reasoning_after_a_tool_keeps_its_position_and_is_deferred_until_final() {
    let mut state = pending();
    let mut events = rig_event_to_response_events(reasoning_delta("r1", "before"), &mut state)
        .expect("reasoning starts");
    for event in [
        tool_delta("t1", ToolCallDeltaContent::Name("lookup".into())),
        reasoning_delta("r2", "after"),
        tool("t1", "lookup", json!({"key": 1})),
        StreamedAssistantContent::text("answer"),
    ] {
        assert!(
            rig_event_to_response_events(event, &mut state)
                .expect("buffered suffix")
                .is_empty()
        );
    }
    events.extend(
        rig_event_to_response_events(terminal(FinishReason::ToolCalls), &mut state).expect("final"),
    );
    let done = assert_lifecycle(&events);
    assert!(matches!(
        done.as_slice(),
        [
            ResponseItem::Reasoning { .. },
            ResponseItem::FunctionCall { .. },
            ResponseItem::Reasoning { .. },
            ResponseItem::Message { .. }
        ]
    ));
}

#[test]
fn interleaved_parallel_calls_commit_in_first_seen_order_with_matching_ids_and_input() {
    let mut state = pending();
    for event in [
        tool_delta("t1", ToolCallDeltaContent::Name("lookup".into())),
        tool_delta("t1", ToolCallDeltaContent::Delta("{\"key\": ".into())),
        tool_delta("t2", ToolCallDeltaContent::Name("custom".into())),
        tool_delta(
            "t2",
            ToolCallDeltaContent::Delta("{\"input\":\"raw ".into()),
        ),
        tool_delta("t1", ToolCallDeltaContent::Delta("1}".into())),
        tool_delta("t2", ToolCallDeltaContent::Delta("input\"}".into())),
        tool("t2", "custom", json!({"input": "raw input"})),
        tool("t1", "lookup", json!({"key": 1})),
    ] {
        assert!(
            rig_event_to_response_events(event, &mut state)
                .expect("buffered tool")
                .is_empty()
        );
    }
    let events =
        rig_event_to_response_events(terminal(FinishReason::ToolCalls), &mut state).expect("final");
    let done = assert_lifecycle(&events);
    assert_eq!(
        events.len(),
        7,
        "two contiguous Added/Delta/Done groups and Completed"
    );
    assert!(
        matches!(done.as_slice(), [ResponseItem::FunctionCall { id: Some(id), call_id, arguments, .. }, ResponseItem::CustomToolCall { id: Some(custom_id), call_id: custom_call_id, input, .. }]
        if id.as_str() == "t1" && call_id == "wire-t1" && arguments == "{\"key\": 1}"
            && custom_id.as_str() == "t2" && custom_call_id == "wire-t2" && input == "raw input")
    );
}

#[test]
fn unsuccessful_terminals_never_publish_buffered_tools() {
    for reason in [
        FinishReason::Length,
        FinishReason::ContentFilter,
        FinishReason::Other("pause_turn".into()),
        FinishReason::Other("provider_failure".into()),
        FinishReason::Other("model_context_window_exceeded".into()),
    ] {
        let mut state = pending();
        assert!(
            rig_event_to_response_events(tool("t1", "lookup", json!({})), &mut state)
                .expect("buffer tool")
                .is_empty()
        );
        let error = rig_event_to_response_events(terminal(reason.clone()), &mut state)
            .expect_err("unsuccessful terminal");
        if reason == FinishReason::Other("model_context_window_exceeded".into()) {
            assert!(matches!(error, ApiError::ContextWindowExceeded));
        } else {
            assert!(matches!(error, ApiError::Stream(_)));
        }
        assert!(!state.completed_emitted());
    }
}

#[test]
fn a_terminal_cannot_confirm_an_unfinished_call() {
    let mut state = pending();
    for event in [
        tool("t1", "lookup", json!({})),
        tool_delta("t2", ToolCallDeltaContent::Name("lookup".into())),
        tool_delta("t2", ToolCallDeltaContent::Delta("{\"key\":".into())),
    ] {
        assert!(
            rig_event_to_response_events(event, &mut state)
                .expect("buffered tool")
                .is_empty()
        );
    }
    let error = rig_event_to_response_events(terminal(FinishReason::ToolCalls), &mut state)
        .expect_err("unconfirmed call");
    assert!(
        matches!(error, ApiError::Stream(message) if message.contains("unconfirmed tool call"))
    );
    assert!(!state.completed_emitted());
}

#[test]
fn a_late_signature_cannot_duplicate_or_rewrite_a_completed_reasoning_item() {
    let mut state = pending();
    rig_event_to_response_events(reasoning_delta("r1", "thinking"), &mut state).expect("reasoning");
    let boundary =
        rig_event_to_response_events(StreamedAssistantContent::text("answer"), &mut state)
            .expect("text boundary");
    assert!(matches!(
        boundary.first(),
        Some(ResponseEvent::OutputItemDone(
            ResponseItem::Reasoning { .. }
        ))
    ));
    let error = rig_event_to_response_events(signed_reasoning("r1", "thinking"), &mut state)
        .expect_err("late reasoning update");
    assert!(
        matches!(error, ApiError::Stream(message) if message.contains("reasoning changed after"))
    );
    assert!(!state.completed_emitted());
}
