//! Complete indexed Anthropic SSE reconstruction for pause and final capture.

use serde_json::Value;

/// Reassembles the assistant content blocks of one Anthropic SSE body in
/// original index order. A block starts from its `content_block_start` JSON
/// (preserving every field, known or not) and accumulates the four streamed
/// delta kinds to their terminal state: `text_delta` → text,
/// `input_json_delta` → input (parsed), `thinking_delta` → thinking,
/// `signature_delta` → signature. Blocks complete on their start frame
/// (`redacted_thinking`, results) pass through untouched. Unknown delta
/// kinds, malformed input and incomplete captures fail before continuation;
/// they cannot be replayed faithfully. Citation deltas accumulate by index.
#[cfg(test)]
pub(crate) async fn raw_assistant_content(bytes: &[u8]) -> Result<Vec<Value>, codex_api::ApiError> {
    Ok(raw_indexed_assistant_content(bytes)
        .await?
        .into_iter()
        .map(|(_, block)| block)
        .collect())
}

pub(crate) async fn raw_indexed_assistant_content(
    bytes: &[u8],
) -> Result<Vec<(u64, Value)>, codex_api::ApiError> {
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
    if complete
        .keys()
        .enumerate()
        .any(|(position, index)| u64::try_from(position).ok() != Some(*index))
    {
        return Err(invalid("missing content block index"));
    }
    Ok(complete.into_iter().collect())
}
