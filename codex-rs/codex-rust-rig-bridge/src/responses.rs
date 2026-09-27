//! Same-protocol Responses passthrough over rig.
//!
//! Serializes Codex's `ResponsesApiRequest` verbatim and sends it through
//! rig's OpenAI Responses client (`post_sse` + `HttpClientExt::send_streaming`),
//! then decodes the raw SSE bytes with codex-api's Responses decoder under a
//! STRICT terminal policy scoped to this path. The Chat/Anthropic conversion
//! pipeline (role rewriting, message merging, tool filtering, reasoning
//! envelopes) is never entered.

use std::sync::Arc;
use std::time::Duration;

use codex_api::ApiError;
use codex_api::Provider;
use codex_api::ResponseEvent;
use codex_api::ResponseStream;
use codex_api::ResponsesApiRequest;
use codex_api::ResponsesStreamEvent;
use codex_api::SharedAuthProvider;
use codex_api::StreamResponse;
use codex_api::TransportError;
use codex_api::process_responses_event;
use codex_protocol::models::ResponseItem;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use http::HeaderMap;
use rig_core::http_client::HttpClientExt;
use tokio::sync::mpsc;

/// Mirrors the Chat/Anthropic pump's channel capacity (see `stream.rs`).
const RESPONSE_STREAM_CHANNEL_CAPACITY: usize = 256;

/// Cassette handle the stream pump fills with the raw wire SSE bytes when
/// recording (same shape as the Chat/Anthropic event recorder). `None` = no
/// recording overhead.
pub type RigSseRecorder = Option<Arc<std::sync::Mutex<Vec<u8>>>>;

/// One streaming turn on the Responses wire via rig.
pub(crate) async fn stream_responses_via_rig(
    request: &ResponsesApiRequest,
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: HeaderMap,
    idle_timeout: Duration,
) -> Result<ResponseStream, ApiError> {
    stream_responses_via_rig_inner(
        request,
        api_provider,
        api_auth,
        extra_headers,
        idle_timeout,
        None,
    )
    .await
}

/// Same as [`stream_responses_via_rig`], additionally teeing the raw wire SSE
/// bytes into the recorder (live-cassette recording).
pub async fn stream_responses_via_rig_with_sse_recording(
    request: &ResponsesApiRequest,
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: HeaderMap,
    idle_timeout: Duration,
    recorder: RigSseRecorder,
) -> Result<ResponseStream, ApiError> {
    stream_responses_via_rig_inner(
        request,
        api_provider,
        api_auth,
        extra_headers,
        idle_timeout,
        recorder,
    )
    .await
}

async fn stream_responses_via_rig_inner(
    request: &ResponsesApiRequest,
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: HeaderMap,
    idle_timeout: Duration,
    sse_recorder: RigSseRecorder,
) -> Result<ResponseStream, ApiError> {
    reject_cross_protocol_history(&request.input)?;

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
    let http = crate::transport::RigHttpClient {
        inner: crate::client::http_client(&headers, crate::RigProtocol::Responses)?,
        query,
        request_id: request_id.clone(),
        authorization_override: headers
            .get(http::header::AUTHORIZATION)
            .filter(|_| crate::client::bearer_token(&headers).is_none())
            .cloned(),
        protocol: crate::RigProtocol::Responses,
        // Chat/Anthropic-only knobs; all empty on the Responses wire.
        disable_anthropic_parallel: false,
        disable_anthropic_thinking: false,
        anthropic_effort: None,
        anthropic_service_tier: None,
        tool_strict: Default::default(),
        tool_result_errors: Default::default(),
        anthropic_usage: Arc::new(std::sync::Mutex::new(Default::default())),
        responses_sse_recorder: sse_recorder,
    };

    let client = crate::client::build_responses_client(&base_url, &headers, http)?;

    let body = serde_json::to_vec(request).map_err(|error| ApiError::InvalidRequest {
        message: format!("failed to serialize responses request: {error}"),
    })?;

    let model = request.model.clone();
    tracing::info!(
        model = %model,
        protocol = "responses",
        bridge = "rig",
        endpoint = "POST {base_url}/responses",
        input_items = request.input.len(),
        "Dispatching responses stream via rig"
    );

    let stream_request = client
        .post_sse("/responses")
        .map_err(map_http_error)?
        .body(body)
        .map_err(|error| {
            ApiError::Transport(TransportError::Network(format!(
                "rig responses request build failed: {error}"
            )))
        })?;
    // No retry here on purpose: codex-core's stream retry budget owns
    // reattempts; layering another loop would multiply them.
    let response = HttpClientExt::send_streaming(&client, stream_request)
        .await
        .map_err(map_http_error)?;

    let (parts, body_stream) = response.into_parts();
    let bytes = adapt_body_stream(body_stream);
    let stream_response = StreamResponse {
        status: parts.status,
        headers: parts.headers,
        bytes,
    };

    let upstream_request_id = request_id.lock().ok().and_then(|slot| slot.clone());
    let (tx, rx) = mpsc::channel(RESPONSE_STREAM_CHANNEL_CAPACITY);
    tokio::spawn(strict_responses_pump(
        stream_response.bytes,
        tx,
        idle_timeout,
    ));
    Ok(ResponseStream {
        rx_event: rx,
        upstream_request_id,
    })
}

/// Phase 1 supports new sessions and same-protocol resume. History carrying
/// this bridge's Chat/Anthropic replay envelopes means the session was
/// produced on another wire: fail fast instead of silently dropping
/// reasoning or guessing a projection (phase 2).
fn reject_cross_protocol_history(input: &[ResponseItem]) -> Result<(), ApiError> {
    for item in input {
        if let ResponseItem::Reasoning {
            encrypted_content: Some(value),
            ..
        } = item
            && crate::reasoning::is_replay_envelope(value)
        {
            return Err(ApiError::InvalidRequest {
                message: "session history contains rig chat/anthropic reasoning \
                          envelopes; cross-protocol resume is not supported yet — \
                          start a new session or resume on the wire that produced it"
                    .into(),
            });
        }
    }
    Ok(())
}

/// Maps rig transport errors onto codex's taxonomy. Non-2xx provider
/// responses arrive as `InvalidStatusCodeWithDetails` carrying status,
/// headers, and body; preserving them keeps codex-core's 401-recovery loop
/// and Retry-After handling working.
fn map_http_error(error: rig_core::http_client::Error) -> ApiError {
    if let rig_core::http_client::Error::InvalidStatusCodeWithDetails {
        status,
        body,
        headers,
    } = error
    {
        let retry_after = codex_http_client::RetryAfter::from_headers(&headers);
        return ApiError::Transport(TransportError::Http {
            status,
            url: None,
            headers: Some(*headers),
            body: Some(body),
            retry_after,
        });
    }
    ApiError::Transport(TransportError::Network(format!(
        "rig responses request failed: {error}"
    )))
}

/// Converts rig's byte stream into codex's. Stream errors already passed
/// through the transport's URL sanitization; wrap them as network errors.
fn adapt_body_stream(body: rig_core::http_client::sse::BoxedStream) -> codex_api::ByteStream {
    Box::pin(body.map(|chunk| {
        chunk.map_err(|error| TransportError::Network(format!("rig responses stream: {error}")))
    }))
}

/// Event kinds whose payloads MUST decode into Codex events. `Ok(None)` from
/// the shared decoder on one of these means a required field was missing or
/// unparseable — under the strict policy that is an error, never a silent
/// skip. Unknown/non-critical kinds stay compatibly ignored upstream.
const CRITICAL_EVENT_KINDS: [&str; 11] = [
    "response.created",
    "response.output_item.added",
    "response.output_item.done",
    "response.output_text.delta",
    "response.custom_tool_call_input.delta",
    "response.reasoning_summary_text.delta",
    "response.reasoning_summary_text.done",
    "response.reasoning_text.delta",
    "response.completed",
    "response.failed",
    "response.incomplete",
];

/// Strict SSE pump for the Rig Responses path (native behavior is untouched
/// and stays lenient): terminal errors surface immediately, corrupted
/// critical events fail the turn, truncation never reports success, and no
/// synthetic Created/item-ID/Completed events are injected.
pub(crate) async fn strict_responses_pump(
    bytes: codex_api::ByteStream,
    tx: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
) {
    let mut frames = bytes.eventsource();
    let mut terminal_error: Option<ApiError> = None;
    loop {
        let frame = tokio::select! {
            biased;
            _ = tx.closed() => return,
            frame = tokio::time::timeout(idle_timeout, frames.next()) => frame,
        };
        let sse = match frame {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(error))) => {
                let _ = tx
                    .send(Err(ApiError::Stream(format!("SSE error: {error}"))))
                    .await;
                return;
            }
            Ok(None) => {
                // Stream ended: only a prior terminal error or an already
                // delivered Completed may end the pump.
                let error = terminal_error.take().unwrap_or_else(|| {
                    ApiError::Stream("stream closed before response.completed".into())
                });
                let _ = tx.send(Err(error)).await;
                return;
            }
            Err(_elapsed) => {
                let _ = tx
                    .send(Err(ApiError::Stream("idle timeout waiting for SSE".into())))
                    .await;
                return;
            }
        };
        // Compatibility: some Responses-compatible gateways (GLM) append a
        // Chat-style `data: [DONE]` frame the Responses wire does not define.
        // The terminal is response.completed; ignore the sentinel.
        if sse.data.trim() == "[DONE]" {
            continue;
        }
        let event: ResponsesStreamEvent = match serde_json::from_str(&sse.data) {
            Ok(event) => event,
            Err(error) => {
                let _ = tx
                    .send(Err(ApiError::Stream(format!(
                        "malformed responses SSE event: {error}"
                    ))))
                    .await;
                return;
            }
        };
        let kind = event.kind().to_string();
        match process_responses_event(event) {
            Ok(Some(event)) => {
                let is_completed = matches!(event, ResponseEvent::Completed { .. });
                if tx.send(Ok(event)).await.is_err() {
                    return;
                }
                if is_completed {
                    return;
                }
            }
            Ok(None) => {
                if CRITICAL_EVENT_KINDS.contains(&kind.as_str()) {
                    let _ = tx
                        .send(Err(ApiError::Stream(format!(
                            "responses event `{kind}` was missing required fields"
                        ))))
                        .await;
                    return;
                }
                tracing::trace!(kind = %kind, "ignoring non-critical responses event");
            }
            Err(error) => {
                // failed/incomplete: surface immediately and stop — nothing
                // after a terminal failure may report success.
                let _ = tx.send(Err(error.into_api_error())).await;
                return;
            }
        }
    }
}

#[cfg(test)]
#[path = "responses_tests.rs"]
mod tests;

/// Offline replay of a recorded raw Responses SSE body through the same
/// strict terminal policy the live path uses (cassette parity with the
/// Chat/Anthropic `replay_fixture_events`). The SSE text must be the wire
/// bytes exactly as captured.
pub async fn replay_responses_sse(sse: &str) -> Result<Vec<ResponseEvent>, ApiError> {
    let bytes = bytes::Bytes::copy_from_slice(sse.as_bytes());
    let stream: codex_api::ByteStream =
        Box::pin(futures::stream::iter(vec![Ok::<_, TransportError>(bytes)]));
    let (tx, mut rx) = mpsc::channel(RESPONSE_STREAM_CHANNEL_CAPACITY);
    tokio::spawn(strict_responses_pump(stream, tx, Duration::from_secs(30)));
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        match event {
            Ok(event) => events.push(event),
            Err(error) => return Err(error),
        }
    }
    Ok(events)
}
