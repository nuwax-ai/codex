//! Observe the frames the bridge's usage report needs, and hold Anthropic's
//! terminal in order. Chat's terminal policy stays rig's: a reply ends at its
//! `finish_reason`, which lets a sentinel-less dialect — MiniMax-M3 sends no
//! `[DONE]` — complete while a wire cut before any reason still fails as
//! truncation.
use crate::RigProtocol;
use bytes::Bytes;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use rig_core::http_client::BoxedStream;
use rig_core::http_client::Error;
use std::sync::Arc;
use std::sync::Mutex;

/// The per-protocol wire usage accumulator a terminal check feeds.
pub(crate) enum WireUsageObserver {
    Anthropic(Arc<Mutex<crate::usage::AnthropicUsage>>),
    Chat(Arc<Mutex<crate::usage::ChatUsage>>),
}

impl WireUsageObserver {
    /// Observes one decoded frame. `Err` carries the fail-fast usage message
    /// (malformed or missing counters) or reports poisoned shared state.
    fn observe(&self, value: &serde_json::Value) -> Result<(), String> {
        match self {
            Self::Anthropic(usage) => usage
                .lock()
                .map_err(|_| "Anthropic usage state is unavailable".to_string())
                .and_then(|mut usage| usage.observe(value)),
            Self::Chat(usage) => usage
                .lock()
                .map_err(|_| "Chat usage state is unavailable".to_string())
                .and_then(|mut usage| usage.observe(value)),
        }
    }
}

pub(crate) fn with_terminal_check(
    body: BoxedStream,
    protocol: RigProtocol,
    usage: WireUsageObserver,
) -> BoxedStream {
    Box::pin(futures::stream::unfold(
        (
            Box::pin(body.eventsource()),
            false,
            None::<String>,
            usage,
            false,
        ),
        move |(mut frames, done, mut pending_terminal, usage, mut chat_terminal_seen)| async move {
            if done {
                return None;
            }
            loop {
                let event = match frames.next().await {
                    Some(Ok(event)) => event,
                    Some(Err(eventsource_stream::EventStreamError::Transport(error))) => {
                        return Some((
                            Err(Error::transient(error)),
                            (frames, true, None, usage, false),
                        ));
                    }
                    Some(Err(error)) => {
                        return Some((
                            Err(Error::permanent(error)),
                            (frames, true, None, usage, false),
                        ));
                    }
                    None => {
                        // Chat's exhausted frame stream reaches rig as-is: its
                        // decoder ends a reply whose `finish_reason` arrived
                        // and answers `ProviderError::Truncated` for one that
                        // never did. Requiring `[DONE]` here pre-empted that
                        // and made every sentinel-less gateway a truncation,
                        // even though the provider had delivered its terminal.
                        if protocol == RigProtocol::Chat {
                            return None;
                        }
                        return Some((Err(Error::StreamEnded), (frames, true, None, usage, false)));
                    }
                };
                // Parse actual SSE frames, including split UTF-8, CRLF and
                // multiline data; a sentinel inside generated text is inert.
                let mut encoded = format!("event: {}\nid: {}\n", event.event, event.id);
                for line in event.data.split('\n') {
                    encoded.push_str("data: ");
                    encoded.push_str(line);
                    encoded.push('\n');
                }
                encoded.push('\n');
                let done = match protocol {
                    RigProtocol::Chat => {
                        if event.data == "[DONE]" {
                            true
                        } else {
                            // Match Rig's primary-choice selection and its
                            // empty-string-as-absent finish_reason policy.
                            // Unknown/corrupt frames still reach Rig's own
                            // classifier instead of becoming a fake terminal.
                            let value = serde_json::from_str::<serde_json::Value>(&event.data).ok();
                            // A Chat frame may carry a complete usage object
                            // (the terminal chunk, or a trailing usage-only
                            // chunk): retain its counters with wire presence.
                            // A present-but-malformed usage value fails the
                            // stream here — rig's own lenient reader would
                            // silently fold a default terminal instead.
                            if let Some(value) = value.as_ref()
                                && !value["usage"].is_null()
                                && let Err(message) = usage.observe(value)
                            {
                                let error = std::io::Error::other(message);
                                return Some((
                                    Err(Error::permanent(error)),
                                    (frames, true, None, usage, false),
                                ));
                            }
                            let primary = value
                                .as_ref()
                                .and_then(|value| value["choices"].as_array())
                                .and_then(|choices| {
                                    choices.iter().find(|choice| {
                                        choice["index"].is_null()
                                            || choice["index"].as_u64() == Some(0)
                                    })
                                });
                            // First terminal wins for all subsequent primary
                            // content, including deltas without a finish reason.
                            // Usage-only frames (choices=[]) still reach Rig,
                            // and EOF reaches it too, which is why the flag
                            // only drops late primaries: the terminal decision
                            // itself is rig's.
                            if chat_terminal_seen && primary.is_some() {
                                tracing::warn!(
                                    "dropping a late Chat primary-choice chunk after the first terminal"
                                );
                                continue;
                            }
                            if primary.is_some_and(|choice| {
                                choice["finish_reason"]
                                    .as_str()
                                    .is_some_and(|reason| !reason.is_empty())
                            }) {
                                chat_terminal_seen = true;
                            }
                            false
                        }
                    }
                    RigProtocol::Anthropic => {
                        let mut value: serde_json::Value = match serde_json::from_str(&event.data) {
                            Ok(value) => value,
                            Err(error) => {
                                return Some((
                                    Err(Error::permanent(error)),
                                    (frames, true, None, usage, false),
                                ));
                            }
                        };
                        // Missing usage on a required frame, or a malformed
                        // counter, fails the stream here: rig's lenient
                        // reader would fold a default terminal instead of the
                        // retryable failure the fork contract promises.
                        if let Err(message) = usage.observe(&value) {
                            let error = std::io::Error::other(message);
                            return Some((
                                Err(Error::permanent(error)),
                                (frames, true, None, usage, false),
                            ));
                        }
                        // Rig 0.42 adds four Anthropic counters with unchecked
                        // u64 arithmetic. Bound only its decoder-facing copy,
                        // including message_start's input fallback. Original
                        // counters were observed above and are restored by
                        // AnthropicUsage::apply before bridge conversion; the
                        // raw tee and presence-preserving report stay intact.
                        let counters = match value["type"].as_str() {
                            Some("message_start") => value
                                .get_mut("message")
                                .and_then(|message| message.get_mut("usage")),
                            Some("message_delta") => value.get_mut("usage"),
                            _ => None,
                        };
                        let mut rewritten = false;
                        if let Some(counters) = counters {
                            for name in [
                                "input_tokens",
                                "output_tokens",
                                "cache_read_input_tokens",
                                "cache_creation_input_tokens",
                            ] {
                                if let Some(counter) = counters.get_mut(name)
                                    && counter.as_u64().is_some_and(|count| count > u64::MAX / 4)
                                {
                                    *counter = serde_json::Value::from(0);
                                    rewritten = true;
                                }
                            }
                        }
                        if rewritten {
                            encoded = format!(
                                "event: {}\nid: {}\ndata: {value}\n\n",
                                event.event, event.id
                            );
                        }
                        match value["type"].as_str() {
                            Some("message_delta") if !value["delta"]["stop_reason"].is_null() => {
                                if pending_terminal.replace(encoded).is_some() {
                                    let error =
                                        std::io::Error::other("duplicate Anthropic terminal delta");
                                    return Some((
                                        Err(Error::permanent(error)),
                                        (frames, true, None, usage, false),
                                    ));
                                }
                                // Rig 0.42 emits Final on this delta, before
                                // message_stop. Hold it to detect truncation
                                // or an intervening in-band error.
                                continue;
                            }
                            Some("message_stop") => {
                                if let Some(terminal) = pending_terminal.take() {
                                    encoded.insert_str(0, &terminal);
                                }
                                true
                            }
                            Some("error") => true,
                            _ => false,
                        }
                    }
                    // Exhaustiveness: the transport never routes the Responses
                    // passthrough through this check (raw bytes go straight to
                    // codex-api's decoder with its own strict terminal policy).
                    RigProtocol::Responses => {
                        let error = std::io::Error::other(
                            "with_terminal_check does not apply to the Responses passthrough",
                        );
                        return Some((
                            Err(Error::permanent(error)),
                            (frames, true, None, usage, false),
                        ));
                    }
                };
                return Some((
                    Ok(Bytes::from(encoded)),
                    (frames, done, pending_terminal, usage, chat_terminal_seen),
                ));
            }
        },
    ))
}

#[cfg(test)]
#[path = "sse_tests.rs"]
mod tests;
