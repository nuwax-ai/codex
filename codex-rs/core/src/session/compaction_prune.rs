//! Deterministic, model-free shrinkage of tool output.
//!
//! The first stage of migration compaction (see FORK.md, "迁移压缩"): before any
//! summary is requested, oversized tool results are cut down to a head/tail
//! window with a marker in the middle. It costs no model call, cannot
//! hallucinate, and is replayable because it only rewrites the in-memory surface
//! for this request.

use codex_history::ResponseItemEnvelope;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ImageReference;
use codex_protocol::models::ResponseItem;

/// What one pruning pass changed, for the compaction log and the summary note.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PruneOutcome {
    pub pruned_results: usize,
    pub chars_removed: usize,
}

impl PruneOutcome {
    pub(crate) fn is_empty(&self) -> bool {
        self.pruned_results == 0
    }
}

/// Call ids of text tool outputs longer than `min_chars_to_prune`.
pub(crate) fn oversized_tool_output_call_ids(
    items: &[ResponseItemEnvelope],
    min_chars_to_prune: usize,
) -> Vec<String> {
    items
        .iter()
        .filter_map(|envelope| match &envelope.item {
            ResponseItem::FunctionCallOutput {
                call_id: Some(call_id),
                output,
                ..
            }
            | ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } => match &output.body {
                FunctionCallOutputBody::Text(text) if text.chars().count() > min_chars_to_prune => {
                    Some(call_id.clone())
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Re-apply a persisted head/tail window to the tool output with `call_id`.
/// Returns whether the item was found and shortened.
pub(crate) fn prune_tool_output_by_call_id(
    items: &mut [ResponseItemEnvelope],
    call_id: &str,
    keep_head_chars: usize,
    keep_tail_chars: usize,
) -> bool {
    for envelope in items {
        let body = match &mut envelope.item {
            ResponseItem::FunctionCallOutput {
                call_id: Some(existing),
                output,
                ..
            } if existing == call_id => &mut output.body,
            ResponseItem::CustomToolCallOutput {
                call_id: existing,
                output,
                ..
            } if existing == call_id => &mut output.body,
            _ => continue,
        };
        return prune_body(
            body,
            keep_head_chars,
            keep_tail_chars,
            keep_head_chars.saturating_add(keep_tail_chars),
        )
        .is_some();
    }
    false
}

/// Keep `keep_head_chars` characters from the start and `keep_tail_chars` from
/// the end of every text tool result longer than `min_chars_to_prune`,
/// replacing the middle with a marker.
///
/// Slicing is by Unicode scalar value, so a retained boundary never splits a
/// character. Non-text bodies (content items with images) are left untouched.
pub(crate) fn prune_oversized_tool_results(
    items: &mut [ResponseItemEnvelope],
    keep_head_chars: usize,
    keep_tail_chars: usize,
    min_chars_to_prune: usize,
) -> PruneOutcome {
    let mut outcome = PruneOutcome::default();
    for envelope in items {
        match &mut envelope.item {
            ResponseItem::FunctionCallOutput { output, .. } => {
                if let Some(removed) = prune_body(
                    &mut output.body,
                    keep_head_chars,
                    keep_tail_chars,
                    min_chars_to_prune,
                ) {
                    outcome.pruned_results += 1;
                    outcome.chars_removed += removed;
                }
            }
            ResponseItem::CustomToolCallOutput { output, .. } => {
                if let Some(removed) = prune_body(
                    &mut output.body,
                    keep_head_chars,
                    keep_tail_chars,
                    min_chars_to_prune,
                ) {
                    outcome.pruned_results += 1;
                    outcome.chars_removed += removed;
                }
            }
            _ => {}
        }
    }
    outcome
}

fn prune_body(
    body: &mut FunctionCallOutputBody,
    keep_head_chars: usize,
    keep_tail_chars: usize,
    min_chars_to_prune: usize,
) -> Option<usize> {
    let FunctionCallOutputBody::Text(text) = body else {
        return None;
    };
    let total = text.chars().count();
    if total <= min_chars_to_prune {
        return None;
    }
    let head: String = text.chars().take(keep_head_chars).collect();
    let tail_len = keep_tail_chars.min(total.saturating_sub(keep_head_chars));
    let tail: String = text.chars().skip(total.saturating_sub(tail_len)).collect();
    let removed = total.saturating_sub(head.chars().count() + tail.chars().count());
    if removed == 0 {
        return None;
    }
    *text = format!("{head}\n…<pruned {removed} chars>…\n{tail}");
    Some(removed)
}


/// Number of most recent inline images in a surface that are always kept.
/// Older images are replaced with a text placeholder when a surface must
/// shrink: the model keeps the latest visual context and every earlier image
/// is recoverable by re-running its producing tool (Fork 2026-11-10, see
/// FORK.md §5.5). Images inside the protected current-turn input are never
/// candidates (the caller clips them out before this pass runs).
pub(crate) const PROTECTED_RECENT_IMAGES: usize = 2;

/// Placeholder that replaces a pruned image. It names the tool that produced
/// it so the model can re-run it if the visual is needed again.
pub(crate) const PRUNED_IMAGE_PLACEHOLDER: &str =
    "[image omitted to fit the request byte budget; re-run the tool to view it again]";

/// True when a content item carries an inline image.
pub(crate) fn is_inline_image(item: &FunctionCallOutputContentItem) -> bool {
    matches!(
        item,
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline { .. },
            ..
        }
    )
}

/// Replaces every inline image older than the newest `keep_recent` with
/// `PRUNED_IMAGE_PLACEHOLDER` (text). Returns how many images were replaced.
///
/// "Older" is positional: the newest `keep_recent` images anywhere in the
/// surface survive, including images inside text tool outputs and user
/// messages; everything before them is eligible. Images already replaced are
/// not double-counted.
pub(crate) fn prune_stale_images(
    items: &mut [ResponseItemEnvelope],
    keep_recent: usize,
) -> Vec<(String, usize)> {
    // Collect every image position first: newest `keep_recent` survive.
    let mut positions: Vec<(usize, usize)> = Vec::new();
    for (item_index, envelope) in items.iter().enumerate() {
        match &envelope.item {
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. } => {
                if let FunctionCallOutputBody::ContentItems(content) = &output.body {
                    for (content_index, content_item) in content.iter().enumerate() {
                        if is_inline_image(content_item) {
                            positions.push((item_index, content_index));
                        }
                    }
                }
            }
            ResponseItem::Message { content, .. } => {
                for (content_index, content_item) in content.iter().enumerate() {
                    if matches!(
                        content_item,
                        ContentItem::InputImage {
                            image: ImageReference::Inline { .. },
                            ..
                        }
                    ) {
                        positions.push((item_index, content_index));
                    }
                }
            }
            _ => {}
        }
    }
    if positions.len() <= keep_recent {
        return Vec::new();
    }
    let prunable = positions.len() - keep_recent;
    let mut per_call_id: Vec<(String, usize)> = Vec::new();
    for (item_index, content_index) in positions.into_iter().take(prunable) {
        let placeholder = PRUNED_IMAGE_PLACEHOLDER.to_string();
        let call_id = tool_call_id_of(&items[item_index].item);
        let replaced_one = match &mut items[item_index].item {
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. } => {
                match &mut output.body {
                    FunctionCallOutputBody::ContentItems(content) => {
                        replace_image_at(content, content_index, placeholder)
                    }
                    _ => false,
                }
            }
            ResponseItem::Message { content, .. } => {
                replace_message_image_at(content, content_index, placeholder)
            }
            _ => false,
        };
        if replaced_one {
            match per_call_id.iter_mut().find(|(id, _)| *id == call_id) {
                Some((_, count)) => *count += 1,
                None => per_call_id.push((call_id, 1)),
            }
        }
    }
    per_call_id
}

/// The stable call id of a tool-output item, or an empty string for items that
/// are not tool outputs (user-message images). Records keyed on the empty id
/// are skipped on replay, which is safe: user-message images are not part of
/// the retained-tail budget in practice (the boundary protects the current
/// turn) and the placeholder still stands in the live surface.
fn tool_call_id_of(item: &ResponseItem) -> String {
    match item {
        ResponseItem::FunctionCallOutput { call_id, .. } => {
            call_id.clone().unwrap_or_default()
        }
        ResponseItem::CustomToolCallOutput { call_id, .. } => call_id.clone(),
        _ => String::new(),
    }
}

/// Replaces the image at `index` in a tool-output content list with a text
/// placeholder. Returns whether a replacement happened.
fn replace_image_at(
    content: &mut [FunctionCallOutputContentItem],
    index: usize,
    placeholder: String,
) -> bool {
    match content.get_mut(index) {
        Some(FunctionCallOutputContentItem::InputImage { .. }) => {
            content[index] = FunctionCallOutputContentItem::InputText { text: placeholder };
            true
        }
        _ => false,
    }
}

/// Replaces the image at `index` in a message content list with a text
/// placeholder. Returns whether a replacement happened.
fn replace_message_image_at(content: &mut [ContentItem], index: usize, placeholder: String) -> bool {
    match content.get_mut(index) {
        Some(ContentItem::InputImage { .. }) => {
            content[index] = ContentItem::InputText { text: placeholder };
            true
        }
        _ => false,
    }
}


/// Re-applies a persisted image replacement: the `count` oldest inline images
/// of the tool output with `call_id` become `PRUNED_IMAGE_PLACEHOLDER`.
/// Returns whether the output was found. Used by rollout replay to reproduce
/// the live surface byte-for-byte (Fork 2026-11-10, FORK.md §5.5).
pub(crate) fn replay_pruned_images(
    items: &mut [ResponseItemEnvelope],
    call_id: &str,
    count: usize,
) -> bool {
    let mut remaining = count;
    for envelope in items.iter_mut() {
        let is_target = match &envelope.item {
            ResponseItem::FunctionCallOutput {
                call_id: Some(existing),
                ..
            } => existing == call_id,
            ResponseItem::CustomToolCallOutput {
                call_id: existing, ..
            } => existing == call_id,
            _ => false,
        };
        if !is_target {
            continue;
        }
        let content = match &mut envelope.item {
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. } => match &mut output.body {
                FunctionCallOutputBody::ContentItems(content) => content,
                _ => return false,
            },
            _ => return false,
        };
        for item in content.iter_mut() {
            if remaining == 0 {
                break;
            }
            if matches!(
                item,
                FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline { .. },
                    ..
                }
            ) {
                *item = FunctionCallOutputContentItem::InputText {
                    text: PRUNED_IMAGE_PLACEHOLDER.to_string(),
                };
                remaining -= 1;
            }
        }
        return true;
    }
    false
}

/// Total characters of text content in the surface, used by tests and logs to
/// confirm a pass actually reduced what the model will receive.
#[cfg(test)]
pub(crate) fn surface_text_chars(items: &[ResponseItemEnvelope]) -> usize {
    items
        .iter()
        .map(|envelope| match &envelope.item {
            ResponseItem::FunctionCallOutput { output, .. } => body_chars(&output.body),
            ResponseItem::CustomToolCallOutput { output, .. } => body_chars(&output.body),
            ResponseItem::Message { content, .. } => content
                .iter()
                .map(|item| match item {
                    ContentItem::InputText { text } => text.chars().count(),
                    _ => 0,
                })
                .sum(),
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
fn body_chars(body: &FunctionCallOutputBody) -> usize {
    match body {
        FunctionCallOutputBody::Text(text) => text.chars().count(),
        FunctionCallOutputBody::ContentItems(items) => items.len(),
    }
}

#[cfg(test)]
#[path = "compaction_prune_tests.rs"]
mod tests;
