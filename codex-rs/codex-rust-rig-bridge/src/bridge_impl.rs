//! `ModelBridge` implementation — thin adapter over `stream_via_rig` so
//! codex-core dispatches through the neutral trait without knowing rig's
//! protocol types. The neutral protocol mirrors the provider's explicit
//! `wire_api`: Responses is a same-protocol passthrough inside the bridge.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use codex_api::ApiError;
use codex_api::ModelBridge;
use codex_api::ModelWireProtocol;
use codex_api::Provider;
use codex_api::ResponseStream;
use codex_api::ResponsesApiRequest;
use codex_api::SharedAuthProvider;
use http::HeaderMap;

use crate::client::RigProtocol;

/// The rig bridge as a `ModelBridge` implementor (stateless unit).
#[derive(Debug)]
pub struct RigModelBridge;

impl ModelBridge for RigModelBridge {
    fn name(&self) -> &'static str {
        "rig"
    }

    fn stream<'a>(
        &'a self,
        request: &'a ResponsesApiRequest,
        provider: &'a Provider,
        auth: &'a SharedAuthProvider,
        extra_headers: HeaderMap,
        protocol: ModelWireProtocol,
        idle_timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<ResponseStream, ApiError>> + Send + 'a>> {
        let protocol = match protocol {
            ModelWireProtocol::Responses => RigProtocol::Responses,
            ModelWireProtocol::Anthropic => RigProtocol::Anthropic,
            ModelWireProtocol::ChatCompletions => RigProtocol::Chat,
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
