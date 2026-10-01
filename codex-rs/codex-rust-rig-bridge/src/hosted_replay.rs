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
/// Hard cap per pair: serialized bytes of the pair's blocks. Bytes/4 is an
/// approximation of tokens, not a precise token limit — the payload drops
/// whole, never truncated mid-ciphertext.
pub(crate) const MAX_PAIR_BYTES: usize = 40_960;

/// The parsed replay payload persisted on `WebSearchCall.wire_blocks`.
pub(crate) struct Envelope {
    /// Credential-free source identity at capture time. `None` for legacy
    /// bare arrays — those never replay.
    pub(crate) source: Option<String>,
    pub(crate) blocks: Vec<Value>,
}

/// Builds the envelope written at capture time.
pub(crate) fn envelope(source: &str, blocks: Vec<Value>) -> Value {
    json!({"version": ENVELOPE_VERSION, "source": source, "blocks": blocks})
}

/// Parses a persisted payload. `None` for shapes this bridge cannot
/// interpret (nulls, unknown versions, missing blocks).
pub(crate) fn parse_envelope(value: &Value) -> Option<Envelope> {
    if let Some(blocks) = value.as_array() {
        // Legacy phase-D payload: a bare block array with no origin.
        return Some(Envelope {
            source: None,
            blocks: blocks.clone(),
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
    })
}

/// True when the payload was captured by the same source identity that is
/// about to send this request.
pub(crate) fn replayable(envelope: &Envelope, current_source: &str) -> bool {
    envelope.source.as_deref() == Some(current_source)
}

/// One assistant group scheduled for wire replay.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReplayGroup {
    /// Index of the assistant message the pairs followed in history.
    pub(crate) index: usize,
    pub(crate) blocks: Vec<Value>,
}

/// Applies the request-side hard caps and shape checks to the collected
/// replay groups — the authoritative pass, covering payloads this process
/// never captured (old rollouts, imports). See the module docs for the
/// downgrade rules.
pub(crate) fn sanitize_for_request(groups: Vec<ReplayGroup>) -> Vec<ReplayGroup> {
    let indices: Vec<usize> = groups.iter().map(|group| group.index).collect();
    let mut flat: Vec<(usize, Vec<Value>)> = Vec::new();
    for (position, group) in groups.iter().enumerate() {
        flat.extend(pair_group_blocks(&group.blocks, position));
    }
    if flat.len() > MAX_REPLAY_PAIRS_PER_REQUEST {
        let dropped = flat.len() - MAX_REPLAY_PAIRS_PER_REQUEST;
        tracing::warn!(
            dropped,
            MAX_REPLAY_PAIRS_PER_REQUEST,
            "web-search replay pairs exceed the per-request cap; oldest dropped"
        );
        flat.drain(..dropped);
    }
    let mut result: Vec<ReplayGroup> = Vec::new();
    for (position, blocks) in flat {
        let index = indices[position];
        match result.last_mut().filter(|last| last.index == index) {
            Some(last) => last.blocks.extend(blocks),
            None => result.push(ReplayGroup { index, blocks }),
        }
    }
    result
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
                let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if id.is_empty() || name.is_empty() {
                    tracing::warn!(
                        "web-search replay call without id or name; dropping its payload"
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

#[cfg(test)]
#[path = "hosted_replay_tests.rs"]
mod tests;
