//! Hosted (server-tool) Anthropic wire tests, grouped by concern.
//!
//! - `common`: SSE fixtures and the capture→replay harness shared below.
//! - `request_translation`: declaring hosted tools, constraints, and
//!   tool_choice on the outbound request.
//! - `item_mapping`: turning streamed server-tool blocks into Codex items.
//! - `replay`: persisted-payload replay gating by source and opt-out.
//! - `pairing`: pair identity, dedupe, caps, late/mixed results, citations.

mod common;
mod item_mapping;
mod late_results;
mod pairing;
mod replay;
mod request_translation;

pub(crate) use common::SearchReplay;
pub(crate) use common::capture_search_replay;
