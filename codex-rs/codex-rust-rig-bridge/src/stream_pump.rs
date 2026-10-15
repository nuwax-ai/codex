//! The bridge's stream pump: converts one rig streaming attempt into Codex
//! `ResponseEvent`s on a channel, re-reading the Anthropic wire tee at
//! terminal time for hosted-search blocks, and handing a paused attempt's
//! raw content to the continuation chainer. Extracted from
//! [`crate::stream::stream_via_rig_attempt`] so the attempt assembly and the
//! event pump stay independently reviewable.

use codex_api::ApiError;
use codex_api::ResponseEvent;
use codex_api::TransportError;
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use crate::RigEventRecorder;
use crate::RigProtocol;
use crate::convert_response::PendingRigMessage;
use crate::convert_response::StreamTerminal;
use crate::convert_response::rig_event_to_response_events;
use crate::stream::map_provider_error;

/// A paused turn continues bridge-internally at most this many times before
/// the pump surfaces the pause as an explicit error.
pub(crate) const PAUSE_CONTINUATION_LIMIT: u32 = 4;

/// Everything the pump needs from one assembled attempt. Field values are
/// moved into the spawned task; shared handles are the same `Arc`s the
/// attempt (or its chainer) observes.
pub(crate) struct PumpContext {
    pub(crate) protocol: RigProtocol,
    pub(crate) idle_timeout: Duration,
    pub(crate) pause_depth: u32,
    pub(crate) source: String,
    pub(crate) custom_tool_names: Arc<std::collections::HashSet<String>>,
    /// The eagerly prefetched first stream item (rig defers HTTP failures
    /// into the stream, so the attempt already waited for it).
    pub(crate) first_event: Option<
        Result<
            rig_core::streaming::Item<rig_core::streaming::StreamEvent>,
            rig_core::error::ProviderError,
        >,
    >,
    pub(crate) events_recorder: RigEventRecorder,
    /// Anthropic wire tee, present on every Anthropic attempt.
    pub(crate) sse_tee: Option<Arc<Mutex<Vec<u8>>>>,
    pub(crate) anthropic_usage: Arc<Mutex<crate::usage::AnthropicUsage>>,
    /// Chat wire usage accumulator, shared with the transport (see
    /// [`crate::usage::ChatUsage`]).
    pub(crate) chat_usage: Arc<Mutex<crate::usage::ChatUsage>>,
    /// Filled with the paused attempt's captured raw content for the chainer.
    pub(crate) paused_capture: Arc<Mutex<Option<crate::hosted_replay::PauseCapture>>>,
    /// Call blocks of still-pending (result-less) searches persisted in the
    /// request history, keyed by server call id.
    pub(crate) replay_calls: HashMap<String, serde_json::Value>,
}

/// Spawns the pump for one attempt. The returned task ends when the attempt
/// reaches a terminal state (Completed, pause handoff, error, truncation) or
/// the consumer drops the receiving stream.
pub(crate) fn spawn_pump(
    tx: tokio::sync::mpsc::Sender<Result<ResponseEvent, ApiError>>,
    mut rig_stream: rig_core::streaming::CompletionStream,
    context: PumpContext,
) -> tokio::task::JoinHandle<()> {
    let PumpContext {
        protocol,
        idle_timeout,
        pause_depth,
        source,
        custom_tool_names,
        first_event,
        events_recorder,
        sse_tee,
        anthropic_usage,
        chat_usage,
        paused_capture,
        replay_calls,
    } = context;
    let mut next_event = first_event;
    tokio::spawn(async move {
        let tx = tx;
        let pump_source = source.clone();
        let mut pending = PendingRigMessage::new(custom_tool_names, source);

        // rig streams have no start event; synthesize `Created` so the
        // event sequence matches the genai bridge (A/B parity) and any
        // consumer waiting for it sees one. Continuation attempts are part
        // of the SAME user-visible turn: exactly one Created per turn.
        if pause_depth == 0
            && tx
                .send(Ok(ResponseEvent::Created { response_id: None }))
                .await
                .is_err()
        {
            return;
        }

        loop {
            let item = match next_event.take() {
                Some(item) => Some(item),
                None => {
                    // Real cancellation, not an idle-timeout substitute:
                    // when the consumer drops the stream, stop waiting on
                    // the model immediately — dropping this future drops
                    // the in-flight request and releases its socket.
                    tokio::select! {
                        biased;
                        _ = tx.closed() => return,
                        item = tokio::time::timeout(idle_timeout, rig_stream.next()) => match item {
                            Ok(item) => item,
                            Err(_elapsed) => {
                                let _ = tx
                                    .send(Err(ApiError::Transport(TransportError::Timeout)))
                                    .await;
                                return;
                            }
                        },
                    }
                }
            };
            match item {
                Some(Ok(event)) => {
                    if let Some(rec) = &events_recorder
                        && let Ok(mut buf) = rec.lock()
                    {
                        buf.record_event(event.clone());
                    }
                    let mut events = match rig_event_to_response_events(event, &mut pending) {
                        Ok(events) => events,
                        Err(error) => {
                            let _ = tx.send(Err(error)).await;
                            return;
                        }
                    };
                    // rig's public streaming surface omits Anthropic
                    // server-tool blocks; re-read them from the teed wire
                    // bytes and splice their items in front of the Completed
                    // terminal. Copy the bytes out first so no lock is held
                    // across the await.
                    if pending.completed_emitted()
                        && let Some(tee) = &sse_tee
                        && let Some(sse_bytes) = tee.lock().ok().map(|bytes| bytes.clone())
                        && !events.is_empty()
                    {
                        let layout = match crate::hosted_replay::raw_indexed_assistant_content(
                            &sse_bytes,
                        )
                        .await
                        {
                            Ok(raw) => {
                                crate::hosted_tools::response_layout(&raw, pending.segments())
                            }
                            Err(error) => {
                                tracing::warn!(%error, "hosted layout capture failed; replaying response-scoped pairs only");
                                None
                            }
                        };
                        let injected = crate::hosted_tools::captured_search_events(
                            &sse_bytes,
                            pump_source.as_str(),
                            pending.response_id(),
                            &replay_calls,
                            layout.as_deref(),
                        )
                        .await;
                        if !injected.is_empty() {
                            let terminal = events.split_off(events.len() - 1);
                            events.extend(injected);
                            events.extend(terminal);
                        }
                    }
                    for ev in events {
                        if tx.send(Ok(ev)).await.is_err() {
                            return;
                        }
                    }
                    if pending.completed_emitted() {
                        return;
                    }
                }
                Some(Err(e)) => {
                    // rig's contract: a malformed frame surfaces as Err but
                    // the stream may continue; only a transport error is
                    // terminal. Forward the error and stop — codex's retry
                    // machinery handles reattempts.
                    tracing::error!(error = %e, "rig stream error");
                    let _ = tx.send(Err(map_provider_error(e))).await;
                    return;
                }
                None => break,
            }
        }

        // The stream ended without an in-stream terminal: rig 0.44 folds
        // the provider's end into a response via `finish()`.
        let mut response = match rig_stream.finish().await {
            Ok(response) => response,
            Err(error) => {
                // A reply the provider did not end is truncation, never a
                // successful completion.
                let _ = tx.send(Err(map_provider_error(error))).await;
                return;
            }
        };

        let mut paused = false;
        if protocol == RigProtocol::Anthropic {
            let normalized = anthropic_usage
                .lock()
                .map(|usage| usage.apply(&mut response.usage))
                .map_err(|_| ApiError::Stream("Anthropic usage state is unavailable".into()));
            if let Err(error) = normalized {
                let _ = tx.send(Err(error)).await;
                return;
            }
            if let Some(tee) = &sse_tee
                && let Some(sse_bytes) = tee.lock().ok().map(|bytes| bytes.clone())
                && let Some(reason) = anthropic_stop_reason(&sse_bytes)
            {
                match reason.as_str() {
                    "pause_turn" => paused = true,
                    // Rig 0.44 folds both context exhaustion and max_tokens
                    // into Length. Preserve the wire distinction so core can
                    // recover by compacting rather than increasing the cap.
                    "model_context_window_exceeded" => {
                        response = response
                            .with_finish_reason(rig_core::completion::FinishReason::Other(reason));
                    }
                    _ => {}
                }
            }
        }

        // Record the terminal AFTER wire-counter correction so a cassette
        // replay reproduces the corrected usage the live stream emitted.
        if let Some(rec) = &events_recorder
            && let Ok(mut buf) = rec.lock()
        {
            buf.record_terminal(&stream_terminal_of(&response));
        }

        // Fork (nuwax-codex) D3: a paused turn continues bridge-internally —
        // flush this attempt's content WITHOUT a Completed terminal and hand
        // the raw wire blocks to the chainer for the official re-send recipe.
        if paused {
            if pause_depth >= PAUSE_CONTINUATION_LIMIT {
                let _ = tx
                    .send(Err(ApiError::Stream(format!(
                        "Anthropic turn kept pausing (pause_turn) beyond the \
                         continuation cap of {PAUSE_CONTINUATION_LIMIT}"
                    ))))
                    .await;
                return;
            }
            let terminal = stream_terminal_of(&response);
            let mut events =
                match crate::convert_response::paused_final_events(terminal, &mut pending) {
                    Ok(events) => events,
                    Err(error) => {
                        let _ = tx.send(Err(error)).await;
                        return;
                    }
                };
            let mut capture = crate::hosted_replay::PauseCapture {
                items: events
                    .iter()
                    .filter_map(|event| match event {
                        ResponseEvent::OutputItemDone(item) => Some(item.clone()),
                        _ => None,
                    })
                    .collect(),
                raw_content: Vec::new(),
            };
            if let Some(tee) = &sse_tee
                && let Some(sse_bytes) = tee.lock().ok().map(|bytes| bytes.clone())
            {
                let raw =
                    match crate::hosted_replay::raw_indexed_assistant_content(&sse_bytes).await {
                        Ok(raw) => raw,
                        Err(error) => {
                            let _ = tx.send(Err(error)).await;
                            return;
                        }
                    };
                let layout = crate::hosted_tools::response_layout(&raw, pending.segments());
                let injected = crate::hosted_tools::captured_search_events(
                    &sse_bytes,
                    pump_source.as_str(),
                    pending.response_id(),
                    &replay_calls,
                    layout.as_deref(),
                )
                .await;
                capture
                    .items
                    .extend(injected.iter().filter_map(|event| match event {
                        ResponseEvent::OutputItemDone(item) => Some(item.clone()),
                        _ => None,
                    }));
                capture.raw_content = raw.into_iter().map(|(_, block)| block).collect();
                events.extend(injected);
            }
            if let Ok(mut slot) = paused_capture.lock() {
                *slot = Some(capture);
            }
            for ev in events {
                if tx.send(Ok(ev)).await.is_err() {
                    return;
                }
            }
            return;
        }

        let report = match protocol {
            RigProtocol::Anthropic => anthropic_usage
                .lock()
                .map(|usage| usage.snapshot_report())
                .ok()
                .flatten(),
            RigProtocol::Chat => chat_usage
                .lock()
                .map(|usage| usage.snapshot_report())
                .ok()
                .flatten(),
            RigProtocol::Responses => None,
        };
        pending.wire_reported_usage = report;
        let terminal = stream_terminal_of(&response);
        let mut events =
            match crate::convert_response::handle_stream_terminal(terminal, &mut pending) {
                Ok(events) => events,
                Err(error) => {
                    let _ = tx.send(Err(error)).await;
                    return;
                }
            };
        if pending.completed_emitted()
            && let Some(tee) = &sse_tee
            && let Some(sse_bytes) = tee.lock().ok().map(|bytes| bytes.clone())
            && !events.is_empty()
        {
            let layout = match crate::hosted_replay::raw_indexed_assistant_content(&sse_bytes).await
            {
                Ok(raw) => crate::hosted_tools::response_layout(&raw, pending.segments()),
                Err(error) => {
                    tracing::warn!(%error, "hosted layout capture failed; replaying response-scoped pairs only");
                    None
                }
            };
            let injected = crate::hosted_tools::captured_search_events(
                &sse_bytes,
                pump_source.as_str(),
                pending.response_id(),
                &replay_calls,
                layout.as_deref(),
            )
            .await;
            if !injected.is_empty() {
                let terminal_event = events.split_off(events.len() - 1);
                events.extend(injected);
                events.extend(terminal_event);
            }
        }
        for ev in events {
            if tx.send(Ok(ev)).await.is_err() {
                return;
            }
        }
    })
}

/// Extracts the terminal facts the converter needs from rig's folded
/// response.
fn stream_terminal_of(response: &rig_core::completion::CompletionResponse) -> StreamTerminal {
    StreamTerminal {
        finish_reason: response.finish_reason(),
        response_id: response.origin.response_id.clone(),
        model: response.origin.response_model.clone(),
        usage: response.usage,
    }
}

/// Last `message_delta` stop reason on the teed Anthropic wire, `None` when
/// the bytes hold none. Bounded by the tee cap.
fn anthropic_stop_reason(sse_bytes: &[u8]) -> Option<String> {
    let mut stop_reason = None;
    for line in sse_bytes.split(|byte| *byte == b'\n') {
        let Some(payload) = line.strip_prefix(b"data:") else {
            continue;
        };
        let payload = serde_json::from_slice::<serde_json::Value>(
            payload.strip_prefix(b" ").unwrap_or(payload),
        )
        .ok();
        let Some(payload) = payload else {
            continue;
        };
        if payload.get("type").and_then(serde_json::Value::as_str) == Some("message_delta")
            && let Some(reason) = payload
                .get("delta")
                .and_then(|delta| delta.get("stop_reason"))
                .and_then(serde_json::Value::as_str)
        {
            stop_reason = Some(reason.to_string());
        }
    }
    stop_reason
}
