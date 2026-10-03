//! Source-gated, bounded hosted-search replay. v3 records response ownership and
//! stable segment IDs; v1/v2 replay pairs only and never attribute citations.

use serde_json::Value;
use serde_json::json;
use std::collections::HashSet;

pub(crate) const ENVELOPE_VERSION: u64 = 3;
pub(crate) const UNKNOWN_WIRE_INDEX: u64 = u64::MAX;
pub(crate) const MAX_REPLAY_PAIRS_PER_REQUEST: usize = 64;
/// Conservative serialized-byte fallback with framing reserve. This is not a
/// tokenizer-exact guarantee for an unknown provider; ciphertext is never cut.
pub(crate) const MAX_PAIR_BYTES: usize = 9_800;
pub(crate) const MAX_LAYOUTS_PER_REQUEST: usize = 64;
pub(crate) const MAX_LAYOUT_BYTES_PER_REQUEST: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SegmentKind {
    Text,
    Reasoning,
    Tool,
}

#[derive(Clone, Debug)]
pub(crate) struct CapturedSegment {
    pub(crate) id: String,
    pub(crate) kind: SegmentKind,
}

pub(crate) fn segment_id(response: &str, ordinal: usize) -> String {
    format!("rigseg_{response}_{ordinal}")
}

/// The ID itself persists ownership through rollout serialization, resume and
/// fork without extending the shared Responses passthrough metadata contract.
pub(crate) fn segment_response(id: &str) -> Option<&str> {
    let rest = id.strip_prefix("rigseg_")?;
    let (response, ordinal) = rest.rsplit_once('_')?;
    (valid_response_id(response) && ordinal.parse::<usize>().is_ok()).then_some(response)
}

fn valid_response_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

pub(crate) struct Envelope {
    pub(crate) source: Option<String>,
    pub(crate) blocks: Vec<Value>,
    pub(crate) response_id: Option<String>,
    pub(crate) block_indices: Option<Vec<u64>>,
    pub(crate) layout: Option<Vec<Value>>,
    pub(crate) cited_text: Vec<Value>,
}

/// Every sibling carries the response ID even when the full layout was dropped.
pub(crate) fn envelope(
    source: &str,
    blocks: Vec<Value>,
    block_indices: Vec<u64>,
    response_id: &str,
    layout: Option<&[Value]>,
) -> Value {
    let mut value = json!({"version":ENVELOPE_VERSION,"source":source,
        "response_id":response_id,"blocks":blocks,"block_indices":block_indices});
    if let Some(layout) = layout
        && !layout.is_empty()
        && let Some(object) = value.as_object_mut()
    {
        object.insert("layout".into(), Value::Array(layout.to_vec()));
    }
    value
}

pub(crate) fn parse_envelope(value: &Value) -> Option<Envelope> {
    if let Some(blocks) = value.as_array() {
        return Some(Envelope {
            source: None,
            blocks: blocks.clone(),
            response_id: None,
            block_indices: None,
            layout: None,
            cited_text: Vec::new(),
        });
    }
    let object = value.as_object()?;
    let version = object.get("version")?.as_u64()?;
    if !matches!(version, 1 | 2 | ENVELOPE_VERSION) {
        return None;
    }
    let blocks = object.get("blocks")?.as_array()?.clone();
    // Old layouts have no response/segment ownership. Their text is never
    // promoted to v3, even if an imported envelope happens to add an ID.
    let (response_id, block_indices, layout) = if version == ENVELOPE_VERSION {
        let response = object.get("response_id")?.as_str()?;
        if !valid_response_id(response) {
            return None;
        }
        let indices: Option<Vec<u64>> = object
            .get("block_indices")?
            .as_array()?
            .iter()
            .map(Value::as_u64)
            .collect();
        let indices = indices?;
        if indices.len() != blocks.len() {
            return None;
        }
        let mut seen = HashSet::new();
        let mut previous = None;
        for index in indices
            .iter()
            .copied()
            .filter(|index| *index != UNKNOWN_WIRE_INDEX)
        {
            if !seen.insert(index) || previous.is_some_and(|previous| previous >= index) {
                return None;
            }
            previous = Some(index);
        }
        let layout = match object.get("layout") {
            Some(layout) => Some(layout.as_array()?.clone()),
            None => None,
        };
        (Some(response.to_string()), Some(indices), layout)
    } else {
        (None, None, None)
    };
    let cited_text = if version < ENVELOPE_VERSION {
        match object.get("cited_text") {
            Some(blocks) => blocks.as_array()?.clone(),
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };
    Some(Envelope {
        source: object
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string),
        blocks,
        response_id,
        block_indices,
        layout,
        cited_text,
    })
}

pub(crate) fn replayable(envelope: &Envelope, current_source: &str) -> bool {
    envelope.source.as_deref() == Some(current_source)
}

pub(crate) fn validated_envelope(value: &Value, current_source: &str) -> Option<Envelope> {
    if !value.is_object()
        || serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > MAX_PAIR_BYTES)
    {
        return None;
    }
    let envelope = parse_envelope(value)?;
    if !replayable(&envelope, current_source)
        || !valid_cited_text(&envelope.cited_text)
        || envelope.layout.as_ref().is_some_and(|layout| {
            !valid_layout(layout, envelope.response_id.as_deref().unwrap_or_default())
        })
        || !valid_pair_blocks(&envelope.blocks)
    {
        return None;
    }
    if let Some(indices) = &envelope.block_indices {
        for (block, index) in envelope.blocks.iter().zip(indices) {
            if *index == UNKNOWN_WIRE_INDEX && block["type"] != "server_tool_use" {
                return None;
            }
        }
    }
    Some(envelope)
}

fn valid_cited_text(blocks: &[Value]) -> bool {
    blocks.iter().all(|block| {
        block["type"] == "text" && block["text"].is_string() && block["citations"].is_array()
    })
}

pub(crate) fn valid_layout(layout: &[Value], response: &str) -> bool {
    let mut previous = None;
    let mut segment_parts = std::collections::HashMap::<&str, u64>::new();
    layout.iter().all(|entry| {
        let Some(index) = entry.get("index").and_then(Value::as_u64) else {
            return false;
        };
        if index == UNKNOWN_WIRE_INDEX || previous.is_some_and(|previous| previous >= index) {
            return false;
        }
        previous = Some(index);
        match entry.get("kind").and_then(Value::as_str) {
            Some("pair") => true,
            Some(kind @ ("text" | "cited" | "segment")) => {
                let Some(owner) = entry.get("owner").and_then(Value::as_str) else {
                    return false;
                };
                if segment_response(owner) != Some(response) {
                    return false;
                }
                if kind == "segment" {
                    let Some(part) = entry.get("part").and_then(Value::as_u64) else {
                        return false;
                    };
                    let expected = segment_parts.entry(owner).or_default();
                    if part != *expected {
                        return false;
                    }
                    *expected += 1;
                    true
                } else {
                    entry["block"]["type"] == "text"
                        && entry["block"]["text"].is_string()
                        && (kind != "cited" || entry["block"]["citations"].is_array())
                }
            }
            _ => false,
        }
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BlockSite {
    pub(crate) layout: u32,
    pub(crate) wire: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SegmentSite {
    pub(crate) id: String,
    /// Offset/length of this named item's actual serialized assistant blocks.
    pub(crate) start: usize,
    pub(crate) len: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResponseSite {
    pub(crate) id: String,
    /// Explicit insertion boundary for a response with no surviving segments.
    pub(crate) anchor: usize,
    pub(crate) segments: Vec<SegmentSite>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReplayGroup {
    pub(crate) index: usize,
    pub(crate) blocks: Vec<Value>,
    pub(crate) block_sites: Option<Vec<BlockSite>>,
    /// Aligned with responses. An empty layout means pair-only at that response.
    pub(crate) layouts: Vec<Vec<Value>>,
    pub(crate) responses: Vec<ResponseSite>,
    pub(crate) cited_text: Vec<Value>,
}

impl ReplayGroup {
    pub(crate) fn has_identity(&self) -> bool {
        self.block_sites.is_some() && !self.responses.is_empty()
    }
}

pub(crate) use crate::hosted_replay_budget::sanitize_for_request;

pub(crate) fn valid_call_id(block: &Value) -> Option<&str> {
    let id = block.get("id")?.as_str()?;
    let name = block.get("name")?.as_str()?;
    (!id.is_empty() && !name.is_empty() && block.get("input").is_some_and(Value::is_object))
        .then_some(id)
}
pub(crate) fn valid_result(block: &Value) -> bool {
    let Some(content) = block.get("content") else {
        return false;
    };
    match block.get("type").and_then(Value::as_str) {
        Some("web_search_tool_result") => content.is_array() || content.is_object(),
        Some("tool_result") => content.is_array() || content.is_string(),
        _ => false,
    }
}
fn valid_pair_blocks(blocks: &[Value]) -> bool {
    let mut calls = std::collections::HashMap::<&str, bool>::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("server_tool_use") => {
                let Some(id) = valid_call_id(block) else {
                    return false;
                };
                if calls.insert(id, false).is_some() {
                    return false;
                }
            }
            Some("web_search_tool_result" | "tool_result") => {
                let Some(result) = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .and_then(|id| calls.get_mut(id))
                else {
                    return false;
                };
                if *result || !valid_result(block) {
                    return false;
                }
                *result = true;
            }
            _ => return false,
        }
    }
    !calls.is_empty()
}

/// The paused attempt's recovered state: history items for events and
/// persistence plus the verbatim raw assistant content blocks for the
/// continuation request.
pub(crate) struct PauseCapture {
    pub(crate) items: Vec<codex_protocol::models::ResponseItem>,
    /// Every assistant content block of the paused attempt, reassembled in
    /// original order with all fields the wire carried (thinking signatures,
    /// citations, interleaving) — the official continuation recipe resends
    /// the paused assistant message unchanged.
    pub(crate) raw_content: Vec<Value>,
}

/// Raw responses generated during this turn. Convert only the original input
/// prefix, then append each paused response without replacing older history.
#[derive(Clone)]
pub(crate) struct PauseReplay {
    pub(crate) original_input_len: usize,
    pub(crate) messages: Vec<Vec<Value>>,
}

#[cfg(test)]
pub(crate) use crate::hosted_capture::raw_assistant_content;
pub(crate) use crate::hosted_capture::raw_indexed_assistant_content;

#[cfg(test)]
#[path = "hosted_replay_tests.rs"]
mod tests;
