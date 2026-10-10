//! Bounded capture of a turn's partial transcript at an output-cap failure.
//!
//! A cap exhaustion fails the turn closed (never a synthesized Completed),
//! but the transcript the model streamed before the cap is evidence the user
//! already saw. This module snapshots the turn's streamed items into a
//! bounded [`CapPartialEvent`] emitted once, immediately before the failure
//! terminal, and persisted in both history modes.
//!
//! Bounds follow the cap decisions (D1-c): 16 KiB per fragment, 64 KiB and
//! 32 fragments per turn, prefix-keep at UTF-8 boundaries. Without a verified
//! tokenizer the capture is byte/count-bounded only — `token_evidence`
//! records `Unverified` and the fragments are diagnostics, never model
//! instructions, executable tool calls, or synthesized completions.

use std::time::Duration;

use codex_history::ResponseItemEnvelope;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::CapBudgetKind;
use codex_protocol::protocol::CapPartialBudget;
use codex_protocol::protocol::CapPartialEvent;
use codex_protocol::protocol::CapPartialFragment;
use codex_protocol::protocol::CapPartialFragmentKind;
use codex_protocol::protocol::TokenBudgetEvidence;

const MAX_FRAGMENT_BYTES: usize = 16 * 1024;
const MAX_TURN_BYTES: usize = 64 * 1024;
const MAX_FRAGMENTS: usize = 32;

/// Builds the bounded partial record for `turn_id` from the turn's streamed
/// items, or `None` when nothing assistant-produced preceded the cap.
pub(crate) fn build_cap_partial(
    turn_id: &str,
    response_key: &str,
    items: &[ResponseItemEnvelope],
) -> Option<CapPartialEvent> {
    let streamed = trailing_turn_items(items);
    let mut fragments = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut enforced: Vec<CapBudgetKind> = Vec::new();

    for envelope in streamed {
        let item = &envelope.item;
        let (kind, text, item_id) = match item {
            ResponseItem::AgentMessage { id, content, .. } => (
                CapPartialFragmentKind::AssistantText,
                codex_protocol::models::plaintext_agent_message_content(content)
                    .unwrap_or_default(),
                id.as_deref().map(|id| id.to_string()),
            ),
            ResponseItem::Message {
                role, id, content, ..
            } if role == "assistant" => (
                CapPartialFragmentKind::AssistantText,
                content
                    .iter()
                    .filter_map(|part| match part {
                        codex_protocol::models::ContentItem::OutputText { text } => {
                            Some(text.as_str())
                        }
                        _ => None,
                    })
                    .collect::<String>(),
                id.as_deref().map(|id| id.to_string()),
            ),
            ResponseItem::Reasoning {
                summary,
                content,
                id,
                ..
            } => {
                let text = summary
                    .iter()
                    .map(|part| match part {
                        codex_protocol::models::ReasoningItemReasoningSummary::SummaryText {
                            text,
                        } => text.as_str(),
                    })
                    .chain(content.iter().flatten().filter_map(|part| match part {
                        codex_protocol::models::ReasoningItemContent::ReasoningText { text } => {
                            Some(text.as_str())
                        }
                        codex_protocol::models::ReasoningItemContent::Text { .. } => None,
                    }))
                    .collect::<String>();
                (
                    CapPartialFragmentKind::Reasoning,
                    text,
                    id.as_deref().map(|id| id.to_string()),
                )
            }
            ResponseItem::FunctionCall {
                call_id,
                arguments,
                name,
                ..
            } => (
                CapPartialFragmentKind::ToolArguments,
                format!("call {name}: {arguments}"),
                Some(call_id.to_string()),
            ),
            _ => continue,
        };
        if text.is_empty() {
            continue;
        }
        if fragments.len() >= MAX_FRAGMENTS {
            enforced.push(CapBudgetKind::TurnFragments);
            break;
        }
        let (text, truncated) = match truncate_at_char_boundary(&text, MAX_FRAGMENT_BYTES) {
            Some(prefix) => (prefix, true),
            None => (text, false),
        };
        if truncated {
            enforced.push(CapBudgetKind::FragmentBytes);
        }
        if total_bytes + text.len() as u64 > MAX_TURN_BYTES as u64 {
            let remaining = (MAX_TURN_BYTES as u64).saturating_sub(total_bytes) as usize;
            let Some(prefix) = truncate_at_char_boundary(&text, remaining) else {
                break;
            };
            enforced.push(CapBudgetKind::TurnBytes);
            fragments.push(fragment(kind, item_id, prefix, /*truncated*/ true));
            break;
        }
        total_bytes += text.len() as u64;
        fragments.push(fragment(kind, item_id, text, truncated));
    }

    if fragments.is_empty() {
        return None;
    }
    Some(CapPartialEvent {
        turn_id: turn_id.to_string(),
        response_key: response_key.to_string(),
        reported_usage: None,
        budget: CapPartialBudget {
            fragment_count: fragments.len() as u32,
            total_bytes,
            enforced_caps: enforced,
            token_evidence: TokenBudgetEvidence::Unverified,
        },
        fragments,
    })
}

fn fragment(
    kind: CapPartialFragmentKind,
    item_id: Option<String>,
    text: String,
    truncated: bool,
) -> CapPartialFragment {
    CapPartialFragment {
        fragment_key: item_id
            .clone()
            .unwrap_or_else(|| format!("idx-{}", text.len())),
        kind,
        item_id,
        text,
        truncated,
    }
}

/// Returns a prefix cut at a UTF-8 boundary when `text` exceeds `max_bytes`.
fn truncate_at_char_boundary(text: &str, max_bytes: usize) -> Option<String> {
    if text.len() <= max_bytes {
        return None;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(text[..end].to_string())
}

/// The trailing assistant-produced items of a turn: everything after the
/// last user-role item, which is the transcript the model streamed in the
/// turn that just failed.
pub(crate) fn trailing_turn_items(items: &[ResponseItemEnvelope]) -> &[ResponseItemEnvelope] {
    let start = items
        .iter()
        .rposition(|envelope| {
            matches!(
                &envelope.item,
                ResponseItem::Message { role, .. } if role == "user"
            )
        })
        .map(|position| position + 1)
        .unwrap_or(0);
    &items[start..]
}

#[allow(dead_code)]
const _: () = {
    // Bounds stay pinned by tests; this keeps Duration referenced if the
    // module grows timing knobs.
    let _ = Duration::ZERO;
};

#[cfg(test)]
#[path = "cap_partial_tests.rs"]
mod tests;
