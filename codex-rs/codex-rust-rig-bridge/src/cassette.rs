//! Bridge-boundary cassette: records the rig events a bridge receives and
//! replays them through the CURRENT conversion code, so bridge refactors
//! and rig upgrades can be regression-tested offline.
//!
//! This sits at a deliberate boundary: rig's SSE → `StreamedAssistantContent`
//! parsing is rig's responsibility (tested by rig itself); OUR conversion
//! (`StreamedAssistantContent` → codex `ResponseEvent`) is what this cassette
//! exercises. Recording captures the intermediate events; replay feeds them
//! into the live conversion functions — a conversion bug introduced after
//! recording makes replay fail.

use std::collections::HashSet;
use std::sync::Arc;

use codex_api::ResponseEvent;
use rig_core::streaming::StreamedAssistantContent;

use crate::convert_response::PendingRigMessage;
use crate::convert_response::rig_event_to_response_events;

/// One recorded turn at the rig-event boundary.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct RigEventFixture {
    pub vendor: String,
    pub tag: String,
    /// The rig events exactly as the stream delivered them, serialized with
    /// rig's own serde derives. No credentials ever appear here (auth lives
    /// in HTTP headers, which are not captured at this boundary).
    pub rig_events: Vec<StreamedAssistantContent>,
    /// Names of custom (freeform) tools declared in the recorded request.
    /// Replay needs these to restore CustomToolCall items instead of
    /// FunctionCall for tools that were declared as custom.
    #[serde(default)]
    pub custom_tools: Vec<String>,
}

/// Replays recorded rig events through the CURRENT bridge conversion code,
/// producing the codex events the current implementation would emit.
///
/// This is the core value of the Rig-event cassette: even after the
/// bridge or rig is refactored, the same input events must produce the
/// same output events — any divergence is a conversion regression.
pub fn replay_rig_events(
    rig_events: &[StreamedAssistantContent],
    custom_tool_names: HashSet<String>,
) -> Result<Vec<ResponseEvent>, codex_api::ApiError> {
    let mut pending = PendingRigMessage::new(Arc::new(custom_tool_names), "cassette".into());
    let mut out = Vec::new();
    for event in rig_events {
        out.extend(rig_event_to_response_events(event.clone(), &mut pending)?);
    }
    if !pending.completed_emitted() {
        return Err(codex_api::ApiError::Stream(
            "Rig cassette ended without a terminal record".into(),
        ));
    }
    Ok(out)
}

/// Convenience wrapper that replays from a fixture without custom tools
/// (the common scenario for recorded scenarios).
pub fn replay_fixture_events(
    fixture: &RigEventFixture,
) -> Result<Vec<ResponseEvent>, codex_api::ApiError> {
    let custom_tools: HashSet<String> = fixture.custom_tools.iter().cloned().collect();
    replay_rig_events(&fixture.rig_events, custom_tools)
}

/// Extracts the names of custom (freeform) tools from a request's tool
/// list, for recording into the fixture alongside the rig events.
pub fn extract_custom_tool_names(request: &codex_api::ResponsesApiRequest) -> HashSet<String> {
    crate::request_tools::request_tools(request)
        .meta
        .custom_names
}
