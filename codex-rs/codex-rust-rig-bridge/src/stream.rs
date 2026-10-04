//! Entry point: streams a Codex `ResponsesApiRequest` through rig and emits
//! Codex `ResponseEvent`s — the same contract as `stream_via_genai`.

use std::sync::Arc;
use std::time::Duration;

use codex_api::ApiError;
use codex_api::Provider;
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

/// Shared handle the stream pump fills with Rig events when the caller
/// wants to record the bridge boundary (cassette mode). `None` = no
/// recording overhead.
pub type RigEventRecorder =
    Option<Arc<std::sync::Mutex<Vec<rig_core::streaming::StreamedAssistantContent>>>>;

pub use crate::request_capture::FinalRequestRecorder;

/// The recorders a test run may attach to one stream (D2). Event-level and
/// wire-level capture travel together so a single call site owns both.
#[derive(Clone, Default)]
pub struct RigTurnRecorders {
    pub events: RigEventRecorder,
    pub final_request: FinalRequestRecorder,
}

const RESPONSE_STREAM_CHANNEL_CAPACITY: usize = 256;

/// Hard ceiling for one paused assistant message's raw continuation content
/// (thinking + signatures + text + server-tool blocks). This whole-response
/// budget predates the persisted-envelope budget split and must not silently
/// shrink with `hosted_replay::MAX_PAIR_BYTES`, which now scopes only saved
/// hosted envelopes and layouts.
const MAX_PAUSE_CONTENT_BYTES: usize = 40_960;

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
    let (stream, recorders) = stream_via_rig_with_recorders(
        request,
        api_provider,
        api_auth,
        extra_headers,
        protocol,
        idle_timeout,
        RigTurnRecorders {
            events: recorder,
            final_request: None,
        },
    )
    .await?;
    Ok((stream, recorders.events))
}

/// [`stream_via_rig_with_recording`] with the D2 final-request capture
/// attached alongside the event recorder.
pub async fn stream_via_rig_with_recorders(
    request: &ResponsesApiRequest,
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: HeaderMap,
    protocol: RigProtocol,
    idle_timeout: Duration,
    recorders: RigTurnRecorders,
) -> Result<(ResponseStream, RigTurnRecorders), ApiError> {
    stream_via_rig_with_context(
        request,
        api_provider,
        api_auth,
        extra_headers,
        protocol,
        idle_timeout,
        recorders,
        crate::wire_budget::RigCallContext::default(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn stream_via_rig_with_context(
    request: &ResponsesApiRequest,
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: HeaderMap,
    protocol: RigProtocol,
    idle_timeout: Duration,
    recorders: RigTurnRecorders,
    context: crate::wire_budget::RigCallContext,
) -> Result<(ResponseStream, RigTurnRecorders), ApiError> {
    if protocol == RigProtocol::Responses {
        if recorders.events.is_some() {
            return Err(ApiError::InvalidRequest {
                message: RECORDER_REJECTS_RESPONSES_MSG.into(),
            });
        }
        let stream = crate::responses::stream_responses_via_rig_with_context(
            request,
            api_provider,
            api_auth,
            extra_headers,
            idle_timeout,
            None,
            None,
            recorders.final_request.clone(),
            context,
        )
        .await?;
        return Ok((stream, recorders));
    }
    stream_via_rig_attempt(
        request,
        api_provider,
        api_auth,
        extra_headers,
        protocol,
        idle_timeout,
        recorders,
        /*pause_depth*/ 0,
        /*pause_replay*/ None,
        context,
    )
    .await
}

/// Hard cap on bridge-internal pause_turn continuations (spec §1.2): a
/// turn that keeps pausing past this fails with a clear error instead of
/// looping forever.
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
    recorders: RigTurnRecorders,
    pause_depth: u32,
    pause_replay: Option<crate::hosted_replay::PauseReplay>,
    context: crate::wire_budget::RigCallContext,
) -> impl Future<Output = Result<(ResponseStream, RigTurnRecorders), ApiError>> + Send {
    async move {
        if protocol == RigProtocol::Responses {
            return Err(ApiError::InvalidRequest {
                message: RECORDER_REJECTS_RESPONSES_MSG.into(),
            });
        }
        let auth_identity = match (&context.auth_domain_kind, &context.auth_domain) {
            (Some(kind), Some(domain)) => {
                Some(serde_json::to_string(&(kind, domain)).map_err(|_| {
                    ApiError::InvalidRequest {
                        message: "cannot encode model authorization identity".into(),
                    }
                })?)
            }
            (_, domain) => domain.clone(),
        };
        let source = crate::client::reasoning_source_with_auth_domain(
            api_provider,
            protocol,
            &request.model,
            auth_identity.as_deref(),
        )?;
        // An opted-out replay payload is unused: drop it from the conversion
        // copy before SDK validation, including unknown legacy wire shapes.
        let mut conversion_request = std::borrow::Cow::Borrowed(request);
        if let Some(replay) = &pause_replay {
            for message in &replay.messages {
                let bytes = serde_json::to_vec(message)
                    .map_err(|_| ApiError::Stream("Paused content is not serializable".into()))?;
                if bytes.len() > MAX_PAUSE_CONTENT_BYTES {
                    return Err(ApiError::Stream("Paused content exceeds the raw replay byte budget; cannot truncate signed content".into()));
                }
            }
            conversion_request
                .to_mut()
                .input
                .truncate(replay.original_input_len);
        }
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
        if let Some(max_output_tokens) =
            request.max_output_tokens.or(api_provider.max_output_tokens)
        {
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
            retry: Some(api_provider.retry.clone()),
            inner: crate::client::http_client(protocol)?,
            request_headers: crate::client::request_headers(&headers, protocol),
            wire_auth: None,
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
            final_request_recorder: recorders.final_request.clone(),
            wire_budget: crate::wire_budget::WireBudget {
                context_window_tokens: context.context_window_tokens,
                output_tokens: completion_request.max_tokens,
            },
            anthropic_sse_tee: anthropic_sse_tee.clone(),
            // Official pause recipe: append each complete paused response to
            // the original request, preserving its raw assistant content.
            anthropic_pause_raw_content: pause_replay
                .as_ref()
                .map(|replay| replay.messages.clone()),
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
        // Reconcile late results only with calls actually projected on this
        // attempt, including raw pause messages; never arbitrary stored IDs.
        let replayed_search_calls = http
            .anthropic_websearch_replay
            .iter()
            .flat_map(|group| group.blocks.iter())
            .chain(http.anthropic_pause_raw_content.iter().flatten().flatten())
            .filter(|block| block["type"] == "server_tool_use")
            .count();
        if replayed_search_calls > crate::hosted_replay::MAX_REPLAY_PAIRS_PER_REQUEST {
            return Err(ApiError::Stream(
                "Paused replay exceeds the per-request search-call budget".into(),
            ));
        }
        let pending_replay_calls = pending_calls_from_wire(
            http.anthropic_websearch_replay
                .iter()
                .flat_map(|group| group.blocks.iter())
                .chain(http.anthropic_pause_raw_content.iter().flatten().flatten()),
        );

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
        // Filled by the pump when the attempt ends PAUSED; the chainer below
        // turns it into a bridge-internal continuation attempt.
        let paused_capture: Arc<std::sync::Mutex<Option<crate::hosted_replay::PauseCapture>>> =
            Arc::new(std::sync::Mutex::new(None));
        let pump_paused_capture = paused_capture.clone();
        let pump_task = crate::stream_pump::spawn_pump(
            tx.clone(),
            rig_stream,
            crate::stream_pump::PumpContext {
                protocol,
                idle_timeout,
                pause_depth,
                source: source.clone(),
                custom_tool_names,
                first_event: next_event.take(),
                events_recorder: recorders.events.clone(),
                sse_tee: anthropic_sse_tee,
                anthropic_usage,
                paused_capture: pump_paused_capture,
                replay_calls: pending_replay_calls,
            },
        );

        // Fork (nuwax-codex) D3: chain pause continuations onto the same
        // channel. The chainer holds a Sender clone so the stream stays open
        // after the pump ends; if the attempt paused, it re-sends the request
        // with the paused assistant content appended (official recipe) and
        // forwards the continuation's events. Usage/request IDs of continuation
        // attempts are their own — the final Completed carries the last
        // attempt's numbers.
        if protocol == RigProtocol::Anthropic
            && pause_depth < crate::stream_pump::PAUSE_CONTINUATION_LIMIT
        {
            let chainer_tx = tx;
            let mut continuation_request = request.clone();
            let provider = api_provider.clone();
            let auth = api_auth.clone();
            let headers = chainer_headers;
            let idle = idle_timeout;
            let continuation_recorders = recorders.clone();
            tokio::spawn(async move {
                let _ = pump_task.await;
                // A cancelled turn must not spawn a continuation attempt.
                if chainer_tx.is_closed() {
                    return;
                }
                let capture = paused_capture.lock().ok().and_then(|mut slot| slot.take());
                if let Some(capture) = capture {
                    // The official recipe resends paused assistant responses
                    // unchanged, after the original input, at the transport.
                    // Synthetic items are retained for events, not converted
                    // a second time.
                    let mut replay = pause_replay.unwrap_or(crate::hosted_replay::PauseReplay {
                        original_input_len: continuation_request.input.len(),
                        messages: Vec::new(),
                    });
                    replay.messages.push(capture.raw_content);
                    continuation_request.input.extend(capture.items);
                    let continued = tokio::select! {
                        biased;
                        _ = chainer_tx.closed() => return,
                        result = stream_via_rig_attempt(
                        &continuation_request,
                        &provider,
                        &auth,
                        headers,
                        RigProtocol::Anthropic,
                        idle,
                        continuation_recorders,
                        pause_depth + 1,
                        Some(replay),
                        context,
                    )
                    => result,
                    };
                    match continued {
                        Ok((mut stream, _)) => loop {
                            let event = tokio::select! {
                                biased;
                                _ = chainer_tx.closed() => return,
                                event = stream.next() => event,
                            };
                            let Some(event) = event else {
                                return;
                            };
                            if chainer_tx.send(event).await.is_err() {
                                return;
                            }
                        },
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
            recorders,
        ))
    }
}

/// Maps rig errors onto codex's transport taxonomy, preserving the HTTP
/// status when rig surfaced one so codex-core's 401-recovery loop still
/// triggers.
pub(crate) fn map_completion_error(e: rig_core::completion::request::CompletionError) -> ApiError {
    use rig_core::completion::request::CompletionError;
    if let Some(error) =
        crate::wire_budget::api_error(&e).or_else(|| crate::request_capture::api_error(&e))
    {
        return error;
    }
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
fn pending_calls_from_wire<'a>(
    blocks: impl Iterator<Item = &'a serde_json::Value>,
) -> std::collections::HashMap<String, serde_json::Value> {
    let mut pending = std::collections::HashMap::new();
    for block in blocks {
        match block.get("type").and_then(serde_json::Value::as_str) {
            Some("server_tool_use") => {
                if block
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(crate::hosted_tools::is_web_search_server_use)
                    && let Some(id) = block.get("id").and_then(serde_json::Value::as_str)
                {
                    pending.insert(id.to_string(), block.clone());
                }
            }
            Some("web_search_tool_result" | "tool_result") => {
                if let Some(id) = block.get("tool_use_id").and_then(serde_json::Value::as_str) {
                    pending.remove(id);
                }
            }
            _ => {}
        }
    }
    pending
}

#[cfg(test)]
#[path = "stream_replay_tests.rs"]
mod replay_tests;
