use crate::TruncationPolicy;
use crate::approx_bytes_for_tokens;
use codex_protocol::models::FunctionCallOutputContentItem;

/// Hard cap for model-visible text and charged media in one image-bearing tool result.
pub const MAX_TOOL_OUTPUT_VISIBLE_TOKENS: usize = 10_000;
/// Bound reference identifiers and data-URL headers separately from image payloads.
pub const MAX_IMAGE_REFERENCE_BYTES: usize = 8 * 1024;
// Covers one bounded timing/status header, three consolidated omission markers,
// and the existing inline truncation notice. It is inside the hard cap.
const CONTROL_HEADROOM_TOKENS: usize = 256;
const MAX_TOOL_OUTPUT_HEADER_BYTES: usize = 256;

fn bounded_image_output_policy(policy: TruncationPolicy) -> TruncationPolicy {
    let content_tokens = MAX_TOOL_OUTPUT_VISIBLE_TOKENS - CONTROL_HEADROOM_TOKENS;
    match policy {
        TruncationPolicy::Tokens(tokens) => TruncationPolicy::Tokens(tokens.min(content_tokens)),
        TruncationPolicy::Bytes(bytes) => {
            TruncationPolicy::Bytes(bytes.min(approx_bytes_for_tokens(content_tokens)))
        }
    }
}

pub(crate) fn tool_output_items_policy(
    items: &[FunctionCallOutputContentItem],
    policy: TruncationPolicy,
) -> TruncationPolicy {
    // The screenshot backstop must not override explicit exec/MCP text limits.
    // Keep the image cap on later history passes even if all images were omitted.
    if items.iter().any(|item| match item {
        FunctionCallOutputContentItem::InputImage { .. } => true,
        FunctionCallOutputContentItem::InputText { text } => {
            matches!(omission_marker(text), Some((OmissionKind::Image, _)))
        }
        FunctionCallOutputContentItem::InputAudio { .. }
        | FunctionCallOutputContentItem::EncryptedContent { .. } => false,
    }) {
        bounded_image_output_policy(policy)
    } else {
        policy
    }
}

pub(crate) fn is_tool_output_header(text: &str) -> bool {
    if text.len() > MAX_TOOL_OUTPUT_HEADER_BYTES {
        return false;
    }
    if let Some(seconds) = text
        .strip_prefix("Wall time: ")
        .and_then(|text| text.strip_suffix(" seconds\nOutput:"))
    {
        return seconds.parse::<f64>().is_ok();
    }
    let Some((status, timing)) = text.split_once('\n') else {
        return false;
    };
    let status_valid = matches!(
        status,
        "Script completed" | "Script failed" | "Script terminated"
    ) || status
        .strip_prefix("Script running with cell ID ")
        .is_some_and(|id| {
            !id.is_empty()
                && id.len() <= 64
                && id
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        });
    status_valid
        && timing.starts_with("Wall time ")
        && (timing.ends_with(" seconds\nOutput:\n")
            || timing.contains(" seconds (code-mode ") && timing.ends_with(" seconds)\nOutput:\n"))
}

pub(crate) enum OmissionKind {
    Image,
    Text,
    Audio,
}

pub(crate) fn omission_marker(text: &str) -> Option<(OmissionKind, usize)> {
    if text.len() > MAX_TOOL_OUTPUT_HEADER_BYTES {
        return None;
    }
    let (count, suffix) = text.strip_prefix("[omitted ")?.split_once(' ')?;
    let count = count.parse::<usize>().ok()?;
    match suffix {
        "text items ...]" => Some((OmissionKind::Text, count)),
        "audio items ...]" => Some((OmissionKind::Audio, count)),
        _ if omitted_images_marker(count)
            == (FunctionCallOutputContentItem::InputText {
                text: text.to_owned(),
            }) =>
        {
            Some((OmissionKind::Image, count))
        }
        _ => None,
    }
}

/// Bounded omission text shared by the producer and the history-preparation backstop.
pub fn omitted_images_marker(dropped: usize) -> FunctionCallOutputContentItem {
    FunctionCallOutputContentItem::InputText {
        text: format!(
            "[omitted {dropped} image(s): over the per-output image limits (max {} images, {} MiB each, {} MiB total, {} KiB reference/header, {} visual tokens; unknown original costs {} tokens plus control headroom) ...]",
            MAX_TOOL_OUTPUT_IMAGES,
            MAX_TOOL_OUTPUT_IMAGE_BYTES / (1024 * 1024),
            MAX_TOOL_OUTPUT_IMAGE_TOTAL_BYTES / (1024 * 1024),
            MAX_IMAGE_REFERENCE_BYTES / 1024,
            MAX_TOOL_OUTPUT_VISIBLE_TOKENS,
            TOOL_OUTPUT_IMAGE_ORIGINAL_VISUAL_TOKENS,
        ),
    }
}

/// Named per-tool-output media limits. These are entrance bounds for images
/// inside ONE tool result (screenshots, MCP `image` blocks, Code Mode
/// `image()`): they complement — never replace — the wire byte limit, the
/// output reserve, and the decode-safety pixel cap in `codex-utils-image`.
///
/// The values deliberately bound the model-visible cost of a single tool
/// result without silently discarding the newest evidence: when limits bite,
/// the newest images are retained and the dropped ones are reported through
/// one bounded text marker.
pub const MAX_TOOL_OUTPUT_IMAGES: usize = 8;
pub const MAX_TOOL_OUTPUT_IMAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TOOL_OUTPUT_IMAGE_TOTAL_BYTES: usize = 24 * 1024 * 1024;
/// Visual token cost charged for one standard (high/auto) prepared image.
/// Mirrors the fork's history estimator and the bridge's wire-image budget
/// (~7,373-byte prepared PNG ≈ 1,844 tokens); keep the three in sync.
pub const TOOL_OUTPUT_IMAGE_VISUAL_TOKENS: usize = 1_844;
/// Visual token cost for a `detail: original` image: the conservative patch
/// ceiling used when pixel dimensions are unknown at this layer.
pub const TOOL_OUTPUT_IMAGE_ORIGINAL_VISUAL_TOKENS: usize = 10_000;

/// Conservative visual cost, without decoding untrusted image bytes.
fn charged_visual_tokens(
    image: &codex_protocol::models::ImageReference,
    detail: Option<codex_protocol::models::ImageDetail>,
) -> usize {
    match detail {
        Some(codex_protocol::models::ImageDetail::Original) => match image {
            codex_protocol::models::ImageReference::Inline { image_url } => {
                codex_utils_image::estimate_original_image_tokens(image_url)
                    .unwrap_or(TOOL_OUTPUT_IMAGE_ORIGINAL_VISUAL_TOKENS)
            }
            codex_protocol::models::ImageReference::File { .. } => {
                TOOL_OUTPUT_IMAGE_ORIGINAL_VISUAL_TOKENS
            }
        },
        _ => TOOL_OUTPUT_IMAGE_VISUAL_TOKENS,
    }
}

pub(crate) fn image_budget_charge(visual_tokens: usize, policy: TruncationPolicy) -> usize {
    match policy {
        TruncationPolicy::Bytes(_) => approx_bytes_for_tokens(visual_tokens),
        TruncationPolicy::Tokens(_) => visual_tokens,
    }
}

/// Which images of one tool result survive the entrance limits.
pub struct ImageAdmission {
    /// Per input index: whether the InputImage item is kept.
    pub keep: Vec<bool>,
    /// Images replaced by the single consolidated omission marker.
    pub dropped: usize,
    /// Total visual tokens of the kept images.
    pub visual_tokens: usize,
}

pub fn admit_tool_output_images(
    items: &[FunctionCallOutputContentItem],
    policy: TruncationPolicy,
) -> ImageAdmission {
    let image_indices: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| matches!(item, FunctionCallOutputContentItem::InputImage { .. }))
        .map(|(index, _)| index)
        .collect();
    let mut keep = vec![false; items.len()];
    let mut dropped = 0usize;

    // Entrance bound 1: one image may not exceed the per-image encoded size
    // before anything decodes it.
    let mut candidates: Vec<usize> = image_indices
        .iter()
        .copied()
        .filter(|&index| {
            let FunctionCallOutputContentItem::InputImage { image, .. } = &items[index] else {
                unreachable!("filtered to images");
            };
            match image {
                codex_protocol::models::ImageReference::Inline { image_url } => {
                    let header_fits = !image_url
                        .get(..5)
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
                        || image_url
                            .split_once(',')
                            .map_or(image_url.len(), |(header, _)| header.len())
                            <= MAX_IMAGE_REFERENCE_BYTES;
                    image_url.len() <= MAX_TOOL_OUTPUT_IMAGE_BYTES && header_fits
                }
                // File references carry no payload here; the wire layer
                // already charges them full visual cost.
                codex_protocol::models::ImageReference::File { file_id } => {
                    file_id.len() <= MAX_IMAGE_REFERENCE_BYTES
                }
            }
        })
        .collect();
    dropped += image_indices.len().saturating_sub(candidates.len());

    // Entrance bound 2: at most MAX_TOOL_OUTPUT_IMAGES, keeping the NEWEST
    // (the latest screenshot is the evidence the turn still needs).
    if candidates.len() > MAX_TOOL_OUTPUT_IMAGES {
        let excess = candidates.len() - MAX_TOOL_OUTPUT_IMAGES;
        dropped += excess;
        candidates.drain(..excess);
    }

    // Entrance bound 3: aggregate encoded bytes, again dropping oldest first.
    let encoded_len = |index: usize| {
        let FunctionCallOutputContentItem::InputImage { image, .. } = &items[index] else {
            unreachable!("filtered to images");
        };
        match image {
            codex_protocol::models::ImageReference::Inline { image_url } => image_url.len(),
            codex_protocol::models::ImageReference::File { .. } => 0,
        }
    };
    let mut total: usize = candidates.iter().copied().map(encoded_len).sum();
    while total > MAX_TOOL_OUTPUT_IMAGE_TOTAL_BYTES && candidates.len() > 1 {
        let removed = candidates.remove(0);
        total -= encoded_len(removed);
        dropped += 1;
    }

    // Entrance bound 4: text and standard-detail images share the
    // per-output budget. Keep images newest-first while their cumulative
    // visual cost fits. Original detail uses bounded header dimensions where
    // available and otherwise costs the conservative patch ceiling.
    let budget_bytes = bounded_image_output_policy(policy).byte_budget();
    let mut visual_total = 0usize;
    let mut visual_kept: Vec<usize> = Vec::new();
    for &index in candidates.iter().rev() {
        let FunctionCallOutputContentItem::InputImage { image, detail } = &items[index] else {
            unreachable!("filtered to images");
        };
        let cost = charged_visual_tokens(image, *detail);
        if approx_bytes_for_tokens(visual_total + cost) <= budget_bytes {
            visual_total += cost;
            visual_kept.push(index);
        } else {
            dropped += 1;
        }
    }
    for &index in &visual_kept {
        keep[index] = true;
    }

    ImageAdmission {
        keep,
        dropped,
        visual_tokens: visual_total,
    }
}
