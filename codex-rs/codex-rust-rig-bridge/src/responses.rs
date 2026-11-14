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
use codex_protocol::models::{FunctionCallOutputBody, ResponseItem};
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
        None,
        crate::wire_budget::RigCallContext::default(),
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
        None,
        crate::wire_budget::RigCallContext::default(),
    )
    .await
}

/// Records each final HTTP request in addition to raw Responses events.
#[allow(clippy::too_many_arguments)]
pub async fn stream_responses_via_rig_with_capture(
    request: &ResponsesApiRequest,
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: HeaderMap,
    idle_timeout: Duration,
    sse_recorder: RigSseRecorder,
    turn_state: Option<Arc<OnceLock<String>>>,
    final_request_recorder: crate::FinalRequestRecorder,
) -> Result<ResponseStream, ApiError> {
    stream_responses_via_rig_with_context(
        request,
        api_provider,
        api_auth,
        extra_headers,
        idle_timeout,
        sse_recorder,
        turn_state,
        final_request_recorder,
        crate::wire_budget::RigCallContext::default(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn stream_responses_via_rig_with_context(
    request: &ResponsesApiRequest,
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: HeaderMap,
    idle_timeout: Duration,
    sse_recorder: RigSseRecorder,
    turn_state: Option<Arc<OnceLock<String>>>,
    final_request_recorder: crate::FinalRequestRecorder,
    context: crate::wire_budget::RigCallContext,
) -> Result<ResponseStream, ApiError> {
    stream_responses_via_rig_inner(
        request,
        api_provider,
        api_auth,
        extra_headers,
        idle_timeout,
        sse_recorder,
        turn_state,
        final_request_recorder,
        context,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn stream_responses_via_rig_inner(
    request: &ResponsesApiRequest,
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: HeaderMap,
    idle_timeout: Duration,
    sse_recorder: RigSseRecorder,
    turn_state: Option<Arc<OnceLock<String>>>,
    final_request_recorder: crate::FinalRequestRecorder,
    context: crate::wire_budget::RigCallContext,
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
        retry: Some(api_provider.retry.clone()),
        inner: crate::client::http_client(crate::RigProtocol::Responses)?,
        request_headers: crate::client::request_headers(&headers, crate::RigProtocol::Responses),
        wire_auth: None,
        anthropic_pause_raw_content: None,
        query: query.clone(),
        request_id: request_id.clone(),
        authorization_override: headers
            .get(http::header::AUTHORIZATION)
            .filter(|_| crate::client::bearer_token(&headers).is_none())
            .cloned(),
        protocol: crate::RigProtocol::Responses,
        // Chat/Anthropic-only knobs; all empty on the Responses wire.
        disable_anthropic_parallel: false,
        anthropic_service_tier: None,
        anthropic_tool_choice: None,
        chat_drop_orphan_tool_choice: false,
        anthropic_websearch_replay: Vec::new(),
        tool_strict: Default::default(),
        tool_result_errors: Default::default(),
        anthropic_server_tools: Vec::new(),
        anthropic_usage: Arc::new(std::sync::Mutex::new(Default::default())),
        chat_usage: Arc::new(std::sync::Mutex::new(Default::default())),
        final_request_recorder,
        wire_budget: crate::wire_budget::WireBudget {
            context_window_tokens: context.context_window_tokens,
            output_tokens: request.max_output_tokens.or(api_provider.max_output_tokens),
            // The passthrough serializes Codex's request verbatim, so
            // `detail: "original"` reaches the wire walk intact and is
            // already priced there by the same estimator the floor uses.
            vision_floor_tokens: 0,
        },
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
    wire_request.max_output_tokens = request.max_output_tokens.or(api_provider.max_output_tokens);
    let projected = project_cross_protocol_history(&mut wire_request.input);
    // Fork (nuwax-codex, 2026-11-13): Zhipu's official `/v1/responses` rejects
    // a conversation whose final input item is a `function_call_output` —
    // `400 messages 参数非法。请检查文档。` — regardless of output shape, tools,
    // or streaming. Standard OpenAI Responses semantics allow it, and every
    // image-reading tool (view_image et al.) ends its turn with exactly that
    // shape, so append a minimal user turn after a trailing tool result.
    // Verified against open.bigmodel.cn/api/v1/responses and harmless on MiMo
    // (my-docs/proxy-responses-image-bug-2026-11-13.md).
    // Default ON: the array form breaks several gateway Responses
    // converters (Zhipu verified; the array shape is new enough that other
    // implementations likely share the gap), while the moved shape is valid
    // everywhere including OpenAI-official. Providers can opt out with
    // `responses_trailing_tool_result_user_turn = false`.
    if api_provider.trailing_tool_result_user_turn.unwrap_or(true) {
        downgrade_original_detail_to_auto(&mut wire_request.input);
        move_tool_result_arrays_to_user_turns(&mut wire_request.input);
        append_trailing_tool_result_user_turn(&mut wire_request.input);
    }

    let model = request.model.clone();
    tracing::info!(
        model = %model,
        protocol = "responses",
        bridge = "rig",
        endpoint = "POST /responses",
        input_items = request.input.len(),
        projected_history_payloads = projected,
        "Dispatching responses stream via rig"
    );
    let body = serde_json::to_vec(&wire_request).map_err(|error| ApiError::InvalidRequest {
        message: format!("failed to serialize responses request: {error}"),
    })?;

    // rig 0.44 has no provider post_sse builder: the passthrough assembles
    // the request itself. The transport decorator owns auth synthesis
    // (wire_auth), query joining, and per-attempt headers on send — the
    // URI carries no query, `prepare` appends the provider's pairs.
    let uri = format!("{}/responses", base_url.trim_end_matches('/'));
    let mut stream_request = http::Request::builder()
        .method(http::Method::POST)
        .uri(uri)
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(http::header::ACCEPT, "text/event-stream")
        .body(bytes::Bytes::from(body))
        .map_err(|error| {
            ApiError::Transport(TransportError::Network(format!(
                "rig responses request build failed: {error}"
            )))
        })?;
    // Preserve the resolved auth scheme exactly: wire_auth keeps what the
    // provider configured; an explicit caller header wins over synthesis.
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
    // The decorator owns provider-configured HTTP retries. This existing
    // handshake deadline also bounds Retry-After waits; Core independently
    // owns subsequent sampling retries after a failed stream.
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

/// Request-time cross-protocol projection: history recorded on the
/// Chat/Anthropic wires carries this bridge's replay envelopes in
/// `Reasoning.encrypted_content`. Those payloads are bridge-internal and
/// vendor-specific signatures that must not be replayed on the Responses
/// wire, so the request COPY clears the field (serializing as `null`, the
/// same on-wire shape as any reasoning item without ciphertext) while
/// keeping the item, its summary, and its identity. Everything else —
/// including non-envelope ciphertext whose origin is not known here —
/// passes through verbatim; cross-provider ciphertext may be rejected. The persisted rollout is never
/// rewritten. Anthropic search wire blocks are also cleared from the copy;
/// returns the number of projected items.

/// Fork (nuwax-codex, 2026-11-13): Zhipu's official `/v1/responses` rejects
/// a conversation whose final input item is a `function_call_output` —
/// `400 messages 参数非法。请检查文档。` — regardless of output shape, tools,
/// or streaming. Standard OpenAI Responses semantics allow it, and every
/// image-reading tool (view_image et al.) ends its turn with exactly that
/// shape, so append a minimal user turn after a trailing tool result.
/// Verified against open.bigmodel.cn/api/v1/responses and harmless on MiMo
/// (my-docs/proxy-responses-image-bug-2026-11-13.md).
/// Fork (nuwax-codex, 2026-11-13): the Nuwax model proxy rejects every
/// `function_call_output` whose `output` is a content-parts array — even a
/// text-only one (`messages 参数非法`; verified matrix A-E in
/// my-docs/proxy-responses-image-bug-2026-11-13.md). Standard shape puts
/// view_image's screenshot there, so every image-reading turn would fail and
/// poison the rest of the session (every later turn replays the array).
///
/// Rewrite: images (and the result's text) move into a user message inserted
/// right after the tool result; the tool result keeps a plain-string output.
/// This is the exact shape the proxy accepts (matrix E) and OpenAI-official
/// treats as equivalent — the model sees the image right after the call.
/// Plain-string outputs are untouched.
/// Fork (nuwax-codex, 2026-11-14): Zhipu's `/v1/responses` rejects
/// `input_image` with `detail: "original"` outright (`400 API 调用参数有误`,
/// any image size — verified matrix in my-docs/proxy-responses-image-bug-
/// 2026-11-13.md). The Chat bridge already rewrites original → high for the
/// same reason; the Responses passthrough must match, or every image-carrying
/// turn on a detail-original provider 400s. The safe neutral target is
/// `auto`: legal on OpenAI official, accepted by Zhipu, and it lets each
/// model pick a detail it actually supports (some reject `high`).
pub(crate) fn downgrade_original_detail_to_auto(input: &mut [ResponseItem]) {
    for item in input {
        if let ResponseItem::Message { content, .. } = item {
            for part in content {
                if let codex_protocol::models::ContentItem::InputImage { detail, .. } = part {
                    if matches!(detail, Some(codex_protocol::models::ImageDetail::Original)) {
                        // `auto` lets the model pick a detail it supports —
                        // safer than `high`, which some models reject.
                        *detail = Some(codex_protocol::models::ImageDetail::Auto);
                    }
                }
            }
        }
        if let ResponseItem::FunctionCallOutput {
            output:
                codex_protocol::models::FunctionCallOutputPayload {
                    body: FunctionCallOutputBody::ContentItems(parts),
                    ..
                },
            ..
        } = item
        {
            for part in parts {
                if let codex_protocol::models::FunctionCallOutputContentItem::InputImage {
                    detail,
                    ..
                } = part
                {
                    if matches!(detail, Some(codex_protocol::models::ImageDetail::Original)) {
                        // `auto` lets the model pick a detail it supports —
                        // safer than `high`, which some models reject.
                        *detail = Some(codex_protocol::models::ImageDetail::Auto);
                    }
                }
            }
        }
    }
}

pub(crate) fn move_tool_result_arrays_to_user_turns(input: &mut Vec<ResponseItem>) {
    let mut rebuilt = Vec::with_capacity(input.len() + 4);
    for item in input.drain(..) {
        let ResponseItem::FunctionCallOutput {
            id,
            call_id,
            name,
            namespace,
            output,
            internal_chat_message_metadata_passthrough,
        } = item
        else {
            rebuilt.push(item);
            continue;
        };
        let FunctionCallOutputBody::ContentItems(parts) = output.body else {
            rebuilt.push(ResponseItem::FunctionCallOutput {
                id,
                call_id,
                name,
                namespace,
                output,
                internal_chat_message_metadata_passthrough,
            });
            continue;
        };
        let mut images = Vec::new();
        let mut texts = Vec::new();
        for part in parts {
            match part {
                codex_protocol::models::FunctionCallOutputContentItem::InputImage {
                    image,
                    detail,
                } => images.push((image, detail)),
                codex_protocol::models::FunctionCallOutputContentItem::InputText { text } => {
                    texts.push(text)
                }
                codex_protocol::models::FunctionCallOutputContentItem::EncryptedContent {
                    encrypted_content,
                } => texts.push(format!("(encrypted content omitted: {encrypted_content})")),
                codex_protocol::models::FunctionCallOutputContentItem::InputAudio { .. } => {
                    // Audio has no place in the string fallback or the image
                    // user turn; drop it rather than fail the whole request.
                }
            }
        }
        let text = if texts.is_empty() {
            "(The tool returned the image(s) attached to the next message.)".to_string()
        } else {
            texts.join("\n")
        };
        rebuilt.push(ResponseItem::FunctionCallOutput {
            id,
            call_id: call_id.clone(),
            name: name.clone(),
            namespace,
            output: codex_protocol::models::FunctionCallOutputPayload {
                body: FunctionCallOutputBody::Text(text),
                success: output.success,
            },
            internal_chat_message_metadata_passthrough,
        });
        if !images.is_empty() {
            let mut content: Vec<codex_protocol::models::ContentItem> = images
                .into_iter()
                .map(
                    |(image, detail)| codex_protocol::models::ContentItem::InputImage {
                        image,
                        detail,
                    },
                )
                .collect();
            content.push(codex_protocol::models::ContentItem::InputText {
                text: match &name {
                    Some(name) => format!("(image(s) above are the result of tool {name}.)"),
                    None => "(image(s) above are a tool result.)".into(),
                },
            });
            rebuilt.push(ResponseItem::Message {
                id: None,
                role: "user".into(),
                content,
                phase: None,
                internal_chat_message_metadata_passthrough: None,
                encrypted_content: None,
                status: None,
            });
        }
    }
    *input = rebuilt;
}

pub(crate) fn append_trailing_tool_result_user_turn(
    input: &mut Vec<codex_protocol::models::ResponseItem>,
) {
    if matches!(
        input.last(),
        Some(codex_protocol::models::ResponseItem::FunctionCallOutput { .. })
    ) {
        input.push(codex_protocol::models::ResponseItem::Message {
            id: None,
            role: "user".into(),
            content: vec![codex_protocol::models::ContentItem::InputText {
                text: "(The tool results above are complete. Continue based on them.)".into(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
            encrypted_content: None,
            status: None,
        });
    }
}

pub(crate) fn project_cross_protocol_history(
    input: &mut [codex_protocol::models::ResponseItem],
) -> usize {
    let mut projected = 0;
    for item in input {
        // This fork's persisted Anthropic payload is not a Responses field.
        // Keep the search item itself and clear only the request copy.
        if let codex_protocol::models::ResponseItem::WebSearchCall { wire_blocks, .. } = item {
            if wire_blocks.take().is_some() {
                projected += 1;
            }
            continue;
        }
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
    if let Some(error) =
        crate::wire_budget::api_error(&error).or_else(|| crate::request_capture::api_error(&error))
    {
        return error;
    }
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
            headers: Some(headers),
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
fn adapt_body_stream(body: rig_core::http_client::BoxedStream) -> codex_api::ByteStream {
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
