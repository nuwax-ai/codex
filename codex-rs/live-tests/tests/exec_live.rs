//! Binary-level live suite: drives the compiled `codex-exec` end to end
//! across all three MiMo wire protocols and both chat bridges, using the
//! unforgeable-marker technique (see `codex_live_tests::run_marker_turn`).
//!
//! Build the binary once before running:
//!
//! ```text
//! cargo build -p codex-exec --bin codex-exec
//! cargo nextest run -p codex-live-tests --test exec_live --no-capture
//! ```
//!
//! Skipped when `MIMO_API_KEY` is not configured.

use codex_live_tests::live_config;
use codex_live_tests::run_marker_turn;

#[tokio::test(flavor = "multi_thread")]
async fn e2e_chat_completions_via_genai() -> anyhow::Result<()> {
    let Some(cfg) = live_config() else {
        return Ok(());
    };
    run_marker_turn(
        "chat-genai",
        &cfg,
        &cfg.base_url,
        "chat",
        Some("genai"),
        "",
        Some("via genai"),
    )
    .await
}

/// Fork default for third-party Responses providers: `wire_api = "responses"`
/// with no `experimental_bridge` routes through the rig bridge, which drops
/// hosted tools MiMo does not support (no `web_search = "disabled"` needed).
#[tokio::test(flavor = "multi_thread")]
async fn e2e_responses_via_rig_default() -> anyhow::Result<()> {
    let Some(cfg) = live_config() else {
        return Ok(());
    };
    run_marker_turn(
        "responses-rig-default",
        &cfg,
        &cfg.base_url,
        "responses",
        None,
        "",
        Some("via rig"),
    )
    .await
}

/// Escape hatch: `experimental_bridge = "native"` forces the upstream
/// transport even for third-party providers (asserted via the native
/// Responses SSE telemetry instead of the bridge dispatch log). Hosted
/// tools are disabled because MiMo's Responses gateway rejects `web_search`.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_responses_native_escape_hatch() -> anyhow::Result<()> {
    let Some(cfg) = live_config() else {
        return Ok(());
    };
    run_marker_turn(
        "responses-native",
        &cfg,
        &cfg.base_url,
        "responses",
        Some("native"),
        "web_search = \"disabled\"\n",
        Some("event.kind=response.completed"),
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_anthropic_via_genai() -> anyhow::Result<()> {
    let Some(cfg) = live_config() else {
        return Ok(());
    };
    run_marker_turn(
        "anthropic-genai",
        &cfg,
        &cfg.anthropic_base_url,
        "chat",
        Some("genai"),
        "",
        Some("via genai"),
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_chat_completions_via_rig() -> anyhow::Result<()> {
    let Some(cfg) = live_config() else {
        return Ok(());
    };
    run_marker_turn(
        "chat-rig",
        &cfg,
        &cfg.base_url,
        "chat",
        Some("rig"),
        "",
        Some("via rig"),
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_anthropic_via_rig() -> anyhow::Result<()> {
    let Some(cfg) = live_config() else {
        return Ok(());
    };
    run_marker_turn(
        "anthropic-rig",
        &cfg,
        &cfg.anthropic_base_url,
        "chat",
        Some("rig"),
        "",
        Some("via rig"),
    )
    .await
}

/// The fork default: without `experimental_bridge`, chat providers must be
/// served by the rig bridge (asserted via the dispatch log on stderr).
#[tokio::test(flavor = "multi_thread")]
async fn e2e_chat_default_bridge_is_rig() -> anyhow::Result<()> {
    let Some(cfg) = live_config() else {
        return Ok(());
    };
    run_marker_turn(
        "chat-default",
        &cfg,
        &cfg.base_url,
        "chat",
        None,
        "",
        Some("via rig"),
    )
    .await
}
