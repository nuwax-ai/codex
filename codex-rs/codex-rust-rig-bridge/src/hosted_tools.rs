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

pub(crate) use crate::hosted_tools_capture::WebSearchWireCapture;
pub(crate) use crate::hosted_tools_capture::WireIndexed;
pub(crate) use crate::hosted_tools_capture::web_search_blocks_from_anthropic_sse;

/// One use/result pair in original call order. A missing result means the
/// call is still pending (mixed server/client turn: the server tool runs
/// after the client tool results return).
pub(crate) struct PairedWebSearchBlocks {
    pub(crate) call: Value,
    /// Wire index of the call block; [`UNKNOWN_WIRE_INDEX`] for a foreign
    /// call clone (late result closing a call captured by an earlier
    /// response).
    pub(crate) call_index: u64,
    pub(crate) result: Option<Value>,
    /// Wire index of the result block in its own response.
    pub(crate) result_index: Option<u64>,
}

/// Pairs uses with their results by id. Returns the pairs plus every result
/// that matched no call in THIS response — a mixed server/client turn
/// delivers those in a follow-up response, where the pump re-associates them
/// with the pending call persisted in the request history — and every
/// finished text block for the identity layout.
pub(crate) struct PairedWebSearch {
    pub(crate) pairs: Vec<PairedWebSearchBlocks>,
    pub(crate) unmatched_results: Vec<WireIndexed>,
}

pub(crate) fn pair_web_search_blocks(capture: WebSearchWireCapture) -> PairedWebSearch {
    let WebSearchWireCapture {
        uses,
        results,
        text_blocks: _,
    } = capture;
    let mut remaining_results = results;
    let pairs = uses
        .into_iter()
        .map(|WireIndexed { index, block: call }| {
            let id = call.get("id").and_then(Value::as_str).map(str::to_string);
            let position = id.as_deref().and_then(|id| {
                remaining_results
                    .iter()
                    .position(|WireIndexed { block, .. }| {
                        block.get("tool_use_id").and_then(Value::as_str) == Some(id)
                    })
            });
            let (result, result_index) =
                match position.map(|position| remaining_results.swap_remove(position)) {
                    Some(WireIndexed {
                        index: result_index,
                        block,
                    }) => (Some(block), Some(result_index)),
                    None => (None, None),
                };
            PairedWebSearchBlocks {
                call,
                call_index: index,
                result,
                result_index,
            }
        })
        .collect();
    PairedWebSearch {
        pairs,
        unmatched_results: remaining_results,
    }
}

/// Maps complete wire blocks to the stable segments the stream actually emitted.
/// Category/position selects a named owner only at capture; replay never searches
/// for text. A mismatch drops this layout while retaining response-scoped pairs.
pub(crate) fn response_layout(
    blocks: &[(u64, Value)],
    segments: &[crate::hosted_replay::CapturedSegment],
) -> Option<Vec<Value>> {
    use crate::hosted_replay::SegmentKind;
    let mut entries = Vec::new();
    let mut cursor = 0usize;
    let mut active: Option<(SegmentKind, usize, u64)> = None;
    for (index, block) in blocks {
        let kind = match block.get("type").and_then(Value::as_str) {
            Some("server_tool_use" | "web_search_tool_result" | "tool_result") => {
                entries.push(json!({"kind":"pair","index":index}));
                continue;
            }
            Some("text") if block["text"].as_str() == Some("") => continue,
            Some("text") => SegmentKind::Text,
            Some("thinking" | "redacted_thinking") => SegmentKind::Reasoning,
            Some("tool_use") => SegmentKind::Tool,
            _ => return None,
        };
        if kind == SegmentKind::Tool || active.as_ref().is_none_or(|(last, _, _)| *last != kind) {
            let segment = segments.get(cursor)?;
            if segment.kind != kind {
                return None;
            }
            active = Some((kind, cursor, 0));
            cursor += 1;
        }
        let (_, position, part) = active.as_mut()?;
        let owner = &segments[*position].id;
        if kind == SegmentKind::Text {
            let cited = block
                .get("citations")
                .and_then(Value::as_array)
                .is_some_and(|citations| !citations.is_empty());
            entries.push(json!({"kind":if cited {"cited"} else {"text"},
                "index":index,"owner":owner,"block":block}));
        } else {
            entries.push(json!({"kind":"segment","index":index,"owner":owner,"part":part}));
            *part += 1;
        }
    }
    (cursor == segments.len()).then_some(entries)
}

/// Emits a completed web-search call item for one recovered pair, carrying
/// the raw blocks in the versioned replay envelope for faithful same-source
/// replay. `in_progress` marks a pending call whose result has not arrived
/// (mixed server/client turn); its result block arrives in a follow-up
/// response. `layout` is the response's identity layout and rides the FIRST
/// pair envelope of the response; siblings resolve against it.
pub(crate) fn web_search_call_events(
    pair: PairedWebSearchBlocks,
    source: &str,
    response_id: &str,
    layout: Option<&[Value]>,
) -> Vec<ResponseEvent> {
    let PairedWebSearchBlocks {
        call,
        call_index,
        result,
        result_index,
    } = pair;
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
    // Bound the full envelope, including the identity layout, before
    // persisting it. This byte cap is not an exact token bound. The call
    // still completes when its replay payload is dropped whole.
    let mut block_indices = vec![call_index];
    let mut blocks = vec![call];
    if let Some(result) = result {
        block_indices.push(result_index.unwrap_or(crate::hosted_replay::UNKNOWN_WIRE_INDEX));
        blocks.push(result);
    }
    let mut payload =
        crate::hosted_replay::envelope(source, blocks, block_indices, response_id, layout);
    if layout.is_some()
        && serde_json::to_vec(&payload).map_or(true, |bytes| {
            bytes.len() > crate::hosted_replay::MAX_PAIR_BYTES
        })
        && let Some(object) = payload.as_object_mut()
    {
        // A large carrier cannot erase a valid sibling or change its response.
        object.remove("layout");
        tracing::warn!(
            "hosted response layout exceeds its cap; retaining response-scoped pair only"
        );
    }
    let wire_blocks = if serde_json::to_string(&payload).map_or(true, |serialized| {
        serialized.len() > crate::hosted_replay::MAX_PAIR_BYTES
    }) {
        tracing::warn!(
            max_pair_bytes = crate::hosted_replay::MAX_PAIR_BYTES,
            "web-search replay envelope exceeds the byte cap; dropping its payload"
        );
        None
    } else {
        Some(payload)
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

/// Recovers pairs and the response layout once for both paused and final attempts.
/// Late results carry only a foreign call clone; request dedupe keeps that call
/// at its original response and replays the new result at this response's boundary.
pub(crate) async fn captured_search_events(
    bytes: &[u8],
    source: &str,
    response_id: &str,
    pending_calls: &std::collections::HashMap<String, Value>,
    layout: Option<&[Value]>,
) -> Vec<ResponseEvent> {
    let paired = pair_web_search_blocks(web_search_blocks_from_anthropic_sse(bytes).await);
    let mut first = true;
    let mut events = Vec::new();
    for pair in paired.pairs {
        let carrier = first.then_some(layout).flatten();
        first = false;
        events.extend(web_search_call_events(pair, source, response_id, carrier));
    }
    for result in paired.unmatched_results {
        let Some(id) = result.block.get("tool_use_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(call) = pending_calls.get(id).cloned() else {
            tracing::warn!(
                id,
                "web-search result matches no pending projected call; dropping it"
            );
            continue;
        };
        let carrier = first.then_some(layout).flatten();
        first = false;
        events.extend(web_search_call_events(
            PairedWebSearchBlocks {
                call,
                call_index: crate::hosted_replay::UNKNOWN_WIRE_INDEX,
                result: Some(result.block),
                result_index: Some(result.index),
            },
            source,
            response_id,
            carrier,
        ));
    }
    events
}

#[cfg(test)]
#[path = "hosted_tools_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "hosted_capture_tests.rs"]
mod capture_tests;
