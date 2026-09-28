//! Hosted (server-side) tool translation for the chat-family wires.
//!
//! Codex declares hosted tools in the Responses shape (e.g.
//! `{"type":"web_search"}`). Chat Completions has no hosted-tool concept;
//! Anthropic exposes server-side tools as typed entries (e.g.
//! `web_search_20250305`). This module owns the per-tool translation table so
//! supporting the next hosted tool is a table entry plus a response mapping,
//! not a scattered special case. Vendors behind Anthropic-compatible gateways
//! may rename the executed tool (GLM serves `web_search_prime`), so response
//! recognition matches on the tool family, never on one exact name.

use codex_api::ResponseEvent;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::WebSearchAction;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::Value;
use serde_json::json;

/// Translates one hosted Responses tool declaration into its Anthropic
/// server-tool entry. `None` means the tool has no Anthropic equivalent and
/// must be dropped (with a warning) rather than guessed at.
pub(crate) fn anthropic_server_tool(hosted: &Value) -> Option<Value> {
    match hosted.get("type").and_then(Value::as_str) {
        // Anthropic's server-side web search. The dated suffix is the
        // official tool version; verified live against GLM's
        // Anthropic-compatible gateway.
        Some("web_search") => Some(json!({
            "type": "web_search_20250305",
            "name": "web_search",
        })),
        other => {
            tracing::warn!(
                tool_type = ?other,
                "Hosted tool has no Anthropic server-tool equivalent; dropping it"
            );
            None
        }
    }
}

/// True when a streamed Anthropic `server_tool_use` block belongs to the web
/// search family. Anthropic names the tool `web_search`; GLM's gateway
/// executes `web_search_prime`.
pub(crate) fn is_web_search_server_use(tool_name: &str) -> bool {
    tool_name.contains("web_search")
}

/// Extracts the search action from a web-search `server_tool_use` input:
/// Anthropic sends `query`, GLM sends `search_query`.
pub(crate) fn web_search_action(input: &Value) -> WebSearchAction {
    let query = input
        .get("query")
        .or_else(|| input.get("search_query"))
        .and_then(Value::as_str)
        .map(str::to_string);
    WebSearchAction::Search {
        query,
        queries: None,
    }
}

/// One `server_tool_use` block being assembled from its wire frames.
#[derive(Default)]
struct OpenServerToolUse {
    index: u64,
    id: String,
    name: String,
    /// Input inlined on `content_block_start` (GLM's shape).
    initial_input: Value,
    /// Input accumulated from `input_json_delta` frames (Anthropic's shape).
    input_json: String,
}

impl OpenServerToolUse {
    fn finish(self) -> Value {
        let input = serde_json::from_str::<Value>(&self.input_json)
            .ok()
            .filter(|input| !input.is_null())
            .unwrap_or(self.initial_input);
        json!({"id": self.id, "name": self.name, "input": input})
    }
}

/// Assembled web-search `server_tool_use` blocks from one Anthropic SSE body.
///
/// rig 0.42 models `server_tool_use` internally but never exposes it on its
/// public streaming surface (frame data is deliberately internal), so the
/// bridge tees the raw wire bytes at the transport and re-reads the blocks
/// from them. SSE framing goes through `eventsource-stream`, the same decoder
/// codex-api uses, so CRLF/multi-line data handling matches the spec instead
/// of hand-rolled splitting. Unmodeled vendor result blocks (Anthropic's
/// `web_search_tool_result`, GLM's assistant-side `tool_result`) carry no
/// Codex item and are ignored. Malformed frames are skipped: the semantic
/// stream has already validated the response by the time this runs.
pub(crate) async fn web_search_blocks_from_anthropic_sse(bytes: &[u8]) -> Vec<Value> {
    let mut blocks = Vec::new();
    let mut open: Option<OpenServerToolUse> = None;
    let frames = futures::stream::iter(vec![Ok::<_, std::convert::Infallible>(
        bytes::Bytes::copy_from_slice(bytes),
    )])
    .eventsource();
    futures::pin_mut!(frames);
    while let Some(Ok(frame)) = frames.next().await {
        let Ok(event) = serde_json::from_str::<Value>(&frame.data) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("content_block_start") => {
                let block = &event["content_block"];
                if block.get("type").and_then(Value::as_str) == Some("server_tool_use") {
                    open = Some(OpenServerToolUse {
                        index: event.get("index").and_then(Value::as_u64).unwrap_or(0),
                        id: string_field(block, "id"),
                        name: string_field(block, "name"),
                        initial_input: block.get("input").cloned().unwrap_or(json!({})),
                        input_json: String::new(),
                    });
                }
            }
            Some("content_block_delta")
                if event["delta"].get("type").and_then(Value::as_str)
                    == Some("input_json_delta")
                    && open.as_ref().is_some_and(|open| {
                        event.get("index").and_then(Value::as_u64) == Some(open.index)
                    }) =>
            {
                if let Some(fragment) = event["delta"].get("partial_json").and_then(Value::as_str)
                    && let Some(open) = open.as_mut()
                {
                    open.input_json.push_str(fragment);
                }
            }
            Some("content_block_stop") => {
                if let Some(open) = open.take()
                    && is_web_search_server_use(&open.name)
                {
                    blocks.push(open.finish());
                }
            }
            _ => {}
        }
    }
    blocks
}

fn string_field(block: &Value, key: &str) -> String {
    block
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Emits a completed web-search call item pair for one assembled
/// `server_tool_use` block. Server-side results have no Codex item, matching
/// the Responses wire where the call item also closes without results.
pub(crate) fn web_search_call_events(block: &Value) -> Vec<ResponseEvent> {
    let id = block
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("ws_{}", crate::convert_response::unique_suffix()));
    let action = block.get("input").map(web_search_action);
    let make = |status: Option<String>| ResponseItem::WebSearchCall {
        id: Some(ResponseItemId::from_server(id.clone())),
        status,
        action: action.clone(),
        internal_chat_message_metadata_passthrough: None,
    };
    vec![
        ResponseEvent::OutputItemAdded(make(None)),
        ResponseEvent::OutputItemDone(make(Some("completed".into()))),
    ]
}

#[cfg(test)]
#[path = "hosted_tools_tests.rs"]
mod tests;
