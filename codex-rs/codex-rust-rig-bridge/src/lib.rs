//! Bridge crate that converts between Codex types (`ResponseItem`, `ResponseEvent`)
//! and rig (rig-core) types (`Message`, `StreamEvent`) for providers configured
//! with `wire_api = "chat"`.
//!
//! Sibling of `codex-rust-genai-bridge` with an identical public surface; the
//! active bridge is selected per provider via `experimental_bridge` in config.
//! See `my-docs/rig-bridge-implementation-plan.md` for the design.

mod bridge_impl;
mod client;
mod convert_request;
mod convert_response;
mod reasoning;
mod request_messages;
mod request_tools;
mod response_tools;
mod stream;
mod transport;

pub use bridge_impl::RigChatBridge;
pub use client::RigProtocol;
pub use client::protocol_for_base_url;
pub use reasoning::REPLAY_PREFIX;
pub use reasoning::is_replay_envelope;
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
