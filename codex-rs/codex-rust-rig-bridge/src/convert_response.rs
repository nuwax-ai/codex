//! Converts rig streaming events into Codex `ResponseEvent`s, preserving
//! the event-ordering contract codex's turn loop expects:
//! `OutputItemAdded` before the first delta, `OutputItemDone(Reasoning)`
//! before `OutputItemDone(Message)`, and exactly one terminal `Completed`.

use std::collections::HashMap;

use codex_api::ResponseEvent;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::ResponseItemId;
use rig_core::completion::request::FinishReason;
use rig_core::completion::request::Usage as RigUsage;
use rig_core::completion::message::ProviderCallId;
use rig_core::completion::message::Reasoning as RigReasoning;
use rig_core::streaming::StreamFinal;
use rig_core::streaming::StreamedAssistantContent;
use rig_core::streaming::ToolCallDeltaContent;

/// Accumulated state for one streamed assistant turn.
pub(crate) struct PendingRigMessage {
    /// Names of custom (freeform) tools declared in this request: their
    /// completed calls are restored as CustomToolCall items so codex
    /// dispatches the Custom payload handlers expect.
    custom_tools: std::sync::Arc<std::collections::HashSet<String>>,
    text_buffer: String,
    text_item_id: Option<String>,
    text_item_added: bool,
    reasoning_buffer: String,
    reasoning_item_id: Option<String>,
    reasoning_item_added: bool,
    reasoning_content_index: usize,
    /// rig correlates partial tool-call fragments with an internal id that is
    /// stable across the call's deltas and matches the completed ToolCall.
    tools: HashMap<String, PendingRigTool>,
    tool_order: Vec<String>,
    /// Whether the terminal `Completed` event has been produced.
    completed: bool,
}

struct PendingRigTool {
    name: String,
    /// Wire id used for codex's `call_id` (provider-issued when present).
    call_id: String,
    /// Final arguments. Prefers the concatenation of streamed argument
    /// deltas (byte-identical to what consumers saw via
    /// `ToolCallInputDelta`); falls back to the complete ToolCall event's
    /// serialized value when no deltas arrived.
    arguments: String,
    item_added: bool,
    /// Argument deltas that arrived BEFORE the tool name (rig streams can
    /// send argument fragments first). They are held back so the
    /// `OutputItemAdded` event always precedes the first
    /// `ToolCallInputDelta`, then flushed in order.
    pending_deltas: Vec<String>,
    /// Set only by the complete ToolCall event. rig discards calls whose
    /// arguments never became parseable (finish_reason=length); Final must
    /// not resurrect them as completed FunctionCall items.
    confirmed: bool,
}

impl PendingRigTool {
    fn empty() -> Self {
        Self {
            name: String::new(),
            call_id: String::new(),
            arguments: String::new(),
            item_added: false,
            pending_deltas: Vec::new(),
            confirmed: false,
        }
    }
}

/// A per-turn unique suffix for synthesized item IDs. Codex retains
/// assistant history keyed by item ID; a fixed `txt_0` every turn would
/// make each new message REPLACE the previous one in retained history.
fn unique_suffix() -> String {
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}{seq:x}")
}

impl PendingRigMessage {
    pub(crate) fn new(custom_tools: std::sync::Arc<std::collections::HashSet<String>>) -> Self {
        Self {
            custom_tools,
            text_buffer: String::new(),
            text_item_id: None,
            text_item_added: false,
            reasoning_buffer: String::new(),
            reasoning_item_id: None,
            reasoning_item_added: false,
            reasoning_content_index: 0,
            tools: HashMap::new(),
            tool_order: Vec::new(),
            completed: false,
        }
    }

    /// Whether the terminal Completed event has already been produced.
    pub(crate) fn completed_emitted(&self) -> bool {
        self.completed
    }

    fn entry_for(&mut self, internal_call_id: String) -> &mut PendingRigTool {
        if !self.tools.contains_key(&internal_call_id) {
            self.tool_order.push(internal_call_id.clone());
            self.tools.insert(internal_call_id.clone(), PendingRigTool::empty());
        }
        self.tools
            .get_mut(&internal_call_id)
            .expect("entry just inserted")
    }
}

/// Converts one rig stream event into zero or more codex events.
pub(crate) fn rig_event_to_response_events(
    event: StreamedAssistantContent,
    pending: &mut PendingRigMessage,
) -> Vec<ResponseEvent> {
    match event {
        StreamedAssistantContent::Text(text) => {
            let mut events = Vec::new();
            ensure_message_item_added(pending, &mut events);
            pending.text_buffer.push_str(&text.text);
            events.push(ResponseEvent::OutputTextDelta(text.text));
            events
        }
        StreamedAssistantContent::ReasoningDelta { reasoning, .. } => {
            let mut events = Vec::new();
            ensure_reasoning_item_added(pending, &mut events);
            let idx = pending.reasoning_content_index;
            pending.reasoning_content_index += 1;
            pending.reasoning_buffer.push_str(&reasoning);
            events.push(ResponseEvent::ReasoningContentDelta {
                delta: reasoning,
                content_index: idx as i64,
            });
            events
        }
        StreamedAssistantContent::ToolCallDelta {
            internal_call_id,
            content,
        } => {
            let mut events = Vec::new();
            match content {
                ToolCallDeltaContent::Name(name) => {
                    let entry = pending.entry_for(internal_call_id.clone());
                    entry.name = name;
                    // The name may arrive AFTER argument deltas; establishing
                    // the item now also releases the buffered deltas in
                    // order, preserving the Added-before-delta contract.
                    if !entry.item_added && !entry.name.is_empty() {
                        entry.item_added = true;
                        let name = entry.name.clone();
                        events.push(ResponseEvent::OutputItemAdded(
                            ResponseItem::FunctionCall {
                                id: Some(ResponseItemId::from_server(
                                    internal_call_id.clone(),
                                )),
                                name,
                                namespace: None,
                                arguments: String::new(),
                                encrypted_function_args: None,
                                call_id: internal_call_id.clone(),
                                internal_chat_message_metadata_passthrough: None,
                            },
                        ));
                        for delta in std::mem::take(&mut entry.pending_deltas) {
                            events.push(ResponseEvent::ToolCallInputDelta {
                                item_id: internal_call_id.clone(),
                                call_id: Some(internal_call_id.clone()),
                                delta,
                            });
                        }
                    }
                }
                ToolCallDeltaContent::Delta(args) => {
                    const MAX_TOOL_ARGS_BYTES: usize = 1_048_576;
                    let entry = pending.entry_for(internal_call_id.clone());
                    if entry.arguments.len() + args.len() > MAX_TOOL_ARGS_BYTES {
                        tracing::warn!(
                            limit = MAX_TOOL_ARGS_BYTES,
                            "tool arguments exceeded size cap; dropping further deltas"
                        );
                        entry.confirmed = false;
                        return events;
                    }
                    entry.arguments.push_str(&args);
                    if entry.item_added {
                        if !args.is_empty() {
                            events.push(ResponseEvent::ToolCallInputDelta {
                                item_id: internal_call_id.clone(),
                                call_id: Some(internal_call_id),
                                delta: args,
                            });
                        }
                    } else {
                        // Name not yet known: buffer so no delta precedes
                        // the OutputItemAdded event.
                        if !args.is_empty() {
                            entry.pending_deltas.push(args);
                        }
                    }
                }
            }
            events
        }
        StreamedAssistantContent::ToolCall {
            tool_call,
            internal_call_id,
        } => {
            // The complete tool call is authoritative: record name, wire id
            // and arguments; emit `OutputItemAdded` here only when the
            // deltas never did (name-less fragments).
            let mut events = Vec::new();
            let wire_call_id = provider_call_id(tool_call.provider.as_ref())
                .unwrap_or_else(|| tool_call.id.to_string());
            let name = tool_call.function.name.clone();
            let entry = pending.entry_for(internal_call_id.clone());
            entry.name = name.clone();
            entry.call_id = wire_call_id.clone();
            entry.confirmed = true;
            // Prefer the streamed-delta concatenation (byte-identity with
            // what consumers already saw) — but fall back to the
            // authoritative value when the raw concatenation is not valid
            // JSON (rig repairs placeholder/null-prefixed fragments).
            if entry.arguments.is_empty()
                || serde_json::from_str::<serde_json::Value>(&entry.arguments).is_err()
            {
                if !entry.arguments.is_empty() {
                    tracing::warn!(
                        raw_len = entry.arguments.len(),
                        "streamed tool arguments unparseable; using rig's authoritative value"
                    );
                }
                entry.arguments = tool_call.function.arguments.to_string();
            }
            if !entry.item_added {
                entry.item_added = true;
                events.push(ResponseEvent::OutputItemAdded(
                    ResponseItem::FunctionCall {
                        id: Some(ResponseItemId::from_server(internal_call_id.clone())),
                        name,
                        namespace: None,
                        arguments: String::new(),
                        encrypted_function_args: None,
                        call_id: wire_call_id,
                        internal_chat_message_metadata_passthrough: None,
                    },
                ));
                for delta in std::mem::take(&mut entry.pending_deltas) {
                    events.push(ResponseEvent::ToolCallInputDelta {
                        item_id: internal_call_id.clone(),
                        call_id: Some(internal_call_id.clone()),
                        delta,
                    });
                }
            }
            events
        }
        StreamedAssistantContent::Reasoning { reasoning, .. } => {
            // Complete reasoning block supersedes the accumulated deltas; the
            // Done item is emitted at stream end like the genai bridge.
            let text = reasoning_text(&reasoning);
            if !text.is_empty() {
                pending.reasoning_buffer = text;
            }
            Vec::new()
        }
        StreamedAssistantContent::Final(final_record) => {
            handle_stream_final(final_record, pending)
        }
        // Note: usage normalization for Anthropic happens inside
        // handle_stream_final via the provider tag (see map_usage).
        StreamedAssistantContent::Unknown(unknown) => {
            tracing::debug!(?unknown, "Ignoring unmodeled rig stream item");
            Vec::new()
        }
    }
}

fn handle_stream_final(
    final_record: StreamFinal,
    pending: &mut PendingRigMessage,
) -> Vec<ResponseEvent> {
    let mut events: Vec<ResponseEvent> = Vec::new();

    // 1. Reasoning item completes first — thinking precedes content on the
    //    wire (DeepSeek reasoning_content, Anthropic thinking blocks).
    if !pending.reasoning_buffer.is_empty() {
        let reasoning_text = std::mem::take(&mut pending.reasoning_buffer);
        let reasoning_id = pending
            .reasoning_item_id
            .take()
            .unwrap_or_else(|| format!("rsn_{}", unique_suffix()));
        events.push(ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
            id: Some(ResponseItemId::from_server(reasoning_id)),
            summary: vec![],
            content: Some(vec![ReasoningItemContent::ReasoningText {
                text: reasoning_text.clone(),
            }]),
            encrypted_content: Some(reasoning_text),
            internal_chat_message_metadata_passthrough: None,
        }));
    }

    // 2. Assistant message (only when actual text arrived).
    if !pending.text_buffer.is_empty() {
        let text = std::mem::take(&mut pending.text_buffer);
        events.push(ResponseEvent::OutputItemDone(ResponseItem::Message {
            id: pending.text_item_id.take().map(ResponseItemId::from_server),
            role: "assistant".into(),
            content: vec![ContentItem::OutputText { text }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }));
    }

    // 3. Each CONFIRMED tool call completes as a FunctionCall item carrying
    //    the final serialized arguments in arrival order. Calls that only
    //    produced deltas but no complete ToolCall event were discarded by
    //    rig (e.g. truncated output) and must not be resurrected.
    let tools = std::mem::take(&mut pending.tools);
    for internal_id in std::mem::take(&mut pending.tool_order) {
        let Some(tool) = tools.get(&internal_id) else {
            continue;
        };
        if !tool.confirmed {
            tracing::warn!(
                tool = %tool.name,
                "dropping unconfirmed tool call (no complete ToolCall event from rig)"
            );
            continue;
        }
        if pending.custom_tools.contains(tool.name.as_str()) {
            // Custom (freeform) tool: unwrap the {"input": string} wrapper
            // the request side declared and restore the CustomToolCall item
            // codex's dispatch expects for Custom payloads.
            let input = serde_json::from_str::<serde_json::Value>(&tool.arguments)
                .ok()
                .and_then(|v| v.get("input").and_then(|i| i.as_str()).map(str::to_string))
                .unwrap_or_else(|| tool.arguments.clone());
            events.push(ResponseEvent::OutputItemDone(ResponseItem::CustomToolCall {
                id: Some(ResponseItemId::from_server(internal_id.clone())),
                status: None,
                call_id: tool.call_id.clone(),
                name: tool.name.clone(),
                namespace: None,
                input,
                internal_chat_message_metadata_passthrough: None,
            }));
            continue;
        }
        events.push(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
            id: Some(ResponseItemId::from_server(internal_id.clone())),
            name: tool.name.clone(),
            namespace: None,
            arguments: tool.arguments.clone(),
            encrypted_function_args: None,
            call_id: tool.call_id.clone(),
            internal_chat_message_metadata_passthrough: None,
        }));
    }

    // 4. Terminal Completed with usage and end-of-turn semantics.
    let end_turn = final_record
        .finish_reason
        .as_ref()
        .map(|reason| !matches!(reason, FinishReason::ToolCalls));
    let token_usage = if final_record.usage.has_values() {
        Some(map_usage(&final_record.usage, &final_record.provider))
    } else {
        None
    };
    events.push(ResponseEvent::Completed {
        response_id: final_record.response_id.clone().unwrap_or_default(),
        token_usage,
        usage_metadata: None,
        end_turn,
    });
    pending.completed = true;
    events
}

pub(crate) fn map_usage(usage: &RigUsage, provider: &str) -> TokenUsage {
    // rig's Anthropic usage reports cache read/write OUTSIDE input_tokens,
    // while codex's TokenUsage.input_tokens is the TOTAL input (its
    // non-cached derivation subtracts cached_input_tokens). Normalize for
    // the Anthropic wire so cached turns don't undercount input to zero.
    let is_anthropic = provider.contains("anthropic");
    let input_tokens = if is_anthropic {
        usage.input_tokens + usage.cached_input_tokens + usage.cache_creation_input_tokens
    } else {
        usage.input_tokens
    };
    TokenUsage {
        input_tokens: input_tokens as i64,
        cached_input_tokens: usage.cached_input_tokens as i64,
        cache_write_input_tokens: usage.cache_creation_input_tokens as i64,
        output_tokens: usage.output_tokens as i64,
        reasoning_output_tokens: usage.reasoning_tokens as i64,
        total_tokens: usage.total_tokens as i64,
        codex_rollout_budget_units: None,
    }
}

fn ensure_message_item_added(pending: &mut PendingRigMessage, events: &mut Vec<ResponseEvent>) {
    if !pending.text_item_added {
        pending.text_item_added = true;
        let item_id = format!("txt_{}", unique_suffix());
        pending.text_item_id = Some(item_id.clone());
        events.push(ResponseEvent::OutputItemAdded(ResponseItem::Message {
            id: Some(ResponseItemId::from_server(item_id)),
            role: "assistant".into(),
            content: vec![],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }));
    }
}

fn ensure_reasoning_item_added(pending: &mut PendingRigMessage, events: &mut Vec<ResponseEvent>) {
    if !pending.reasoning_item_added {
        pending.reasoning_item_added = true;
        let item_id = format!("rsn_{}", unique_suffix());
        pending.reasoning_item_id = Some(item_id.clone());
        events.push(ResponseEvent::OutputItemAdded(ResponseItem::Reasoning {
            id: Some(ResponseItemId::from_server(item_id)),
            summary: vec![],
            content: None,
            encrypted_content: None,
            internal_chat_message_metadata_passthrough: None,
        }));
    }
}

/// The wire-level call id a provider would recognize on replay (OpenAI
/// `call_…`), preferring the correlator over the output-item handle.
fn provider_call_id(provider: Option<&ProviderCallId>) -> Option<String> {
    provider.map(|p| p.call_id.clone())
}

fn reasoning_text(reasoning: &RigReasoning) -> String {
    reasoning
        .content
        .iter()
        .filter_map(|block| match block {
            rig_core::completion::message::ReasoningContent::Text { text, .. } => {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn complete_tool(id: &str, wire_call_id: &str, name: &str, arguments: &str) -> StreamedAssistantContent {
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
        let mut pending = PendingRigMessage::new(std::sync::Arc::new(Default::default()));
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
        assert!(final_args.contains(": "), "raw spacing preserved: {final_args}");
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
            .position(|e| matches!(e, ResponseEvent::OutputItemAdded(ResponseItem::Message { .. })))
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
                matches!(e, ResponseEvent::OutputItemDone(ResponseItem::Reasoning { .. }))
            })
            .expect("reasoning done");
        let message_done = events
            .iter()
            .position(|e| {
                matches!(e, ResponseEvent::OutputItemDone(ResponseItem::Message { .. }))
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
        let mut pending = PendingRigMessage::new(std::sync::Arc::new(Default::default()));
        let events = rig_event_to_response_events(text_delta("hi"), &mut pending);
        assert!(events.iter().all(|e| !matches!(e, ResponseEvent::Completed { .. })));
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

    /// A completed call for a declared custom tool restores CustomToolCall
    /// with the unwrapped input string (codex dispatch expects Custom
    /// payloads for apply_patch-style handlers); function tools keep the
    /// FunctionCall item.
    #[test]
    fn custom_tool_done_restores_custom_tool_call() {
        let custom: std::collections::HashSet<String> =
            ["apply_patch".to_string()].into_iter().collect();
        let mut pending = PendingRigMessage::new(std::sync::Arc::new(custom));

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
            PendingRigMessage::new(std::sync::Arc::new(Default::default()));
        // name arrives → establishes item; deltas accumulate; NO complete event
        let name_event = StreamedAssistantContent::ToolCallDelta {
            internal_call_id: "t1".into(),
            content: ToolCallDeltaContent::Name("get_weather".into()),
        };
        let delta_event = StreamedAssistantContent::ToolCallDelta {
            internal_call_id: "t1".into(),
            content: ToolCallDeltaContent::Delta(r#"{"city":"# .into()),
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
