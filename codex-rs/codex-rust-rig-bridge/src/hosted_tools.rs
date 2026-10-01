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

use codex_api::ApiError;
use codex_api::ResponseEvent;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::WebSearchAction;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::Value;
use serde_json::json;

/// Translates one hosted Responses tool declaration into its Anthropic
/// server-tool entry. `Ok(None)` means the tool has no Anthropic equivalent
/// and is dropped (with a warning) rather than guessed at. `Err` means the
/// declaration selects a mode the Messages wire cannot express — failing the
/// request keeps the user's choice instead of silently widening it.
pub(crate) fn anthropic_server_tool(hosted: &Value) -> Result<Option<Value>, String> {
    match hosted.get("type").and_then(Value::as_str) {
        // Anthropic's server-side web search. The dated suffix is the
        // official tool version; verified live against GLM's
        // Anthropic-compatible gateway.
        Some("web_search") => Ok(Some(web_search_server_tool(hosted)?)),
        other => {
            tracing::warn!(
                tool_type = ?other,
                "Hosted tool has no Anthropic server-tool equivalent; dropping it"
            );
            Ok(None)
        }
    }
}

/// Translates the request's hosted tool declarations for the Anthropic wire.
/// Non-representable search modes abort the request before anything is sent.
pub(crate) fn translate_anthropic_server_tools(hosted: &[Value]) -> Result<Vec<Value>, ApiError> {
    let mut entries = Vec::new();
    for hosted in hosted {
        match anthropic_server_tool(hosted) {
            Ok(Some(entry)) => entries.push(entry),
            Ok(None) => {}
            Err(message) => return Err(ApiError::InvalidRequest { message }),
        }
    }
    Ok(entries)
}

/// The `user_location` fields the Messages tool accepts (an approximate
/// city/region/country/timezone); at least one must be set.
const ANTHROPIC_LOCATION_FIELDS: [&str; 4] = ["city", "region", "country", "timezone"];

fn web_search_server_tool(hosted: &Value) -> Result<Value, String> {
    // Codex distinguishes cached (`external_web_access: false`), indexed
    // (`indexed_web_access: true`) and live search. The Messages tool is live
    // web search only: the other modes cannot be expressed without silently
    // widening what the user selected.
    if hosted.get("indexed_web_access").and_then(Value::as_bool) == Some(true) {
        return Err(
            "web_search indexed mode (indexed_web_access) has no Anthropic Messages \
             equivalent; configure the live search mode instead"
                .into(),
        );
    }
    if hosted.get("external_web_access").and_then(Value::as_bool) == Some(false) {
        return Err(
            "web_search cached mode (external_web_access=false) has no Anthropic \
             Messages equivalent; configure the live search mode instead"
                .into(),
        );
    }
    let mut tool = serde_json::Map::new();
    tool.insert("type".into(), json!("web_search_20250305"));
    tool.insert("name".into(), json!("web_search"));
    if let Some(domains) = hosted
        .pointer("/filters/allowed_domains")
        .and_then(Value::as_array)
    {
        let domains = validate_allowed_domains(domains)?;
        if domains.is_empty() {
            return Err(
                "web_search allowed_domains must not be empty; it would match no results".into(),
            );
        }
        tool.insert("allowed_domains".into(), json!(domains));
    }
    if let Some(location) = hosted
        .get("user_location")
        .filter(|location| !location.is_null())
    {
        validate_user_location(location)?;
        tool.insert("user_location".into(), location.clone());
    }
    // Tuning knobs with no Messages equivalent: dropped loudly rather than
    // erroring, so a config that also sets them keeps a working live search.
    for knob in ["search_context_size", "search_content_types"] {
        if hosted.get(knob).is_some_and(|value| !value.is_null()) {
            tracing::warn!(
                knob,
                "web_search field has no Anthropic Messages equivalent; dropping it"
            );
        }
    }
    Ok(Value::Object(tool))
}

/// Bare domains, optionally with a path; wildcards only in the path. The
/// Messages API rejects other shapes with a 400 at request time — catching
/// them here names the offending entry.
fn validate_allowed_domains(domains: &[Value]) -> Result<Vec<String>, String> {
    let mut parsed = Vec::with_capacity(domains.len());
    for domain in domains {
        let Some(domain) = domain.as_str().map(str::trim) else {
            return Err(format!(
                "web_search allowed_domains entries must be strings; found {domain}"
            ));
        };
        if domain.is_empty() {
            return Err(
                "web_search allowed_domains must not be empty; it would match no results".into(),
            );
        }
        if domain.contains("://") {
            return Err(format!(
                "web_search allowed_domains entries must be bare domains without a scheme; found {domain:?}"
            ));
        }
        let (host, path) = match domain.split_once('/') {
            Some((host, _)) => (host, true),
            None => (domain, false),
        };
        if host.contains('*') {
            return Err(format!(
                "web_search allowed_domains does not allow wildcards in the domain; found {domain:?}"
            ));
        }
        if !path && domain.contains('*') {
            return Err(format!(
                "web_search allowed_domains wildcards are only valid in a path; found {domain:?}"
            ));
        }
        parsed.push(domain.to_string());
    }
    Ok(parsed)
}

fn validate_user_location(location: &Value) -> Result<(), String> {
    if let Some(kind) = location.get("type").and_then(Value::as_str)
        && kind != "approximate"
    {
        return Err(format!(
            "web_search user_location.type must be \"approximate\"; found {kind:?}"
        ));
    }
    let has_field = ANTHROPIC_LOCATION_FIELDS.iter().any(|field| {
        location
            .get(*field)
            .is_some_and(|value| value.as_str().is_some_and(|value| !value.is_empty()))
    });
    if !has_field {
        return Err(
            "web_search user_location needs at least one of city, region, country or timezone"
                .into(),
        );
    }
    Ok(())
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
        // `type` is required for replay: the wire block must parse as a
        // server_tool_use union member on follow-up requests.
        json!({"type": "server_tool_use", "id": self.id, "name": self.name, "input": input})
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
                            id: string_field(block, "id"),
                            name: string_field(block, "name"),
                            initial_input: block.get("input").cloned().unwrap_or(json!({})),
                            input_json: String::new(),
                        });
                    }
                    // Both result shapes arrive complete on the start frame;
                    // a stray stop/delta for them needs no assembly.
                    Some("web_search_tool_result") => capture.results.push(block.clone()),
                    Some("tool_result") if block.get("tool_use_id").is_some() => {
                        // GLM's gateway reports search results as an
                        // assistant-side tool_result block; only blocks that
                        // reference a server call id are search results.
                        capture.results.push(block.clone());
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
            Some("content_block_stop") => {
                if let Some(open) = open.take()
                    && is_web_search_server_use(&open.name)
                {
                    capture.uses.push(open.finish());
                }
            }
            _ => {}
        }
    }
    capture
}

/// Everything the tee recovered for one turn's web-search activity.
#[derive(Default)]
pub(crate) struct WebSearchWireCapture {
    /// `server_tool_use` blocks (web-search family only).
    pub(crate) uses: Vec<Value>,
    /// Result blocks: official `web_search_tool_result` or GLM's
    /// assistant-side `tool_result`, matched by id below.
    pub(crate) results: Vec<Value>,
}

/// One use/result pair in original call order. A missing result means the
/// call is still pending (mixed server/client turn: the server tool runs
/// after the client tool results return).
pub(crate) struct PairedWebSearchBlocks {
    pub(crate) call: Value,
    pub(crate) result: Option<Value>,
}

/// Pairs uses with their results by id. Returns the pairs plus every result
/// that matched no call in THIS response — a mixed server/client turn
/// delivers those in a follow-up response, where the pump re-associates them
/// with the pending call persisted in the request history.
pub(crate) struct PairedWebSearch {
    pub(crate) pairs: Vec<PairedWebSearchBlocks>,
    pub(crate) unmatched_results: Vec<Value>,
}

pub(crate) fn pair_web_search_blocks(capture: WebSearchWireCapture) -> PairedWebSearch {
    let mut remaining_results = capture.results;
    let pairs = capture
        .uses
        .into_iter()
        .map(|call| {
            let id = call.get("id").and_then(Value::as_str).map(str::to_string);
            let position = id.as_deref().and_then(|id| {
                remaining_results.iter().position(|result| {
                    result.get("tool_use_id").and_then(Value::as_str) == Some(id)
                })
            });
            let result = position.map(|position| remaining_results.remove(position));
            PairedWebSearchBlocks { call, result }
        })
        .collect();
    PairedWebSearch {
        pairs,
        unmatched_results: remaining_results,
    }
}

/// Emits a completed web-search call item for one recovered pair, carrying
/// the raw blocks in the versioned replay envelope for faithful same-source
/// replay. `in_progress` marks a pending call whose result has not arrived
/// (mixed server/client turn); its result block arrives in a follow-up
/// response.
pub(crate) fn web_search_call_events(
    pair: PairedWebSearchBlocks,
    source: &str,
) -> Vec<ResponseEvent> {
    let PairedWebSearchBlocks { call, result } = pair;
    let id = call
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("ws_{}", crate::convert_response::unique_suffix()));
    let action = call.get("input").map(web_search_action);
    let status = if result.is_some() {
        "completed"
    } else {
        "in_progress"
    };
    // Event-side first defense: a pair larger than ~10K tokens (bytes/4)
    // loses its replay payload (the call itself still completes) — never
    // truncated mid-ciphertext. The request-side sanitize pass re-checks
    // every payload, whatever produced it.
    let blocks = match &result {
        Some(result) => vec![call, result.clone()],
        None => vec![call],
    };
    let wire_blocks = if serde_json::to_string(&blocks).map_or(true, |serialized| {
        serialized.len() > crate::hosted_replay::MAX_PAIR_BYTES
    }) {
        tracing::warn!(
            max_pair_bytes = crate::hosted_replay::MAX_PAIR_BYTES,
            "web-search result pair exceeds the replay size cap; dropping its replay payload"
        );
        None
    } else {
        Some(crate::hosted_replay::envelope(source, blocks))
    };
    let make = |status: Option<String>, wire_blocks: Option<Value>| ResponseItem::WebSearchCall {
        id: Some(ResponseItemId::from_server(id.clone())),
        status,
        action: action.clone(),
        wire_blocks,
        internal_chat_message_metadata_passthrough: None,
    };
    vec![
        ResponseEvent::OutputItemAdded(make(None, wire_blocks.clone())),
        ResponseEvent::OutputItemDone(make(Some(status.to_string()), wire_blocks)),
    ]
}

fn string_field(block: &Value, key: &str) -> String {
    block
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Builds the state that continues a PAUSED Anthropic turn. The
/// user-visible items keep today's shape (one text message plus search
/// calls reusing the D2 replay channel); the continuation REQUEST instead
/// re-sends the paused assistant message's raw content blocks verbatim
/// (thinking/signatures, citations, interleaving included) per the
/// official recipe.
pub(crate) async fn assistant_continuation_items(
    bytes: &[u8],
    source: &str,
) -> crate::hosted_replay::PauseCapture {
    let capture = web_search_blocks_from_anthropic_sse(bytes).await;
    let pairs = pair_web_search_blocks(capture).pairs;
    let mut items = Vec::new();
    let text = assembled_text(bytes).await;
    if !text.is_empty() {
        items.push(ResponseItem::Message {
            id: None,
            role: "assistant".into(),
            content: vec![codex_protocol::models::ContentItem::OutputText { text }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        });
    }
    for pair in pairs {
        // Reuse the event builder (envelope + size cap included), then keep
        // the item.
        for event in web_search_call_events(pair, source) {
            if let ResponseEvent::OutputItemDone(item) = event {
                items.push(item);
            }
        }
    }
    crate::hosted_replay::PauseCapture {
        items,
        raw_content: crate::hosted_replay::raw_assistant_content(bytes).await,
    }
}

/// Concatenates the text deltas of plain text blocks (in stream order) into
/// one string. The continuation history approximates the original
/// text/pair interleaving (documented limitation: text first, pairs after).
async fn assembled_text(bytes: &[u8]) -> String {
    let frames = futures::stream::iter(vec![Ok::<_, std::convert::Infallible>(
        bytes::Bytes::copy_from_slice(bytes),
    )])
    .eventsource();
    (async {
        let mut open_text = false;
        let mut text = String::new();
        let mut frames = frames;
        while let Some(Ok(frame)) = frames.next().await {
            let Ok(event) = serde_json::from_str::<Value>(&frame.data) else {
                continue;
            };
            match event.get("type").and_then(Value::as_str) {
                Some("content_block_start")
                    if event["content_block"]["type"].as_str() == Some("text") =>
                {
                    open_text = true;
                }
                Some("content_block_delta")
                    if open_text && event["delta"]["type"].as_str() == Some("text_delta") =>
                {
                    if let Some(fragment) = event["delta"].get("text").and_then(Value::as_str) {
                        text.push_str(fragment);
                    }
                }
                Some("content_block_stop") => open_text = false,
                _ => {}
            }
        }
        text
    })
    .await
}

#[cfg(test)]
#[path = "hosted_tools_tests.rs"]
mod tests;
