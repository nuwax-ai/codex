//! `ChatModelBridge` implementation — a thin adapter over the crate's free
//! function so codex-core can dispatch through the neutral trait without
//! knowing genai's adapter types.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use codex_api::ApiError;
use codex_api::ChatModelBridge;
use codex_api::ChatWireProtocol;
use codex_api::Provider;
use codex_api::ResponsesApiRequest;
use codex_api::ResponseStream;
use codex_api::SharedAuthProvider;
use genai::adapter::AdapterKind;
use http::HeaderMap;

/// The genai bridge as a `ChatModelBridge` implementor (stateless unit).
#[derive(Debug)]
pub struct GenaiChatBridge;

impl ChatModelBridge for GenaiChatBridge {
    fn name(&self) -> &'static str {
        "genai"
    }

    fn stream<'a>(
        &'a self,
        request: &'a ResponsesApiRequest,
        provider: &'a Provider,
        auth: &'a SharedAuthProvider,
        extra_headers: HeaderMap,
        protocol: ChatWireProtocol,
        idle_timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<ResponseStream, ApiError>> + Send + 'a>> {
        let adapter_kind = match protocol {
            ChatWireProtocol::Anthropic => AdapterKind::Anthropic,
            ChatWireProtocol::ChatCompletions => AdapterKind::OpenAI,
        };
        Box::pin(crate::stream_via_genai(
            request,
            provider,
            auth,
            extra_headers,
            adapter_kind,
            idle_timeout,
        ))
    }
}
