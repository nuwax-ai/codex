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
