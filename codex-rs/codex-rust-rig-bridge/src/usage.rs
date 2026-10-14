//! Retain Anthropic's cumulative usage across message_start/message_delta.
//! Rig 0.42 drops optional counters absent from the terminal delta and treats
//! an explicit zero input count as missing. Correct its normalized final usage
//! from the wire before recording or converting that final event.
use rig_core::completion::Usage;
use serde_json::Value;

#[derive(Default)]
pub(crate) struct AnthropicUsage {
    input: Option<u64>,
    output: Option<u64>,
    cached: Option<u64>,
    cache_creation: Option<u64>,
    reasoning: Option<u64>,
}

impl AnthropicUsage {
    pub(crate) fn observe(&mut self, event: &Value) {
        let usage = match event["type"].as_str() {
            Some("message_start") => {
                *self = Self::default();
                &event["message"]["usage"]
            }
            Some("message_delta") => &event["usage"],
            _ => return,
        };
        for (counter, name) in [
            (&mut self.input, "input_tokens"),
            (&mut self.output, "output_tokens"),
            (&mut self.cached, "cache_read_input_tokens"),
            (&mut self.cache_creation, "cache_creation_input_tokens"),
        ] {
            // Missing/null means unchanged; a reported zero is authoritative.
            if let Some(value) = usage[name].as_u64() {
                *counter = Some(value);
            }
        }
        if let Some(value) = usage["output_tokens_details"]
            .get("thinking_tokens")
            .and_then(Value::as_u64)
        {
            self.reasoning = Some(value);
        }
    }

    /// Snapshots the observed counters as a presence-preserving report.
    ///
    /// Returns `None` when nothing was observed. The Anthropic wire never
    /// reports a total, so the report is at best `Incomplete`; explicit zeros
    /// observed on the wire stay zeros here.
    pub(crate) fn snapshot_report(
        &self,
    ) -> Option<codex_protocol::protocol::ReportedResponseUsage> {
        let counts = codex_protocol::protocol::ReportedUsageCounters {
            input_tokens: self.input.and_then(|value| i64::try_from(value).ok()),
            cached_input_tokens: self.cached.and_then(|value| i64::try_from(value).ok()),
            cache_write_input_tokens: self
                .cache_creation
                .and_then(|value| i64::try_from(value).ok()),
            output_tokens: self.output.and_then(|value| i64::try_from(value).ok()),
            reasoning_output_tokens: self.reasoning.and_then(|value| i64::try_from(value).ok()),
            total_tokens: None,
        };
        codex_protocol::protocol::ReportedResponseUsage::from_counters(counts)
    }

    pub(crate) fn apply(&self, usage: &mut Usage) {
        for (target, value) in [
            (&mut usage.input_tokens, self.input),
            (&mut usage.output_tokens, self.output),
            (&mut usage.cached_input_tokens, self.cached),
            (&mut usage.cache_creation_input_tokens, self.cache_creation),
            (&mut usage.reasoning_tokens, self.reasoning),
        ] {
            if let Some(value) = value {
                *target = Some(value);
            }
        }
        // 0.44's `input_tokens` already includes cached and cache-written
        // input, so the total never re-adds them.
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
    /// (absent or null) leave the previous snapshot unchanged.
    pub(crate) fn observe(&mut self, value: &Value) {
        let usage = &value["usage"];
        if !usage.is_object() {
            return;
        }
        *self = Self {
            input: usage["prompt_tokens"].as_u64(),
            output: usage["completion_tokens"].as_u64(),
            cached: usage["prompt_tokens_details"]["cached_tokens"].as_u64(),
            reasoning: usage["completion_tokens_details"]["reasoning_tokens"].as_u64(),
            total: usage["total_tokens"].as_u64(),
        };
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
