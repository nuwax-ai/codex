//! `ChatModelBridge` implementation — thin adapter over `stream_via_rig` so
//! codex-core dispatches through the neutral trait without knowing rig's
//! protocol types.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use codex_api::ApiError;
use codex_api::ChatModelBridge;
use codex_api::ChatWireProtocol;
use codex_api::Provider;
use codex_api::ResponseStream;
use codex_api::ResponsesApiRequest;
use codex_api::SharedAuthProvider;
use http::HeaderMap;

use crate::client::RigProtocol;

/// The rig bridge as a `ChatModelBridge` implementor (stateless unit).
#[derive(Debug)]
pub struct RigChatBridge;

impl ChatModelBridge for RigChatBridge {
    fn name(&self) -> &'static str {
        "rig"
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
        let protocol = match protocol {
            ChatWireProtocol::Anthropic => RigProtocol::Anthropic,
            ChatWireProtocol::ChatCompletions => RigProtocol::Chat,
        };
        Box::pin(crate::stream_via_rig(
            request,
            provider,
            auth,
            extra_headers,
            protocol,
            idle_timeout,
        ))
    }
}
