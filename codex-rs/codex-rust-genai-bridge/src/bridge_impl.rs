//! `ModelBridge` implementation — a thin adapter over the crate's free
//! function so codex-core can dispatch through the neutral trait without
//! knowing genai's adapter types. Genai speaks Chat Completions and Anthropic
//! Messages only; the Responses wire belongs to the rig bridge (passthrough)
//! or the native transport.

use std::future::Future;
use std::pin::Pin;

use codex_api::ApiError;
use codex_api::ModelBridge;
use codex_api::ModelBridgeOptions;
use codex_api::ModelWireProtocol;
use codex_api::Provider;
use codex_api::ResponseStream;
use codex_api::ResponsesApiRequest;
use codex_api::SharedAuthProvider;
use genai::adapter::AdapterKind;

/// The genai bridge as a `ModelBridge` implementor (stateless unit).
#[derive(Debug)]
pub struct GenaiChatBridge;

impl ModelBridge for GenaiChatBridge {
    fn name(&self) -> &'static str {
        "genai"
    }

    fn stream<'a>(
        &'a self,
        request: &'a ResponsesApiRequest,
        provider: &'a Provider,
        auth: &'a SharedAuthProvider,
        options: ModelBridgeOptions,
    ) -> Pin<Box<dyn Future<Output = Result<ResponseStream, ApiError>> + Send + 'a>> {
        let adapter_kind = match options.protocol {
            ModelWireProtocol::Anthropic => AdapterKind::Anthropic,
            ModelWireProtocol::ChatCompletions => AdapterKind::OpenAI,
            ModelWireProtocol::Responses => {
                return Box::pin(std::future::ready(Err(ApiError::InvalidRequest {
                    message: "the genai bridge does not implement wire_api = \"responses\"; \
                              use the default rig bridge (remove experimental_bridge) or \
                              experimental_bridge = \"native\""
                        .into(),
                })));
            }
        };
        Box::pin(crate::stream_via_genai(
            request,
            provider,
            auth,
            options.extra_headers,
            adapter_kind,
            options.idle_timeout,
        ))
    }
}
