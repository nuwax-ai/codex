use crate::TruncationPolicy;
use crate::approx_bytes_for_tokens;
use codex_protocol::models::FunctionCallOutputContentItem;

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

/// The visual cost charged against the shared per-output budget, if this
/// detail level is charged here at all.
pub fn charged_visual_tokens(detail: Option<codex_protocol::models::ImageDetail>) -> Option<usize> {
    match detail {
        Some(codex_protocol::models::ImageDetail::Original) => None,
        _ => Some(TOOL_OUTPUT_IMAGE_VISUAL_TOKENS),
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
                    image_url.len() <= MAX_TOOL_OUTPUT_IMAGE_BYTES
                }
                // File references carry no payload here; the wire layer
                // already charges them full visual cost.
                codex_protocol::models::ImageReference::File { .. } => true,
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
    // visual cost fits; the newest image always survives so a limit can
    // never silently delete every image of a result. `original`-detail
    // images opt OUT of this charge: their cost is accounted patch-based in
    // the context window (and by compaction), so here only the count/size
    // bounds above constrain them.
    let budget = policy.token_budget();
    let mut visual_total = 0usize;
    let mut visual_kept: Vec<usize> = Vec::new();
    for &index in candidates.iter().rev() {
        let FunctionCallOutputContentItem::InputImage { detail, .. } = &items[index] else {
            unreachable!("filtered to images");
        };
        let Some(cost) = charged_visual_tokens(*detail) else {
            visual_kept.push(index);
            continue;
        };
        if visual_kept.is_empty() || visual_total + cost <= budget {
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
