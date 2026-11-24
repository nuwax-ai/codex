//! Buffer arguments until the stream's `End` confirms a tool call. The
//! completed `ToolCall` carries the provider's id and repaired arguments;
//! publishing earlier would make the Added/Delta/Done values disagree with
//! each other.

use codex_api::ApiError;
use codex_api::ResponseEvent;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ResponseItem;
use rig_core::completion::message::ToolCall;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

const MAX_TOOL_ARGS_BYTES: usize = 1_048_576;

#[derive(Default)]
struct PendingTool {
    raw: String,
    done: Option<ResponseItem>,
    events: Vec<ResponseEvent>,
    /// The provider's own output budget cut this call mid-arguments (rig
    /// closes it without a native item). It must never execute; the turn's
    /// terminal — usually the output cap — is what explains the cut.
    cut: bool,
}

pub(crate) struct PendingTools {
    custom: Arc<HashSet<String>>,
    calls: HashMap<String, PendingTool>,
    order: Vec<String>,
}

impl PendingTools {
    pub(crate) fn new(custom: Arc<HashSet<String>>) -> Self {
        Self {
            custom,
            calls: HashMap::new(),
            order: Vec::new(),
        }
    }

    fn entry(&mut self, id: String) -> &mut PendingTool {
        self.calls.entry(id.clone()).or_insert_with(|| {
            self.order.push(id);
            PendingTool::default()
        })
    }

    pub(crate) fn delta(&mut self, id: String, fragment: &str) -> Result<(), ApiError> {
        let entry = self.entry(id);
        if entry.raw.len().saturating_add(fragment.len()) > MAX_TOOL_ARGS_BYTES {
            return Err(invalid_tool("arguments exceeded the 1 MiB limit"));
        }
        entry.raw.push_str(fragment);
        Ok(())
    }

    pub(crate) fn complete(&mut self, id: String, call: ToolCall) -> Result<(), ApiError> {
        let custom = self.custom.contains(call.function.name.as_str());
        // 0.44 carries arguments as a map; non-object arguments the provider
        // sent arrive in `invalid_arguments`. Rig already restores a
        // double-encoded JSON string on its own (`ToolFunction::parse`), so
        // what reaches here is text nothing can parse: a truncated object or
        // the payload sent as free-form text.
        //
        // Fork (2026-11-24): such a call used to fail the whole turn
        // ("Invalid Rig tool call: arguments must be a JSON object"), which
        // MiMo's chat wire produced intermittently — 1 in 3 payload attempts,
        // on every platform. The provider had a real intent, so hand its bytes
        // to the tool instead: the call is emitted and the tool layer answers
        // with `FunctionCallError::RespondToModel`, which the model can act on
        // and retry inside the same turn.
        let arguments_map = call.function.arguments.clone();
        let mut unparsed_arguments: Option<String> = None;
        if let Some(raw) = call.function.invalid_arguments.as_deref() {
            // Rig closes a call the provider's own output budget cut with no
            // native item — the provider never stated it complete. Erroring
            // here would replace the turn's real terminal (the cap error)
            // with a tool-shape complaint and make core resample a budget
            // failure. Mark it cut instead: nothing executes, and `finish`
            // fails the turn only when the terminal itself was benign.
            if call.native.is_none() {
                let entry = self.entry(id);
                entry.cut = true;
                return Ok(());
            }
            tracing::warn!(
                tool = %call.function.name,
                bytes = raw.len(),
                "provider sent tool arguments that are not a JSON object; \
                 handing them to the tool layer so the model can retry"
            );
            unparsed_arguments = Some(raw.to_string());
        }
        let entry = self.entry(id.clone());
        if entry.done.is_some() {
            return Err(invalid_tool("duplicate completed tool call"));
        }
        let encoded = serde_json::Value::Object(arguments_map.clone()).to_string();
        if encoded.len() > MAX_TOOL_ARGS_BYTES {
            return Err(invalid_tool("arguments exceeded the 1 MiB limit"));
        }
        // Retain whitespace when the raw deltas encode the authoritative value.
        // Otherwise Rig's repaired value is emitted once, before Done.
        let arguments = if serde_json::from_str::<serde_json::Value>(&entry.raw)
            .ok()
            .as_ref()
            == Some(&serde_json::Value::Object(arguments_map.clone()))
        {
            std::mem::take(&mut entry.raw)
        } else {
            encoded
        };
        // The call's one identity: the provider's id, or one rig issued.
        let call_id = call.id.wire().into_owned();
        let (added, done, delta) = if custom {
            // A custom (free-form) tool is the natural home for arguments that
            // arrived as plain text: its input *is* the payload, so hand the
            // provider's bytes through and let the handler validate them.
            let input = match unparsed_arguments.as_deref() {
                Some(raw) => raw.to_string(),
                None => call
                    .function
                    .arguments
                    .get("input")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| {
                        arguments_map
                            .get("input")
                            .and_then(serde_json::Value::as_str)
                    })
                    .ok_or_else(|| {
                        invalid_tool("custom tool arguments must contain a string input")
                    })?
                    .to_string(),
            };
            if input.len() > MAX_TOOL_ARGS_BYTES {
                return Err(invalid_tool("arguments exceeded the 1 MiB limit"));
            }
            let mut item = ResponseItem::CustomToolCall {
                id: Some(ResponseItemId::from_server(id.clone())),
                status: None,
                call_id: call_id.clone(),
                name: call.function.name.to_string(),
                namespace: None,
                input: String::new(),
                internal_chat_message_metadata_passthrough: None,
                encrypted_content: None,
            };
            let added = item.clone();
            if let ResponseItem::CustomToolCall { input: body, .. } = &mut item {
                *body = input.clone();
            }
            (added, item, input)
        } else {
            // Function tools take a JSON string. Unparsed arguments are passed
            // through verbatim: core's parse failure becomes a
            // `RespondToModel` error the model can act on, which is strictly
            // better than discarding the turn.
            let arguments = unparsed_arguments.unwrap_or(arguments);
            let mut item = ResponseItem::FunctionCall {
                id: Some(ResponseItemId::from_server(id.clone())),
                status: None,
                call_id: call_id.clone(),
                name: call.function.name.to_string(),
                namespace: None,
                arguments: String::new(),
                encrypted_function_args: None,
                internal_chat_message_metadata_passthrough: None,
                encrypted_content: None,
            };
            let added = item.clone();
            if let ResponseItem::FunctionCall {
                arguments: body, ..
            } = &mut item
            {
                *body = arguments.clone();
            }
            (added, item, arguments)
        };
        entry.done = Some(done);
        entry.events = vec![
            ResponseEvent::OutputItemAdded(added),
            ResponseEvent::ToolCallInputDelta {
                item_id: id,
                call_id: Some(call_id),
                delta,
            },
        ];
        Ok(())
    }

    pub(crate) fn finish(&mut self) -> Result<HashMap<String, Vec<ResponseEvent>>, ApiError> {
        if self
            .calls
            .values()
            .any(|tool| tool.done.is_none() && !tool.cut)
        {
            return Err(invalid_tool("stream ended with an unconfirmed tool call"));
        }
        // A benign terminal (stop/tool calls) with a cut call means the
        // provider ended cleanly after sending arguments nothing can execute
        // — fail fast rather than silently dropping model intent.
        if self.calls.values().any(|tool| tool.cut) {
            return Err(invalid_tool(
                "a tool call closed with incomplete arguments and no provider item",
            ));
        }
        let mut calls = HashMap::new();
        for id in std::mem::take(&mut self.order) {
            if let Some(tool) = self.calls.remove(&id) {
                let mut events = tool.events;
                if let Some(done) = tool.done {
                    events.push(ResponseEvent::OutputItemDone(done));
                }
                calls.insert(id, events);
            }
        }
        Ok(calls)
    }
}

fn invalid_tool(message: &str) -> ApiError {
    ApiError::Stream(format!("Invalid Rig tool call: {message}"))
}
