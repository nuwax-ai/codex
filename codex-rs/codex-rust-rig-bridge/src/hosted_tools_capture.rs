//! Indexed raw server-search and text-block recovery from Anthropic SSE.

use crate::hosted_tools::is_web_search_server_use;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::Value;
use serde_json::json;

/// One `server_tool_use` block being assembled from its wire frames.
#[derive(Default)]
struct OpenServerToolUse {
    index: u64,
    name: String,
    /// Verbatim start block, retaining vendor fields outside typed Rig data.
    base: Value,
    /// Input inlined on `content_block_start` (GLM's shape).
    initial_input: Value,
    /// Input accumulated from `input_json_delta` frames (Anthropic's shape).
    input_json: String,
}

impl OpenServerToolUse {
    fn finish(mut self) -> Value {
        let input = serde_json::from_str::<Value>(&self.input_json)
            .ok()
            .filter(|input| !input.is_null())
            .unwrap_or(self.initial_input);
        if let Some(object) = self.base.as_object_mut() {
            object.insert("input".into(), input);
        }
        self.base
    }
}

/// Raw web-search blocks recovered from one Anthropic SSE body: the
/// `server_tool_use` calls (streamed) and their result blocks — the official
/// `web_search_tool_result` (arrives complete in one frame) and GLM's
/// non-standard assistant-side `tool_result`.
///
/// rig 0.42 models `server_tool_use` internally but never exposes it on its
/// public streaming surface (frame data is deliberately internal), so the
/// bridge tees the raw wire bytes at the transport and re-reads the blocks
/// from them. SSE framing goes through `eventsource-stream`, the same decoder
/// codex-api uses, so CRLF/multi-line data handling matches the spec instead
/// of hand-rolled splitting. Malformed frames are skipped: the semantic
/// stream has already validated the response by the time this runs.
pub(crate) async fn web_search_blocks_from_anthropic_sse(bytes: &[u8]) -> WebSearchWireCapture {
    let mut capture = WebSearchWireCapture::default();
    let mut open: Option<OpenServerToolUse> = None;
    // Citations can arrive on start or in citations_delta frames. Keep every
    // text block by index until its own stop, including inline initial text.
    let mut open_cited = std::collections::BTreeMap::<u64, Value>::new();
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
                match block.get("type").and_then(Value::as_str) {
                    Some("server_tool_use") => {
                        open = Some(OpenServerToolUse {
                            index: event.get("index").and_then(Value::as_u64).unwrap_or(0),
                            name: string_field(block, "name"),
                            base: block.clone(),
                            initial_input: block.get("input").cloned().unwrap_or(json!({})),
                            input_json: String::new(),
                        });
                    }
                    // Both result shapes arrive complete on the start frame;
                    // a stray stop/delta for them needs no assembly.
                    Some("web_search_tool_result") => {
                        if let Some(index) = event.get("index").and_then(Value::as_u64) {
                            capture.results.push(WireIndexed {
                                index,
                                block: block.clone(),
                            });
                        }
                    }
                    Some("tool_result") if block.get("tool_use_id").is_some() => {
                        // GLM's gateway reports search results as an
                        // assistant-side tool_result block; only blocks that
                        // reference a server call id are search results.
                        if let Some(index) = event.get("index").and_then(Value::as_u64) {
                            capture.results.push(WireIndexed {
                                index,
                                block: block.clone(),
                            });
                        }
                    }
                    Some("text") => {
                        if let Some(index) = event.get("index").and_then(Value::as_u64) {
                            open_cited.insert(index, block.clone());
                        }
                    }
                    _ => {}
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
            Some("content_block_delta") => {
                if let Some(index) = event.get("index").and_then(Value::as_u64)
                    && let Some(block) = open_cited.get_mut(&index)
                    && let Some(object) = block.as_object_mut()
                {
                    match event["delta"].get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            if let Some(fragment) =
                                event["delta"].get("text").and_then(Value::as_str)
                            {
                                let value = object
                                    .entry("text")
                                    .or_insert_with(|| Value::String(String::new()));
                                if let Value::String(text) = value {
                                    text.push_str(fragment);
                                }
                            }
                        }
                        Some("citations_delta") => {
                            if let Some(citation) = event["delta"].get("citation") {
                                let value = object
                                    .entry("citations")
                                    .or_insert_with(|| Value::Array(Vec::new()));
                                if let Value::Array(citations) = value {
                                    citations.push(citation.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("content_block_stop") => {
                let index = event.get("index").and_then(Value::as_u64);
                if open.as_ref().is_some_and(|open| Some(open.index) == index)
                    && let Some(open) = open.take()
                    && is_web_search_server_use(&open.name)
                {
                    capture.uses.push(WireIndexed {
                        index: open.index,
                        block: open.finish(),
                    });
                }
                if let Some(index) = index
                    && let Some(block) = open_cited.remove(&index)
                {
                    // Every finished text block is recorded — cited or not.
                    // The layout needs the uncited ones too: they are the
                    // identity that keeps citations with the block that
                    // actually carried them.
                    capture.text_blocks.push(WireIndexed { index, block });
                }
            }
            _ => {}
        }
    }
    capture
}

/// A recovered block paired with the wire content-block index it streamed
/// under. The index is the position identity replay relies on.
#[derive(Debug, PartialEq)]
pub(crate) struct WireIndexed {
    pub(crate) index: u64,
    pub(crate) block: Value,
}

/// Everything the tee recovered for one turn's web-search activity.
#[derive(Default)]
pub(crate) struct WebSearchWireCapture {
    /// `server_tool_use` blocks (web-search family only), with wire index.
    pub(crate) uses: Vec<WireIndexed>,
    /// Result blocks: official `web_search_tool_result` or GLM's
    /// assistant-side `tool_result`, matched by id below, with wire index.
    pub(crate) results: Vec<WireIndexed>,
    /// Every finished text block of the response, cited or not, with wire
    /// index, in stop order.
    pub(crate) text_blocks: Vec<WireIndexed>,
}

fn string_field(block: &Value, key: &str) -> String {
    block
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
