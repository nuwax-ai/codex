//! Bridge-boundary cassette: records the rig events a bridge receives and
//! replays them through the CURRENT conversion code, so bridge refactors
//! and rig upgrades can be regression-tested offline.
//!
//! This sits at a deliberate boundary: rig's SSE → `Item<StreamEvent>`
//! parsing is rig's responsibility (tested by rig itself); OUR conversion
//! (`Item` → codex `ResponseEvent`) is what this cassette exercises.
//! Recording captures the intermediate events plus the folded terminal —
//! rig 0.44 moved the provider's end out of the event stream into
//! `Streamed::finish()`, so events alone can no longer reproduce a
//! `Completed`. Replay feeds both into the live conversion functions — a
//! conversion bug introduced after recording makes replay fail.
//!
//! Fixtures recorded against rig 0.42 (`rig_events` holding untagged
//! `StreamedAssistantContent` JSON) are translated on read, never
//! re-recorded: the translation is part of what the replay exercises.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use codex_api::ResponseEvent;
use rig_core::completion::request::FinishReason;
use rig_core::completion::request::Usage as RigUsage;
use rig_core::streaming::Item;
use rig_core::streaming::Part;
use rig_core::streaming::PartKind;
use rig_core::streaming::StreamEvent;
use serde_json::Value;

use crate::convert_response::PendingRigMessage;
use crate::convert_response::StreamTerminal;
use crate::convert_response::handle_stream_terminal;
use crate::convert_response::rig_event_to_response_events;

/// The typed terminal facts a recording captures at `finish()` time.
#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
pub struct TerminalRecord {
    #[serde(default)]
    pub finish_reason: Option<FinishReason>,
    #[serde(default)]
    pub response_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub usage: RigUsage,
}

impl TerminalRecord {
    fn from_terminal(terminal: &StreamTerminal) -> Self {
        Self {
            finish_reason: terminal.finish_reason.clone(),
            response_id: terminal.response_id.clone(),
            model: terminal.model.clone(),
            usage: terminal.usage,
        }
    }

    fn into_terminal(self) -> StreamTerminal {
        StreamTerminal {
            finish_reason: self.finish_reason,
            response_id: self.response_id,
            model: self.model,
            usage: self.usage,
        }
    }
}

/// What the stream pump fills when the caller wants to record the bridge
/// boundary (cassette mode). `None` = no recording overhead.
#[derive(Default, Clone)]
pub struct RigRecording {
    pub events: Vec<Item<StreamEvent>>,
    pub terminal: Option<TerminalRecord>,
}

impl RigRecording {
    pub(crate) fn record_event(&mut self, event: Item<StreamEvent>) {
        self.events.push(event);
    }

    pub(crate) fn record_terminal(&mut self, terminal: &StreamTerminal) {
        self.terminal = Some(TerminalRecord::from_terminal(terminal));
    }
}

/// One recorded turn at the rig-event boundary.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct RigEventFixture {
    pub vendor: String,
    pub tag: String,
    /// Rig 0.44 stream items plus the folded terminal. Anthropic
    /// cumulative usage is corrected from wire counters before recording.
    /// HTTP auth headers are not captured at this boundary.
    #[serde(default)]
    pub items: Vec<Item<StreamEvent>>,
    #[serde(default)]
    pub terminal: Option<TerminalRecord>,
    /// Legacy (rig 0.42) recordings: untagged `StreamedAssistantContent`
    /// JSON, translated on read by [`translate_legacy_events`].
    #[serde(default)]
    pub rig_events: Vec<Value>,
    /// Names of custom (freeform) tools declared in the recorded request.
    /// Replay needs these to restore CustomToolCall items instead of
    /// FunctionCall for tools that were declared as custom.
    #[serde(default)]
    pub custom_tools: Vec<String>,
}

/// The result of replaying a recording through the current conversion.
///
/// The live pump delivers the streamed prefix (Created, item deltas)
/// BEFORE a terminal error reaches the consumer; a replay that returned
/// only `Result` would drop that prefix on error terminals, so the events
/// and the terminal error travel together.
#[derive(Debug)]
pub struct RigReplay {
    /// Every event the live stream would have delivered, in order.
    pub events: Vec<ResponseEvent>,
    /// The terminal error the live stream would have sent after `events`.
    pub error: Option<codex_api::ApiError>,
}

impl RigReplay {
    /// The events, or the terminal error when the attempt failed.
    pub fn into_result(self) -> Result<Vec<ResponseEvent>, codex_api::ApiError> {
        self.error.map_or(Ok(self.events), Err)
    }
}

/// Replays a recording through the CURRENT bridge conversion code, producing
/// the codex events the current implementation would emit.
///
/// This is the core value of the Rig-event cassette: even after the bridge
/// or rig is refactored, the same input must produce the same output — any
/// divergence is a conversion regression.
pub fn replay_rig_events(
    recording: &RigRecording,
    custom_tool_names: HashSet<String>,
) -> RigReplay {
    let mut pending = PendingRigMessage::new(Arc::new(custom_tool_names), "cassette".into());
    let mut out = vec![ResponseEvent::Created { response_id: None }];
    for event in &recording.events {
        match rig_event_to_response_events(event.clone(), &mut pending) {
            Ok(events) => out.extend(events),
            Err(error) => {
                return RigReplay {
                    events: out,
                    error: Some(error),
                };
            }
        }
        // The live pump stops polling the provider as soon as completion is
        // emitted.
        if pending.completed_emitted() {
            return RigReplay {
                events: out,
                error: None,
            };
        }
    }
    let Some(terminal) = recording.terminal.clone() else {
        return RigReplay {
            events: out,
            error: Some(codex_api::ApiError::Stream(
                "Rig cassette ended without a terminal record".into(),
            )),
        };
    };
    match handle_stream_terminal(terminal.into_terminal(), &mut pending) {
        Ok(events) => {
            out.extend(events);
            RigReplay {
                events: out,
                error: None,
            }
        }
        Err(error) => RigReplay {
            events: out,
            error: Some(error),
        },
    }
}

/// Convenience wrapper that replays from a fixture without custom tools
/// (the common scenario for recorded scenarios).
pub fn replay_fixture_events(fixture: &RigEventFixture) -> RigReplay {
    let custom_tools: HashSet<String> = fixture.custom_tools.iter().cloned().collect();
    if !fixture.rig_events.is_empty() || fixture.terminal.is_none() {
        // Legacy (0.42) fixtures carry their events in `rig_events`; the
        // translation below is part of the replay contract.
        let recording =
            match translate_legacy_events(&fixture.rig_events, fixture.terminal.as_ref()) {
                Ok(recording) => recording,
                Err(error) => {
                    return RigReplay {
                        events: Vec::new(),
                        error: Some(error),
                    };
                }
            };
        return replay_rig_events(&recording, custom_tools);
    }
    let recording = RigRecording {
        events: fixture.items.clone(),
        terminal: fixture.terminal.clone(),
    };
    replay_rig_events(&recording, custom_tools)
}

/// Translates rig 0.42 `StreamedAssistantContent` JSON (untagged) into a
/// rig 0.44 recording: growth events become part-indexed items, complete
/// events become `End` items carrying converted content, and the legacy
/// `Final` record becomes the terminal. Old recordings are never re-written.
fn translate_legacy_events(
    rig_events: &[Value],
    terminal: Option<&TerminalRecord>,
) -> Result<RigRecording, codex_api::ApiError> {
    let mut recording = RigRecording::default();
    let text_part = 0u32;
    let mut next_part = 1u32;
    let mut reasoning_ids: HashMap<String, u32> = HashMap::new();
    let mut tool_ids: HashMap<String, u32> = HashMap::new();
    for event in rig_events {
        // The legacy enum is untagged: discriminate by field shape.
        if event.get("kind").is_some() {
            // StreamFinal: usage/finish/model fields.
            recording.terminal = Some(legacy_terminal(event));
            continue;
        }
        if let Some(text) = event.get("text").and_then(Value::as_str) {
            recording.record_event(Item::Event(StreamEvent::Text {
                part: part_from_index(text_part)?,
                text: text.to_string(),
            }));
            continue;
        }
        if let (Some(id), Some(reasoning)) = (
            event.get("id").and_then(Value::as_str),
            event.get("reasoning"),
        ) {
            let part = *reasoning_ids.entry(id.to_string()).or_insert_with(|| {
                let part = next_part;
                next_part += 1;
                part
            });
            if let Some(text) = reasoning.as_str() {
                recording.record_event(Item::Event(StreamEvent::Reasoning {
                    part: part_from_index(part)?,
                    text: text.to_string(),
                }));
            } else {
                let converted = crate::reasoning::convert_legacy_reasoning_value(reasoning)?;
                for (index, block) in converted.into_iter().enumerate() {
                    // A 0.42 Reasoning may contain multiple signed, redacted
                    // or encrypted blocks. The first confirms its deltas;
                    // the rest each need a distinct 0.44 stream part.
                    let part = if index == 0 {
                        part
                    } else {
                        let part = next_part;
                        next_part += 1;
                        part
                    };
                    recording.record_event(Item::Event(StreamEvent::End {
                        part: part_from_index(part)?,
                        content: rig_core::completion::message::AssistantContent::Reasoning(block),
                    }));
                }
            }
            continue;
        }
        if let (Some(id), Some(content)) = (
            event.get("internal_call_id").and_then(Value::as_str),
            event.get("content"),
        ) {
            let part = *tool_ids.entry(id.to_string()).or_insert_with(|| {
                let part = next_part;
                next_part += 1;
                part
            });
            match content.get("Delta").and_then(Value::as_str) {
                Some(fragment) => {
                    recording.record_event(Item::Event(StreamEvent::Arguments {
                        part: part_from_index(part)?,
                        json: fragment.to_string(),
                    }));
                }
                // The legacy Name fragment announced the tool; 0.44 opens a
                // tool part with its name in `Start`.
                None => {
                    let name = content.get("Name").and_then(Value::as_str).unwrap_or("");
                    recording.record_event(Item::Event(StreamEvent::Start {
                        part: part_from_index(part)?,
                        kind: PartKind::ToolCall,
                        name: Some(name.to_string())
                            .filter(|name| !name.is_empty())
                            .and_then(|name| {
                                rig_core::completion::message::ToolName::new(name).ok()
                            }),
                    }));
                }
            }
            continue;
        }
        if let Some(call) = event.get("tool_call") {
            if let Some(id) = event.get("internal_call_id").and_then(Value::as_str) {
                let part = *tool_ids.entry(id.to_string()).or_insert_with(|| {
                    let part = next_part;
                    next_part += 1;
                    part
                });
                let converted = legacy_tool_call(call)?;
                recording.record_event(Item::Event(StreamEvent::End {
                    part: part_from_index(part)?,
                    content: rig_core::completion::message::AssistantContent::ToolCall(converted),
                }));
            }
            continue;
        }
        if let Some(unknown) = event.get("unknown") {
            let _ = unknown;
            continue;
        }
    }
    if recording.terminal.is_none()
        && let Some(terminal) = terminal
    {
        recording.terminal = Some(terminal.clone());
    }
    Ok(recording)
}

/// Builds the terminal record from a serialized 0.42 `StreamFinal`.
fn legacy_terminal(final_record: &Value) -> TerminalRecord {
    let protocol = if final_record["provider"]
        .as_str()
        .is_some_and(|provider| provider.contains("anthropic"))
    {
        crate::RigProtocol::Anthropic
    } else {
        crate::RigProtocol::Chat
    };
    let usage = serde_json::from_value::<LegacyUsage>(
        final_record.get("usage").cloned().unwrap_or(Value::Null),
    )
    .unwrap_or_default()
    .into_usage(protocol);
    let finish_reason = final_record
        .get("finish_reason")
        .cloned()
        .and_then(|value| serde_json::from_value::<FinishReason>(value).ok());
    let response_id = final_record
        .get("response_id")
        .and_then(Value::as_str)
        .or_else(|| final_record.get("message_id").and_then(Value::as_str))
        .map(str::to_string);
    let model = final_record
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string);
    TerminalRecord {
        finish_reason,
        response_id,
        model,
        usage,
    }
}

/// The 0.42 usage shape: plain integers with zero as the missing sentinel.
#[derive(serde::Deserialize, Default)]
struct LegacyUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    reasoning_tokens: u64,
}

impl LegacyUsage {
    /// 0.44 semantics: `input_tokens` includes cached and cache-written
    /// input. Only Anthropic's legacy input excludes cache; Chat's prompt
    /// count and reported total are already inclusive. Zero was the old
    /// missing sentinel, so only nonzero legacy counters count as reported.
    fn into_usage(self, protocol: crate::RigProtocol) -> RigUsage {
        let reported = |value: u64| (value > 0).then_some(value);
        let input = match protocol {
            crate::RigProtocol::Anthropic => self
                .input_tokens
                .saturating_add(self.cached_input_tokens)
                .saturating_add(self.cache_creation_input_tokens),
            crate::RigProtocol::Chat | crate::RigProtocol::Responses => self.input_tokens,
        };
        let total = match protocol {
            crate::RigProtocol::Anthropic => input.saturating_add(self.output_tokens),
            crate::RigProtocol::Chat | crate::RigProtocol::Responses => {
                if self.total_tokens == 0 {
                    input.saturating_add(self.output_tokens)
                } else {
                    self.total_tokens
                }
            }
        };
        let mut usage = RigUsage::new();
        usage.input_tokens = reported(input);
        usage.output_tokens = reported(self.output_tokens);
        usage.total_tokens = reported(total);
        usage.cached_input_tokens = reported(self.cached_input_tokens);
        usage.cache_creation_input_tokens = reported(self.cache_creation_input_tokens);
        usage.reasoning_tokens = reported(self.reasoning_tokens);
        usage
    }
}

/// Builds a `Part` from a position. `Part::new` is crate-private upstream;
/// the serde-transparent form is the public spelling. Every `u32` is a
/// valid position, but the failure surfaces as an error, never a panic.
fn part_from_index(index: u32) -> Result<Part, codex_api::ApiError> {
    serde_json::from_value::<Part>(serde_json::Value::from(index)).map_err(|error| {
        codex_api::ApiError::Stream(format!(
            "legacy stream position {index} is invalid: {error}"
        ))
    })
}

/// Converts a serialized 0.42 `ToolCall` into the 0.44 shape.
fn legacy_tool_call(
    call: &Value,
) -> Result<rig_core::completion::message::ToolCall, codex_api::ApiError> {
    let id = call
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| {
            call.get("provider")
                .and_then(|provider| provider.get("call_id"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default()
        .to_string();
    let name = call
        .get("function")
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let arguments = call
        .get("function")
        .and_then(|function| function.get("arguments"))
        .and_then(|arguments| arguments.as_object())
        .cloned()
        .unwrap_or_default();
    let function = rig_core::completion::message::ToolFunction {
        name: rig_core::completion::message::ToolName::new(name).map_err(|_| {
            codex_api::ApiError::Stream("legacy cassette tool name must not be empty".into())
        })?,
        arguments,
        invalid_arguments: None,
    };
    Ok(rig_core::completion::message::ToolCall::from_wire(
        id, function,
    ))
}

/// Extracts the names of custom (freeform) tools from a request's tool
/// list, for recording into the fixture alongside the rig events.
pub fn extract_custom_tool_names(request: &codex_api::ResponsesApiRequest) -> HashSet<String> {
    crate::request_tools::request_tools(request)
        .map(|selection| selection.meta.custom_names)
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "cassette_tests.rs"]
mod tests;
