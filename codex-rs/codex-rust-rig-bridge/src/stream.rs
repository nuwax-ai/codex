//! Entry point: streams a Codex `ResponsesApiRequest` through rig and emits
//! Codex `ResponseEvent`s — the same contract as `stream_via_genai`.

use std::sync::Arc;
use std::time::Duration;

use codex_api::ApiError;
use codex_api::Provider;
use codex_api::ResponseEvent;
use codex_api::ResponseStream;
use codex_api::ResponsesApiRequest;
use codex_api::SharedAuthProvider;
use codex_api::TransportError;
use futures::Future;
use futures::StreamExt;
use http::HeaderMap;
use rig_core::completion::CompletionModel;
use tokio::sync::mpsc;

use crate::client::RigProtocol;
use crate::convert_request::responses_request_to_completion_request;
use crate::convert_response::PendingRigMessage;
use crate::convert_response::rig_event_to_response_events;

/// Shared handle the stream pump fills with Rig events when the caller
/// wants to record the bridge boundary (cassette mode). `None` = no
/// recording overhead.
pub type RigEventRecorder =
    Option<Arc<std::sync::Mutex<Vec<rig_core::streaming::StreamedAssistantContent>>>>;

const RESPONSE_STREAM_CHANNEL_CAPACITY: usize = 256;

/// Shared by the entry guard and the exhaustive dispatch match below: the
/// rig-event recorder observes the Chat/Anthropic conversion boundary, which
/// the Responses passthrough never enters.
const RECORDER_REJECTS_RESPONSES_MSG: &str = "the rig event recorder only covers the \
                                              Chat/Anthropic conversion pipeline; use \
                                              stream_via_rig for the Responses passthrough";

/// Streams a turn through rig. The wire protocol comes from the provider's
/// explicit `wire_api`: `Responses` is a same-protocol passthrough (no Chat
/// conversion); Chat Completions and Anthropic Messages go through the
/// history/tool conversion pipeline below.
pub async fn stream_via_rig(
    request: &ResponsesApiRequest,
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: HeaderMap,
    protocol: RigProtocol,
    idle_timeout: Duration,
) -> Result<ResponseStream, ApiError> {
    if protocol == RigProtocol::Responses {
        return crate::responses::stream_responses_via_rig(
            request,
            api_provider,
            api_auth,
            extra_headers,
            idle_timeout,
            /*turn_state*/ None,
        )
        .await;
    }
    stream_via_rig_with_recording(
        request,
        api_provider,
        api_auth,
        extra_headers,
        protocol,
        idle_timeout,
        None,
    )
    .await
    .map(|(stream, _)| stream)
}

/// Same as [`stream_via_rig`], but optionally records Rig events after wire
/// usage correction (the input to the bridge's conversion) alongside
/// the normal codex event flow. The recorder is filled asynchronously by
/// the pump; read it after the stream completes.
pub async fn stream_via_rig_with_recording(
    request: &ResponsesApiRequest,
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: HeaderMap,
    protocol: RigProtocol,
    idle_timeout: Duration,
    recorder: RigEventRecorder,
) -> Result<(ResponseStream, RigEventRecorder), ApiError> {
    stream_via_rig_attempt(
        request,
        api_provider,
        api_auth,
        extra_headers,
        protocol,
        idle_timeout,
        recorder,
        /*pause_depth*/ 0,
        /*pause_raw_content*/ None,
    )
    .await
}

/// Hard cap on bridge-internal pause_turn continuations (spec §1.2): a
/// turn that keeps pausing past this fails with a clear error instead of
/// looping forever.
const PAUSE_CONTINUATION_LIMIT: u32 = 4;

// Recursive continuations need an explicit Send future contract; rewriting
// this as async fn makes Send inference circular at tokio::spawn. Keep the
// attempt arguments aligned with the public entry until the pump is extracted.
#[allow(clippy::manual_async_fn, clippy::too_many_arguments)]
fn stream_via_rig_attempt(
    request: &ResponsesApiRequest,
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: HeaderMap,
    protocol: RigProtocol,
    idle_timeout: Duration,
    recorder: RigEventRecorder,
    pause_depth: u32,
    pause_raw_content: Option<Vec<serde_json::Value>>,
) -> impl Future<Output = Result<(ResponseStream, RigEventRecorder), ApiError>> + Send {
    async move {
        if protocol == RigProtocol::Responses {
            return Err(ApiError::InvalidRequest {
                message: RECORDER_REJECTS_RESPONSES_MSG.into(),
            });
        }
        let source = crate::client::reasoning_source(api_provider, protocol, &request.model)?;
        // An opted-out replay payload is unused: drop it from the conversion
        // copy before SDK validation, including unknown legacy wire shapes.
        let mut conversion_request = std::borrow::Cow::Borrowed(request);
        if protocol == RigProtocol::Anthropic && api_provider.hosted_results_replay == Some(false) {
            for item in &mut conversion_request.to_mut().input {
                if let codex_protocol::models::ResponseItem::WebSearchCall { wire_blocks, .. } =
                    item
                {
                    *wire_blocks = None;
                }
            }
        }
        let (mut completion_request, tool_meta) =
            responses_request_to_completion_request(&conversion_request, protocol, &source)?;
        // Fork (nuwax-codex): an explicit provider output budget overrides the
        // bridge's default cap (Anthropic requires max_tokens on the wire; Chat
        // accepts it optionally). The Responses passthrough is verbatim and
        // never reaches here.
        if let Some(max_output_tokens) = api_provider.max_output_tokens {
            completion_request.max_tokens = Some(max_output_tokens);
        }
        let (base_url, query) = crate::client::endpoint(&api_provider.base_url, api_provider)?;
        let mut headers = api_provider.headers.clone();
        let chainer_headers = extra_headers.clone();
        headers.extend(extra_headers);
        // Resolve once: refreshable credentials and gateway conflict checks are
        // part of the outbound auth contract, not the synchronous telemetry snapshot.
        headers.extend(
            api_auth
                .resolve_auth_headers()
                .await
                .map_err(|error| ApiError::Transport(error.into()))?,
        );
        let request_id = Arc::new(std::sync::Mutex::new(None));
        let anthropic_usage = Arc::new(std::sync::Mutex::new(
            crate::usage::AnthropicUsage::default(),
        ));
        // Hosted (Responses) tools only exist on the Anthropic wire, where they
        // translate into server-tool entries; Chat drops them. Non-representable
        // search modes fail the request before anything is sent.
        let anthropic_server_tools = if protocol == RigProtocol::Anthropic {
            crate::hosted_tools::translate_anthropic_server_tools(&tool_meta.hosted_tools)?
        } else {
            Vec::new()
        };
        // rig never exposes Anthropic server-tool blocks on its public streaming
        // surface; the transport tees the wire bytes and the pump re-reads them
        // at terminal time. The tee also feeds the pause_turn continuation
        // (verbatim assistant content), so every Anthropic attempt keeps it.
        let anthropic_sse_tee = (protocol == RigProtocol::Anthropic)
            .then(|| Arc::new(std::sync::Mutex::new(Vec::<u8>::new())));
        // The wire shape of the requested tool choice, restored by the transport
        // when hosted-only tools left the serialized body without one.
        let advertised: Vec<&str> = completion_request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        let anthropic_tool_choice = match protocol {
            RigProtocol::Anthropic => Some(crate::convert_request::anthropic_tool_choice(
                &request.tool_choice,
                &advertised,
            )?),
            RigProtocol::Chat | RigProtocol::Responses => None,
        };
        let http = crate::transport::RigHttpClient {
            inner: crate::client::http_client(protocol)?,
            request_headers: crate::client::request_headers(&headers, protocol),
            // Persisted web-search pairs replay unless the provider opts out.
            anthropic_websearch_replay: if protocol == RigProtocol::Anthropic
                && api_provider.hosted_results_replay != Some(false)
            {
                tool_meta.websearch_replay
            } else {
                Vec::new()
            },
            query,
            disable_anthropic_parallel: protocol == RigProtocol::Anthropic
                && !request.parallel_tool_calls,
            request_id: request_id.clone(),
            anthropic_usage: anthropic_usage.clone(),
            authorization_override: headers
                .get(http::header::AUTHORIZATION)
                .filter(|_| crate::client::bearer_token(&headers).is_none())
                .cloned(),
            protocol,
            responses_sse_recorder: None,
            anthropic_sse_tee: anthropic_sse_tee.clone(),
            // Official pause recipe: verbatim replacement of the trailing
            // assistant's content on the continuation request.
            anthropic_pause_raw_content: pause_raw_content,
            tool_strict: tool_meta.strict,
            tool_result_errors: tool_meta.result_errors,
            disable_anthropic_thinking: protocol == RigProtocol::Anthropic
                && request
                    .reasoning
                    .as_ref()
                    .and_then(|value| value.effort.as_ref())
                    == Some(&codex_protocol::openai_models::ReasoningEffort::None),
            anthropic_effort: (protocol == RigProtocol::Anthropic)
                .then(|| crate::convert_request::anthropic_effort(request))
                .flatten(),
            anthropic_service_tier: (protocol == RigProtocol::Anthropic)
                .then(|| crate::convert_request::anthropic_service_tier(request))
                .flatten(),
            anthropic_server_tools,
            anthropic_tool_choice,
            // validate_tool_choice already rejected `required`/specific choices
            // for a hosted-only request, so at most `auto`/`none` dangle here.
            chat_drop_orphan_tool_choice: protocol == RigProtocol::Chat
                && completion_request.tools.is_empty(),
        };

        let model = request.model.clone();
        tracing::info!(
            model = %model,
            protocol = ?protocol,
            message_count = completion_request.chat_history.len(),
            tool_count = completion_request.tools.len(),
            has_reasoning_effort = completion_request
                .additional_params
                .as_ref()
                .is_some_and(|p| p.get("reasoning_effort").is_some()),
            "Dispatching chat stream via rig"
        );

        let stream_response = match protocol {
            RigProtocol::Chat => {
                let chat_model =
                    crate::client::build_chat_model(&model, &base_url, &headers, http)?;
                chat_model.stream(completion_request).await
            }
            RigProtocol::Anthropic => {
                let anthropic_model =
                    crate::client::build_anthropic_model(&model, &base_url, &headers, http)?;
                anthropic_model.stream(completion_request).await
            }
            // Unreachable: rejected at the top of this function.
            RigProtocol::Responses => {
                return Err(ApiError::InvalidRequest {
                    message: RECORDER_REJECTS_RESPONSES_MSG.into(),
                });
            }
        }
        .map_err(|e| {
            tracing::error!(model = %model, error = %e, "rig stream failed");
            map_completion_error(e)
        })?;

        let mut rig_stream = stream_response;

        // Eagerly resolve the FIRST stream item before returning: rig defers
        // HTTP failures (401/5xx) into the stream, so without this a bad-auth
        // response surfaces mid-stream where codex-core's 401-recovery loop —
        // which only inspects start errors — can never trigger.
        let mut next_event = match tokio::time::timeout(idle_timeout, rig_stream.next()).await {
            Err(_elapsed) => {
                return Err(ApiError::Transport(TransportError::Timeout));
            }
            Ok(Some(Err(e))) => {
                tracing::error!(model = %model, error = %e, "rig stream failed before first event");
                return Err(map_completion_error(e));
            }
            Ok(first) => first,
        };

        let (tx, rx) = mpsc::channel(RESPONSE_STREAM_CHANNEL_CAPACITY);

        let custom_tool_names = std::sync::Arc::new(tool_meta.custom_names);
        let pump_recorder = recorder.clone();
        let pump_sse_tee = anthropic_sse_tee;
        // Filled by the pump when the attempt ends PAUSED; the chainer below
        // turns it into a bridge-internal continuation attempt.
        let paused_capture: Arc<std::sync::Mutex<Option<crate::hosted_replay::PauseCapture>>> =
            Arc::new(std::sync::Mutex::new(None));
        let pump_paused_capture = paused_capture.clone();
        let pump_source = source.clone();
        let pump_request = request.clone();
        let pump_tx = tx.clone();
        let pump_task = tokio::spawn(async move {
            let tx = pump_tx;
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
                    None => match tokio::time::timeout(idle_timeout, rig_stream.next()).await {
                        Ok(item) => item,
                        Err(_elapsed) => {
                            let _ = tx
                                .send(Err(ApiError::Transport(TransportError::Timeout)))
                                .await;
                            return;
                        }
                    },
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
                        if let Some(rec) = &pump_recorder
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
                            let rig_core::streaming::StreamedAssistantContent::Final(record) =
                                event
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
                                items: Vec::new(),
                                raw_content: Vec::new(),
                            };
                            if let Some(tee) = &pump_sse_tee
                                && let Some(sse_bytes) = tee.lock().ok().map(|bytes| bytes.clone())
                            {
                                capture = crate::hosted_tools::assistant_continuation_items(
                                    &sse_bytes,
                                    &pump_source,
                                )
                                .await;
                            }
                            let items = capture.items.clone();
                            // The user-visible events carry the recovered pairs
                            // too (same recovery as a completed turn): an Added
                            // without status, then the item itself as Done.
                            for item in &items {
                                if let codex_protocol::models::ResponseItem::WebSearchCall {
                                    ..
                                } = item
                                {
                                    let mut added = item.clone();
                                    if let codex_protocol::models::ResponseItem::WebSearchCall {
                                        status,
                                        ..
                                    } = &mut added
                                    {
                                        *status = None;
                                    }
                                    events.push(ResponseEvent::OutputItemAdded(added));
                                    events.push(ResponseEvent::OutputItemDone(item.clone()));
                                }
                            }
                            if let Ok(mut slot) = pump_paused_capture.lock() {
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
                            && let Some(tee) = &pump_sse_tee
                            && let Some(sse_bytes) = tee.lock().ok().map(|bytes| bytes.clone())
                            && !events.is_empty()
                        {
                            let capture =
                                crate::hosted_tools::web_search_blocks_from_anthropic_sse(
                                    &sse_bytes,
                                )
                                .await;
                            let mut paired = crate::hosted_tools::pair_web_search_blocks(capture);
                            let mut injected = Vec::new();
                            let mut cited_text = std::mem::take(&mut paired.cited_text);
                            for pair in paired.pairs {
                                let cited = std::mem::take(&mut cited_text);
                                injected.extend(crate::hosted_tools::web_search_call_events(
                                    pair,
                                    &pump_source,
                                    cited,
                                ));
                            }
                            // A result with no call in THIS response closes a
                            // mixed server/client turn: re-associate it with
                            // the pending call persisted in the request
                            // history and emit a NEW completed item (the old
                            // in_progress item stays untouched, append-only).
                            for result in paired.unmatched_results {
                                let id = result
                                    .get("tool_use_id")
                                    .and_then(serde_json::Value::as_str)
                                    .map(str::to_string);
                                let Some(id) = id.filter(|id| !id.is_empty()) else {
                                    tracing::warn!(
                                        "web-search result without tool_use_id; dropping it"
                                    );
                                    continue;
                                };
                                let Some(call) =
                                    pending_call_from_history(&pump_request.input, &id)
                                else {
                                    tracing::warn!(
                                        id,
                                        "web-search result matches no pending call; dropping it"
                                    );
                                    continue;
                                };
                                injected.extend(crate::hosted_tools::web_search_call_events(
                                    crate::hosted_tools::PairedWebSearchBlocks {
                                        call,
                                        result: Some(result),
                                    },
                                    &pump_source,
                                    Vec::new(),
                                ));
                            }
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
        });

        // Fork (nuwax-codex) D3: chain pause continuations onto the same
        // channel. The chainer holds a Sender clone so the stream stays open
        // after the pump ends; if the attempt paused, it re-sends the request
        // with the paused assistant content appended (official recipe) and
        // forwards the continuation's events. Usage/request IDs of continuation
        // attempts are their own — the final Completed carries the last
        // attempt's numbers.
        if protocol == RigProtocol::Anthropic && pause_depth < PAUSE_CONTINUATION_LIMIT {
            let chainer_tx = tx;
            let mut continuation_request = request.clone();
            let provider = api_provider.clone();
            let auth = api_auth.clone();
            let headers = chainer_headers;
            let idle = idle_timeout;
            tokio::spawn(async move {
                let _ = pump_task.await;
                let capture = paused_capture.lock().ok().and_then(|mut slot| slot.take());
                if let Some(capture) = capture {
                    // The official recipe resends the paused assistant
                    // message UNCHANGED: the raw content blocks ride the
                    // request as a verbatim replacement for the trailing
                    // assistant the items synthesize — or, for a pause with
                    // no convertible items (thinking-only), as a whole
                    // appended assistant message at the transport.
                    continuation_request.input.extend(capture.items);
                    let continued = stream_via_rig_attempt(
                        &continuation_request,
                        &provider,
                        &auth,
                        headers,
                        RigProtocol::Anthropic,
                        idle,
                        /*recorder*/ None,
                        pause_depth + 1,
                        Some(capture.raw_content),
                    )
                    .await;
                    match continued {
                        Ok((mut stream, _)) => {
                            while let Some(event) = stream.next().await {
                                if chainer_tx.send(event).await.is_err() {
                                    return;
                                }
                            }
                        }
                        Err(error) => {
                            let _ = chainer_tx.send(Err(error)).await;
                        }
                    }
                }
            });
        }

        Ok((
            ResponseStream {
                rx_event: rx,
                upstream_request_id: request_id.lock().ok().and_then(|slot| slot.clone()),
                // The bridge has no graceful-interrupt channel yet; cancellation
                // drops the stream, matching native's plain SSE spawns.
                interrupt: None,
            },
            recorder,
        ))
    }
}

/// Maps rig errors onto codex's transport taxonomy, preserving the HTTP
/// status when rig surfaced one so codex-core's 401-recovery loop still
/// triggers.
fn map_completion_error(e: rig_core::completion::request::CompletionError) -> ApiError {
    use rig_core::completion::request::CompletionError;
    // Both variants can carry an HTTP status; the Provider variant is what
    // non-2xx provider responses actually arrive as (rig defers them into
    // the stream), so both must map to Http{status} for codex-core's
    // 401-recovery loop to trigger.
    if let Some(status) = e.provider_response_status() {
        let headers = e.provider_response_headers().cloned();
        let retry_after = headers
            .as_ref()
            .and_then(codex_http_client::RetryAfter::from_headers);
        let body = e
            .provider_response_body()
            .map(str::to_string)
            .or_else(|| Some(format!("rig request failed: {e}")));
        return ApiError::Transport(TransportError::Http {
            status,
            url: None,
            headers,
            body,
            retry_after,
        });
    }
    match e {
        CompletionError::RequestError(_) | CompletionError::UrlError(_) => {
            ApiError::InvalidRequest {
                message: format!("rig request failed: {e}"),
            }
        }
        _ => ApiError::Transport(TransportError::Network(format!("rig error: {e}"))),
    }
}

/// Finds the `server_tool_use` call block for a still-pending (result-less)
/// search in the request history, so a late result can close it. Returns
/// None when the id is unknown or already completed — a completed pair must
/// not be closed twice.
fn pending_call_from_history(
    input: &[codex_protocol::models::ResponseItem],
    id: &str,
) -> Option<serde_json::Value> {
    let envelopes: Vec<_> = input
        .iter()
        .filter_map(|item| match item {
            codex_protocol::models::ResponseItem::WebSearchCall { wire_blocks, .. } => {
                wire_blocks.as_ref()
            }
            _ => None,
        })
        .filter_map(crate::hosted_replay::parse_envelope)
        .collect();
    let has_result = |blocks: &[serde_json::Value]| {
        blocks.iter().any(|block| {
            matches!(
                block.get("type").and_then(serde_json::Value::as_str),
                Some("web_search_tool_result") | Some("tool_result")
            ) && block.get("tool_use_id").and_then(serde_json::Value::as_str) == Some(id)
        })
    };
    if envelopes
        .iter()
        .any(|envelope| has_result(&envelope.blocks))
    {
        return None;
    }
    envelopes
        .iter()
        .flat_map(|envelope| envelope.blocks.iter())
        .find(|block| {
            block.get("type").and_then(serde_json::Value::as_str) == Some("server_tool_use")
                && block.get("id").and_then(serde_json::Value::as_str) == Some(id)
        })
        .cloned()
}
