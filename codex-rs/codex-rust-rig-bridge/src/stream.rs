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
    if protocol == RigProtocol::Responses {
        return Err(ApiError::InvalidRequest {
            message: RECORDER_REJECTS_RESPONSES_MSG.into(),
        });
    }
    let source = crate::client::reasoning_source(api_provider, protocol, &request.model)?;
    let (completion_request, tool_meta) =
        responses_request_to_completion_request(request, protocol, &source)?;
    let (base_url, query) = crate::client::endpoint(&api_provider.base_url, api_provider)?;
    let mut headers = api_provider.headers.clone();
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
    let http = crate::transport::RigHttpClient {
        inner: crate::client::http_client(&headers, protocol)?,
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
            let chat_model = crate::client::build_chat_model(&model, &base_url, &headers, http)?;
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
    tokio::spawn(async move {
        let mut pending = PendingRigMessage::new(custom_tool_names, source);

        // rig streams have no start event; synthesize `Created` so the
        // event sequence matches the genai bridge (A/B parity) and any
        // consumer waiting for it sees one.
        if tx
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
                    let events = match rig_event_to_response_events(event, &mut pending) {
                        Ok(events) => events,
                        Err(error) => {
                            let _ = tx.send(Err(error)).await;
                            return;
                        }
                    };
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

    Ok((
        ResponseStream {
            rx_event: rx,
            upstream_request_id: request_id.lock().ok().and_then(|slot| slot.clone()),
        },
        recorder,
    ))
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
