//! Buffer arguments until Rig confirms a tool call. Rig can repair fragmented
//! JSON and does not expose the provider call ID until that event; publishing
//! earlier would make the Added/Delta/Done values disagree with each other.

use codex_api::ApiError;
use codex_api::ResponseEvent;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ResponseItem;
use rig_core::completion::message::ToolCall;
use rig_core::streaming::ToolCallDeltaContent;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

const MAX_TOOL_ARGS_BYTES: usize = 1_048_576;

#[derive(Default)]
struct PendingTool {
    raw: String,
    done: Option<ResponseItem>,
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

    pub(crate) fn delta(
        &mut self,
        id: String,
        delta: ToolCallDeltaContent,
    ) -> Result<(), ApiError> {
        let entry = self.entry(id);
        if let ToolCallDeltaContent::Delta(text) = delta {
            if entry.raw.len().saturating_add(text.len()) > MAX_TOOL_ARGS_BYTES {
                return Err(invalid_tool("arguments exceeded the 1 MiB limit"));
            }
            entry.raw.push_str(&text);
        }
        Ok(())
    }

    pub(crate) fn complete(
        &mut self,
        id: String,
        call: ToolCall,
    ) -> Result<Vec<ResponseEvent>, ApiError> {
        let custom = self.custom.contains(&call.function.name);
        let entry = self.entry(id.clone());
        if entry.done.is_some() {
            return Err(invalid_tool("duplicate completed tool call"));
        }
        let encoded = call.function.arguments.to_string();
        if encoded.len() > MAX_TOOL_ARGS_BYTES {
            return Err(invalid_tool("arguments exceeded the 1 MiB limit"));
        }
        // Retain whitespace when the raw deltas encode the authoritative value.
        // Otherwise Rig's repaired value is emitted once, before Done.
        let arguments = if serde_json::from_str::<serde_json::Value>(&entry.raw)
            .ok()
            .as_ref()
            == Some(&call.function.arguments)
        {
            std::mem::take(&mut entry.raw)
        } else {
            encoded
        };
        let call_id = call
            .provider
            .as_ref()
            .map(|provider| provider.call_id.clone())
            .unwrap_or_else(|| call.id.to_string());
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
                name: call.function.name,
                namespace: None,
                input: String::new(),
                internal_chat_message_metadata_passthrough: None,
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
                name: call.function.name,
                namespace: None,
                arguments: String::new(),
                encrypted_function_args: None,
                internal_chat_message_metadata_passthrough: None,
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
        Ok(vec![
            ResponseEvent::OutputItemAdded(added),
            ResponseEvent::ToolCallInputDelta {
                item_id: id,
                call_id: Some(call_id),
                delta,
            },
        ])
    }

    pub(crate) fn finish(&mut self) -> Vec<ResponseEvent> {
        std::mem::take(&mut self.order)
            .into_iter()
            .filter_map(|id| {
                self.calls
                    .remove(&id)
                    .and_then(|tool| tool.done)
                    .map(ResponseEvent::OutputItemDone)
            })
            .collect()
    }
}

fn invalid_tool(message: &str) -> ApiError {
    ApiError::Stream(format!("Invalid Rig tool call: {message}"))
}
