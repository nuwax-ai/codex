//! Entry point: streams a Codex `ResponsesApiRequest` through rig and emits
//! Codex `ResponseEvent`s — the same contract as `stream_via_genai`.

use std::time::Duration;

use codex_api::ApiError;
use codex_api::Provider;
use codex_api::ResponsesApiRequest;
use codex_api::ResponseEvent;
use codex_api::ResponseStream;
use codex_api::SharedAuthProvider;
use codex_api::TransportError;
use futures::StreamExt;
use http::HeaderMap;
use reqwest_rig as reqwest13;
use rig_core::completion::CompletionModel;
use tokio::sync::mpsc;

use crate::client::RigProtocol;
use crate::convert_request::responses_request_to_completion_request;
use crate::convert_response::PendingRigMessage;
use crate::convert_response::rig_event_to_response_events;

const RESPONSE_STREAM_CHANNEL_CAPACITY: usize = 256;

/// Streams a turn through rig. The wire protocol is selected from the
/// provider's base URL (`/anthropic` gateways → Anthropic Messages;
/// everything else → OpenAI Chat Completions).
pub async fn stream_via_rig(
    request: &ResponsesApiRequest,
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: HeaderMap,
    protocol: RigProtocol,
    idle_timeout: Duration,
) -> Result<ResponseStream, ApiError> {
    let (mut completion_request, custom_tool_names) =
        responses_request_to_completion_request(request).ok_or(ApiError::InvalidRequest {
            message: "No convertible messages in request".into(),
        })?;

    if protocol == RigProtocol::Anthropic {
        // Anthropic's wire requires max_tokens; codex does not model an
        // output cap, so default generously.
        if completion_request.max_tokens.is_none() {
            completion_request.max_tokens = Some(crate::client::DEFAULT_ANTHROPIC_MAX_TOKENS);
        }
        // additional_params carries OpenAI-only knobs (parallel_tool_calls,
        // reasoning_effort, ...) that flatten verbatim onto the wire. On the
        // Anthropic protocol they are unknown fields with undefined behavior:
        // GLM's gateway measurably disables thinking when it sees
        // `parallel_tool_calls`, so drop them there entirely.
        completion_request.additional_params = None;
    }

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
            let chat_model = crate::client::build_chat_model(
                &model,
                &api_provider.base_url,
                api_provider,
                api_auth,
                &extra_headers,
            )?;
            chat_model.stream(completion_request).await
        }
        RigProtocol::Anthropic => {
            let anthropic_model = crate::client::build_anthropic_model(
                &model,
                &api_provider.base_url,
                api_provider,
                api_auth,
                &extra_headers,
            )?;
            anthropic_model.stream(completion_request).await
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

    let custom_tool_names = std::sync::Arc::new(custom_tool_names);
    tokio::spawn(async move {
        let mut pending = PendingRigMessage::new(custom_tool_names);

        // rig streams have no start event; synthesize `Created` so the
        // event sequence matches the genai bridge (A/B parity) and any
        // consumer waiting for it sees one.
        if tx.send(Ok(ResponseEvent::Created { response_id: None })).await.is_err() {
            return;
        }

        loop {
            let item = match next_event.take() {
                Some(item) => Some(item),
                None => match tokio::time::timeout(idle_timeout, rig_stream.next()).await {
                    Ok(item) => item,
                    Err(_elapsed) => {
                        let _ =
                            tx.send(Err(ApiError::Transport(TransportError::Timeout))).await;
                        return;
                    }
                },
            };
            match item {
                Some(Ok(event)) => {
                    let events = rig_event_to_response_events(event, &mut pending);
                    for ev in events {
                        if tx.send(Ok(ev)).await.is_err() {
                            return;
                        }
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

    Ok(ResponseStream {
        rx_event: rx,
        upstream_request_id: None,
    })
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
    let status: Option<http::StatusCode> = match &e {
        CompletionError::HttpError(http_error) => http_error_status(http_error)
            .map(|s| http::StatusCode::from_u16(s.as_u16()))
            .and_then(|s| s.ok()),
        CompletionError::ProviderResponse(provider_error) => provider_error.status,
        _ => None,
    };
    if let Some(status) = status {
        return ApiError::Transport(TransportError::Http {
            status,
            url: None,
            headers: None,
            body: Some(format!("rig request failed: {e}")),
            retry_after: None,
        });
    }
    ApiError::Transport(TransportError::Network(format!("rig error: {e}")))
}

fn http_error_status(error: &rig_core::http_client::Error) -> Option<reqwest13::StatusCode> {
    use rig_core::http_client::Error;
    match error {
        Error::InvalidStatusCode(status)
        | Error::InvalidStatusCodeWithMessage(status, _) => Some(*status),
        Error::InvalidStatusCodeWithDetails { status, .. } => Some(*status),
        _ => None,
    }
}
