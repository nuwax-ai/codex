//! Bridge crate that converts between Codex types (`ResponseItem`, `ResponseEvent`)
//! and rig (rig-core) types (`Message`, `StreamEvent`) for providers configured
//! with `wire_api = "chat"`.
//!
//! Sibling of `codex-rust-genai-bridge` with an identical public surface; the
//! active bridge is selected per provider via `experimental_bridge` in config.
//! See `my-docs/rig-bridge-implementation-plan.md` for the design.

mod client;
mod convert_request;
mod convert_response;
mod stream;

pub use stream::stream_via_rig;
