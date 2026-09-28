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
            // tool-result error markers) may touch it.
            let chat_family_rewrite = self.protocol != crate::RigProtocol::Responses
                && (self.disable_anthropic_parallel
                    || self.disable_anthropic_thinking
                    || (self.protocol == crate::RigProtocol::Anthropic
                        && !self.tool_result_errors.is_empty())
                    || (self.protocol == crate::RigProtocol::Anthropic
                        && !self.anthropic_server_tools.is_empty())
                    || !self.tool_strict.is_empty()
                    || self.anthropic_effort.is_some()
                    || self.anthropic_service_tier.is_some());
            if chat_family_rewrite {
                let mut body: serde_json::Value = serde_json::from_slice(request.body())
                    .map_err(|error| Error::Instance(error.into()))?;
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
                        let Some(value) =
                            name.as_deref().and_then(|name| self.tool_strict.get(name))
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
                    if let Some(effort) = &self.anthropic_effort {
                        // Merge into any existing output_config (rig may have
                        // serialized output_config.format from output_schema).
                        let config = body_map
                            .entry("output_config")
                            .or_insert_with(|| serde_json::json!({}));
                        if let Some(config) = config.as_object_mut() {
                            config.insert("effort".into(), effort.clone().into());
                        }
                    }
                    if let Some(tier) = &self.anthropic_service_tier {
                        body_map.insert("service_tier".into(), tier.clone().into());
                    }
                }
                if self.protocol == crate::RigProtocol::Anthropic
                    && let Some(messages) = body
                        .get_mut("messages")
                        .and_then(serde_json::Value::as_array_mut)
                {
                    for message in messages {
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
                }
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

/// Copies every wire chunk into the shared buffer while passing the stream
/// through unchanged. Recording only; `None` returns the stream untouched.
fn tee_wire_bytes(
    body: rig_core::http_client::sse::BoxedStream,
    recorder: Option<Arc<Mutex<Vec<u8>>>>,
) -> rig_core::http_client::sse::BoxedStream {
    match recorder {
        Some(recorder) => Box::pin(body.map(move |chunk| {
            if let Ok(bytes) = &chunk
                && let Ok(mut buffer) = recorder.lock()
            {
                buffer.extend_from_slice(bytes);
            }
            chunk
        })) as rig_core::http_client::sse::BoxedStream,
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
