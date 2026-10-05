//! Verify the wire terminal before allowing Rig to publish its final record.
use crate::RigProtocol;
use bytes::Bytes;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use rig_core::http_client::Error;
use rig_core::http_client::sse::BoxedStream;

pub(crate) fn with_terminal_check(
    body: BoxedStream,
    protocol: RigProtocol,
    usage: std::sync::Arc<std::sync::Mutex<crate::usage::AnthropicUsage>>,
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
                        return Some((Err(error), (frames, true, None, usage, false)));
                    }
                    Some(Err(error)) => {
                        return Some((
                            Err(Error::Instance(Box::new(error))),
                            (frames, true, None, usage, false),
                        ));
                    }
                    None => {
                        let error = std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "model stream ended before its SSE terminal",
                        );
                        return Some((
                            Err(Error::Instance(Box::new(error))),
                            (frames, true, None, usage, false),
                        ));
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
                            // and EOF still fails unless [DONE] arrives.
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
                        let value: serde_json::Value = match serde_json::from_str(&event.data) {
                            Ok(value) => value,
                            Err(error) => {
                                return Some((
                                    Err(Error::Instance(Box::new(error))),
                                    (frames, true, None, usage, false),
                                ));
                            }
                        };
                        let observed = usage.lock().map(|mut usage| usage.observe(&value)).is_ok();
                        if !observed {
                            let error =
                                std::io::Error::other("Anthropic usage state is unavailable");
                            return Some((
                                Err(Error::Instance(Box::new(error))),
                                (frames, true, None, usage, false),
                            ));
                        }
                        match value["type"].as_str() {
                            Some("message_delta") if !value["delta"]["stop_reason"].is_null() => {
                                if pending_terminal.replace(encoded).is_some() {
                                    let error =
                                        std::io::Error::other("duplicate Anthropic terminal delta");
                                    return Some((
                                        Err(Error::Instance(Box::new(error))),
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
                            Err(Error::Instance(Box::new(error))),
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
