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
        (Box::pin(body.eventsource()), false, None::<String>, usage),
        move |(mut frames, done, mut pending_terminal, usage)| async move {
            if done {
                return None;
            }
            loop {
                let event = match frames.next().await {
                    Some(Ok(event)) => event,
                    Some(Err(eventsource_stream::EventStreamError::Transport(error))) => {
                        return Some((Err(error), (frames, true, None, usage)));
                    }
                    Some(Err(error)) => {
                        return Some((
                            Err(Error::Instance(Box::new(error))),
                            (frames, true, None, usage),
                        ));
                    }
                    None => {
                        let error = std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "model stream ended before its SSE terminal",
                        );
                        return Some((
                            Err(Error::Instance(Box::new(error))),
                            (frames, true, None, usage),
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
                    RigProtocol::Chat => event.data == "[DONE]",
                    RigProtocol::Anthropic => {
                        let value: serde_json::Value = match serde_json::from_str(&event.data) {
                            Ok(value) => value,
                            Err(error) => {
                                return Some((
                                    Err(Error::Instance(Box::new(error))),
                                    (frames, true, None, usage),
                                ));
                            }
                        };
                        let observed = usage.lock().map(|mut usage| usage.observe(&value)).is_ok();
                        if !observed {
                            let error =
                                std::io::Error::other("Anthropic usage state is unavailable");
                            return Some((
                                Err(Error::Instance(Box::new(error))),
                                (frames, true, None, usage),
                            ));
                        }
                        match value["type"].as_str() {
                            Some("message_delta") if !value["delta"]["stop_reason"].is_null() => {
                                if pending_terminal.replace(encoded).is_some() {
                                    let error =
                                        std::io::Error::other("duplicate Anthropic terminal delta");
                                    return Some((
                                        Err(Error::Instance(Box::new(error))),
                                        (frames, true, None, usage),
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
                            (frames, true, None, usage),
                        ));
                    }
                };
                return Some((
                    Ok(Bytes::from(encoded)),
                    (frames, done, pending_terminal, usage),
                ));
            }
        },
    ))
}
