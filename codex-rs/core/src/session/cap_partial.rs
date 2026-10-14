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
//! 32 fragments per sampling, prefix-keep at UTF-8 boundaries. Identity and
//! serialization totals (D1-d) bound what those text budgets did not cover:
//! each provider identity string up to 256 bytes (over-budget identities
//! drop whole, never truncated into aliased keys), 8 KiB of stored identity
//! bytes per event, and a 128 KiB ceiling on the event's JSON serialization
//! accounted on the escaped form as text is stored. The finished
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
/// Per-identity budget for provider-minted streaming identities (item ids,
/// tool call ids, summary section keys). An identity over this budget is
/// dropped whole and never stored or keyed: truncating it into a stored key
/// could alias two distinct identities that share a prefix.
const MAX_IDENTITY_BYTES: usize = 256;
/// Total raw bytes of stored identity strings (fragment keys plus item ids)
/// in one event.
const MAX_EVENT_METADATA_BYTES: usize = 8 * 1024;
/// Hard ceiling on the event's JSON serialization: escaped fragment text
/// plus escaped identities plus the structural envelope.
const MAX_EVENT_SERIALIZED_BYTES: usize = 128 * 1024;
/// Serialization headroom reserved for structural fields — field names,
/// punctuation, the budget block, and a full usage report — across the
/// worst case of 32 fragments. Also covers both host keys: each admitted
/// 256-byte key can expand to 1,536 escaped bytes. Fragment identities and
/// text are accounted separately and do not draw from this reserve.
const EVENT_ENVELOPE_BYTES: usize = 8 * 1024;

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
    /// Raw bytes of stored identity strings, against the event metadata
    /// total.
    identity_bytes: usize,
    /// Escaped-JSON bytes the stored identities contribute to the event
    /// serialization total.
    serialized_identity_bytes: usize,
    /// Escaped-JSON bytes the stored fragment text contributes to the event
    /// serialization total.
    serialized_text_bytes: usize,
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
        let index = match self.position_for_new(kind, key, key) {
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
        let Some(index) = self.position_for_new(kind, key, key) else {
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
                    if let codex_protocol::models::ContentItem::OutputText { text, .. } = part {
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
        // Gate the item id before composing the section key, so an
        // over-budget id never reaches the `format!`.
        if item_id.len() > MAX_IDENTITY_BYTES {
            self.record_once(CapBudgetKind::IdentityBytes);
            return;
        }
        let key = format!("{item_id}:summary:{summary_index}");
        if self.positions.contains_key(&key) {
            return;
        }
        // Admit the actual key and parent id together. Charging the section
        // key twice can reject a summary that still fits the metadata budget.
        let Some(index) = self.position_for_new(CapPartialFragmentKind::Reasoning, &key, item_id)
        else {
            return;
        };
        self.append_bounded(index, text);
        self.positions.entry(item_id.to_string()).or_insert(index);
    }

    /// Finishes the capture for a sampling that exhausted its cap, or
    /// `None` when nothing assistant-produced preceded the cap.
    pub(crate) fn finish(
        mut self,
        turn_id: &str,
        response_key: &str,
        reported_usage: Option<&ReportedResponseUsage>,
    ) -> Option<CapPartialEvent> {
        if self.fragments.is_empty() {
            return None;
        }
        // Host-minted keys are bounded by construction (UUID-derived); if
        // one ever exceeds the identity budget it is emitted empty — never
        // truncated, which could alias a different key sharing the prefix —
        // and the drop stays observable in the enforced caps.
        let turn_id = if turn_id.len() > MAX_IDENTITY_BYTES {
            self.record_once(CapBudgetKind::IdentityBytes);
            String::new()
        } else {
            turn_id.to_string()
        };
        let response_key = if response_key.len() > MAX_IDENTITY_BYTES {
            self.record_once(CapBudgetKind::IdentityBytes);
            String::new()
        } else {
            response_key.to_string()
        };
        Some(CapPartialEvent {
            turn_id,
            response_key,
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

    fn position_for_new(
        &mut self,
        kind: CapPartialFragmentKind,
        key: &str,
        item_id: &str,
    ) -> Option<usize> {
        if let Some(&index) = self.positions.get(key) {
            return Some(index);
        }
        if self.fragments.len() >= MAX_FRAGMENTS {
            self.record_once(CapBudgetKind::TurnFragments);
            self.closed = Some(CapBudgetKind::TurnFragments);
            return None;
        }
        // Identity gate: an over-budget identity is refused whole — never
        // truncated into storage — while the capture stays open for other
        // identities. Checked before any clone of the key.
        if key.len() > MAX_IDENTITY_BYTES {
            self.record_once(CapBudgetKind::IdentityBytes);
            return None;
        }
        let item_id = match kind {
            CapPartialFragmentKind::ToolArguments => Some(item_id),
            CapPartialFragmentKind::AssistantText | CapPartialFragmentKind::Reasoning => {
                (!item_id.is_empty()).then_some(item_id)
            }
        };
        let identity_delta = key.len() + item_id.map_or(0, str::len);
        let serialized_delta = json_escaped_len(key) + item_id.map_or(0, json_escaped_len);
        if self.identity_bytes + identity_delta > MAX_EVENT_METADATA_BYTES {
            self.record_once(CapBudgetKind::MetadataBytes);
            self.closed = Some(CapBudgetKind::MetadataBytes);
            return None;
        }
        if self.serialized_identity_bytes
            + self.serialized_text_bytes
            + serialized_delta
            + EVENT_ENVELOPE_BYTES
            > MAX_EVENT_SERIALIZED_BYTES
        {
            self.record_once(CapBudgetKind::EventBytes);
            self.closed = Some(CapBudgetKind::EventBytes);
            return None;
        }
        let index = self.fragments.len();
        self.fragments.push(CapPartialFragment {
            fragment_key: key.to_string(),
            kind,
            item_id: item_id.map(str::to_owned),
            text: String::new(),
            truncated: false,
        });
        self.stored_bytes.push(0);
        self.identity_bytes += identity_delta;
        self.serialized_identity_bytes += serialized_delta;
        self.positions.insert(key.to_string(), index);
        Some(index)
    }

    /// Appends `text` to fragment `index`, keeping at most the fragment and
    /// turn byte budgets plus the event serialization total, dropping
    /// everything past them.
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
        let raw_binding = fragment_remaining.min(turn_remaining);
        let candidate = match prefix_at_char_boundary(text, raw_binding) {
            Some(prefix) => prefix,
            None => text,
        };
        // The serialization total applies to the escaped form, so a delta of
        // control characters can bind earlier than the raw budgets.
        let serialized_remaining = MAX_EVENT_SERIALIZED_BYTES
            .saturating_sub(EVENT_ENVELOPE_BYTES)
            .saturating_sub(self.serialized_identity_bytes)
            .saturating_sub(self.serialized_text_bytes);
        let (stored, serialized_fit) =
            match escaped_prefix_at_char_boundary(candidate, serialized_remaining) {
                Some(prefix) => (prefix, false),
                None => (candidate, true),
            };
        let fits_raw = candidate.len() == text.len();
        self.fragments[index].text.push_str(stored);
        self.stored_bytes[index] += stored.len();
        self.total_bytes += stored.len();
        self.serialized_text_bytes += json_escaped_len(stored);
        if !fits_raw {
            self.fragments[index].truncated = true;
            // The smaller raw remaining budget is the one the text exceeded.
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
        if !serialized_fit {
            self.fragments[index].truncated = true;
            self.record_once(CapBudgetKind::EventBytes);
            self.closed = Some(CapBudgetKind::EventBytes);
        }
        if self.total_bytes >= MAX_TURN_BYTES {
            self.closed = Some(CapBudgetKind::TurnBytes);
            // Closing on the turn budget is itself an enforced cap, even
            // when no further content ever arrives to record the drop.
            self.record_once(CapBudgetKind::TurnBytes);
        }
        if self.serialized_identity_bytes + self.serialized_text_bytes + EVENT_ENVELOPE_BYTES
            >= MAX_EVENT_SERIALIZED_BYTES
        {
            self.closed = Some(CapBudgetKind::EventBytes);
            self.record_once(CapBudgetKind::EventBytes);
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

/// Escaped size `text` occupies inside a serde_json string literal. Matches
/// serde_json's table exactly: quotes and backslashes double, the five
/// named control escapes double, other control characters expand to
/// `\u00XX`, and non-ASCII bytes pass through unescaped.
fn json_escaped_len(text: &str) -> usize {
    text.bytes()
        .map(|byte| match byte {
            b'"' | b'\\' => 2,
            0x08 | 0x09 | 0x0A | 0x0C | 0x0D => 2,
            0x00..=0x1F => 6,
            _ => 1,
        })
        .sum()
}

/// Returns the longest prefix of `text` whose escaped JSON size fits
/// `max_escaped`, cut at a char boundary, or `None` when the whole text
/// fits.
fn escaped_prefix_at_char_boundary(text: &str, max_escaped: usize) -> Option<&str> {
    let mut escaped = 0usize;
    let mut end = 0usize;
    for (index, byte) in text.bytes().enumerate() {
        let cost = match byte {
            b'"' | b'\\' => 2,
            0x08 | 0x09 | 0x0A | 0x0C | 0x0D => 2,
            0x00..=0x1F => 6,
            _ => 1,
        };
        if escaped + cost > max_escaped {
            break;
        }
        escaped += cost;
        end = index + 1;
    }
    if end == text.len() {
        return None;
    }
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(&text[..end])
}

#[cfg(test)]
#[path = "cap_partial_tests.rs"]
mod tests;
