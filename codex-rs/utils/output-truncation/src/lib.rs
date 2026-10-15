//! Shared byte/token truncation for tool and exec output.

mod image_limits;

pub use image_limits::ImageAdmission;
pub use image_limits::MAX_IMAGE_REFERENCE_BYTES;
pub use image_limits::MAX_TOOL_OUTPUT_IMAGE_BYTES;
pub use image_limits::MAX_TOOL_OUTPUT_IMAGE_TOTAL_BYTES;
pub use image_limits::MAX_TOOL_OUTPUT_IMAGES;
pub use image_limits::MAX_TOOL_OUTPUT_VISIBLE_TOKENS;
use image_limits::OmissionKind;
pub use image_limits::TOOL_OUTPUT_IMAGE_ORIGINAL_VISUAL_TOKENS;
pub use image_limits::TOOL_OUTPUT_IMAGE_VISUAL_TOKENS;
pub use image_limits::admit_tool_output_images;
use image_limits::bounded_tool_output_policy;
use image_limits::image_budget_charge;
use image_limits::is_tool_output_header;
use image_limits::omission_marker;
pub use image_limits::omitted_images_marker;

use std::borrow::Cow;

use codex_protocol::mcp::CallToolResult;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
pub use codex_utils_string::approx_bytes_for_tokens;
pub use codex_utils_string::approx_token_count;
pub use codex_utils_string::approx_tokens_from_byte_count;
use codex_utils_string::truncate_middle_chars;
use codex_utils_string::truncate_middle_with_token_budget;

pub use codex_protocol::protocol::TruncationPolicy;

/// Adds the existing 20% allowance for serialization and headers.
/// Saved history budgets already include this allowance.
pub fn with_serialization_allowance(policy: TruncationPolicy) -> TruncationPolicy {
    policy * 1.2
}

pub fn formatted_truncate_text(content: &str, policy: TruncationPolicy) -> String {
    if content.len() <= policy.byte_budget() {
        return content.to_string();
    }

    let original_token_count = approx_token_count(content);
    let total_lines = content.lines().count();
    let result = truncate_text(content, policy);
    format!(
        "Warning: truncated output (original token count: {original_token_count})\nTotal output lines: {total_lines}\n\n{result}"
    )
}

pub fn truncate_text(content: &str, policy: TruncationPolicy) -> String {
    match policy {
        TruncationPolicy::Bytes(bytes) => truncate_middle_chars(content, bytes),
        TruncationPolicy::Tokens(tokens) => truncate_middle_with_token_budget(content, tokens).0,
    }
}

/// Replaces an oversized serialized MCP result with a bounded text preview while preserving its
/// error status and supported bounded metadata.
pub fn truncate_mcp_tool_result(
    result: &CallToolResult,
    max_bytes: usize,
) -> Cow<'_, CallToolResult> {
    let Ok(serialized) = serde_json::to_string(result) else {
        return Cow::Borrowed(result);
    };
    if serialized.len() <= max_bytes {
        return Cow::Borrowed(result);
    }

    let meta = None;

    let mut preview_budget = max_bytes;
    loop {
        let preview = if preview_budget == 0 {
            String::new()
        } else {
            truncate_text(&serialized, TruncationPolicy::Bytes(preview_budget))
        };
        let truncated = CallToolResult {
            content: vec![serde_json::json!({
                "type": "text",
                "text": preview,
            })],
            structured_content: None,
            is_error: result.is_error,
            meta: meta.clone(),
        };
        let Ok(truncated_serialized) = serde_json::to_string(&truncated) else {
            return Cow::Borrowed(result);
        };
        let truncated_len = truncated_serialized.len();
        if truncated_len <= max_bytes || preview_budget == 0 {
            return Cow::Owned(truncated);
        }

        // The preview is serialized as a JSON string, so escaping can make it exceed its raw byte
        // budget. Scale the next budget by the observed size of the complete replacement.
        preview_budget = (preview_budget.saturating_mul(max_bytes) / truncated_len)
            .min(preview_budget.saturating_sub(1));
    }
}

/// Applies the existing byte/token budget without changing success metadata or media ordering.
pub fn truncate_function_output_payload(
    output: &mut FunctionCallOutputPayload,
    policy: TruncationPolicy,
    estimate_audio_token_count: impl Fn(&str) -> usize,
) {
    let policy = bounded_tool_output_policy(policy);
    match &mut output.body {
        FunctionCallOutputBody::Text(text) => {
            let header_len = text.find("\nOutput:").and_then(|index| {
                [index + "\nOutput:".len(), index + "\nOutput:\n".len()]
                    .into_iter()
                    .find(|&end| text.get(..end).is_some_and(is_tool_output_header))
            });
            *text = if let Some(header_len) = header_len {
                let (header, body) = text.split_at(header_len);
                format!("{header}{}", truncate_text(body, policy))
            } else {
                truncate_text(text, policy)
            };
        }
        FunctionCallOutputBody::ContentItems(items) => {
            *items = truncate_function_output_items_with_policy(
                items,
                policy,
                estimate_audio_token_count,
            );
        }
    }
}

pub fn formatted_truncate_text_content_items_with_policy(
    items: &[FunctionCallOutputContentItem],
    policy: TruncationPolicy,
) -> (Vec<FunctionCallOutputContentItem>, Option<usize>) {
    let policy = bounded_tool_output_policy(policy);
    if items.iter().any(|item| {
        matches!(item, FunctionCallOutputContentItem::InputText { text }
        if is_tool_output_header(text) || omission_marker(text).is_some())
    }) {
        return (
            truncate_function_output_items_with_policy(items, policy, |_| 0),
            None,
        );
    }
    // The image entrance limits apply before any text collapsing so the
    // same per-result bounds hold on every producer (Code Mode, MCP,
    // function tools).
    let admission = admit_tool_output_images(items, policy);
    let items: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(index, item)| {
            !matches!(item, FunctionCallOutputContentItem::InputImage { .. })
                || admission.keep[*index]
        })
        .map(|(_, item)| item.clone())
        .collect();
    let text_segments = items
        .iter()
        .filter_map(|item| match item {
            FunctionCallOutputContentItem::InputText { text } => Some(text.as_str()),
            FunctionCallOutputContentItem::InputImage { .. }
            | FunctionCallOutputContentItem::InputAudio { .. }
            | FunctionCallOutputContentItem::EncryptedContent { .. } => None,
        })
        .collect::<Vec<_>>();

    if text_segments.is_empty() {
        let mut out = items;
        if admission.dropped > 0 {
            out.push(omitted_images_marker(admission.dropped));
        }
        return (out, None);
    }

    let mut combined = String::new();
    for text in &text_segments {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(text);
    }

    // Kept images reserve their visual cost from the same budget the text
    // collapses against, mirroring the item-wise truncator.
    let image_allowance = image_budget_charge(admission.visual_tokens, policy);
    let text_budget = policy_saturating_sub(policy, image_allowance);
    if combined.len() <= text_budget.byte_budget() {
        let mut out = items;
        if admission.dropped > 0 {
            out.push(omitted_images_marker(admission.dropped));
        }
        return (out, None);
    }

    let original_token_count = approx_token_count(&combined);
    let mut out = vec![FunctionCallOutputContentItem::InputText {
        text: formatted_truncate_text(&combined, text_budget),
    }];
    out.extend(items.iter().filter_map(|item| match item {
        FunctionCallOutputContentItem::InputImage { image, detail } => {
            Some(FunctionCallOutputContentItem::InputImage {
                image: image.clone(),
                detail: *detail,
            })
        }
        FunctionCallOutputContentItem::InputAudio { audio_url } => {
            Some(FunctionCallOutputContentItem::InputAudio {
                audio_url: audio_url.clone(),
            })
        }
        FunctionCallOutputContentItem::EncryptedContent { encrypted_content } => {
            Some(FunctionCallOutputContentItem::EncryptedContent {
                encrypted_content: encrypted_content.clone(),
            })
        }
        FunctionCallOutputContentItem::InputText { .. } => None,
    }));
    if admission.dropped > 0 {
        out.push(omitted_images_marker(admission.dropped));
    }

    (out, Some(original_token_count))
}

fn policy_saturating_sub(policy: TruncationPolicy, charge: usize) -> TruncationPolicy {
    match policy {
        TruncationPolicy::Bytes(bytes) => TruncationPolicy::Bytes(bytes.saturating_sub(charge)),
        TruncationPolicy::Tokens(tokens) => TruncationPolicy::Tokens(tokens.saturating_sub(charge)),
    }
}

pub fn truncate_function_output_items_with_policy(
    items: &[FunctionCallOutputContentItem],
    policy: TruncationPolicy,
    estimate_audio_token_count: impl Fn(&str) -> usize,
) -> Vec<FunctionCallOutputContentItem> {
    let policy = bounded_tool_output_policy(policy);
    let admission = admit_tool_output_images(items, policy);
    let mut omitted_image_items = admission.dropped;
    let mut out: Vec<FunctionCallOutputContentItem> = Vec::with_capacity(items.len());
    let mut remaining_budget = match policy {
        TruncationPolicy::Bytes(_) => policy.byte_budget(),
        TruncationPolicy::Tokens(_) => policy.token_budget(),
    };
    remaining_budget =
        remaining_budget.saturating_sub(image_budget_charge(admission.visual_tokens, policy));
    let mut omitted_text_items = 0usize;
    let mut omitted_audio_items = 0usize;

    for (index, item) in items.iter().enumerate() {
        match item {
            FunctionCallOutputContentItem::InputText { text } => {
                // Only one bounded leading header is control data. Similar text
                // elsewhere, or an oversized forged header, spends the ordinary budget.
                if index == 0 && is_tool_output_header(text) {
                    out.push(item.clone());
                    continue;
                }
                if let Some((kind, count)) = omission_marker(text) {
                    match kind {
                        OmissionKind::Image => {
                            omitted_image_items = omitted_image_items.saturating_add(count)
                        }
                        OmissionKind::Text => {
                            omitted_text_items = omitted_text_items.saturating_add(count)
                        }
                        OmissionKind::Audio => {
                            omitted_audio_items = omitted_audio_items.saturating_add(count)
                        }
                    }
                    continue;
                }
                // Empty text contributes no model content but still consumes an API array slot.
                if text.is_empty() {
                    continue;
                }
                if remaining_budget == 0 {
                    omitted_text_items += 1;
                    continue;
                }

                let cost = match policy {
                    TruncationPolicy::Bytes(_) => text.len(),
                    TruncationPolicy::Tokens(_) => approx_token_count(text),
                };

                if cost <= remaining_budget {
                    out.push(FunctionCallOutputContentItem::InputText { text: text.clone() });
                    remaining_budget = remaining_budget.saturating_sub(cost);
                } else {
                    let snippet_policy = match policy {
                        TruncationPolicy::Bytes(_) => TruncationPolicy::Bytes(remaining_budget),
                        TruncationPolicy::Tokens(_) => TruncationPolicy::Tokens(remaining_budget),
                    };
                    let snippet = truncate_text(text, snippet_policy);
                    if snippet.is_empty() {
                        omitted_text_items += 1;
                    } else {
                        out.push(FunctionCallOutputContentItem::InputText { text: snippet });
                    }
                    remaining_budget = 0;
                }
            }
            FunctionCallOutputContentItem::InputImage { image, detail } => {
                // Admission already decided membership: images that survive
                // are charged their visual cost up front (the budget was
                // reduced before this loop), so text and media share one
                // bounded per-output budget.
                if !admission.keep[index] {
                    continue;
                }
                out.push(FunctionCallOutputContentItem::InputImage {
                    image: image.clone(),
                    detail: *detail,
                });
            }
            FunctionCallOutputContentItem::InputAudio { audio_url } => {
                let token_cost = estimate_audio_token_count(audio_url);
                let cost = match policy {
                    TruncationPolicy::Bytes(_) => approx_bytes_for_tokens(token_cost),
                    TruncationPolicy::Tokens(_) => token_cost,
                };
                if cost <= remaining_budget {
                    out.push(FunctionCallOutputContentItem::InputAudio {
                        audio_url: audio_url.clone(),
                    });
                    remaining_budget = remaining_budget.saturating_sub(cost);
                } else {
                    omitted_audio_items += 1;
                }
            }
            FunctionCallOutputContentItem::EncryptedContent { encrypted_content } => {
                out.push(FunctionCallOutputContentItem::EncryptedContent {
                    encrypted_content: encrypted_content.clone(),
                });
            }
        }
    }

    if omitted_image_items > 0 {
        out.push(omitted_images_marker(omitted_image_items));
    }
    if omitted_text_items > 0 {
        out.push(FunctionCallOutputContentItem::InputText {
            text: format!("[omitted {omitted_text_items} text items ...]"),
        });
    }
    if omitted_audio_items > 0 {
        out.push(FunctionCallOutputContentItem::InputText {
            text: format!("[omitted {omitted_audio_items} audio items ...]"),
        });
    }

    out
}

pub fn approx_tokens_from_byte_count_i64(bytes: i64) -> i64 {
    if bytes <= 0 {
        return 0;
    }

    let bytes = usize::try_from(bytes).unwrap_or(usize::MAX);
    i64::try_from(approx_tokens_from_byte_count(bytes)).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod truncate_tests;
