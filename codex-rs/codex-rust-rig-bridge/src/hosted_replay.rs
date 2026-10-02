//! Versioned replay carrier for hosted (server-side) search results, plus
//! the request-side hard caps and source gate every path shares (fresh
//! capture, resume, fork, import, provider switch).
//!
//! The phase-D format was a bare block array with no origin. That cannot be
//! replayed safely across endpoints — the payloads are vendor ciphertext —
//! so capture now writes a versioned envelope carrying the same
//! credential-free source identity `client::reasoning_source` computes.
//! Replay requires an exact source match; legacy arrays and mismatched or
//! unknown sources are conservatively downgraded (pair not replayed, call
//! item kept), never guessed at and never truncated.

use serde_json::Value;
use serde_json::json;

pub(crate) const ENVELOPE_VERSION: u64 = 1;
/// Hard cap per request: total replayed search pairs (one pair = one
/// `server_tool_use` plus its matching result) across every assistant group.
pub(crate) const MAX_REPLAY_PAIRS_PER_REQUEST: usize = 64;
/// Serialized byte cap for one captured or loaded envelope, including its
/// citations. This is a byte budget, not an exact token limit. Payloads drop
/// whole, never truncated mid-ciphertext.
pub(crate) const MAX_PAIR_BYTES: usize = 40_960;

/// The parsed replay payload persisted on `WebSearchCall.wire_blocks`.
pub(crate) struct Envelope {
    /// Credential-free source identity at capture time. `None` for legacy
    /// bare arrays — those never replay.
    pub(crate) source: Option<String>,
    pub(crate) blocks: Vec<Value>,
    /// Finished text blocks that carried search citations, in stream order;
    /// replayed after the pair blocks inside the same assistant group.
    pub(crate) cited_text: Vec<Value>,
}

/// Builds the envelope written at capture time.
pub(crate) fn envelope(source: &str, blocks: Vec<Value>, cited_text: Vec<Value>) -> Value {
    // `cited_text` rides the envelope only when present; empty stays absent
    // so payloads without citations keep their minimal shape.
    let mut value = json!({"version": ENVELOPE_VERSION, "source": source, "blocks": blocks});
    if !cited_text.is_empty()
        && let Some(object) = value.as_object_mut()
    {
        object.insert("cited_text".into(), Value::Array(cited_text));
    }
    value
}

/// Parses a persisted payload. `None` for shapes this bridge cannot
/// interpret (nulls, unknown versions, missing blocks).
pub(crate) fn parse_envelope(value: &Value) -> Option<Envelope> {
    if let Some(blocks) = value.as_array() {
        // Legacy phase-D payload: a bare block array with no origin.
        return Some(Envelope {
            source: None,
            blocks: blocks.clone(),
            cited_text: Vec::new(),
        });
    }
    let object = value.as_object()?;
    if object.get("version").and_then(Value::as_u64) != Some(ENVELOPE_VERSION) {
        return None;
    }
    Some(Envelope {
        source: object
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string),
        blocks: object.get("blocks")?.as_array()?.clone(),
        cited_text: match object.get("cited_text") {
            Some(blocks) => blocks.as_array()?.clone(),
            None => Vec::new(),
        },
    })
}

/// True when the payload was captured by the same source identity that is
/// about to send this request.
pub(crate) fn replayable(envelope: &Envelope, current_source: &str) -> bool {
    envelope.source.as_deref() == Some(current_source)
}

/// Gates imported history before deduplication or construction of Rig's raw
/// content anchor. Malformed or foreign payloads cannot suppress valid items
/// or make the SDK reject an otherwise usable conversation.
pub(crate) fn validated_envelope(value: &Value, current_source: &str) -> Option<Envelope> {
    let object = value.as_object()?;
    if object.get("version").and_then(Value::as_u64) != Some(ENVELOPE_VERSION)
        || serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > MAX_PAIR_BYTES)
    {
        return None;
    }
    let envelope = parse_envelope(value)?;
    if !replayable(&envelope, current_source) || !valid_cited_text(&envelope.cited_text) {
        return None;
    }
    let blocks: Vec<Value> = pair_group_blocks(&envelope.blocks, /*position*/ 0)
        .into_iter()
        .flat_map(|(_, blocks)| blocks)
        .collect();
    if blocks.is_empty() || blocks.len() != envelope.blocks.len() {
        return None;
    }
    Some(envelope)
}

fn valid_cited_text(blocks: &[Value]) -> bool {
    blocks.iter().all(|block| {
        block.get("type").and_then(Value::as_str) == Some("text")
            && block.get("text").and_then(Value::as_str).is_some()
            && block.get("citations").is_some_and(Value::is_array)
    })
}

/// One assistant group scheduled for wire replay.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReplayGroup {
    /// Index of the assistant message the pairs followed in history.
    pub(crate) index: usize,
    pub(crate) blocks: Vec<Value>,
    /// Cited text blocks replayed after the pair blocks in this group.
    pub(crate) cited_text: Vec<Value>,
}

/// Applies the request-side hard caps and shape checks to the collected
/// replay groups — the authoritative pass, covering payloads this process
/// never captured (old rollouts, imports). See the module docs for the
/// downgrade rules.
pub(crate) fn sanitize_for_request(groups: Vec<ReplayGroup>) -> Vec<ReplayGroup> {
    // A completed envelope can project its call in an earlier assistant and
    // its result in a later one. Budget the whole pair, then retain each block
    // at its original group/block position. Imported result-only envelopes
    // never reach this pass: validated_envelope requires their original call.
    let mut pairs: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut calls = std::collections::HashMap::<String, usize>::new();
    for (position, group) in groups.iter().enumerate() {
        if !valid_cited_text(&group.cited_text) {
            tracing::warn!("web-search replay carries malformed cited text; dropping its payload");
            continue;
        }
        for (block_index, block) in group.blocks.iter().enumerate() {
            match block.get("type").and_then(Value::as_str) {
                Some("server_tool_use") => {
                    if let Some(id) = valid_call_id(block)
                        && let std::collections::hash_map::Entry::Vacant(entry) =
                            calls.entry(id.to_string())
                    {
                        entry.insert(pairs.len());
                        pairs.push(vec![(position, block_index)]);
                    }
                }
                Some("web_search_tool_result" | "tool_result") => {
                    if let Some(pair) = block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .and_then(|id| calls.get(id))
                        .map(|index| &mut pairs[*index])
                        && pair.len() == 1
                    {
                        if valid_result(block) {
                            pair.push((position, block_index));
                        } else {
                            pair.clear();
                        }
                    }
                }
                _ => {}
            }
        }
    }
    pairs.retain(|pair| {
        if pair.is_empty() {
            return false;
        }
        let blocks: Vec<&Value> = pair
            .iter()
            .map(|(position, index)| &groups[*position].blocks[*index])
            .collect();
        let mut positions = std::collections::BTreeSet::new();
        let cited_text: Vec<&Value> = pair
            .iter()
            .filter(|(position, _)| positions.insert(*position))
            .flat_map(|(position, _)| &groups[*position].cited_text)
            .collect();
        serde_json::to_vec(&json!({"blocks": blocks, "cited_text": cited_text}))
            .is_ok_and(|bytes| bytes.len() <= MAX_PAIR_BYTES)
    });
    if pairs.len() > MAX_REPLAY_PAIRS_PER_REQUEST {
        let dropped = pairs.len() - MAX_REPLAY_PAIRS_PER_REQUEST;
        tracing::warn!(
            dropped,
            MAX_REPLAY_PAIRS_PER_REQUEST,
            "web-search replay pairs exceed the per-request cap; oldest dropped"
        );
        pairs.drain(..dropped);
    }
    let kept: std::collections::HashSet<(usize, usize)> = pairs.into_iter().flatten().collect();
    let mut result: Vec<ReplayGroup> = Vec::new();
    for (position, group) in groups.into_iter().enumerate() {
        let blocks: Vec<Value> = group
            .blocks
            .into_iter()
            .enumerate()
            .filter_map(|(index, block)| kept.contains(&(position, index)).then_some(block))
            .collect();
        if blocks.is_empty() {
            continue;
        }
        match result.last_mut().filter(|last| last.index == group.index) {
            Some(last) => {
                last.blocks.extend(blocks);
                last.cited_text.extend(group.cited_text);
            }
            None => result.push(ReplayGroup {
                index: group.index,
                blocks,
                cited_text: group.cited_text,
            }),
        }
    }
    result
}

fn valid_call_id(block: &Value) -> Option<&str> {
    let id = block.get("id")?.as_str()?;
    let name = block.get("name")?.as_str()?;
    (!id.is_empty() && !name.is_empty() && block.get("input").is_some_and(Value::is_object))
        .then_some(id)
}

fn valid_result(block: &Value) -> bool {
    let Some(content) = block.get("content") else {
        return false;
    };
    match block.get("type").and_then(Value::as_str) {
        Some("web_search_tool_result") => content.is_array() || content.is_object(),
        Some("tool_result") => content.is_array() || content.is_string(),
        _ => false,
    }
}

/// Splits one group's blocks into per-pair block lists with the shape and
/// size checks: a pair is a `server_tool_use` plus its matching result
/// block; calls without an id or name drop their payload; results whose
/// `tool_use_id` matches no call in the group drop; oversized pairs drop
/// whole. Returns `(source-group position, pair blocks)` in order.
fn pair_group_blocks(blocks: &[Value], position: usize) -> Vec<(usize, Vec<Value>)> {
    let mut pairs: Vec<(String, Vec<Value>)> = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("server_tool_use") => {
                let Some(id) = valid_call_id(block) else {
                    tracing::warn!(
                        "web-search replay call without id, name, or object input; dropping its payload"
                    );
                    continue;
                };
                if pairs.iter().any(|(existing_id, _)| existing_id == id) {
                    tracing::warn!(
                        id,
                        "web-search replay repeats a call id inside one payload; dropping it"
                    );
                    continue;
                }
                pairs.push((id.to_string(), vec![block.clone()]));
            }
            Some("web_search_tool_result") | Some("tool_result") => {
                let tool_use_id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match pairs
                    .iter_mut()
                    .rev()
                    .find(|(id, _)| id == tool_use_id && !tool_use_id.is_empty())
                {
                    Some((_, blocks)) => blocks.push(block.clone()),
                    None => tracing::warn!(
                        "web-search replay result without a matching call; dropping it"
                    ),
                }
            }
            other => {
                tracing::warn!(
                    block_type = ?other,
                    "web-search replay group carries an unrecognized block; dropping it"
                );
            }
        }
    }
    let mut result = Vec::new();
    for (id, blocks) in pairs {
        if blocks.len() > 2 || blocks.iter().skip(1).any(|block| !valid_result(block)) {
            tracing::warn!(
                id,
                "web-search replay carries a malformed result; dropping its payload"
            );
            continue;
        }
        match serde_json::to_string(&blocks) {
            Ok(serialized) if serialized.len() > MAX_PAIR_BYTES => tracing::warn!(
                id,
                MAX_PAIR_BYTES,
                "web-search replay pair exceeds the size cap; dropping its payload"
            ),
            Ok(_) => result.push((position, blocks)),
            Err(error) => tracing::warn!(
                error = %error,
                "web-search replay pair is not serializable; dropping its payload"
            ),
        }
    }
    result
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

/// Reassembles the assistant content blocks of one Anthropic SSE body in
/// original index order. A block starts from its `content_block_start` JSON
/// (preserving every field, known or not) and accumulates the four streamed
/// delta kinds to their terminal state: `text_delta` → text,
/// `input_json_delta` → input (parsed), `thinking_delta` → thinking,
/// `signature_delta` → signature. Blocks complete on their start frame
/// (`redacted_thinking`, results) pass through untouched. Unknown delta
/// kinds, malformed input and incomplete captures fail before continuation;
/// they cannot be replayed faithfully. Citation deltas accumulate by index.
pub(crate) async fn raw_assistant_content(bytes: &[u8]) -> Result<Vec<Value>, codex_api::ApiError> {
    use eventsource_stream::Eventsource;
    use futures::StreamExt;
    let frames = futures::stream::iter(vec![Ok::<_, std::convert::Infallible>(
        bytes::Bytes::copy_from_slice(bytes),
    )])
    .eventsource();
    futures::pin_mut!(frames);
    #[derive(Default)]
    struct Open {
        base: Value,
        input_json: String,
    }
    let mut open = std::collections::BTreeMap::<u64, Open>::new();
    let mut complete = std::collections::BTreeMap::<u64, Value>::new();
    let invalid = |reason: &str| {
        codex_api::ApiError::Stream(format!(
            "Cannot faithfully reconstruct paused Anthropic content: {reason}"
        ))
    };
    let mut stopped = false;
    while let Some(frame) = frames.next().await {
        let frame = frame.map_err(|_| invalid("invalid SSE frame"))?;
        let event: Value =
            serde_json::from_str(&frame.data).map_err(|_| invalid("invalid frame JSON"))?;
        let event_type = event.get("type").and_then(Value::as_str);
        let index = if matches!(
            event_type,
            Some("content_block_start" | "content_block_delta" | "content_block_stop")
        ) {
            event
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| invalid("content block has no index"))?
        } else {
            0
        };
        match event_type {
            Some("content_block_start") => {
                let block = event
                    .get("content_block")
                    .filter(|block| block.is_object())
                    .ok_or_else(|| invalid("missing content block object"))?;
                if open.contains_key(&index) || complete.contains_key(&index) {
                    return Err(invalid("duplicate content block index"));
                }
                open.insert(
                    index,
                    Open {
                        base: block.clone(),
                        input_json: String::new(),
                    },
                );
            }
            Some("content_block_delta") => {
                let delta = &event["delta"];
                let entry = open
                    .get_mut(&index)
                    .ok_or_else(|| invalid("delta without an open block"))?;
                match delta.get("type").and_then(Value::as_str) {
                    Some("input_json_delta") => {
                        let fragment = delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .ok_or_else(|| invalid("input delta is not a string"))?;
                        entry.input_json.push_str(fragment);
                    }
                    Some("citations_delta") => {
                        let citation = delta
                            .get("citation")
                            .filter(|citation| citation.is_object())
                            .ok_or_else(|| invalid("citation delta has no citation object"))?;
                        let object = entry
                            .base
                            .as_object_mut()
                            .ok_or_else(|| invalid("content block is not an object"))?;
                        object
                            .entry("citations")
                            .or_insert_with(|| Value::Array(Vec::new()))
                            .as_array_mut()
                            .ok_or_else(|| invalid("citations is not an array"))?
                            .push(citation.clone());
                    }
                    Some(kind @ ("text_delta" | "thinking_delta" | "signature_delta")) => {
                        let field = match kind {
                            "text_delta" => "text",
                            "thinking_delta" => "thinking",
                            "signature_delta" => "signature",
                            _ => return Err(invalid("unknown text delta kind")),
                        };
                        let fragment = delta
                            .get(field)
                            .and_then(Value::as_str)
                            .ok_or_else(|| invalid("text delta is not a string"))?;
                        let object = entry
                            .base
                            .as_object_mut()
                            .ok_or_else(|| invalid("content block is not an object"))?;
                        let value = object
                            .entry(field)
                            .or_insert_with(|| Value::String(String::new()));
                        let Value::String(text) = value else {
                            return Err(invalid("streamed field is not a string"));
                        };
                        text.push_str(fragment);
                    }
                    _ => return Err(invalid("unsupported content delta kind")),
                }
            }
            Some("content_block_stop") => {
                let mut entry = open
                    .remove(&index)
                    .ok_or_else(|| invalid("stop without an open block"))?;
                if !entry.input_json.is_empty() {
                    let input: Value = serde_json::from_str(&entry.input_json)
                        .map_err(|_| invalid("invalid accumulated input JSON"))?;
                    if !input.is_object() {
                        return Err(invalid("tool input is not an object"));
                    }
                    let object = entry
                        .base
                        .as_object_mut()
                        .ok_or_else(|| invalid("content block is not an object"))?;
                    object.insert("input".into(), input);
                }
                complete.insert(index, entry.base);
            }
            Some("message_stop") => {
                stopped = true;
                break;
            }
            _ => {}
        }
    }
    if !stopped || !open.is_empty() || complete.is_empty() {
        return Err(invalid("incomplete or empty paused content capture"));
    }
    Ok(complete.into_values().collect())
}

#[cfg(test)]
#[path = "hosted_replay_tests.rs"]
mod tests;
