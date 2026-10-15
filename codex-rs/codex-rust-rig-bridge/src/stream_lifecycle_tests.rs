use crate::convert_response::PendingRigMessage;
use crate::convert_response::StreamTerminal;
use crate::convert_response::handle_stream_terminal;
use crate::convert_response::rig_event_to_response_events;
use codex_api::ApiError;
use codex_api::ResponseEvent;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;
use rig_core::completion::message::Reasoning;
use rig_core::completion::request::FinishReason;
use rig_core::completion::request::Usage;
use rig_core::streaming::Item;
use rig_core::streaming::Part;
use rig_core::streaming::PartKind;
use rig_core::streaming::StreamEvent;
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

fn pending() -> PendingRigMessage {
    PendingRigMessage::new(Arc::new(HashSet::from(["custom".into()])), "source".into())
}

fn part_of(index: u32) -> Part {
    serde_json::from_value(serde_json::Value::from(index)).expect("Part is transparent")
}

/// Legacy-shaped test inputs; each helper carries its stream-local id, and
/// [`apply`] allocates one 0.44 part per distinct id.
enum TestEvent {
    Text(String),
    ReasoningDelta {
        id: String,
        text: String,
    },
    ReasoningEnd {
        id: String,
        reasoning: Reasoning,
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
        name: String,
        arguments: serde_json::Map<String, serde_json::Value>,
    },
    Terminal(FinishReason),
    TerminalWithUsage(FinishReason, Usage),
}

fn reasoning_delta(id: &str, text: &str) -> TestEvent {
    TestEvent::ReasoningDelta {
        id: id.into(),
        text: text.into(),
    }
}

fn signed_reasoning_value(id: &str, text: &str) -> Reasoning {
    let signature = format!("signature-{id}");
    Reasoning {
        text: text.into(),
        redacted: false,
        native: Some(rig_core::completion::message::Native {
            item: json!({ "signature": signature }),
            fingerprint: rig_core::completion::message::Fingerprint::of(&signature),
        }),
    }
}

fn signed_reasoning(id: &str, text: &str) -> TestEvent {
    TestEvent::ReasoningEnd {
        id: id.into(),
        reasoning: signed_reasoning_value(id, text),
    }
}

fn tool_delta(id: &str, name: &str) -> TestEvent {
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

fn tool(id: &str, name: &str, arguments: serde_json::Value) -> TestEvent {
    TestEvent::ToolEnd {
        id: id.into(),
        name: name.into(),
        arguments: arguments.as_object().cloned().unwrap_or_default(),
    }
}

fn terminal(reason: FinishReason) -> TestEvent {
    TestEvent::Terminal(reason)
}

/// Converts test events into 0.44 stream items, applying each through the
/// live conversion; terminals go through the terminal handler. The part
/// map lives across calls so interleaved fragments of one id share a part.
struct Driver {
    state: PendingRigMessage,
    next_part: u32,
    parts: HashMap<String, Part>,
}

impl Driver {
    fn new() -> Self {
        Self {
            state: pending(),
            next_part: 0,
            parts: HashMap::new(),
        }
    }

    fn apply(&mut self, events: Vec<TestEvent>) -> Result<Vec<ResponseEvent>, ApiError> {
        let state = &mut self.state;
        let next_part = &mut self.next_part;
        let parts = &mut self.parts;
        let part_for = |key: &str, next_part: &mut u32, parts: &mut HashMap<String, Part>| {
            *parts.entry(key.to_string()).or_insert_with(|| {
                *next_part += 1;
                part_of(*next_part)
            })
        };
        let mut out = Vec::new();
        for event in events {
            match event {
                TestEvent::Text(text) => out.extend(rig_event_to_response_events(
                    Item::Event(StreamEvent::Text {
                        part: part_of(0),
                        text,
                    }),
                    state,
                )?),
                TestEvent::ReasoningDelta { id, text } => {
                    let part = part_for(&id, next_part, parts);
                    out.extend(rig_event_to_response_events(
                        Item::Event(StreamEvent::Reasoning { part, text }),
                        state,
                    )?)
                }
                TestEvent::ReasoningEnd { id, reasoning } => {
                    let part = part_for(&id, next_part, parts);
                    out.extend(rig_event_to_response_events(
                        Item::Event(StreamEvent::End {
                            part,
                            content: rig_core::completion::message::AssistantContent::Reasoning(
                                reasoning,
                            ),
                        }),
                        state,
                    )?)
                }
                TestEvent::ToolName { id, name } => {
                    let part = part_for(&id, next_part, parts);
                    out.extend(rig_event_to_response_events(
                        Item::Event(StreamEvent::Start {
                            part,
                            kind: PartKind::ToolCall,
                            name: Some(
                                rig_core::completion::message::ToolName::new(name)
                                    .expect("non-empty tool name"),
                            ),
                        }),
                        state,
                    )?)
                }
                TestEvent::ToolArgs { id, args } => {
                    let part = part_for(&id, next_part, parts);
                    out.extend(rig_event_to_response_events(
                        Item::Event(StreamEvent::Arguments { part, json: args }),
                        state,
                    )?)
                }
                TestEvent::ToolEnd {
                    id,
                    name,
                    arguments,
                } => {
                    let part = part_for(&id, next_part, parts);
                    out.extend(rig_event_to_response_events(
                        Item::Event(StreamEvent::End {
                            part,
                            content: rig_core::completion::message::AssistantContent::ToolCall(
                                rig_core::completion::message::ToolCall::from_wire(
                                    format!("wire-{id}"),
                                    rig_core::completion::message::ToolFunction {
                                        name: rig_core::completion::message::ToolName::new(name)
                                            .expect("non-empty tool name"),
                                        arguments,
                                        invalid_arguments: None,
                                    },
                                ),
                            ),
                        }),
                        state,
                    )?)
                }
                TestEvent::Terminal(reason) => out.extend(handle_stream_terminal(
                    StreamTerminal {
                        finish_reason: Some(reason),
                        response_id: None,
                        model: None,
                        usage: Usage::new(),
                    },
                    state,
                )?),
                TestEvent::TerminalWithUsage(reason, usage) => out.extend(handle_stream_terminal(
                    StreamTerminal {
                        finish_reason: Some(reason),
                        response_id: None,
                        model: None,
                        usage,
                    },
                    state,
                )?),
            }
        }
        Ok(out)
    }
}

fn drive(input: Vec<TestEvent>) -> Vec<ResponseEvent> {
    let mut driver = Driver::new();
    driver.apply(input).expect("valid stream")
}

/// Model core's single active item and reconstruct each Done from its deltas.
fn assert_lifecycle(events: &[ResponseEvent], expected_completions: usize) -> Vec<ResponseItem> {
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
                        annotations: None,
                        logprobs: None,
                    });
                }
                let ContentItem::OutputText {
                    text,
                    annotations: None,
                    logprobs: None,
                } = &mut content[0]
                else {
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
    assert_eq!(completed, expected_completions);
    done
}

#[test]
fn text_and_reasoning_switches_close_each_item_before_the_next_delta() {
    let events = drive(vec![
        reasoning_delta("r1", "first"),
        signed_reasoning("r1", "first"),
        TestEvent::Text("answer one".into()),
        reasoning_delta("r2", "second"),
        signed_reasoning("r2", "second"),
        TestEvent::Text("answer two".into()),
        terminal(FinishReason::Stop),
    ]);
    let done = assert_lifecycle(&events, /*expected_completions*/ 1);
    assert_eq!(done.len(), 4);
    for (index, id, text) in [(0, "r1", "first"), (2, "r2", "second")] {
        assert_eq!(
            crate::reasoning::replay_reasoning(
                &done[index],
                "source",
                crate::RigProtocol::Anthropic
            ),
            vec![signed_reasoning_value(id, text)]
        );
    }
}

#[test]
fn reasoning_after_a_tool_keeps_its_position_and_is_deferred_until_final() {
    let mut driver = Driver::new();
    let mut events = Vec::new();
    for chunk in [
        vec![reasoning_delta("r1", "before")],
        vec![tool_delta("t1", "lookup")],
        vec![reasoning_delta("r2", "after")],
        vec![tool("t1", "lookup", json!({"key": 1}))],
        vec![TestEvent::Text("answer".into())],
    ] {
        events.extend(driver.apply(chunk).expect("buffered suffix"));
    }
    events.extend(
        driver
            .apply(vec![terminal(FinishReason::ToolCalls)])
            .expect("final"),
    );
    let done = assert_lifecycle(&events, /*expected_completions*/ 1);
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
    let mut driver = Driver::new();
    for event in [
        tool_delta("t1", "lookup"),
        tool_args("t1", "{\"key\": "),
        tool_delta("t2", "custom"),
        tool_args("t2", "{\"input\":\"raw "),
        tool_args("t1", "1}"),
        tool_args("t2", "input\"}"),
        tool("t2", "custom", json!({"input": "raw input"})),
        tool("t1", "lookup", json!({"key": 1})),
    ] {
        assert!(driver.apply(vec![event]).expect("buffered tool").is_empty());
    }
    let events = driver
        .apply(vec![terminal(FinishReason::ToolCalls)])
        .expect("final");
    let state = &driver.state;
    let done = assert_lifecycle(&events, /*expected_completions*/ 1);
    assert_eq!(
        events.len(),
        7,
        "two contiguous Added/Delta/Done groups and Completed"
    );
    let function_id = format!("rigseg_{}_0", state.response_id());
    let custom_id = format!("rigseg_{}_1", state.response_id());
    let function_done = ResponseItem::FunctionCall {
        id: Some(codex_protocol::ResponseItemId::from_server(
            function_id.clone(),
        )),
        name: "lookup".into(),
        namespace: None,
        arguments: "{\"key\": 1}".into(),
        encrypted_function_args: None,
        call_id: "wire-t1".into(),
        internal_chat_message_metadata_passthrough: None,
        encrypted_content: None,
        status: None,
    };
    let custom_done = ResponseItem::CustomToolCall {
        id: Some(codex_protocol::ResponseItemId::from_server(
            custom_id.clone(),
        )),
        status: None,
        name: "custom".into(),
        namespace: None,
        input: "raw input".into(),
        call_id: "wire-t2".into(),
        internal_chat_message_metadata_passthrough: None,
        encrypted_content: None,
    };
    assert_eq!(done, vec![function_done.clone(), custom_done.clone()]);
    let mut function_added = function_done.clone();
    if let ResponseItem::FunctionCall { arguments, .. } = &mut function_added {
        arguments.clear();
    }
    let mut custom_added = custom_done.clone();
    if let ResponseItem::CustomToolCall { input, .. } = &mut custom_added {
        input.clear();
    }
    let expected_events = vec![
        ResponseEvent::OutputItemAdded(function_added),
        ResponseEvent::ToolCallInputDelta {
            item_id: function_id,
            call_id: Some("wire-t1".into()),
            delta: "{\"key\": 1}".into(),
        },
        ResponseEvent::OutputItemDone(function_done),
        ResponseEvent::OutputItemAdded(custom_added),
        ResponseEvent::ToolCallInputDelta {
            item_id: custom_id,
            call_id: Some("wire-t2".into()),
            delta: "raw input".into(),
        },
        ResponseEvent::OutputItemDone(custom_done),
        ResponseEvent::Completed {
            response_id: String::new(),
            token_usage: None,
            usage_metadata: None,
            end_turn: Some(false),
        },
    ];
    assert_eq!(
        serde_json::to_value(&events).expect("observed lifecycle"),
        serde_json::to_value(&expected_events).expect("expected lifecycle"),
        "first-seen order, Added/Delta/Done identities, provider call IDs and raw inputs all survive",
    );
}

#[test]
fn paused_final_flushes_text_tool_text_in_order_without_completed() {
    let mut driver = Driver::new();
    let mut events = Vec::new();
    for chunk in [
        vec![TestEvent::Text("before".into())],
        vec![tool("t1", "lookup", json!({"key": 1}))],
        vec![TestEvent::Text("after".into())],
    ] {
        events.extend(driver.apply(chunk).expect("stream event"));
    }
    events.extend(
        crate::convert_response::paused_final_events(
            StreamTerminal {
                finish_reason: Some(FinishReason::Other("pause_turn".into())),
                response_id: None,
                model: None,
                usage: Usage::new(),
            },
            &mut driver.state,
        )
        .expect("paused final"),
    );
    let done = assert_lifecycle(&events, /*expected_completions*/ 0);
    assert!(
        matches!(done.as_slice(), [ResponseItem::Message { content: before, .. }, ResponseItem::FunctionCall { call_id, .. }, ResponseItem::Message { content: after, .. }]
                if before == &vec![ContentItem::OutputText { text: "before".into(),
         annotations: None,
         logprobs: None,
        }]
                    && call_id == "wire-t1"
                    && after == &vec![ContentItem::OutputText { text: "after".into(),
         annotations: None,
         logprobs: None,
        }])
    );
    assert!(!driver.state.completed_emitted());
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
        let mut driver = Driver::new();
        assert!(
            driver
                .apply(vec![tool("t1", "lookup", json!({}))])
                .expect("buffer tool")
                .is_empty()
        );
        let error = driver
            .apply(vec![terminal(reason.clone())])
            .expect_err("unsuccessful terminal");
        match reason {
            FinishReason::Other(ref inner) if inner == "model_context_window_exceeded" => {
                assert!(matches!(error, ApiError::ContextWindowExceeded));
            }
            FinishReason::Length => assert!(matches!(
                error,
                ApiError::CapExhausted { ref message, .. } if message.contains("Output token limit reached")
            )),
            FinishReason::ContentFilter => assert!(matches!(error, ApiError::ContentFilter)),
            _ => assert!(matches!(error, ApiError::Stream(_))),
        }
        assert!(!driver.state.completed_emitted());
    }
}

#[test]
fn cap_usage_discards_counters_that_exceed_signed_range() {
    let mut driver = Driver::new();
    let mut usage = Usage::new();
    usage.input_tokens = Some(u64::MAX);
    usage.total_tokens = Some(4);
    let error = driver
        .apply(vec![TestEvent::TerminalWithUsage(
            FinishReason::Length,
            usage,
        )])
        .expect_err("output cap");
    let ApiError::CapExhausted { reported_usage, .. } = error else {
        panic!("expected output cap");
    };
    assert_eq!(reported_usage, None);
}

#[test]
fn a_terminal_cannot_confirm_an_unfinished_call() {
    let mut driver = Driver::new();
    for event in [
        tool("t1", "lookup", json!({})),
        tool_delta("t2", "lookup"),
        tool_args("t2", "{\"key\":"),
    ] {
        assert!(driver.apply(vec![event]).expect("buffered tool").is_empty());
    }
    let error = driver
        .apply(vec![terminal(FinishReason::ToolCalls)])
        .expect_err("unconfirmed call");
    assert!(
        matches!(error, ApiError::Stream(message) if message.contains("unconfirmed tool call"))
    );
    assert!(!driver.state.completed_emitted());
}

#[test]
fn a_late_signature_cannot_duplicate_or_rewrite_a_completed_reasoning_item() {
    let mut driver = Driver::new();
    driver
        .apply(vec![reasoning_delta("r1", "thinking")])
        .expect("reasoning");
    let boundary = driver
        .apply(vec![TestEvent::Text("answer".into())])
        .expect("text boundary");
    assert!(matches!(
        boundary.first(),
        Some(ResponseEvent::OutputItemDone(
            ResponseItem::Reasoning { .. }
        ))
    ));
    let error = driver
        .apply(vec![signed_reasoning("r1", "thinking")])
        .expect_err("late reasoning update");
    assert!(
        matches!(error, ApiError::Stream(message) if message.contains("reasoning changed after"))
    );
    assert!(!driver.state.completed_emitted());
}

#[test]
fn cap_fallback_does_not_claim_derived_totals_or_complete_wire_presence() {
    let mut driver = Driver::new();
    let mut usage = Usage::new();
    usage.input_tokens = Some(10);
    usage.output_tokens = Some(5);
    usage.total_tokens = Some(18);
    usage.cached_input_tokens = Some(1);
    usage.cache_creation_input_tokens = Some(2);
    usage.reasoning_tokens = Some(3);
    let error = driver
        .apply(vec![TestEvent::TerminalWithUsage(
            FinishReason::Length,
            usage,
        )])
        .expect_err("cap");
    let ApiError::CapExhausted {
        reported_usage: Some(report),
        ..
    } = error
    else {
        panic!("report");
    };
    assert_eq!(
        report,
        codex_protocol::protocol::ReportedResponseUsage::from_counters(
            codex_protocol::protocol::ReportedUsageCounters {
                input_tokens: Some(10),
                output_tokens: Some(5),
                total_tokens: None,
                cached_input_tokens: Some(1),
                cache_write_input_tokens: Some(2),
                reasoning_output_tokens: Some(3),
            }
        )
        .expect("incomplete report")
    );
}
