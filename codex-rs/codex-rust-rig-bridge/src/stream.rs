//! Entry point: streams a Codex `ResponsesApiRequest` through rig and emits
//! Codex `ResponseEvent`s — the same contract as `stream_via_genai`.

use std::time::Duration;

use codex_api::ApiError;
use codex_api::Provider;
use codex_api::ResponsesApiRequest;
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
    idle_timeout: Duration,
) -> Result<ResponseStream, ApiError> {
    let mut completion_request = responses_request_to_completion_request(request).ok_or(
        ApiError::InvalidRequest {
            message: "No convertible messages in request".into(),
        },
    )?;

    let protocol = crate::client::protocol_for_base_url(&api_provider.base_url);
    // Anthropic's wire requires max_tokens; codex does not model an output
    // cap, so default generously.
    if protocol == RigProtocol::Anthropic && completion_request.max_tokens.is_none() {
        completion_request.max_tokens = Some(crate::client::DEFAULT_ANTHROPIC_MAX_TOKENS);
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

    let (tx, rx) = mpsc::channel(RESPONSE_STREAM_CHANNEL_CAPACITY);

    tokio::spawn(async move {
        let mut pending = PendingRigMessage::new();

        loop {
            match tokio::time::timeout(idle_timeout, rig_stream.next()).await {
                Ok(Some(Ok(event))) => {
                    let events = rig_event_to_response_events(event, &mut pending);
                    for ev in events {
                        if tx.send(Ok(ev)).await.is_err() {
                            return;
                        }
                    }
                }
                Ok(Some(Err(e))) => {
                    // rig's contract: a malformed frame surfaces as Err but
                    // the stream may continue; only a transport error is
                    // terminal. Forward the error and stop — codex's retry
                    // machinery handles reattempts.
                    tracing::error!(error = %e, "rig stream error");
                    let _ = tx.send(Err(map_completion_error(e))).await;
                    return;
                }
                Ok(None) => {
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
                Err(_elapsed) => {
                    let _ = tx.send(Err(ApiError::Transport(TransportError::Timeout))).await;
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
    if let rig_core::completion::request::CompletionError::HttpError(http_error) = &e
        && let Some(status) = http_error_status(http_error)
        && let Ok(status) = http::StatusCode::from_u16(status.as_u16())
    {
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
