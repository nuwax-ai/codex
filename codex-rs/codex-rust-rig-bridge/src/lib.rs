//! Bridge crate serving third-party providers through rig (rig-core):
//! - `wire_api = "responses"` → same-protocol Responses passthrough
//!   (`responses.rs`; Codex's request serialized verbatim to `{base}/responses`);
//! - `wire_api = "chat"` / `"anthropic"` → Chat-Completions / Anthropic-Messages
//!   conversion (`convert_request.rs` & co).
//!
//! Sibling of `codex-rust-genai-bridge` with an identical public surface; the
//! active bridge is selected per provider via `experimental_bridge` in config.
//! See `my-docs/rig-bridge-implementation-plan.md` and
//! `my-docs/rig-responses-phase1/` for the designs.

mod bridge_impl;
mod client;
mod convert_request;
mod convert_response;
mod hosted_tools;
mod reasoning;
mod request_content;
mod request_messages;
mod request_tools;
mod response_tools;
mod responses;
mod sse;
mod stream;
mod transport;
mod usage;

pub use bridge_impl::RigModelBridge;
pub use client::DEFAULT_ANTHROPIC_MAX_TOKENS;
pub use client::RigProtocol;
pub use client::protocol_for_base_url;
pub use reasoning::REPLAY_PREFIX;
pub use reasoning::is_replay_envelope;
pub use responses::RigSseRecorder;
pub use responses::replay_responses_sse;
pub use responses::stream_responses_via_rig_with_sse_recording;
pub mod cassette;
pub use cassette::RigEventFixture;
pub use cassette::extract_custom_tool_names;
pub use cassette::replay_fixture_events;
pub use cassette::replay_rig_events;
pub use stream::RigEventRecorder;
pub use stream::stream_via_rig;
pub use stream::stream_via_rig_with_recording;

#[cfg(test)]
#[path = "stream_contract_tests.rs"]
mod stream_contract_tests;

#[cfg(test)]
#[path = "stream_lifecycle_tests.rs"]
mod stream_lifecycle_tests;
