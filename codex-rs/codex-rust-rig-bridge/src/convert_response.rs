//! Rig stream events to the Codex item lifecycle.
use codex_api::ApiError;
use codex_api::ResponseEvent;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::ReportedResponseUsage;
use codex_protocol::protocol::ReportedUsageCounters;
use codex_protocol::protocol::TokenUsage;
use rig_core::completion::request::FinishReason;
use rig_core::completion::request::Usage as RigUsage;
use rig_core::streaming::Item;
use rig_core::streaming::Part;
use rig_core::streaming::PartKind;
use rig_core::streaming::StreamEvent;
use std::collections::HashSet;

enum DeferredOutput {
    Event(Box<ResponseEvent>),
    Tool(String),
    Reasoning(String),
}

/// The stream's terminal facts, extracted from rig's `CompletionResponse`
/// (or synthesized when replaying a legacy cassette). Decoupled from the
/// rig type so replay paths and the live pump share one implementation.
pub(crate) struct StreamTerminal {
    pub finish_reason: Option<FinishReason>,
    pub response_id: Option<String>,
    pub model: Option<String>,
    pub usage: RigUsage,
}

pub(crate) struct PendingRigMessage {
    tools: crate::response_tools::PendingTools,
    response_id: String,
    /// Presence-preserving usage observed on the raw wire for the response
    /// being converted, supplied by the stream pump before the terminal.
    pub(crate) wire_reported_usage: Option<codex_protocol::protocol::ReportedResponseUsage>,
    segments: Vec<crate::hosted_replay::CapturedSegment>,
    tool_segment_ids: std::collections::HashMap<String, String>,
    text_buffer: String,
    text_item_id: Option<String>,
    reasoning: crate::reasoning::ReasoningState,
    reasoning_item_id: Option<String>,
    source: String,
    completed: bool,
    active_reasoning_parts: HashSet<String>,
    open_reasoning_parts: HashSet<String>,
    closed_reasoning_parts: HashSet<String>,
    deferred_reasoning: std::collections::HashMap<String, Option<ResponseEvent>>,
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
            active_reasoning_parts: HashSet::new(),
            open_reasoning_parts: HashSet::new(),
            closed_reasoning_parts: HashSet::new(),
            deferred_reasoning: std::collections::HashMap::new(),
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
        if self.closed_reasoning_parts.contains(id) {
            return Err(ApiError::Stream(
                "Rig reasoning changed after its item was completed".into(),
            ));
        }
        self.active_reasoning_parts.insert(id.to_string());
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
        if !self.open_reasoning_parts.is_empty()
            && let Some(reasoning_id) = &self.reasoning_item_id
            && !self.deferred_reasoning.contains_key(reasoning_id)
        {
            self.deferred_reasoning.insert(reasoning_id.clone(), None);
            self.suffix
                .push(DeferredOutput::Reasoning(reasoning_id.clone()));
        }
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

/// A stream part's correlation key: parts are stream-local positions, used
/// only to correlate growth events with their `End` within one attempt.
fn part_key(part: Part) -> String {
    part.index().to_string()
}

pub(crate) fn rig_event_to_response_events(
    event: Item<StreamEvent>,
    pending: &mut PendingRigMessage,
) -> Result<Vec<ResponseEvent>, ApiError> {
    if pending.completed {
        return Ok(Vec::new());
    }
    let mut events = Vec::new();
    match event {
        Item::Event(StreamEvent::Text { text, .. }) => {
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
                    encrypted_content: None,
                    status: None,
                }));
            }
            pending.text_buffer.push_str(&text);
            events.push(ResponseEvent::OutputTextDelta(text));
        }
        Item::Event(StreamEvent::Reasoning { part, text }) => {
            let id = part_key(part);
            if !pending.active_reasoning_parts.contains(&id) {
                finish_text(pending, &mut events);
            }
            pending.reasoning_added(&id, &mut events)?;
            let index = pending.reasoning.delta(id, &text);
            events.push(ResponseEvent::ReasoningContentDelta {
                delta: text,
                content_index: index as i64,
            });
        }
        Item::Event(StreamEvent::Arguments { part, json }) => {
            let id = part_key(part);
            pending.tool_position(&id, &mut events);
            pending.tools.delta(id, &json)?;
        }
        Item::Event(StreamEvent::Start {
            kind: PartKind::Reasoning,
            part,
            ..
        }) => {
            finish_text(pending, &mut events);
            let id = part_key(part);
            pending.reasoning_added(&id, &mut events)?;
            pending.open_reasoning_parts.insert(id);
        }
        Item::Event(StreamEvent::Start {
            kind: PartKind::ToolCall,
            part,
            ..
        }) => {
            // A tool part opens once its tool is named; argument fragments
            // may follow. Opening the position here keeps the deferred tool
            // ordering stable even when no fragment ever arrives.
            let id = part_key(part);
            pending.tool_position(&id, &mut events);
        }
        Item::Event(StreamEvent::End { part, content }) => {
            match content {
                rig_core::completion::message::AssistantContent::Text(_) => {
                    // Deltas already streamed the text; the completed content
                    // confirms the accumulated buffer.
                }
                rig_core::completion::message::AssistantContent::Reasoning(reasoning) => {
                    let id = part_key(part);
                    if !pending.active_reasoning_parts.contains(&id) {
                        finish_text(pending, &mut events);
                    }
                    pending.reasoning_added(&id, &mut events)?;
                    pending.open_reasoning_parts.remove(&id);
                    pending.reasoning.complete(id, reasoning);
                }
                rig_core::completion::message::AssistantContent::ToolCall(call) => {
                    let id = part_key(part);
                    pending.tool_position(&id, &mut events);
                    pending.tools.complete(id, call)?;
                }
                // Images and opaque provider items have no Codex item here.
                _ => {}
            }
        }
        Item::Event(StreamEvent::Start { .. }) => {}
        Item::Unknown(unknown) => {
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
    // Chat keeps its reasoning part open while answer text or tools stream.
    // Only End carries the authoritative native reasoning item for replay.
    if !pending.open_reasoning_parts.is_empty() {
        return;
    }
    if let Some((content, encoded)) = pending.reasoning.finish(&pending.source) {
        pending
            .closed_reasoning_parts
            .extend(pending.active_reasoning_parts.drain());
        let id = pending.reasoning_item_id.take();
        let event = ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
            id: id.clone().map(ResponseItemId::from_server),
            summary: vec![],
            content: Some(content),
            encrypted_content: Some(encoded),
            internal_chat_message_metadata_passthrough: None,
        });
        if let Some(slot) = id.and_then(|id| pending.deferred_reasoning.get_mut(&id)) {
            *slot = Some(event);
        } else {
            events.push(event);
        }
    }
}

fn finish_text(pending: &mut PendingRigMessage, events: &mut Vec<ResponseEvent>) {
    if let Some(id) = pending.text_item_id.take() {
        events.push(ResponseEvent::OutputItemDone(ResponseItem::Message {
            id: Some(ResponseItemId::from_server(id)),
            role: "assistant".into(),
            content: vec![ContentItem::OutputText {
                text: std::mem::take(&mut pending.text_buffer),
                annotations: None,
                logprobs: None,
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
            encrypted_content: None,
            status: None,
        }));
    }
}

/// Flushes a PAUSED attempt's content without a Completed terminal: the
/// turn continues via bridge-internal re-request (see stream.rs), and the
/// concatenated events of all attempts form the user-visible turn.
pub(crate) fn paused_final_events(
    terminal: StreamTerminal,
    pending: &mut PendingRigMessage,
) -> Result<Vec<ResponseEvent>, ApiError> {
    let mut events = finish_pending_output(pending)?;
    if let Some(model) = terminal.model {
        events.push(ResponseEvent::ServerModel(model));
    }
    Ok(events)
}

/// Maps rig's normalized usage to a presence-preserving report.
///
/// 0.44 reports counters as `Option`: `None` is unreported, `Some(0)` an
/// explicit zero, so both survive as claimed counters.
fn rig_usage_report(usage: &RigUsage) -> Option<ReportedResponseUsage> {
    let counter = |value: Option<u64>| value.and_then(|value| i64::try_from(value).ok());
    let counts = ReportedUsageCounters {
        input_tokens: counter(usage.input_tokens),
        cached_input_tokens: counter(usage.cached_input_tokens),
        cache_write_input_tokens: counter(usage.cache_creation_input_tokens),
        output_tokens: counter(usage.output_tokens),
        reasoning_output_tokens: counter(usage.reasoning_tokens),
        // Rig may derive this value; raw wire presence is required before
        // it can be claimed as a provider-reported total.
        total_tokens: None,
    };
    ReportedResponseUsage::from_counters(counts)
}

pub(crate) fn handle_stream_terminal(
    terminal: StreamTerminal,
    pending: &mut PendingRigMessage,
) -> Result<Vec<ResponseEvent>, ApiError> {
    // Match native Responses' response.incomplete behavior. Never publish
    // executable tool Done items from a truncated or unsupported terminal.
    let end_turn = match terminal.finish_reason.as_ref() {
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
            // A cap cannot confirm pending tool calls or promote partial
            // content into ordinary completed history. Already-delivered
            // deltas stay visible; durable diagnostics require their own
            // bounded, explicitly partial representation.
            return Err(ApiError::CapExhausted {
                message: "Output token limit reached; increase max_tokens before retrying"
                    .to_string(),
                response_id: terminal.response_id.clone(),
                reported_usage: pending
                    .wire_reported_usage
                    .take()
                    .or_else(|| rig_usage_report(&terminal.usage)),
            });
        }
        // The native Responses decoder classifies content_filter distinctly.
        Some(FinishReason::ContentFilter) => return Err(ApiError::ContentFilter),
        Some(reason @ FinishReason::Other(_)) => {
            return Err(ApiError::Stream(format!(
                "Incomplete Rig response, reason: {reason:?}"
            )));
        }
        // `FinishReason` is non-exhaustive upstream; an unmodeled reason
        // ends the turn the same way an unsupported one does.
        Some(_) => {
            return Err(ApiError::Stream(
                "Incomplete Rig response, reason: unsupported".into(),
            ));
        }
        None => None,
    };
    let mut events = finish_pending_output(pending)?;
    if let Some(model) = terminal.model {
        events.push(ResponseEvent::ServerModel(model));
    }
    let token_usage = terminal
        .usage
        .is_reported()
        .then(|| map_usage(&terminal.usage));
    events.push(ResponseEvent::Completed {
        // One completion identifier for both wires, never a transport
        // request ID.
        response_id: terminal.response_id.unwrap_or_default(),
        token_usage,
        usage_metadata: None,
        end_turn,
    });
    pending.completed = true;
    Ok(events)
}

fn finish_pending_output(pending: &mut PendingRigMessage) -> Result<Vec<ResponseEvent>, ApiError> {
    if !pending.open_reasoning_parts.is_empty() {
        return Err(ApiError::Stream(
            "stream ended with unconfirmed reasoning".into(),
        ));
    }
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
                DeferredOutput::Reasoning(id) => {
                    let event = pending
                        .deferred_reasoning
                        .remove(&id)
                        .flatten()
                        .ok_or_else(|| ApiError::Stream("missing completed reasoning".into()))?;
                    events.push(event);
                }
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

pub(crate) fn map_usage(usage: &RigUsage) -> TokenUsage {
    // 0.44's `input_tokens` already includes cached and cache-written input
    // for every provider, so the total never re-adds them.
    let input_tokens = usage.input_tokens.unwrap_or_default();
    let output_tokens = usage.output_tokens.unwrap_or_default();
    let total = usage
        .total_tokens
        .unwrap_or_else(|| input_tokens.saturating_add(output_tokens));
    let count = |value: Option<u64>| i64::try_from(value.unwrap_or_default()).unwrap_or(i64::MAX);
    TokenUsage {
        input_tokens: count(usage.input_tokens),
        cached_input_tokens: count(usage.cached_input_tokens),
        cache_write_input_tokens: count(usage.cache_creation_input_tokens),
        output_tokens: count(Some(output_tokens)),
        reasoning_output_tokens: count(usage.reasoning_tokens),
        total_tokens: i64::try_from(total).unwrap_or(i64::MAX),
        codex_rollout_budget_units: None,
    }
}

#[cfg(test)]
#[path = "convert_response_tests.rs"]
mod tests;
