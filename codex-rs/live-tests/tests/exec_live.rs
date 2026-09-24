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

/// The upstream-native Responses API path (no bridge involved): guards the
/// fork against regressions in stock codex behavior. Hosted tools are
/// disabled because MiMo's Responses gateway rejects `web_search`.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_responses_api() -> anyhow::Result<()> {
    let Some(cfg) = live_config() else {
        return Ok(());
    };
    run_marker_turn(
        "responses",
        &cfg,
        &cfg.base_url,
        "responses",
        Some("genai"),
        "web_search = \"disabled\"\n",
        None,
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
