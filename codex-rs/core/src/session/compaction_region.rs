//! Choosing which slice of the model-visible surface a migration compaction
//! replaces.
//!
//! Two invariants keep the cut safe:
//! - a tool call and its result must stay on the same side (the model rejects a
//!   history that starts with an orphan result), and
//! - a verbatim tail is always retained, so the recent turns and the next step
//!   survive the summary.
//!
//! The selected span is also capped, because the summary request itself must fit
//! the target model's window: that bound is what lets a conversation continue on
//! a smaller model without ever calling the old one.

use codex_history::ResponseItemEnvelope;
use codex_protocol::models::ResponseItem;

use crate::context_manager::estimate_item_token_count;

/// A half-open range `[start, end)` over the model-visible items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompactionRegion {
    pub start: usize,
    pub end: usize,
    pub tokens: i64,
}

/// How many tool calls one item opens (positive) or closes (negative).
fn tool_pair_delta(item: &ResponseItem) -> i32 {
    match item {
        ResponseItem::FunctionCall { .. }
        | ResponseItem::CustomToolCall { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::ToolSearchCall { .. } => 1,
        ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::ToolSearchOutput { .. } => -1,
        _ => 0,
    }
}

/// Balance of the surface before each index: `prefix[i]` counts tool calls that
/// are still open when the cut sits at `i`. A safe cut requires `prefix[i] == 0`.
fn cut_balances(items: &[ResponseItemEnvelope]) -> Vec<i32> {
    let mut prefix = Vec::with_capacity(items.len() + 1);
    prefix.push(0);
    let mut open = 0;
    for envelope in items {
        open += tool_pair_delta(&envelope.item);
        prefix.push(open);
    }
    prefix
}

fn item_tokens(items: &[ResponseItemEnvelope]) -> Vec<i64> {
    items
        .iter()
        .map(|envelope| estimate_item_token_count(&envelope.item))
        .collect()
}

/// Pick the oldest span that can be replaced while retaining `retain_tail_tokens`
/// verbatim and keeping the span itself under `max_span_tokens`. The returned
/// boundary must additionally land on an item that carries a rollout id: the
/// replacement record resumes by id, and no forward position can satisfy the
/// tail/span constraints (both monotone), so an id-less cut has no legal
/// adjustment and yields `None`.
///
/// `max_span_tokens <= 0` disables the cap (used by tests).
pub(crate) fn select_compaction_region(
    items: &[ResponseItemEnvelope],
    retain_tail_tokens: i64,
    max_span_tokens: i64,
) -> Option<CompactionRegion> {
    if items.len() < 2 {
        return None;
    }
    let prefix = cut_balances(items);
    let tokens = item_tokens(items);
    if tokens.iter().all(|tokens| *tokens <= 0) {
        return None;
    }

    // Walk back from the tail until the retained suffix is big enough.
    let mut end = items.len();
    let mut tail = 0i64;
    while end > 0 && tail < retain_tail_tokens.max(0) {
        end -= 1;
        tail += tokens[end];
    }
    // A cut through an open tool pair is invalid; move it back to the pair start.
    while end > 0 && prefix[end] != 0 {
        end -= 1;
    }

    // Cap the span so a single summary request stays inside the target window:
    // the replacement always starts at the oldest item, so the cap shortens the
    // tail edge rather than skipping the oldest history.
    if max_span_tokens > 0 {
        let mut span = 0i64;
        let mut capped = 0usize;
        while capped < end {
            let next = span + tokens[capped];
            if next > max_span_tokens {
                break;
            }
            span = next;
            capped += 1;
        }
        end = capped;
        while end > 0 && prefix[end] != 0 {
            end -= 1;
        }
    }

    let start = 0usize;
    if start >= end || end >= items.len() {
        return None;
    }

    // The replacement record hides history by the id of the first item the
    // model must still see, so the cut must land on an id carrier — and it
    // cannot be moved forward onto one: the retain walk stopped at the
    // LARGEST end whose tail still satisfies `retain_tail_tokens` (end+1
    // already violates it) and the span walk stopped at the LARGEST end
    // under `max_span_tokens` (end+1 already exceeds it). Both invariants
    // are monotone, so any forward position breaks at least one guarantee
    // the cut was validated for. An id-less boundary therefore means there
    // is no legal cut — report None instead of silently orphaning history
    // (an old checkpoint's replacement entries can lack ids).
    if items[end].id().is_none() {
        return None;
    }
    let tokens: i64 = tokens[start..end].iter().sum();
    Some(CompactionRegion {
        start,
        end,
        tokens,
    })
}

#[cfg(test)]
#[path = "compaction_region_tests.rs"]
mod tests;
