//! Small Rig transport decorator. The SDK owns provider serialization and SSE;
//! Codex retains URL targeting, custom trust roots and response request IDs.

use bytes::Bytes;
use futures::StreamExt;
use rig_core::http_client::Error;
use rig_core::http_client::HttpClientExt;
use rig_core::http_client::LazyBody;
use rig_core::http_client::MultipartForm;
use rig_core::http_client::Request;
use rig_core::http_client::Response;
use rig_core::http_client::StreamingResponse;
use rig_core::wasm_compat::WasmCompatSend;
use std::sync::Arc;
use std::sync::Mutex;

#[derive(Clone, Default)]
pub(crate) struct RigHttpClient {
    pub(crate) inner: reqwest_rig::Client,
    /// Per-attempt headers; never retained in the shared connection pool.
    pub(crate) request_headers: http::HeaderMap,
    pub(crate) query: Vec<(String, String)>,
    pub(crate) disable_anthropic_parallel: bool,
    pub(crate) tool_strict: std::collections::HashMap<String, bool>,
    pub(crate) tool_result_errors: std::collections::HashMap<String, bool>,
    pub(crate) disable_anthropic_thinking: bool,
    /// Anthropic `output_config.effort`, mapped from codex reasoning effort
    /// where the scales overlap. `None` leaves the field untouched.
    pub(crate) anthropic_effort: Option<String>,
    /// Anthropic `service_tier`, mapped from the two semantically matching
    /// OpenAI values. `None` leaves the field untouched.
    pub(crate) anthropic_service_tier: Option<String>,
    /// Anthropic wire shape of the Responses `tool_choice`, restored when
    /// post-injected server tools left the serialized body without one (rig's
    /// streaming path drops `tool_choice` alongside an empty typed tool list).
    /// `None` leaves the serialized choice untouched.
    pub(crate) anthropic_tool_choice: Option<serde_json::Value>,
    /// Chat wire only: the request advertised no function tools (hosted tools
    /// are dropped on Chat), so a serialized `tool_choice` would dangle and
    /// providers reject it — it is removed instead of widening behavior.
    pub(crate) chat_drop_orphan_tool_choice: bool,
    /// Anthropic wire only: persisted web-search wire pairs to splice into
    /// their assistant message's content array ((assistant-message index,
    /// raw blocks in history order)).
    pub(crate) anthropic_websearch_replay: Vec<(usize, Vec<serde_json::Value>)>,
    /// Translated Anthropic server-tool entries for the request's hosted
    /// (Responses) tools; empty when none translate. Chat keeps dropping
    /// hosted tools — they have no Chat Completions representation.
    pub(crate) anthropic_server_tools: Vec<serde_json::Value>,
    pub(crate) request_id: Arc<Mutex<Option<String>>>,
    pub(crate) authorization_override: Option<http::HeaderValue>,
    pub(crate) protocol: crate::RigProtocol,
    pub(crate) anthropic_usage: Arc<Mutex<crate::usage::AnthropicUsage>>,
    /// Responses passthrough only: tee the raw wire SSE bytes for cassette
    /// recording. `None` in production.
    pub(crate) responses_sse_recorder: Option<Arc<Mutex<Vec<u8>>>>,
    /// Anthropic wire only: tee the raw wire SSE bytes so the stream pump can
    /// recover server-tool blocks rig never exposes on its public streaming
    /// surface. `None` in production for the other protocols.
    pub(crate) anthropic_sse_tee: Option<Arc<Mutex<Vec<u8>>>>,
}

impl std::fmt::Debug for RigHttpClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Query strings and headers may carry credentials.
        formatter
            .debug_struct("RigHttpClient")
            .finish_non_exhaustive()
    }
}

impl RigHttpClient {
    fn prepare<T>(&self, mut request: Request<T>) -> Result<Request<T>, Error> {
        for (name, value) in &self.request_headers {
            // Match reqwest default-header precedence: explicit SDK/request
            // headers win over caller defaults.
            if !request.headers().contains_key(name) {
                request.headers_mut().insert(name.clone(), value.clone());
            }
        }
        if let Some(value) = &self.authorization_override {
            request
                .headers_mut()
                .insert(http::header::AUTHORIZATION, value.clone());
        }
        if !self.query.is_empty() {
            let mut url = reqwest_rig::Url::parse(&request.uri().to_string())
                .map_err(|error| Error::Instance(error.into()))?;
            url.query_pairs_mut().extend_pairs(&self.query);
            *request.uri_mut() = url
                .as_str()
                .parse()
                .map_err(|error: http::uri::InvalidUri| Error::Instance(error.into()))?;
        }
        Ok(request)
    }

    /// Borrows this request's Chat/Anthropic rewrite knobs for [`send_streaming`].
    fn chat_family_rewrite(&self) -> ChatFamilyRewrite<'_> {
        ChatFamilyRewrite {
            protocol: self.protocol,
            websearch_replay: &self.anthropic_websearch_replay,
            disable_anthropic_parallel: self.disable_anthropic_parallel,
            disable_anthropic_thinking: self.disable_anthropic_thinking,
            tool_strict: &self.tool_strict,
            tool_result_errors: &self.tool_result_errors,
            anthropic_server_tools: &self.anthropic_server_tools,
            anthropic_tool_choice: self.anthropic_tool_choice.as_ref(),
            anthropic_effort: self.anthropic_effort.as_ref(),
            anthropic_service_tier: self.anthropic_service_tier.as_ref(),
            chat_drop_orphan_tool_choice: self.chat_drop_orphan_tool_choice,
        }
    }
}

/// One Chat/Anthropic wire rewrite of the serialized request body. Kept as a
/// standalone borrow-only struct so the exact body transformation is testable
/// without standing up an HTTP stack; [`send_streaming`] is its only caller.
struct ChatFamilyRewrite<'a> {
    protocol: crate::RigProtocol,
    websearch_replay: &'a [(usize, Vec<serde_json::Value>)],
    disable_anthropic_parallel: bool,
    disable_anthropic_thinking: bool,
    tool_strict: &'a std::collections::HashMap<String, bool>,
    tool_result_errors: &'a std::collections::HashMap<String, bool>,
    anthropic_server_tools: &'a [serde_json::Value],
    anthropic_tool_choice: Option<&'a serde_json::Value>,
    anthropic_effort: Option<&'a String>,
    anthropic_service_tier: Option<&'a String>,
    chat_drop_orphan_tool_choice: bool,
}

impl ChatFamilyRewrite<'_> {
    fn needed(&self) -> bool {
        self.protocol != crate::RigProtocol::Responses
            && (self.disable_anthropic_parallel
                || self.disable_anthropic_thinking
                || self.chat_drop_orphan_tool_choice
                || (self.protocol == crate::RigProtocol::Anthropic
                    && (!self.tool_result_errors.is_empty()
                        || !self.anthropic_server_tools.is_empty()
                        || self.anthropic_tool_choice.is_some()))
                || !self.tool_strict.is_empty()
                || !self.websearch_replay.is_empty()
                || self.anthropic_effort.is_some()
                || self.anthropic_service_tier.is_some())
    }

    fn apply(&self, body: &mut serde_json::Value) {
        // rig's streaming path drops a caller-set `tool_choice` when its typed
        // tool list is empty (Anthropic rejects the dangling field). The
        // server tools injected below re-advertise tools, so the requested
        // choice is restored alongside them — never widened to `auto`.
        if self.protocol == crate::RigProtocol::Anthropic
            && !self.anthropic_server_tools.is_empty()
            && let Some(choice) = self.anthropic_tool_choice
            && body.get("tool_choice").is_none()
            && let Some(body_map) = body.as_object_mut()
        {
            body_map.insert("tool_choice".into(), choice.clone());
        }
        // A hosted-only request drops every Chat tool, leaving the serialized
        // `tool_choice` dangling; providers reject that shape, so it is
        // removed rather than silently widening what the model may call.
        if self.chat_drop_orphan_tool_choice
            && let Some(body_map) = body.as_object_mut()
            && body_map.remove("tool_choice").is_some()
        {
            tracing::warn!(
                "no function tools are advertised on the Chat wire; dropping tool_choice"
            );
        }
        if self.disable_anthropic_parallel
            && let Some(choice) = body
                .get_mut("tool_choice")
                .and_then(serde_json::Value::as_object_mut)
            && choice.get("type").and_then(serde_json::Value::as_str) != Some("none")
        {
            choice.insert("disable_parallel_tool_use".into(), true.into());
        }
        if let Some(tools) = body
            .get_mut("tools")
            .and_then(serde_json::Value::as_array_mut)
        {
            for tool in tools {
                // Chat nests the name under `function`; Anthropic
                // tools carry it (and their `strict` flag) at the
                // top level. Responses never enters this rewrite.
                let name = match self.protocol {
                    crate::RigProtocol::Chat => tool
                        .get("function")
                        .and_then(|function| function.get("name")),
                    crate::RigProtocol::Anthropic => tool.get("name"),
                    crate::RigProtocol::Responses => None,
                }
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
                let Some(value) = name.as_deref().and_then(|name| self.tool_strict.get(name))
                else {
                    continue;
                };
                let target = match self.protocol {
                    crate::RigProtocol::Chat => tool
                        .get_mut("function")
                        .and_then(serde_json::Value::as_object_mut),
                    crate::RigProtocol::Anthropic => tool.as_object_mut(),
                    crate::RigProtocol::Responses => None,
                };
                if let Some(target) = target {
                    target.insert("strict".into(), (*value).into());
                }
            }
        }
        if let Some(body_map) = body.as_object_mut() {
            if self.disable_anthropic_thinking {
                body_map.insert("thinking".into(), serde_json::json!({"type":"disabled"}));
            }
            if self.protocol == crate::RigProtocol::Anthropic
                && !self.anthropic_server_tools.is_empty()
            {
                // Server tools have no function schema; they join the
                // serialized function tools as raw typed entries.
                let tools = body_map
                    .entry("tools")
                    .or_insert_with(|| serde_json::json!([]));
                if let Some(tools) = tools.as_array_mut() {
                    tools.extend(self.anthropic_server_tools.iter().cloned());
                }
            }
            if let Some(effort) = self.anthropic_effort {
                // Merge into any existing output_config (rig may have
                // serialized output_config.format from output_schema).
                let config = body_map
                    .entry("output_config")
                    .or_insert_with(|| serde_json::json!({}));
                if let Some(config) = config.as_object_mut() {
                    config.insert("effort".into(), (*effort).clone().into());
                }
            }
            if let Some(tier) = self.anthropic_service_tier {
                body_map.insert("service_tier".into(), (*tier).clone().into());
            }
        }
        if self.protocol == crate::RigProtocol::Anthropic
            && let Some(messages) = body
                .get_mut("messages")
                .and_then(serde_json::Value::as_array_mut)
        {
            for message in messages.iter_mut() {
                if let Some(content) = message
                    .get_mut("content")
                    .and_then(serde_json::Value::as_array_mut)
                {
                    for block in content {
                        if block["type"] == "tool_result"
                            && let Some(is_error) = block["tool_use_id"]
                                .as_str()
                                .and_then(|id| self.tool_result_errors.get(id))
                        {
                            block["is_error"] = (*is_error).into();
                        }
                    }
                }
            }
            // Fork (nuwax-codex): splice persisted web-search wire pairs
            // (server_tool_use + result, verbatim incl. encrypted content)
            // into the content array of the assistant message each pair
            // followed in history. The index counts assistant-role messages,
            // matching how history conversion numbered them.
            if !self.websearch_replay.is_empty() {
                let mut assistant_seen = 0usize;
                let mut pending: Vec<&(usize, Vec<serde_json::Value>)> =
                    self.websearch_replay.iter().collect();
                for message in messages.iter_mut() {
                    if message.get("role").and_then(serde_json::Value::as_str) != Some("assistant")
                    {
                        continue;
                    }
                    let current = assistant_seen;
                    assistant_seen += 1;
                    let groups: Vec<&(usize, Vec<serde_json::Value>)> = pending
                        .iter()
                        .filter(|(index, _)| *index == current)
                        .copied()
                        .collect();
                    if groups.is_empty() {
                        continue;
                    }
                    pending.retain(|(index, _)| *index != current);
                    let Some(content) = message
                        .get_mut("content")
                        .and_then(serde_json::Value::as_array_mut)
                    else {
                        tracing::warn!(
                            index = current,
                            "assistant message has no content array; web-search replay skipped"
                        );
                        continue;
                    };
                    for (_, blocks) in groups {
                        content.extend(blocks.iter().cloned());
                    }
                }
                if !pending.is_empty() {
                    tracing::warn!(
                        pairs = pending.len(),
                        "web-search replay pairs reference unknown assistant messages; dropped"
                    );
                }
            }
        }
    }
}

impl HttpClientExt for RigHttpClient {
    fn send<T, U>(
        &self,
        request: Request<T>,
    ) -> impl Future<Output = Result<Response<LazyBody<U>>, Error>> + WasmCompatSend + 'static
    where
        T: Into<Bytes> + WasmCompatSend,
        U: From<Bytes> + WasmCompatSend + 'static,
    {
        let request = self.prepare(request).map(|request| request.map(Into::into));
        let inner = self.inner.clone();
        async move {
            HttpClientExt::send(&inner, request?)
                .await
                .map_err(sanitize_error)
                .map(sanitize_response)
        }
    }

    fn send_multipart<U>(
        &self,
        request: Request<MultipartForm>,
    ) -> impl Future<Output = Result<Response<LazyBody<U>>, Error>> + WasmCompatSend + 'static
    where
        U: From<Bytes> + WasmCompatSend + 'static,
    {
        let request = self.prepare(request);
        let inner = self.inner.clone();
        async move {
            HttpClientExt::send_multipart(&inner, request?)
                .await
                .map_err(sanitize_error)
                .map(sanitize_response)
        }
    }

    fn send_streaming<T>(
        &self,
        request: Request<T>,
    ) -> impl Future<Output = Result<StreamingResponse, Error>> + WasmCompatSend
    where
        T: Into<Bytes> + WasmCompatSend,
    {
        let request = self.prepare(request).map(|request| request.map(Into::into));
        async move {
            let mut request: Request<Bytes> = request?;
            // Responses passthrough sends Codex's serialized request verbatim:
            // no Chat/Anthropic-specific body injections (tool strict flags,
            // parallel-tool-use gating, thinking toggles, effort/tier merges,
            // tool-result error markers, tool-choice repair) may touch it.
            let rewrite = self.chat_family_rewrite();
            let chat_family_rewrite = rewrite.needed();
            if chat_family_rewrite {
                let mut body: serde_json::Value = serde_json::from_slice(request.body())
                    .map_err(|error| Error::Instance(error.into()))?;
                rewrite.apply(&mut body);
                *request.body_mut() = serde_json::to_vec(&body)
                    .map_err(|error| Error::Instance(error.into()))?
                    .into();
                request.headers_mut().remove(http::header::CONTENT_LENGTH);
            }
            let response = HttpClientExt::send_streaming(&self.inner, request)
                .await
                .map_err(sanitize_error)?;
            // Compatible gateways may expose only a trace/log correlation ID.
            // Keep canonical request IDs first; this never replaces the model's
            // response ID carried in the SSE body.
            let id = ["x-request-id", "request-id", "x-trace-id", "x-log-id"]
                .into_iter()
                .find_map(|name| {
                    response
                        .headers()
                        .get(name)
                        .and_then(|value| value.to_str().ok())
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_string)
                });
            if let Ok(mut slot) = self.request_id.lock() {
                *slot = id;
            }
            let check_terminal = response.status().is_success();
            let sse_recorder = self.responses_sse_recorder.clone();
            let anthropic_sse_tee = self.anthropic_sse_tee.clone();
            Ok(response.map(|body| {
                let body = Box::pin(body.map(|chunk| chunk.map_err(sanitize_error)))
                    as rig_core::http_client::sse::BoxedStream;
                match self.protocol {
                    // Responses passthrough consumes the raw SSE bytes with
                    // codex-api's decoder and its own strict terminal policy;
                    // no [DONE]/terminal rewriting at this layer. In record
                    // mode, tee the exact wire bytes for offline replay.
                    crate::RigProtocol::Responses => tee_wire_bytes(body, sse_recorder),
                    // rig never exposes server-tool blocks on its public
                    // streaming surface, so the Anthropic pump re-reads them
                    // from the teed wire bytes at terminal time.
                    crate::RigProtocol::Anthropic => crate::sse::with_terminal_check(
                        tee_wire_bytes(body, anthropic_sse_tee),
                        self.protocol,
                        self.anthropic_usage.clone(),
                    ),
                    protocol if check_terminal => crate::sse::with_terminal_check(
                        body,
                        protocol,
                        self.anthropic_usage.clone(),
                    ),
                    _ => body,
                }
            }))
        }
    }
}

/// Upper bound for a wire tee buffer. The tee exists to recover Anthropic
/// server-tool blocks and to record cassettes — an unbounded copy of a
/// runaway stream is a memory hazard, so capture stops (with a warning)
/// once the cap is hit. Degradation is graceful: block recovery parses only
/// complete SSE frames, so a truncated tail can never produce a half block.
const WIRE_TEE_CAP_BYTES: usize = 8 * 1024 * 1024;

/// Copies wire chunks into the shared buffer while passing the stream
/// through unchanged, up to [`WIRE_TEE_CAP_BYTES`]. Recording only; `None`
/// returns the stream untouched.
fn tee_wire_bytes(
    body: rig_core::http_client::sse::BoxedStream,
    recorder: Option<Arc<Mutex<Vec<u8>>>>,
) -> rig_core::http_client::sse::BoxedStream {
    match recorder {
        Some(recorder) => {
            let mut capped = false;
            Box::pin(body.map(move |chunk| {
                if let Ok(bytes) = &chunk
                && !capped
                && let Ok(mut buffer) = recorder.lock()
            {
                    if buffer.len() >= WIRE_TEE_CAP_BYTES {
                        capped = true;
                        tracing::warn!(
                            cap_bytes = WIRE_TEE_CAP_BYTES,
                            "wire tee buffer cap reached; server-tool recovery and                              cassette recording stop here (the stream itself is unaffected)"
                        );
                    } else {
                        let remaining = WIRE_TEE_CAP_BYTES - buffer.len();
                        let take = remaining.min(bytes.len());
                        buffer.extend_from_slice(&bytes[..take]);
                        if take < bytes.len() {
                            capped = true;
                            tracing::warn!(
                                cap_bytes = WIRE_TEE_CAP_BYTES,
                                "wire tee buffer cap reached; server-tool recovery and                                  cassette recording stop here (the stream itself is unaffected)"
                            );
                        }
                    }
                }
                chunk
            })) as rig_core::http_client::sse::BoxedStream
        }
        None => body,
    }
}

fn sanitize_error(error: Error) -> Error {
    match error {
        Error::InvalidStatusCodeWithDetails {
            status,
            body,
            headers,
        } if body.starts_with("failed to read error response body:") => {
            // Rig 0.42 has already formatted reqwest's URL-bearing read error.
            // No response body was obtained; retain status/headers, not that URL.
            Error::InvalidStatusCodeWithDetails {
                status,
                headers,
                body: "failed to read error response body".into(),
            }
        }
        Error::Instance(error) => match error.downcast::<reqwest_rig::Error>() {
            Ok(error) => Error::Instance(Box::new(error.without_url())),
            Err(error) => Error::Instance(error),
        },
        error => error,
    }
}

fn sanitize_response<U: Send + 'static>(response: Response<LazyBody<U>>) -> Response<LazyBody<U>> {
    response.map(|body| Box::pin(async move { body.await.map_err(sanitize_error) }) as LazyBody<U>)
}

#[cfg(test)]
#[path = "transport_rewrite_tests.rs"]
mod tests;
