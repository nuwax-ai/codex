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
use crate::convert_response::rig_event_to_response_events;
use crate::stream::map_completion_error;

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
            rig_core::streaming::StreamedAssistantContent,
            rig_core::completion::request::CompletionError,
        >,
    >,
    pub(crate) events_recorder: RigEventRecorder,
    /// Anthropic wire tee, present on every Anthropic attempt.
    pub(crate) sse_tee: Option<Arc<Mutex<Vec<u8>>>>,
    pub(crate) anthropic_usage: Arc<Mutex<crate::usage::AnthropicUsage>>,
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
    mut rig_stream: rig_core::streaming::StreamingCompletionResponse,
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
                Some(Ok(mut event)) => {
                    if protocol == RigProtocol::Anthropic
                        && let rig_core::streaming::StreamedAssistantContent::Final(record) =
                            &mut event
                    {
                        let normalized = anthropic_usage
                            .lock()
                            .map(|usage| usage.apply(&mut record.usage))
                            .map_err(|_| {
                                ApiError::Stream("Anthropic usage state is unavailable".into())
                            });
                        if let Err(error) = normalized {
                            let _ = tx.send(Err(error)).await;
                            return;
                        }
                    }
                    if let Some(rec) = &events_recorder
                        && let Ok(mut buf) = rec.lock()
                    {
                        buf.push(event.clone());
                    }
                    // Fork (nuwax-codex) D3: a paused turn continues
                    // bridge-internally — flush this attempt's content
                    // WITHOUT a Completed terminal and hand the raw wire
                    // blocks to the chainer for the official re-send recipe.
                    let paused = protocol == RigProtocol::Anthropic
                        && matches!(
                            &event,
                            rig_core::streaming::StreamedAssistantContent::Final(record)
                                if matches!(
                                    record.finish_reason.as_ref(),
                                    Some(rig_core::completion::request::FinishReason::Other(reason))
                                        if reason == "pause_turn"
                                )
                        );
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
                        let rig_core::streaming::StreamedAssistantContent::Final(record) = event
                        else {
                            return;
                        };
                        let mut events = match crate::convert_response::paused_final_events(
                            record,
                            &mut pending,
                        ) {
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
                            let raw = match crate::hosted_replay::raw_indexed_assistant_content(
                                &sse_bytes,
                            )
                            .await
                            {
                                Ok(raw) => raw,
                                Err(error) => {
                                    let _ = tx.send(Err(error)).await;
                                    return;
                                }
                            };
                            let layout =
                                crate::hosted_tools::response_layout(&raw, pending.segments());
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
                    let _ = tx.send(Err(map_completion_error(e))).await;
                    return;
                }
                None => {
                    // rig's contract: ending without a terminal record means
                    // truncation, never a successful completion.
                    if !pending.completed_emitted() {
                        let _ = tx
                            .send(Err(ApiError::Transport(TransportError::Network(
                                "rig stream ended without a terminal record (truncated)"
                                    .to_string(),
                            ))))
                            .await;
                    }
                    return;
                }
            }
        }
    })
}
