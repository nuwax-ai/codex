//! Small Rig transport decorator. The SDK owns provider serialization and SSE;
//! Codex retains URL targeting, custom trust roots and response request IDs.

use bytes::Bytes;
use eventsource_stream::Eventsource;
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
    /// Anthropic `output_config.effort`, mapped from codex reasoning effort
    /// where the scales overlap. `None` leaves the field untouched.
    pub(crate) anthropic_effort: Option<String>,
    /// Anthropic `service_tier`, mapped from the two semantically matching
    /// OpenAI values. `None` leaves the field untouched.
    pub(crate) anthropic_service_tier: Option<String>,
    pub(crate) request_id: Arc<Mutex<Option<String>>>,
    pub(crate) authorization_override: Option<http::HeaderValue>,
    pub(crate) protocol: crate::RigProtocol,
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
            if self.disable_anthropic_parallel
                || !self.tool_strict.is_empty()
                || self.anthropic_effort.is_some()
                || self.anthropic_service_tier.is_some()
            {
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
                        // top level.
                        let name = match self.protocol {
                            crate::RigProtocol::Chat => tool
                                .get("function")
                                .and_then(|function| function.get("name")),
                            crate::RigProtocol::Anthropic => tool.get("name"),
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
                        };
                        if let Some(target) = target {
                            target.insert("strict".into(), (*value).into());
                        }
                    }
                }
                if let Some(body_map) = body.as_object_mut() {
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
                *request.body_mut() = serde_json::to_vec(&body)
                    .map_err(|error| Error::Instance(error.into()))?
                    .into();
                request.headers_mut().remove(http::header::CONTENT_LENGTH);
            }
            let response = HttpClientExt::send_streaming(&self.inner, request)
                .await
                .map_err(sanitize_error)?;
            let id = response
                .headers()
                .get("x-request-id")
                .or_else(|| response.headers().get("request-id"))
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            if let Ok(mut slot) = self.request_id.lock() {
                *slot = id;
            }
            let stop_at_done =
                self.protocol == crate::RigProtocol::Chat && response.status().is_success();
            Ok(response.map(|body| {
                let body = body.map(|chunk| chunk.map_err(sanitize_error));
                if !stop_at_done {
                    return Box::pin(body) as rig_core::http_client::sse::BoxedStream;
                }
                // Rig 0.42 only flushes Chat's terminal record at EOF, even
                // after [DONE]. End the body at that SSE frame so a gateway
                // keeping HTTP open cannot turn a completed turn into a timeout.
                // Use the SSE parser to handle split bytes, CRLF and multiline
                // data; do not search raw chunks for a sentinel substring.
                Box::pin(futures::stream::unfold(
                    (Box::pin(body.eventsource()), false),
                    |(mut frames, done)| async move {
                        if done {
                            return None;
                        }
                        let frame = frames.next().await?;
                        let (bytes, done) = match frame {
                            Ok(event) => {
                                let done = event.data == "[DONE]";
                                let mut encoded =
                                    format!("event: {}\nid: {}\n", event.event, event.id);
                                if let Some(retry) = event.retry {
                                    encoded.push_str(&format!("retry: {}\n", retry.as_millis()));
                                }
                                for line in event.data.split('\n') {
                                    encoded.push_str("data: ");
                                    encoded.push_str(line);
                                    encoded.push('\n');
                                }
                                encoded.push('\n');
                                (Ok(Bytes::from(encoded)), done)
                            }
                            Err(eventsource_stream::EventStreamError::Transport(error)) => {
                                (Err(error), true)
                            }
                            Err(error) => (Err(Error::Instance(Box::new(error))), true),
                        };
                        Some((bytes, (frames, done)))
                    },
                )) as rig_core::http_client::sse::BoxedStream
            }))
        }
    }
}

fn sanitize_error(error: Error) -> Error {
    match error {
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
