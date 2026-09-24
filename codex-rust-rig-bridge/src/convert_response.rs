//! Converts rig streaming events into Codex `ResponseEvent`s, preserving
//! the event-ordering contract codex's turn loop expects:
//! `OutputItemAdded` before the first delta, `OutputItemDone(Reasoning)`
//! before `OutputItemDone(Message)`, and exactly one terminal `Completed`.

use std::collections::HashMap;

use codex_api::ResponseEvent;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::ResponseItemId;
use codex_protocol::protocol::TokenUsage;
use rig_core::completion::request::FinishReason;
use rig_core::streaming::StreamFinal;
use rig_core::streaming::StreamedAssistantContent;
use rig_core::streaming::ToolCallDeltaContent;

/// Accumulated state for one streamed assistant turn.
pub(crate) struct PendingRigMessage {
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
}

struct PendingRigTool {
    name: String,
    /// Wire id used for codex's `call_id` (provider-issued when present).
    call_id: String,
    item_added: bool,
}

impl PendingRigMessage {
    pub(crate) fn new() -> Self {
        Self {
            text_buffer: String::new(),
            text_item_id: None,
            text_item_added: false,
            reasoning_buffer: String::new(),
            reasoning_item_id: None,
            reasoning_item_added: false,
            reasoning_content_index: 0,
            tools: HashMap::new(),
            tool_order: Vec::new(),
        }
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
            let entry = pending
                .tools
                .entry(internal_call_id.clone())
                .or_insert_with(|| {
                    pending.tool_order.push(internal_call_id.clone());
                    PendingRigTool {
                        name: String::new(),
                        call_id: String::new(),
                        item_added: false,
                    }
                });
            match content {
                ToolCallDeltaContent::Name(name) => {
                    entry.name = name;
                }
                ToolCallDeltaContent::Delta(args) => {
                    // Emit OutputItemAdded before the first argument delta so
                    // turn.rs can attach a diff consumer.
                    if !entry.item_added && !entry.name.is_empty() {
                        entry.item_added = true;
                        events.push(ResponseEvent::OutputItemAdded(
                            ResponseItem::FunctionCall {
                                id: Some(ResponseItemId::from_server(
                                    internal_call_id.clone(),
                                )),
                                name: entry.name.clone(),
                                namespace: None,
                                arguments: String::new(),
                                encrypted_function_args: None,
                                call_id: internal_call_id.clone(),
                                internal_chat_message_metadata_passthrough: None,
                            },
                        ));
                    }
                    if !args.is_empty() {
                        events.push(ResponseEvent::ToolCallInputDelta {
                            item_id: internal_call_id.clone(),
                            call_id: Some(internal_call_id.clone()),
                            delta: args,
                        });
                    }
                }
            }
            events
        }
        StreamedAssistantContent::ToolCall {
            tool_call,
            internal_call_id,
        } => {
            // The complete tool call supersedes the fragments above: record
            // the authoritative name/wire id, and emit `OutputItemAdded` now
            // if the deltas never did (name-only fragments).
            let mut events = Vec::new();
            let wire_call_id = provider_call_id(tool_call.provider.as_ref())
                .unwrap_or_else(|| tool_call.id.to_string());
            let entry = pending
                .tools
                .entry(internal_call_id.clone())
                .or_insert_with(|| {
                    pending.tool_order.push(internal_call_id.clone());
                    PendingRigTool {
                        name: String::new(),
                        call_id: String::new(),
                        item_added: false,
                    }
                });
            entry.name = tool_call.function.name.clone();
            entry.call_id = wire_call_id.clone();
            if !entry.item_added {
                entry.item_added = true;
                events.push(ResponseEvent::OutputItemAdded(
                    ResponseItem::FunctionCall {
                        id: Some(ResponseItemId::from_server(internal_call_id.clone())),
                        name: entry.name.clone(),
                        namespace: None,
                        arguments: String::new(),
                        encrypted_function_args: None,
                        call_id: wire_call_id.clone(),
                        internal_chat_message_metadata_passthrough: None,
                    },
                ));
            }
            events
        }
        StreamedAssistantContent::Reasoning { reasoning, .. } => {
            // Complete reasoning block — supersede accumulated deltas. On the
            // codex side we model this as the final reasoning content; the
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
            .unwrap_or_else(|| format!("rsn_{}", pending.reasoning_content_index));
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

    // 3. Each tool call completes as a FunctionCall item carrying the final
    //    serialized arguments.
    let tools = std::mem::take(&mut pending.tools);
    for internal_id in std::mem::take(&mut pending.tool_order) {
        let Some(tool) = tools.get(&internal_id) else {
            continue;
        };
        // Arguments come from rig's aggregated choice on the stream object,
        // which we surface via `StreamingCompletionResponse::finish()` in the
        // pump; here the per-call arguments are reconstructed from the item
        // we recorded — the pump overwrites this with the authoritative value
        // before emitting.
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
        Some(map_usage(&final_record.usage))
    } else {
        None
    };
    events.push(ResponseEvent::Completed {
        response_id: final_record.response_id.clone().unwrap_or_default(),
        token_usage,
        usage_metadata: None,
        end_turn,
    });
    events
}

pub(crate) fn map_usage(usage: &rig_core::completion::request::Usage) -> TokenUsage {
    TokenUsage {
        input_tokens: usage.input_tokens as i64,
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
        let item_id = format!("txt_{}", pending.text_buffer.len());
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
        let item_id = format!("rsn_{}", pending.reasoning_content_index);
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
fn provider_call_id(provider: Option<&rig_core::completion::message::ProviderCallId>) -> Option<String> {
    provider.map(|p| p.call_id.clone().unwrap_or_else(|| p.item_id.clone()))
}

fn reasoning_text(reasoning: &rig_core::completion::message::Reasoning) -> String {
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
