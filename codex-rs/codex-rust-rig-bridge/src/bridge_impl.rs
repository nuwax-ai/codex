//! `ModelBridge` implementation — thin adapter over `stream_via_rig` so
//! codex-core dispatches through the neutral trait without knowing rig's
//! protocol types. The neutral protocol mirrors the provider's explicit
//! `wire_api`: Responses is a same-protocol passthrough inside the bridge.

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
        options: ModelBridgeOptions,
    ) -> Pin<Box<dyn Future<Output = Result<ResponseStream, ApiError>> + Send + 'a>> {
        let recorder = match crate::request_capture::from_env() {
            Ok(recorder) => recorder,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let context = crate::wire_budget::RigCallContext {
            auth_domain: options.auth_domain,
            auth_domain_kind: options.auth_domain_kind,
            context_window_tokens: options.context_window_tokens,
        };
        let protocol = match options.protocol {
            ModelWireProtocol::Responses => {
                return Box::pin(crate::responses::stream_responses_via_rig_with_context(
                    request,
                    provider,
                    auth,
                    options.extra_headers,
                    options.idle_timeout,
                    None,
                    options.turn_state,
                    recorder,
                    context,
                ));
            }
            ModelWireProtocol::Anthropic => RigProtocol::Anthropic,
            ModelWireProtocol::ChatCompletions => RigProtocol::Chat,
        };
        Box::pin(async move {
            crate::stream::stream_via_rig_with_context(
                request,
                provider,
                auth,
                options.extra_headers,
                protocol,
                options.idle_timeout,
                crate::RigTurnRecorders {
                    events: None,
                    final_request: recorder,
                },
                context,
            )
            .await
            .map(|(stream, _)| stream)
        })
    }
}
