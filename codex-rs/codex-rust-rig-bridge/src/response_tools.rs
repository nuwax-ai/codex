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
        // 0.44 carries arguments as a map; non-object arguments the provider
        // sent arrive in `invalid_arguments` and stay unexecutable.
        if call.function.invalid_arguments.is_some() {
            return Err(invalid_tool("arguments must be a JSON object"));
        }
        let custom = self.custom.contains(call.function.name.as_str());
        let entry = self.entry(id.clone());
        if entry.done.is_some() {
            return Err(invalid_tool("duplicate completed tool call"));
        }
        let arguments_map = call.function.arguments.clone();
        let encoded = serde_json::Value::Object(arguments_map.clone()).to_string();
        if encoded.len() > MAX_TOOL_ARGS_BYTES {
            return Err(invalid_tool("arguments exceeded the 1 MiB limit"));
        }
        // Retain whitespace when the raw deltas encode the authoritative value.
        // Otherwise Rig's repaired value is emitted once, before Done.
        let arguments = if serde_json::from_str::<serde_json::Value>(&entry.raw)
            .ok()
            .as_ref()
            == Some(&serde_json::Value::Object(arguments_map))
        {
            std::mem::take(&mut entry.raw)
        } else {
            encoded
        };
        // The call's one identity: the provider's id, or one rig issued.
        let call_id = call.id.wire().into_owned();
        let (added, done, delta) = if custom {
            let input = call
                .function
                .arguments
                .get("input")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid_tool("custom tool arguments must contain a string input"))?
                .to_string();
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
            let mut item = ResponseItem::FunctionCall {
                id: Some(ResponseItemId::from_server(id.clone())),
                call_id: call_id.clone(),
                name: call.function.name.to_string(),
                namespace: None,
                arguments: String::new(),
                encrypted_function_args: None,
                internal_chat_message_metadata_passthrough: None,
                encrypted_content: None,
                status: None,
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
        if self.calls.values().any(|tool| tool.done.is_none()) {
            return Err(invalid_tool("stream ended with an unconfirmed tool call"));
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
