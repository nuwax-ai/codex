//! Rig stream events to the Codex item lifecycle.
use codex_api::ApiError;
use codex_api::ResponseEvent;
use codex_protocol::protocol::ReportedResponseUsage;
use codex_protocol::protocol::ReportedUsageCounters;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TokenUsage;
use rig_core::completion::request::FinishReason;
use rig_core::completion::request::Usage as RigUsage;
use rig_core::streaming::StreamFinal;
use rig_core::streaming::StreamedAssistantContent;
use std::collections::HashSet;

enum DeferredOutput {
    Event(Box<ResponseEvent>),
    Tool(String),
}

pub(crate) struct PendingRigMessage {
    tools: crate::response_tools::PendingTools,
    response_id: String,
    /// Presence-preserving usage observed on the raw wire for the response
    /// being converted, supplied by the stream pump before the Final event.
    pub(crate) wire_reported_usage: Option<codex_protocol::protocol::ReportedResponseUsage>,
    segments: Vec<crate::hosted_replay::CapturedSegment>,
    tool_segment_ids: std::collections::HashMap<String, String>,
    text_buffer: String,
    text_item_id: Option<String>,
    reasoning: crate::reasoning::ReasoningState,
    reasoning_item_id: Option<String>,
    source: String,
    completed: bool,
    active_reasoning_ids: HashSet<String>,
    closed_reasoning_ids: HashSet<String>,
    tool_positions: HashSet<String>,
    suffix: Vec<DeferredOutput>,
}

pub(crate) fn unique_suffix() -> String {
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
            wire_reported_usage: None,
            response_id: unique_suffix(),
            segments: Vec::new(),
            tool_segment_ids: std::collections::HashMap::new(),
            text_buffer: String::new(),
            text_item_id: None,
            reasoning: Default::default(),
            reasoning_item_id: None,
            source,
            completed: false,
            active_reasoning_ids: HashSet::new(),
            closed_reasoning_ids: HashSet::new(),
            tool_positions: HashSet::new(),
            suffix: Vec::new(),
        }
    }
    pub(crate) fn response_id(&self) -> &str {
        &self.response_id
    }
    pub(crate) fn segments(&self) -> &[crate::hosted_replay::CapturedSegment] {
        &self.segments
    }

    fn new_segment(&mut self, kind: crate::hosted_replay::SegmentKind) -> String {
        let id = crate::hosted_replay::segment_id(&self.response_id, self.segments.len());
        self.segments.push(crate::hosted_replay::CapturedSegment {
            id: id.clone(),
            kind,
        });
        id
    }

    pub(crate) fn completed_emitted(&self) -> bool {
        self.completed
    }

    fn reasoning_added(
        &mut self,
        id: &str,
        events: &mut Vec<ResponseEvent>,
    ) -> Result<(), ApiError> {
        if self.closed_reasoning_ids.contains(id) {
            return Err(ApiError::Stream(
                "Rig reasoning changed after its item was completed".into(),
            ));
        }
        self.active_reasoning_ids.insert(id.to_string());
        if self.reasoning_item_id.is_none() {
            let id = self.new_segment(crate::hosted_replay::SegmentKind::Reasoning);
            self.reasoning_item_id = Some(id.clone());
            events.push(ResponseEvent::OutputItemAdded(ResponseItem::Reasoning {
                id: Some(ResponseItemId::from_server(id)),
                summary: vec![],
                content: None,
                encrypted_content: None,
                internal_chat_message_metadata_passthrough: None,
            }));
        }
        Ok(())
    }

    fn tool_position(&mut self, id: &str, events: &mut Vec<ResponseEvent>) {
        finish_reasoning(self, events);
        finish_text(self, events);
        self.suffix
            .extend(events.drain(..).map(Box::new).map(DeferredOutput::Event));
        if self.tool_positions.insert(id.to_string()) {
            let segment_id = self.new_segment(crate::hosted_replay::SegmentKind::Tool);
            self.tool_segment_ids.insert(id.to_string(), segment_id);
            self.suffix.push(DeferredOutput::Tool(id.to_string()));
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
            finish_reasoning(pending, &mut events);
            if pending.text_item_id.is_none() {
                let id = pending.new_segment(crate::hosted_replay::SegmentKind::Text);
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
            finish_text(pending, &mut events);
            pending.reasoning_added(&id, &mut events)?;
            let index = pending.reasoning.delta(id, &reasoning);
            events.push(ResponseEvent::ReasoningContentDelta {
                delta: reasoning,
                content_index: index as i64,
            });
        }
        StreamedAssistantContent::Reasoning { id, reasoning } => {
            finish_text(pending, &mut events);
            pending.reasoning_added(&id, &mut events)?;
            pending.reasoning.complete(id, reasoning);
        }
        StreamedAssistantContent::ToolCallDelta {
            internal_call_id,
            content,
        } => {
            pending.tool_position(&internal_call_id, &mut events);
            pending.tools.delta(internal_call_id, content)?;
        }
        StreamedAssistantContent::ToolCall {
            internal_call_id,
            tool_call,
        } => {
            pending.tool_position(&internal_call_id, &mut events);
            pending.tools.complete(internal_call_id, tool_call)?;
        }
        StreamedAssistantContent::Final(record) => {
            events.extend(handle_stream_final(record, pending)?)
        }
        StreamedAssistantContent::Unknown(unknown) => {
            tracing::debug!(?unknown, "Ignoring unmodeled Rig stream item")
        }
    }
    if !pending.completed && !pending.suffix.is_empty() {
        pending
            .suffix
            .extend(events.into_iter().map(Box::new).map(DeferredOutput::Event));
        return Ok(Vec::new());
    }
    Ok(events)
}

fn finish_reasoning(pending: &mut PendingRigMessage, events: &mut Vec<ResponseEvent>) {
    if let Some((content, encoded)) = pending.reasoning.finish(&pending.source) {
        pending
            .closed_reasoning_ids
            .extend(pending.active_reasoning_ids.drain());
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
}

fn finish_text(pending: &mut PendingRigMessage, events: &mut Vec<ResponseEvent>) {
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
}

/// Flushes a PAUSED attempt's content without a Completed terminal: the
/// turn continues via bridge-internal re-request (see stream.rs), and the
/// concatenated events of all attempts form the user-visible turn.
pub(crate) fn paused_final_events(
    record: StreamFinal,
    pending: &mut PendingRigMessage,
) -> Result<Vec<ResponseEvent>, ApiError> {
    let mut events = finish_pending_output(pending)?;
    if let Some(model) = record.model {
        events.push(ResponseEvent::ServerModel(model));
    }
    Ok(events)
}

/// Maps rig's normalized usage to a presence-preserving report.
///
/// Rig reports plain integers, so an unreported counter is indistinguishable
/// from zero: only nonzero counters are claimed, and the report is therefore
/// at best `Incomplete`.
fn rig_usage_report(usage: &rig_core::completion::Usage) -> Option<ReportedResponseUsage> {
    let counts = ReportedUsageCounters {
        input_tokens: (usage.input_tokens > 0).then_some(usage.input_tokens as i64),
        cached_input_tokens: (usage.cached_input_tokens > 0)
            .then_some(usage.cached_input_tokens as i64),
        cache_write_input_tokens: (usage.cache_creation_input_tokens > 0)
            .then_some(usage.cache_creation_input_tokens as i64),
        output_tokens: (usage.output_tokens > 0).then_some(usage.output_tokens as i64),
        reasoning_output_tokens: (usage.reasoning_tokens > 0)
            .then_some(usage.reasoning_tokens as i64),
        total_tokens: (usage.total_tokens > 0).then_some(usage.total_tokens as i64),
    };
    ReportedResponseUsage::from_counters(counts)
}

fn handle_stream_final(
    record: StreamFinal,
    pending: &mut PendingRigMessage,
) -> Result<Vec<ResponseEvent>, ApiError> {
    // Match native Responses' response.incomplete behavior. Never publish
    // executable tool Done items from a truncated or unsupported terminal.
    let end_turn = match record.finish_reason.as_ref() {
        Some(FinishReason::Stop) => Some(true),
        Some(FinishReason::ToolCalls) => Some(false),
        Some(FinishReason::Other(reason)) if reason == "model_context_window_exceeded" => {
            return Err(ApiError::ContextWindowExceeded);
        }
        // Same terminal-budget semantics as the native Responses decoder:
        // exhausting the caller-selected output cap is a configuration
        // condition, not a transport failure — retrying the same budget is
        // another paid sample, so the error is non-retryable and names the
        // fix. Partial output already streamed stays visible.
        Some(FinishReason::Length) => {
            // The pump supplies presence-true wire counters when it has them
            // (Anthropic); otherwise fall back to rig's normalized usage,
            // where a zero cannot be distinguished from an unreported counter,
            // so zeros stay unreported rather than being asserted as data.
            let reported_usage = pending
                .wire_reported_usage
                .take()
                .or_else(|| rig_usage_report(&record.usage));
            return Err(ApiError::CapExhausted {
                message: "Output token limit reached; increase max_tokens before retrying"
                    .to_string(),
                response_id: record.response_id.or(record.message_id.clone()),
                reported_usage,
            });
        }
        // The native Responses decoder classifies content_filter distinctly.
        Some(FinishReason::ContentFilter) => return Err(ApiError::ContentFilter),
        Some(reason @ FinishReason::Other(_)) => {
            return Err(ApiError::Stream(format!(
                "Incomplete Rig response, reason: {reason:?}"
            )));
        }
        None => None,
    };
    let mut events = finish_pending_output(pending)?;
    if let Some(model) = record.model {
        events.push(ResponseEvent::ServerModel(model));
    }
    let token_usage = record
        .usage
        .has_values()
        .then(|| map_usage(&record.usage, &record.provider));
    events.push(ResponseEvent::Completed {
        // Chat reports response_id; Anthropic reports message_id. Codex uses
        // one completion identifier for both, never a transport request ID.
        response_id: record.response_id.or(record.message_id).unwrap_or_default(),
        token_usage,
        usage_metadata: None,
        end_turn,
    });
    pending.completed = true;
    Ok(events)
}

fn finish_pending_output(pending: &mut PendingRigMessage) -> Result<Vec<ResponseEvent>, ApiError> {
    let mut tool_events = pending.tools.finish()?;
    let mut events = Vec::new();
    finish_reasoning(pending, &mut events);
    finish_text(pending, &mut events);
    if !pending.suffix.is_empty() {
        pending
            .suffix
            .extend(events.drain(..).map(Box::new).map(DeferredOutput::Event));
        for item in std::mem::take(&mut pending.suffix) {
            match item {
                DeferredOutput::Event(event) => events.push(*event),
                DeferredOutput::Tool(id) => {
                    if let Some(mut call) = tool_events.remove(&id) {
                        if let Some(segment_id) = pending.tool_segment_ids.get(&id) {
                            for event in &mut call {
                                match event {
                                    ResponseEvent::OutputItemAdded(
                                        ResponseItem::FunctionCall { id, .. }
                                        | ResponseItem::CustomToolCall { id, .. },
                                    )
                                    | ResponseEvent::OutputItemDone(
                                        ResponseItem::FunctionCall { id, .. }
                                        | ResponseItem::CustomToolCall { id, .. },
                                    ) => {
                                        *id = Some(ResponseItemId::from_server(segment_id.clone()));
                                    }
                                    ResponseEvent::ToolCallInputDelta { item_id, .. } => {
                                        *item_id = segment_id.clone();
                                    }
                                    _ => {}
                                }
                            }
                        }
                        events.extend(call);
                    }
                }
            }
        }
    }
    Ok(events)
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
