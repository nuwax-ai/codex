//! The contract every Chat-Completions bridge implements.
//!
//! Codex core speaks only this trait; concrete bridges (genai, rig) live in
//! their own crates and register implementations behind cargo features.
//! Keeping the trait here — in `codex-api`, the crate both bridges already
//! depend on — avoids any dependency of the bridges on `codex-core` and any
//! dependency of this crate on a concrete bridge.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::Provider;
use crate::ResponsesApiRequest;
use crate::ResponseStream;
use crate::SharedAuthProvider;
use crate::ApiError;
use http::HeaderMap;

/// Which Chat-Completions-family wire the bridge should speak. Neutral so
/// neither bridge's protocol type leaks into core; each bridge maps it onto
/// its own adapter/protocol enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatWireProtocol {
    /// OpenAI-compatible Chat Completions.
    ChatCompletions,
    /// Anthropic Messages.
    Anthropic,
}

/// A Chat-Completions bridge: converts a Codex `ResponsesApiRequest` into a
/// provider request on the given wire and streams back Codex
/// `ResponseEvent`s, preserving the event contract codex's turn loop
/// expects (item-added before deltas, reasoning before message, exactly one
/// terminal `Completed`, HTTP statuses on start errors).
pub trait ChatModelBridge: std::fmt::Debug + Send + Sync {
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
        extra_headers: HeaderMap,
        protocol: ChatWireProtocol,
        idle_timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<ResponseStream, ApiError>> + Send + 'a>>;
}

/// Resolves the neutral wire protocol for a provider: explicit
/// `wire_api = "anthropic"` wins; otherwise `/anthropic`-style base URLs
/// (e.g. MiMo's, GLM's) route to the Anthropic Messages wire, everything
/// else to Chat Completions.
pub fn chat_wire_protocol(wire_anthropic: bool, base_url: &str) -> ChatWireProtocol {
    if wire_anthropic {
        return ChatWireProtocol::Anthropic;
    }
    let path = base_url.split_once("://").map_or(base_url, |(_, rest)| rest);
    if path.contains("/anthropic") {
        ChatWireProtocol::Anthropic
    } else {
        ChatWireProtocol::ChatCompletions
    }
}
