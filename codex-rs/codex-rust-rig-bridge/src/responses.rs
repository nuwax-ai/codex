//! Same-protocol Responses passthrough over rig.
//!
//! Serializes Codex's `ResponsesApiRequest` verbatim and sends it through
//! rig's OpenAI Responses client (`post_sse` + `HttpClientExt::send_streaming`),
//! then decodes the raw SSE bytes with codex-api's Responses decoder under a
//! STRICT terminal policy scoped to this path. The Chat/Anthropic conversion
//! pipeline (role rewriting, message merging, tool filtering, reasoning
//! envelopes) is never entered.

use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use codex_api::ApiError;
use codex_api::Provider;
use codex_api::ResponseEvent;
use codex_api::ResponseStream;
use codex_api::ResponsesApiRequest;
use codex_api::SharedAuthProvider;
use codex_api::StreamResponse;
use codex_api::TransportError;
use futures::StreamExt;
use http::HeaderMap;
use rig_core::http_client::HttpClientExt;

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
    turn_state: Option<Arc<OnceLock<String>>>,
) -> Result<ResponseStream, ApiError> {
    stream_responses_via_rig_inner(
        request,
        api_provider,
        api_auth,
        extra_headers,
        idle_timeout,
        None,
        turn_state,
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
        /*turn_state*/ None,
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
    turn_state: Option<Arc<OnceLock<String>>>,
) -> Result<ResponseStream, ApiError> {
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
        anthropic_tool_choice: None,
        chat_drop_orphan_tool_choice: false,
        anthropic_websearch_replay: Vec::new(),
        tool_strict: Default::default(),
        tool_result_errors: Default::default(),
        anthropic_server_tools: Vec::new(),
        anthropic_usage: Arc::new(std::sync::Mutex::new(Default::default())),
        responses_sse_recorder: sse_recorder,
        anthropic_sse_tee: None,
    };

    let client = crate::client::build_responses_client(&base_url, &headers, http)?;

    // Clone-and-project at the TYPE level, then serialize the copy once.
    // A serde_json::Value round-trip would reorder keys (BTreeMap without
    // `preserve_order`) and re-encode the raw `tools` JSON through Value —
    // a fidelity loss for large integers in tool schemas. Cloning keeps the
    // original byte-for-byte serialization semantics; the Arc<RawValue>
    // tools field clones without copying.
    let mut wire_request = request.clone();
    let projected = project_cross_protocol_history(&mut wire_request.input);

    let model = request.model.clone();
    tracing::info!(
        model = %model,
        protocol = "responses",
        bridge = "rig",
        endpoint = "POST /responses",
        input_items = request.input.len(),
        projected_reasoning_envelopes = projected,
        "Dispatching responses stream via rig"
    );
    let body = serde_json::to_vec(&wire_request).map_err(|error| ApiError::InvalidRequest {
        message: format!("failed to serialize responses request: {error}"),
    })?;

    let mut stream_request = client
        .post_sse("/responses")
        .map_err(map_http_error)?
        .body(body)
        .map_err(|error| {
            ApiError::Transport(TransportError::Network(format!(
                "rig responses request build failed: {error}"
            )))
        })?;
    // Rig's OpenAI client synthesizes Bearer even for api-key-only or
    // unauthenticated providers. Preserve the resolved auth scheme exactly.
    match headers.get(http::header::AUTHORIZATION) {
        Some(value) => {
            stream_request
                .headers_mut()
                .insert(http::header::AUTHORIZATION, value.clone());
        }
        None => {
            stream_request
                .headers_mut()
                .remove(http::header::AUTHORIZATION);
        }
    }
    // No retry here on purpose: codex-core's stream retry budget owns
    // reattempts; layering another loop would multiply them.
    let response = tokio::time::timeout(
        idle_timeout,
        HttpClientExt::send_streaming(&client, stream_request),
    )
    .await
    .map_err(|_| ApiError::Transport(TransportError::Timeout))?
    .map_err(map_http_error)?;

    let (parts, body_stream) = response.into_parts();
    let bytes = adapt_body_stream(body_stream);
    let stream_response = StreamResponse {
        status: parts.status,
        headers: parts.headers,
        bytes,
    };

    // Use the complete Codex decoder: headers and metadata events carry
    // model selection, rate limits, and moderation state outside item events.
    let mut stream =
        codex_api::spawn_strict_response_stream(stream_response, idle_timeout, turn_state);
    stream.upstream_request_id = request_id.lock().ok().and_then(|slot| slot.clone());
    Ok(stream)
}

/// Request-time cross-protocol projection (phase 2): history recorded on the
/// Chat/Anthropic wires carries this bridge's replay envelopes in
/// `Reasoning.encrypted_content`. Those payloads are bridge-internal and
/// vendor-specific signatures that must not be replayed on the Responses
/// wire, so the request COPY clears the field (serializing as `null`, the
/// same on-wire shape as any reasoning item without ciphertext) while
/// keeping the item, its summary, and its identity. Everything else —
/// including non-envelope ciphertext whose origin is not known here —
/// passes through verbatim; cross-provider ciphertext may be rejected. The persisted rollout is never
/// rewritten; returns the number of projected items.
fn project_cross_protocol_history(input: &mut [codex_protocol::models::ResponseItem]) -> usize {
    let mut projected = 0;
    for item in input {
        if let codex_protocol::models::ResponseItem::Reasoning {
            encrypted_content, ..
        } = item
            && encrypted_content
                .as_deref()
                .is_some_and(crate::reasoning::is_replay_envelope)
        {
            *encrypted_content = None;
            projected += 1;
        }
    }
    projected
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
    let mut stream = codex_api::spawn_strict_response_stream(
        StreamResponse {
            status: http::StatusCode::OK,
            headers: HeaderMap::new(),
            bytes: stream,
        },
        Duration::from_secs(30),
        /*turn_state*/ None,
    );
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        match event {
            Ok(event) => events.push(event),
            Err(error) => return Err(error),
        }
    }
    Ok(events)
}
