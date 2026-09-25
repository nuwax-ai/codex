//! Rig stream events to the Codex item lifecycle.
use codex_api::ApiError;
use codex_api::ResponseEvent;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TokenUsage;
use rig_core::completion::request::FinishReason;
use rig_core::completion::request::Usage as RigUsage;
use rig_core::streaming::StreamFinal;
use rig_core::streaming::StreamedAssistantContent;

pub(crate) struct PendingRigMessage {
    tools: crate::response_tools::PendingTools,
    text_buffer: String,
    text_item_id: Option<String>,
    reasoning: crate::reasoning::ReasoningState,
    reasoning_item_id: Option<String>,
    source: String,
    completed: bool,
}

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
    pub(crate) fn new(
        custom_tools: std::sync::Arc<std::collections::HashSet<String>>,
        source: String,
    ) -> Self {
        Self {
            tools: crate::response_tools::PendingTools::new(custom_tools),
            text_buffer: String::new(),
            text_item_id: None,
            reasoning: Default::default(),
            reasoning_item_id: None,
            source,
            completed: false,
        }
    }
    pub(crate) fn completed_emitted(&self) -> bool {
        self.completed
    }

    fn reasoning_added(&mut self, events: &mut Vec<ResponseEvent>) {
        if self.reasoning_item_id.is_none() {
            let id = format!("rsn_{}", unique_suffix());
            self.reasoning_item_id = Some(id.clone());
            events.push(ResponseEvent::OutputItemAdded(ResponseItem::Reasoning {
                id: Some(ResponseItemId::from_server(id)),
                summary: vec![],
                content: None,
                encrypted_content: None,
                internal_chat_message_metadata_passthrough: None,
            }));
        }
    }
}

pub(crate) fn rig_event_to_response_events(
    event: StreamedAssistantContent,
    pending: &mut PendingRigMessage,
) -> Result<Vec<ResponseEvent>, ApiError> {
    if pending.completed {
        return Ok(Vec::new());
    }
    let mut events = Vec::new();
    match event {
        StreamedAssistantContent::Text(text) => {
            if pending.text_item_id.is_none() {
                let id = format!("txt_{}", unique_suffix());
                pending.text_item_id = Some(id.clone());
                events.push(ResponseEvent::OutputItemAdded(ResponseItem::Message {
                    id: Some(ResponseItemId::from_server(id)),
                    role: "assistant".into(),
                    content: vec![],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                }));
            }
            pending.text_buffer.push_str(&text.text);
            events.push(ResponseEvent::OutputTextDelta(text.text));
        }
        StreamedAssistantContent::ReasoningDelta { id, reasoning, .. } => {
            pending.reasoning_added(&mut events);
            let index = pending.reasoning.delta(id, &reasoning);
            events.push(ResponseEvent::ReasoningContentDelta {
                delta: reasoning,
                content_index: index as i64,
            });
        }
        StreamedAssistantContent::Reasoning { id, reasoning } => {
            pending.reasoning_added(&mut events);
            pending.reasoning.complete(id, reasoning);
        }
        StreamedAssistantContent::ToolCallDelta {
            internal_call_id,
            content,
        } => pending.tools.delta(internal_call_id, content)?,
        StreamedAssistantContent::ToolCall {
            internal_call_id,
            tool_call,
        } => events.extend(pending.tools.complete(internal_call_id, tool_call)?),
        StreamedAssistantContent::Final(record) => {
            events.extend(handle_stream_final(record, pending))
        }
        StreamedAssistantContent::Unknown(unknown) => {
            tracing::debug!(?unknown, "Ignoring unmodeled Rig stream item")
        }
    }
    Ok(events)
}

fn handle_stream_final(record: StreamFinal, pending: &mut PendingRigMessage) -> Vec<ResponseEvent> {
    let mut events = Vec::new();
    if let Some((content, encoded)) = pending.reasoning.finish(&pending.source) {
        events.push(ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
            id: pending
                .reasoning_item_id
                .take()
                .map(ResponseItemId::from_server),
            summary: vec![],
            content: Some(content),
            encrypted_content: Some(encoded),
            internal_chat_message_metadata_passthrough: None,
        }));
    }
    if let Some(id) = pending.text_item_id.take() {
        events.push(ResponseEvent::OutputItemDone(ResponseItem::Message {
            id: Some(ResponseItemId::from_server(id)),
            role: "assistant".into(),
            content: vec![ContentItem::OutputText {
                text: std::mem::take(&mut pending.text_buffer),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }));
    }
    events.extend(pending.tools.finish());
    if let Some(model) = record.model {
        events.push(ResponseEvent::ServerModel(model));
    }
    let end_turn = record
        .finish_reason
        .as_ref()
        .map(|reason| !matches!(reason, FinishReason::ToolCalls));
    let token_usage = record
        .usage
        .has_values()
        .then(|| map_usage(&record.usage, &record.provider));
    events.push(ResponseEvent::Completed {
        response_id: record.response_id.unwrap_or_default(),
        token_usage,
        usage_metadata: None,
        end_turn,
    });
    pending.completed = true;
    events
}

pub(crate) fn map_usage(usage: &RigUsage, provider: &str) -> TokenUsage {
    // Anthropic reports cached and newly cached input separately; Codex includes both.
    let input_tokens = if provider.contains("anthropic") {
        usage
            .input_tokens
            .saturating_add(usage.cached_input_tokens)
            .saturating_add(usage.cache_creation_input_tokens)
    } else {
        usage.input_tokens
    };
    let total = if usage.total_tokens == 0 {
        input_tokens.saturating_add(usage.output_tokens)
    } else {
        usage.total_tokens
    };
    let count = |value| i64::try_from(value).unwrap_or(i64::MAX);
    TokenUsage {
        input_tokens: count(input_tokens),
        cached_input_tokens: count(usage.cached_input_tokens),
        cache_write_input_tokens: count(usage.cache_creation_input_tokens),
        output_tokens: count(usage.output_tokens),
        reasoning_output_tokens: count(usage.reasoning_tokens),
        total_tokens: count(total),
        codex_rollout_budget_units: None,
    }
}

#[cfg(test)]
#[path = "convert_response_tests.rs"]
mod tests;
