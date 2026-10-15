//! Retain Anthropic's cumulative usage across message_start/message_delta.
//! Rig 0.42 drops optional counters absent from the terminal delta and treats
//! an explicit zero input count as missing. Correct its normalized final usage
//! from the wire before recording or converting that final event.
//!
//! The observers are also the fork's fail-fast usage contract: rig 0.44's own
//! decoders read counters leniently (`as_u64`, absent = unknown), so a
//! malformed or missing usage frame would silently become a default terminal
//! instead of the retryable stream error the fork has always surfaced. The
//! observers restore that strictness at the wire boundary rig sits behind.
use rig_core::completion::Usage;
use serde_json::Value;

/// A counter that is present must be a valid unsigned integer.
fn require_counter(object: &Value, name: &str, frame: &str) -> Result<Option<u64>, String> {
    let value = &object[name];
    if value.is_null() {
        return Ok(None);
    }
    value.as_u64().map(Some).ok_or_else(|| {
        format!("invalid value: integer `{value}`, expected u64 at `{name}` of the {frame}")
    })
}

/// The usage object a frame must carry: present and an object.
fn require_usage<'a>(value: &'a Value, frame: &str) -> Result<&'a Value, String> {
    if value.is_null() {
        return Err(format!("missing field `usage` at the {frame}"));
    }
    if !value.is_object() {
        return Err(format!(
            "invalid type for `usage` at the {frame}: expected an object"
        ));
    }
    Ok(value)
}

#[derive(Default)]
pub(crate) struct AnthropicUsage {
    input: Option<u64>,
    output: Option<u64>,
    cached: Option<u64>,
    cache_creation: Option<u64>,
    reasoning: Option<u64>,
}

impl AnthropicUsage {
    pub(crate) fn observe(&mut self, event: &Value) -> Result<(), String> {
        let (usage, frame) = match event["type"].as_str() {
            Some("message_start") => {
                *self = Self::default();
                (&event["message"]["usage"], "Anthropic message_start frame")
            }
            Some("message_delta") => (&event["usage"], "Anthropic message_delta frame"),
            _ => return Ok(()),
        };
        let usage = require_usage(usage, frame)?;
        for (counter, name) in [
            (&mut self.input, "input_tokens"),
            (&mut self.output, "output_tokens"),
            (&mut self.cached, "cache_read_input_tokens"),
            (&mut self.cache_creation, "cache_creation_input_tokens"),
        ] {
            // Missing/null means unchanged; a reported zero is authoritative.
            if let Some(value) = require_counter(usage, name, frame)? {
                *counter = Some(value);
            }
        }
        if let Some(details) = usage
            .get("output_tokens_details")
            .filter(|details| details.is_object())
            && let Some(value) = require_counter(details, "thinking_tokens", frame)?
        {
            self.reasoning = Some(value);
        }
        Ok(())
    }

    /// Snapshots the observed counters as a presence-preserving report.
    ///
    /// Returns `None` when nothing was observed. The Anthropic wire never
    /// reports a total, so the report is at best `Incomplete`; explicit zeros
    /// observed on the wire stay zeros here. `input_tokens` is reported
    /// INCLUSIVE of cache read/write — the same semantics `apply` writes and
    /// `map_usage` publishes on the Completed path, so a cap terminal and a
    /// completion of identical wire data agree.
    pub(crate) fn snapshot_report(
        &self,
    ) -> Option<codex_protocol::protocol::ReportedResponseUsage> {
        let counter = |value: Option<u64>| value.and_then(|value| i64::try_from(value).ok());
        let input = self
            .input
            .map(|input| {
                input
                    .saturating_add(self.cached.unwrap_or_default())
                    .saturating_add(self.cache_creation.unwrap_or_default())
            })
            .and_then(|input| i64::try_from(input).ok());
        let counts = codex_protocol::protocol::ReportedUsageCounters {
            input_tokens: input,
            cached_input_tokens: counter(self.cached),
            cache_write_input_tokens: counter(self.cache_creation),
            output_tokens: counter(self.output),
            reasoning_output_tokens: counter(self.reasoning),
            total_tokens: None,
        };
        codex_protocol::protocol::ReportedResponseUsage::from_counters(counts)
    }

    pub(crate) fn apply(&self, usage: &mut Usage) {
        // The accumulator observes every wire usage frame (message_start,
        // intermediate deltas, terminal delta) and is authoritative: a
        // counter explicitly reported in a later frame — including an
        // explicit zero — overwrites the prior value, while null/absent
        // fields leave it alone. Rig 0.44's own fold only keeps the start
        // and terminal frames, so its values cannot be trusted here.
        // `input_tokens` is written INCLUSIVE of cache read/write, matching
        // rig 0.44's `Usage` semantics; the wire field alone is the
        // uncached count.
        if let Some(input) = self.input {
            let inclusive = input
                .saturating_add(self.cached.unwrap_or_default())
                .saturating_add(self.cache_creation.unwrap_or_default());
            usage.input_tokens = Some(inclusive);
        }
        for (target, value) in [
            (&mut usage.output_tokens, self.output),
            (&mut usage.cached_input_tokens, self.cached),
            (&mut usage.cache_creation_input_tokens, self.cache_creation),
            (&mut usage.reasoning_tokens, self.reasoning),
        ] {
            if let Some(value) = value {
                *target = Some(value);
            }
        }
        if let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens) {
            usage.total_tokens = Some(input.saturating_add(output));
        }
    }
}

/// Retains the Chat wire's terminal usage object with per-counter presence.
///
/// A Chat frame may carry a complete `usage` object (the terminal chunk, or a
/// trailing usage-only chunk with empty `choices`); each such object replaces
/// the previous snapshot rather than accumulating. Rig's normalized `Usage`
/// cannot distinguish an unreported counter from an explicit zero, so the
/// cap terminal's usage report reads these wire-observed counters instead.
#[derive(Default)]
pub(crate) struct ChatUsage {
    input: Option<u64>,
    output: Option<u64>,
    cached: Option<u64>,
    reasoning: Option<u64>,
    total: Option<u64>,
}

impl ChatUsage {
    /// Observes one Chat frame; frames without a usable `usage` object
    /// (absent or null) leave the previous snapshot unchanged. A present
    /// object with malformed counters fails the turn.
    pub(crate) fn observe(&mut self, value: &Value) -> Result<(), String> {
        let usage = &value["usage"];
        if usage.is_null() {
            return Ok(());
        }
        let usage = require_usage(usage, "Chat usage object")?;
        *self = Self {
            input: require_counter(usage, "prompt_tokens", "Chat usage object")?,
            output: require_counter(usage, "completion_tokens", "Chat usage object")?,
            cached: require_counter(
                usage
                    .get("prompt_tokens_details")
                    .filter(|details| details.is_object())
                    .unwrap_or(&Value::Null),
                "cached_tokens",
                "Chat usage object",
            )?,
            reasoning: require_counter(
                usage
                    .get("completion_tokens_details")
                    .filter(|details| details.is_object())
                    .unwrap_or(&Value::Null),
                "reasoning_tokens",
                "Chat usage object",
            )?,
            total: require_counter(usage, "total_tokens", "Chat usage object")?,
        };
        Ok(())
    }

    /// Snapshots the observed counters as a presence-preserving report.
    ///
    /// Returns `None` when nothing was observed. Explicit zeros observed on
    /// the wire stay zeros here.
    pub(crate) fn snapshot_report(
        &self,
    ) -> Option<codex_protocol::protocol::ReportedResponseUsage> {
        let counts = codex_protocol::protocol::ReportedUsageCounters {
            input_tokens: self.input.and_then(|value| i64::try_from(value).ok()),
            cached_input_tokens: self.cached.and_then(|value| i64::try_from(value).ok()),
            cache_write_input_tokens: None,
            output_tokens: self.output.and_then(|value| i64::try_from(value).ok()),
            reasoning_output_tokens: self.reasoning.and_then(|value| i64::try_from(value).ok()),
            total_tokens: self.total.and_then(|value| i64::try_from(value).ok()),
        };
        codex_protocol::protocol::ReportedResponseUsage::from_counters(counts)
    }
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
