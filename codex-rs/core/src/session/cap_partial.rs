//! Bounded streaming capture of a sampling's partial transcript at an
//! output-cap failure.
//!
//! A cap exhaustion fails the sampling closed (never a synthesized
//! Completed), but the text the model streamed before the cap is evidence
//! the user already saw. This accumulator observes the SAME stream events
//! the turn loop renders — deltas and whole completed items, at their
//! observation point — and bounds its memory as it goes: bytes past a
//! budget are dropped the moment the budget is hit, never
//! collected-then-truncated from the ordinary history after the failure.
//!
//! Bounds follow the cap decisions (D1-c): 16 KiB per fragment, 64 KiB and
//! 32 fragments per sampling, prefix-keep at UTF-8 boundaries. The finished
//! [`CapPartialEvent`] is diagnostic evidence only — fragments never become
//! executable tool calls, synthesized completions, or model instructions,
//! and without a verified tokenizer the recorded token evidence stays
//! `Unverified`. Emission is gated on
//! [`codex_features::Feature::CapPartialDiagnostics`].

use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::CapBudgetKind;
use codex_protocol::protocol::CapPartialBudget;
use codex_protocol::protocol::CapPartialEvent;
use codex_protocol::protocol::CapPartialFragment;
use codex_protocol::protocol::CapPartialFragmentKind;
use codex_protocol::protocol::ReportedResponseUsage;
use codex_protocol::protocol::TokenBudgetEvidence;
use std::collections::HashMap;
use std::collections::HashSet;

const MAX_FRAGMENT_BYTES: usize = 16 * 1024;
const MAX_TURN_BYTES: usize = 64 * 1024;
const MAX_FRAGMENTS: usize = 32;

/// Incremental bounded capture for one sampling attempt.
///
/// Fragments are keyed by their streaming identity (item id or tool call
/// id) so a delta stream appends to one fragment, and an item that only
/// arrives whole (no deltas) is captured once at `observe_item_done`. A
/// fragment that hits its own byte budget stops storing but leaves the
/// capture open for other fragments; the turn byte or fragment-count
/// budgets close the capture entirely.
#[derive(Default)]
pub(crate) struct CapPartialCapture {
    fragments: Vec<CapPartialFragment>,
    positions: HashMap<String, usize>,
    delta_keys: HashSet<String>,
    stored_bytes: Vec<usize>,
    total_bytes: usize,
    enforced: Vec<CapBudgetKind>,
    /// Set once a turn-level budget is exhausted; every further observation
    /// is dropped without storing. The reason stays attached so content
    /// observed after the close still records which budget dropped it.
    closed: Option<CapBudgetKind>,
    /// Disambiguates whole items that arrive without any id.
    done_without_id: usize,
}

impl CapPartialCapture {
    /// Observes one streamed text delta attributed to `key` (item id or
    /// tool call id). Bytes past the fragment or turn budgets are dropped
    /// immediately.
    pub(crate) fn observe_delta(&mut self, kind: CapPartialFragmentKind, key: &str, delta: &str) {
        if delta.is_empty() {
            return;
        }
        if self.closed.is_some() {
            self.record_observed_after_close(key);
            return;
        }
        let index = match self.position_for_new(kind, key) {
            Some(index) => index,
            None => return,
        };
        self.delta_keys.insert(key.to_string());
        self.append_bounded(index, delta);
    }

    /// Observes one completed response item. Items already fed by deltas
    /// (their key has a fragment) are not captured again; an item that only
    /// arrives whole is captured as one bounded fragment. Non-assistant
    /// items are ignored.
    pub(crate) fn observe_item_done(&mut self, item: &ResponseItem) {
        if self.closed.is_some() {
            // The rendered key is not needed to record the drop.
            if rendered_item(item).is_some() {
                self.record_observed_after_close("");
            }
            return;
        }
        let Some((kind, key)) = rendered_item(item) else {
            return;
        };
        let anonymous_key;
        let key = if key.is_empty() {
            self.done_without_id += 1;
            anonymous_key = format!("done-{}", self.done_without_id);
            anonymous_key.as_str()
        } else {
            key
        };
        if self.positions.contains_key(key) {
            return;
        }
        let Some(index) = self.position_for_new(kind, key) else {
            return;
        };
        match item {
            ResponseItem::AgentMessage { content, .. } => {
                if content.iter().any(|part| {
                    matches!(
                        part,
                        codex_protocol::models::AgentMessageInputContent::EncryptedContent { .. }
                    )
                }) {
                    return;
                }
                for (part_index, part) in content.iter().enumerate() {
                    if part_index > 0 {
                        self.append_bounded(index, "\n");
                    }
                    if let codex_protocol::models::AgentMessageInputContent::InputText { text } =
                        part
                    {
                        self.append_bounded(index, text);
                    }
                }
            }
            ResponseItem::Message { content, .. } => {
                for part in content {
                    if let codex_protocol::models::ContentItem::OutputText { text } = part {
                        self.append_bounded(index, text);
                    }
                }
            }
            ResponseItem::Reasoning {
                summary, content, ..
            } => {
                for part in summary {
                    let codex_protocol::models::ReasoningItemReasoningSummary::SummaryText { text } =
                        part;
                    self.append_bounded(index, text);
                }
                for part in content.iter().flatten() {
                    if let codex_protocol::models::ReasoningItemContent::ReasoningText { text } =
                        part
                    {
                        self.append_bounded(index, text);
                    }
                }
            }
            ResponseItem::FunctionCall {
                name, arguments, ..
            } => {
                for text in ["call ", name, ": ", arguments] {
                    self.append_bounded(index, text);
                }
            }
            ResponseItem::CustomToolCall { name, input, .. } => {
                for text in ["call ", name, ": ", input] {
                    self.append_bounded(index, text);
                }
            }
            _ => {}
        }
    }

    /// Observes text that arrived whole (an item that never streamed, or a
    /// summary delivered done-only). A key already fed by deltas is not
    /// captured again.
    pub(crate) fn observe_whole(&mut self, kind: CapPartialFragmentKind, key: &str, text: &str) {
        if self.closed.is_some() {
            self.record_observed_after_close(key);
            return;
        }
        if self.positions.contains_key(key) {
            return;
        }
        let index = match self.position_for_new(kind, key) {
            Some(index) => index,
            None => return,
        };
        self.append_bounded(index, text);
    }

    /// Captures done-only summary sections independently while marking their
    /// parent item observed so its whole completion cannot duplicate them.
    pub(crate) fn observe_reasoning_summary(
        &mut self,
        item_id: &str,
        summary_index: i64,
        text: &str,
    ) {
        if self.closed.is_some() {
            self.record_observed_after_close(item_id);
            return;
        }
        if self.delta_keys.contains(item_id) {
            return;
        }
        let key = format!("{item_id}:summary:{summary_index}");
        self.observe_whole(CapPartialFragmentKind::Reasoning, &key, text);
        if let Some(&index) = self.positions.get(&key) {
            self.fragments[index].item_id = Some(item_id.to_string());
            self.positions.entry(item_id.to_string()).or_insert(index);
        }
    }

    /// Finishes the capture for a sampling that exhausted its cap, or
    /// `None` when nothing assistant-produced preceded the cap.
    pub(crate) fn finish(
        self,
        turn_id: &str,
        response_key: &str,
        reported_usage: Option<&ReportedResponseUsage>,
    ) -> Option<CapPartialEvent> {
        if self.fragments.is_empty() {
            return None;
        }
        Some(CapPartialEvent {
            turn_id: turn_id.to_string(),
            response_key: response_key.to_string(),
            reported_usage: reported_usage.cloned(),
            budget: CapPartialBudget {
                fragment_count: self.fragments.len() as u32,
                total_bytes: self.total_bytes as u64,
                enforced_caps: self.enforced,
                token_evidence: TokenBudgetEvidence::Unverified,
            },
            fragments: self.fragments,
        })
    }

    fn position_for_new(&mut self, kind: CapPartialFragmentKind, key: &str) -> Option<usize> {
        if let Some(&index) = self.positions.get(key) {
            return Some(index);
        }
        if self.fragments.len() >= MAX_FRAGMENTS {
            self.record_once(CapBudgetKind::TurnFragments);
            self.closed = Some(CapBudgetKind::TurnFragments);
            return None;
        }
        let index = self.fragments.len();
        self.fragments.push(CapPartialFragment {
            fragment_key: key.to_string(),
            kind,
            item_id: item_id_for(kind, key),
            text: String::new(),
            truncated: false,
        });
        self.stored_bytes.push(0);
        self.positions.insert(key.to_string(), index);
        Some(index)
    }

    /// Appends `text` to fragment `index`, keeping at most the fragment and
    /// turn byte budgets and dropping everything past them.
    fn append_bounded(&mut self, index: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(reason) = self.closed {
            self.fragments[index].truncated = true;
            self.record_once(reason);
            return;
        }
        if self.fragments[index].truncated || self.stored_bytes[index] >= MAX_FRAGMENT_BYTES {
            // This fragment already hit its own budget: the delta is dropped
            // and the fragment now provably holds a prefix.
            self.fragments[index].truncated = true;
            self.record_once(CapBudgetKind::FragmentBytes);
            return;
        }
        let fragment_remaining = MAX_FRAGMENT_BYTES - self.stored_bytes[index];
        let turn_remaining = MAX_TURN_BYTES.saturating_sub(self.total_bytes);
        let binding = fragment_remaining.min(turn_remaining);
        let stored = match prefix_at_char_boundary(text, binding) {
            Some(prefix) => prefix,
            None => text,
        };
        let fits = stored.len() == text.len();
        self.fragments[index].text.push_str(stored);
        self.stored_bytes[index] += stored.len();
        self.total_bytes += stored.len();
        if !fits {
            self.fragments[index].truncated = true;
            // The smaller remaining budget is the one the text exceeded.
            let cap = if fragment_remaining <= turn_remaining {
                CapBudgetKind::FragmentBytes
            } else {
                CapBudgetKind::TurnBytes
            };
            self.record_once(cap);
            if turn_remaining <= fragment_remaining {
                self.closed = Some(CapBudgetKind::TurnBytes);
                self.record_once(CapBudgetKind::TurnBytes);
            }
        }
        if self.total_bytes >= MAX_TURN_BYTES {
            self.closed = Some(CapBudgetKind::TurnBytes);
            // Closing on the turn budget is itself an enforced cap, even
            // when no further content ever arrives to record the drop.
            self.record_once(CapBudgetKind::TurnBytes);
        }
    }

    /// Records that assistant content was observed after the capture had
    /// already closed: the diagnostics must show which budget dropped it.
    fn record_observed_after_close(&mut self, key: &str) {
        if (!self.positions.contains_key(key) || key.is_empty())
            && let Some(reason) = self.closed
        {
            self.record_once(reason);
        }
    }

    fn record_once(&mut self, kind: CapBudgetKind) {
        if !self.enforced.contains(&kind) {
            self.enforced.push(kind);
        }
    }
}

/// The fragment key and bounded render for an item that arrives whole.
/// `None` for non-assistant-produced items.
fn rendered_item(item: &ResponseItem) -> Option<(CapPartialFragmentKind, &str)> {
    match item {
        ResponseItem::AgentMessage { id, .. } => Some((
            CapPartialFragmentKind::AssistantText,
            id.as_ref()
                .map(codex_protocol::ResponseItemId::as_str)
                .unwrap_or_default(),
        )),
        ResponseItem::Message { role, id, .. } if role == "assistant" => Some((
            CapPartialFragmentKind::AssistantText,
            id.as_ref()
                .map(codex_protocol::ResponseItemId::as_str)
                .unwrap_or_default(),
        )),
        ResponseItem::Reasoning { id, .. } => Some((
            CapPartialFragmentKind::Reasoning,
            id.as_ref()
                .map(codex_protocol::ResponseItemId::as_str)
                .unwrap_or_default(),
        )),
        ResponseItem::FunctionCall { call_id, .. } => {
            Some((CapPartialFragmentKind::ToolArguments, call_id))
        }
        ResponseItem::CustomToolCall { call_id, .. } => {
            Some((CapPartialFragmentKind::ToolArguments, call_id))
        }
        _ => None,
    }
}

fn item_id_for(kind: CapPartialFragmentKind, key: &str) -> Option<String> {
    match kind {
        CapPartialFragmentKind::ToolArguments => Some(key.to_string()),
        _ => (!key.is_empty()).then(|| key.to_string()),
    }
}

/// Returns a prefix cut at a UTF-8 boundary when `text` exceeds `max_bytes`,
/// or `None` when the whole text fits.
fn prefix_at_char_boundary(text: &str, max_bytes: usize) -> Option<&str> {
    if text.len() <= max_bytes {
        return None;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(&text[..end])
}

#[cfg(test)]
#[path = "cap_partial_tests.rs"]
mod tests;
