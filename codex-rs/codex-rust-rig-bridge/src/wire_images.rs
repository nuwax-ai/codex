//! Final-wire image accounting for the context budget.
//!
//! The wire guard must not charge inline image payloads as text: a prepared
//! screenshot's Base64 is transport encoding, not model-visible prose, and
//! charging it at the shared 4-bytes-per-token text heuristic falsely rejects
//! legal requests on modest context windows. This module walks each
//! protocol's typed content paths on the serialized request and re-costs the
//! images it finds with the same coarse per-image estimate core's history
//! accounting uses. Anything not on a typed image path — text, tool schemas,
//! tool arguments, or a data URL forged inside a text block — keeps counting
//! as plain text bytes.

use crate::RigProtocol;
use serde_json::Value;

/// Model-visible byte estimate for one prepared image, mirroring core's
/// history estimator (`core/src/context_manager/history.rs`,
/// `RESIZED_IMAGE_BYTES_ESTIMATE`): central image preparation bounds the
/// longest edge before an image reaches the wire, so the encoded payload size
/// does not change the vision cost. 7,373 bytes ≈ 1,844 tokens at the shared
/// 4-bytes-per-token heuristic. Keep in sync with core.
const PREPARED_IMAGE_BYTES_ESTIMATE: usize = 7373;

/// Patch ceiling charged for `detail: "original"` images, whose cost depends
/// on pixel dimensions this wire layer does not decode. Core charges the same
/// ceiling when it cannot resolve an Original image's dimensions; this is a
/// named conservative upper bound, not a provider tokenizer claim.
/// See <https://platform.openai.com/docs/guides/images-vision#calculating-costs>.
const ORIGINAL_DETAIL_PATCH_CEILING: u64 = 10_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WireImageCosts {
    /// Serialized inline image payload bytes (data URLs, base64 sources),
    /// excluded from the text byte estimate. Remote URL and file-ID
    /// references contribute no payload bytes; their short reference strings
    /// stay in the text estimate.
    pub(crate) inline_payload_bytes: usize,
    /// Coarse vision token estimate across all identified images.
    pub(crate) vision_tokens: u64,
}

pub(crate) fn measure(protocol: RigProtocol, body: &Value) -> WireImageCosts {
    let mut costs = WireImageCosts::default();
    match protocol {
        RigProtocol::Chat => {
            for block in message_content_blocks(body) {
                chat_image_block(block, &mut costs);
            }
        }
        RigProtocol::Anthropic => {
            for block in message_content_blocks(body) {
                anthropic_image_block(block, &mut costs);
            }
        }
        RigProtocol::Responses => measure_responses_items(body, &mut costs),
    }
    costs
}

/// Content blocks of every `messages[*].content[]` array (Chat and Anthropic).
fn message_content_blocks(body: &Value) -> impl Iterator<Item = &Value> {
    body.get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|message| message.get("content"))
        .filter_map(Value::as_array)
        .flatten()
}

/// Chat: `{"type":"image_url","image_url":{"url":…,"detail":…}}`. User images
/// and supplemental tool-result images (the bridge hoists those into a
/// follow-up user message) share this shape.
fn chat_image_block(block: &Value, costs: &mut WireImageCosts) {
    if block.get("type").and_then(Value::as_str) == Some("image_url")
        && let Some(image) = block.get("image_url")
    {
        record_image_reference(
            image.get("url").and_then(Value::as_str),
            image.get("detail").and_then(Value::as_str),
            costs,
        );
    }
}

/// Anthropic: `{"type":"image","source":…}` at message level, plus the same
/// blocks nested inside `tool_result.content[]` where the bridge keeps
/// base64 tool images. Anthropic wire carries no detail flag, so every image
/// gets the prepared-image estimate.
fn anthropic_image_block(block: &Value, costs: &mut WireImageCosts) {
    match block.get("type").and_then(Value::as_str) {
        Some("image") => {
            let source = block.get("source");
            match source
                .and_then(|source| source.get("type"))
                .and_then(Value::as_str)
            {
                Some("base64") => {
                    if let Some(data) = source
                        .and_then(|source| source.get("data"))
                        .and_then(Value::as_str)
                    {
                        costs.inline_payload_bytes =
                            costs.inline_payload_bytes.saturating_add(data.len());
                        costs.vision_tokens = costs
                            .vision_tokens
                            .saturating_add(prepared_image_vision_tokens());
                    }
                }
                Some("url") => {
                    // Remote reference: charged, never free.
                    costs.vision_tokens = costs
                        .vision_tokens
                        .saturating_add(prepared_image_vision_tokens());
                }
                _ => {}
            }
        }
        Some("tool_result") => {
            if let Some(nested) = block.get("content").and_then(Value::as_array) {
                for block in nested {
                    anthropic_image_block(block, costs);
                }
            }
        }
        _ => {}
    }
}

/// Responses passthrough: `input_image` blocks in `message.content[]` and in
/// `function_call_output.output[]` (MCP outputs are normalized to the latter
/// before serialization).
fn measure_responses_items(body: &Value, costs: &mut WireImageCosts) {
    for item in body
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                for block in item
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    responses_image_block(block, costs);
                }
            }
            Some("function_call_output") => {
                for block in item
                    .get("output")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    responses_image_block(block, costs);
                }
            }
            _ => {}
        }
    }
}

/// Responses: `{"type":"input_image","image_url":…,"detail":…}`; a file
/// reference serializes as `file_id` instead of `image_url`.
fn responses_image_block(block: &Value, costs: &mut WireImageCosts) {
    if block.get("type").and_then(Value::as_str) != Some("input_image") {
        return;
    }
    record_image_reference(
        block.get("image_url").and_then(Value::as_str),
        block.get("detail").and_then(Value::as_str),
        costs,
    );
}

/// Costs one image reference. `url` is `Some` for inline data URLs and remote
/// URLs; `None` leaves the file-reference path, which is charged like a
/// remote image (a short provider-scoped ID cannot be free).
fn record_image_reference(url: Option<&str>, detail: Option<&str>, costs: &mut WireImageCosts) {
    let inline = url.is_some_and(|reference| reference.starts_with("data:"));
    if inline {
        // Base64 and data-URL characters need no JSON escaping, so the string
        // length is exactly its serialized footprint. The data-URL prefix is
        // a handful of excluded bytes; not worth splitting off.
        costs.inline_payload_bytes = costs
            .inline_payload_bytes
            .saturating_add(url.unwrap_or_default().len());
    }
    let vision = match detail {
        // Reuse the entrance's bounded header estimate. Unknown/file references
        // still pay the conservative ceiling; the wire must not make them free.
        Some("original") => url
            .and_then(codex_utils_image::estimate_original_image_tokens)
            .and_then(|tokens| u64::try_from(tokens).ok())
            .unwrap_or(ORIGINAL_DETAIL_PATCH_CEILING),
        _ => prepared_image_vision_tokens(),
    };
    costs.vision_tokens = costs.vision_tokens.saturating_add(vision);
}

fn prepared_image_vision_tokens() -> u64 {
    codex_utils_string::approx_tokens_from_byte_count(PREPARED_IMAGE_BYTES_ESTIMATE)
}

#[cfg(test)]
#[path = "wire_images_tests.rs"]
mod tests;
