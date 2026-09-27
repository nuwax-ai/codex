//! On-disk bridge fixtures. Recording failures must fail the calling scenario.

use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use codex_api::ResponseEvent;
use codex_api::ResponsesApiRequest;

use crate::Bridge;
use crate::LiveConfig;
use crate::repo_root;

/// Cassette mode from `LIVE_CASSETTE`: unset = live only, `record` = live
/// and persist fixtures, `replay` = require recorded fixtures without network calls.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CassetteMode {
    Off,
    Record,
    Replay,
}

pub fn cassette_mode() -> CassetteMode {
    match std::env::var("LIVE_CASSETTE").as_deref() {
        Ok("record") => CassetteMode::Record,
        Ok("replay") => CassetteMode::Replay,
        _ => CassetteMode::Off,
    }
}

/// One recorded bridge-boundary turn: the input request and returned event
/// stream, serialized under `LIVE_FIXTURE_DIR` (default: `tests/fixtures`).
/// Fixtures contain prompts and model text only — never credentials.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct TurnFixture {
    pub vendor: String,
    pub bridge: String,
    pub tag: String,
    /// The caller's request before bridge projection or protocol conversion,
    /// serialized as JSON. This is not an HTTP request body capture. Responses
    /// replay sends the current scenario input and checks its loopback wire body;
    /// freshly converted Chat history can have different generated item IDs.
    pub request: serde_json::Value,
    pub events: Vec<ResponseEvent>,
}

pub fn fixture_path(vendor: &str, bridge: &str, tag: &str) -> Option<PathBuf> {
    let root = fixture_root()?;
    Some(root.join(vendor).join(format!("{bridge}-{tag}.json")))
}

/// A separate recording root keeps a live audit independent of checked-in fixtures.
/// Both recording and replay use the same override.
fn fixture_root() -> Option<PathBuf> {
    std::env::var_os("LIVE_FIXTURE_DIR")
        .map(PathBuf::from)
        .or_else(|| repo_root().map(|root| root.join("codex-rs/live-tests/tests/fixtures")))
}

/// Loads a rig-event fixture (the intermediate events the bridge receives,
/// used for conversion-testing replay).
pub fn load_rig_event_fixture(
    vendor: &str,
    tag: &str,
) -> Result<codex_rust_rig_bridge::RigEventFixture, String> {
    let path = rig_event_fixture_path(vendor, tag)
        .ok_or_else(|| "Cannot locate Rig fixtures".to_string())?;
    let contents =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&contents).map_err(|error| format!("{}: {error}", path.display()))
}

/// Saves a rig-event fixture.
pub fn save_rig_event_fixture(
    vendor: &str,
    tag: &str,
    rig_events: &[rig_core::streaming::StreamedAssistantContent],
    custom_tools: &std::collections::HashSet<String>,
) -> Result<()> {
    let path =
        rig_event_fixture_path(vendor, tag).ok_or_else(|| anyhow!("Cannot locate Rig fixtures"))?;
    let mut custom_tools: Vec<_> = custom_tools.iter().cloned().collect();
    custom_tools.sort();
    let fixture = codex_rust_rig_bridge::RigEventFixture {
        vendor: vendor.to_string(),
        tag: tag.to_string(),
        rig_events: rig_events.to_vec(),
        custom_tools,
    };
    write_fixture(&path, &fixture)?;
    println!("[cassette-rig] recorded {}", path.display());
    Ok(())
}

fn rig_event_fixture_path(vendor: &str, tag: &str) -> Option<PathBuf> {
    let root = fixture_root()?;
    Some(root.join(vendor).join(format!("rig-events-{tag}.json")))
}

/// Path of a recorded raw Responses SSE body (wire bytes, plain text).
fn responses_sse_fixture_path(vendor: &str, tag: &str) -> Option<PathBuf> {
    let root = fixture_root()?;
    Some(root.join(vendor).join(format!("responses-sse-{tag}.txt")))
}

/// Saves a recorded raw Responses SSE body.
pub fn save_responses_sse_fixture(vendor: &str, tag: &str, sse: &str) -> Result<()> {
    let path = responses_sse_fixture_path(vendor, tag)
        .ok_or_else(|| anyhow!("Cannot locate bridge fixtures"))?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Fixture path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Create fixture directory {}", parent.display()))?;
    std::fs::write(&path, sse).with_context(|| format!("Write fixture {}", path.display()))?;
    println!("[cassette-responses] recorded {}", path.display());
    Ok(())
}

/// Loads a recorded raw Responses SSE body; replay runs it through the same
/// strict terminal policy as the live path.
pub fn load_responses_sse_fixture(vendor: &str, tag: &str) -> Result<String, String> {
    let path = responses_sse_fixture_path(vendor, tag)
        .ok_or_else(|| "Cannot locate bridge fixtures".to_string())?;
    std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))
}

pub fn load_fixture(vendor: &str, bridge: &str, tag: &str) -> Option<TurnFixture> {
    let path = fixture_path(vendor, bridge, tag)?;
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

pub fn store_fixture(fixture: &TurnFixture) -> Result<()> {
    let path = fixture_path(&fixture.vendor, &fixture.bridge, &fixture.tag)
        .ok_or_else(|| anyhow!("Cannot locate bridge fixtures"))?;
    write_fixture(&path, fixture)?;
    println!("[cassette] recorded {}", path.display());
    Ok(())
}

fn write_fixture(path: &Path, fixture: &impl serde::Serialize) -> Result<()> {
    let json = serde_json::to_string_pretty(fixture)
        .with_context(|| format!("Serialize fixture {}", path.display()))?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Fixture path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Create fixture directory {}", parent.display()))?;
    std::fs::write(path, json).with_context(|| format!("Write fixture {}", path.display()))
}

pub(crate) fn record_turn(
    cfg: &LiveConfig,
    bridge: Bridge,
    tag: &str,
    request: &ResponsesApiRequest,
    events: &[ResponseEvent],
) -> Result<()> {
    if cassette_mode() != CassetteMode::Record {
        return Ok(());
    }
    store_fixture(&TurnFixture {
        vendor: cfg.vendor.clone(),
        bridge: bridge.name().to_string(),
        tag: tag.to_string(),
        request: serde_json::to_value(request).context("Serialize recorded request")?,
        events: events.to_vec(),
    })
}

#[cfg(test)]
#[path = "cassette_tests.rs"]
mod tests;
