//! The contract every model bridge implements.
//!
//! Codex core speaks only this trait; concrete bridges (rig, genai) live in
//! their own crates and register implementations behind cargo features.
//! Keeping the trait here — in `codex-api`, the crate every bridge already
//! depends on — avoids any dependency of the bridges on `codex-core` and any
//! dependency of this crate on a concrete bridge.
//!
//! The protocol argument is derived from the provider's explicit `wire_api`
//! alone; URL guessing must never override it (a `wire_api = "responses"`
//! provider is served Responses, not silently converted to Chat).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use crate::ApiError;
use crate::Provider;
use crate::ResponseStream;
use crate::ResponsesApiRequest;
use crate::SharedAuthProvider;
use http::HeaderMap;

/// Which wire the bridge should speak. Neutral so neither bridge's protocol
/// type leaks into core; each bridge maps it onto its own adapter/protocol
/// enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelWireProtocol {
    /// OpenAI Responses (`POST {base}/responses`).
    Responses,
    /// OpenAI-compatible Chat Completions.
    ChatCompletions,
    /// Anthropic Messages.
    Anthropic,
}

impl ModelWireProtocol {
    /// Stable wire name for logs and error messages; matches the `wire_api`
    /// config vocabulary.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::ChatCompletions => "chat",
            Self::Anthropic => "anthropic",
        }
    }
}

/// Per-request transport options, including Responses routing state that
/// the next request in the same turn must return to the server.
pub struct ModelBridgeOptions {
    pub extra_headers: HeaderMap,
    pub protocol: ModelWireProtocol,
    pub idle_timeout: Duration,
    pub turn_state: Option<Arc<OnceLock<String>>>,
}

/// A model bridge: sends a Codex `ResponsesApiRequest` on the given wire and
/// streams back Codex `ResponseEvent`s, preserving the event contract codex's
/// turn loop expects (item-added before deltas, reasoning before message,
/// exactly one terminal `Completed`, HTTP statuses on start errors).
pub trait ModelBridge: std::fmt::Debug + Send + Sync {
    /// Stable identifier for logs and error messages ("genai", "rig").
    fn name(&self) -> &'static str;

    /// Starts one streaming turn. Boxed future keeps the trait
    /// object-safe — one virtual call per turn is irrelevant next to a
    /// network round trip.
    fn stream<'a>(
        &'a self,
        request: &'a ResponsesApiRequest,
        provider: &'a Provider,
        auth: &'a SharedAuthProvider,
        options: ModelBridgeOptions,
    ) -> Pin<Box<dyn Future<Output = Result<ResponseStream, ApiError>> + Send + 'a>>;
}
