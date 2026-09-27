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
        if let Some(details) = usage["output_tokens_details"].as_object() {
            self.reasoning = Some(
                details
                    .get("thinking_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            );
        }
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
                *target = value;
            }
        }
        usage.total_tokens = usage
            .input_tokens
            .saturating_add(usage.cached_input_tokens)
            .saturating_add(usage.cache_creation_input_tokens)
            .saturating_add(usage.output_tokens);
    }
}
