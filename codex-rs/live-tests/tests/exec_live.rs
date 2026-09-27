//! Binary-level live matrix: drives the compiled `codex-exec` end to end
//! across both wire protocols for EVERY configured vendor, using the
//! unforgeable-marker technique (see `codex_live_tests::run_marker_turn`).
//! Tests are generated per vendor so nextest reports each vendor separately.
//!
//! The genai bridge is shelved: its scenarios (`chat-genai`,
//! `anthropic-genai`) only run when `LIVE_INCLUDE_GENAI=1` is set.
//!
//! Build the binary once before running:
//!
//! ```text
//! cargo build -p codex-exec --bin codex-exec
//! cargo nextest run -p codex-live-tests --test exec_live --no-capture
//! ```
//!
//! Adding a vendor: add it to the `exec_matrix!` lists below + set its
//! `LIVE_<NAME>_*` variables in `.env.local`.

use codex_live_tests::run_marker_turn;
use codex_live_tests::vendor;

/// Generates one test per (vendor, scenario). Keep the vendor list in sync
/// with `LIVE_VENDORS` in `.env.local`; unconfigured vendors skip at runtime.
macro_rules! exec_matrix {
    ($suffix:ident, [$($vendor:literal),*]) => {
        paste::paste! {
            $(
                #[tokio::test(flavor = "multi_thread")]
                async fn [<$vendor _ $suffix>]() -> anyhow::Result<()> {
                    match vendor($vendor) {
                        Some(cfg) => [<$suffix _scenario>](&cfg).await,
                        None => {
                            println!("vendor `{}` not configured — skipping", $vendor);
                            Ok(())
                        }
                    }
                }
            )*
        }
    };
}

async fn chat_genai_scenario(cfg: &codex_live_tests::LiveConfig) -> anyhow::Result<()> {
    if !codex_live_tests::genai_bridge_enabled() {
        println!(
            "genai bridge shelved (rig is the fork default) — set LIVE_INCLUDE_GENAI=1 to include"
        );
        return Ok(());
    }
    run_marker_turn(
        "chat-genai",
        cfg,
        &cfg.base_url,
        "chat",
        Some("genai"),
        "",
        Some("via genai"),
    )
    .await
}

/// Fork default for third-party Responses providers: `wire_api = "responses"`
/// with no `experimental_bridge` routes through the rig bridge, which now
/// speaks the SAME Responses wire (same-protocol passthrough). The endpoint
/// must be the vendor's explicit Responses URL — never the chat URL.
async fn responses_rig_default_scenario(cfg: &codex_live_tests::LiveConfig) -> anyhow::Result<()> {
    let Some(responses_url) = codex_live_tests::responses_url_or_skip(cfg) else {
        return Ok(());
    };
    run_marker_turn(
        "responses-rig-default",
        cfg,
        &responses_url,
        "responses",
        None,
        // Same-protocol passthrough exposes gateway capability gaps instead
        // of hiding them behind a Chat conversion: MiMo's Responses gateway
        // rejects hosted tools outright (HTTP 400 responses_feature_not_
        // supported). Disabling web_search is the explicit config-level
        // declaration; harmless for gateways that accept it (GLM).
        "web_search = \"disabled\"\n",
        Some("Dispatching responses stream via rig"),
    )
    .await
}

/// Escape hatch: `experimental_bridge = "native"` forces the upstream
/// transport even for third-party providers (asserted via the native
/// Responses SSE telemetry). Hosted tools are disabled for gateways that
/// reject them (MiMo does; harmless elsewhere).
async fn responses_native_scenario(cfg: &codex_live_tests::LiveConfig) -> anyhow::Result<()> {
    let Some(responses_url) = codex_live_tests::responses_url_or_skip(cfg) else {
        return Ok(());
    };
    run_marker_turn(
        "responses-native",
        cfg,
        &responses_url,
        "responses",
        Some("native"),
        "web_search = \"disabled\"\n",
        Some("event.kind=response.completed"),
    )
    .await
}

async fn anthropic_genai_scenario(cfg: &codex_live_tests::LiveConfig) -> anyhow::Result<()> {
    if !codex_live_tests::genai_bridge_enabled() {
        println!(
            "genai bridge shelved (rig is the fork default) — set LIVE_INCLUDE_GENAI=1 to include"
        );
        return Ok(());
    }
    let Some(anthropic_url) = codex_live_tests::anthropic_url_or_skip(cfg) else {
        return Ok(());
    };
    run_marker_turn(
        "anthropic-genai",
        cfg,
        &anthropic_url,
        "anthropic",
        Some("genai"),
        "",
        Some("via genai"),
    )
    .await
}

async fn chat_rig_scenario(cfg: &codex_live_tests::LiveConfig) -> anyhow::Result<()> {
    run_marker_turn(
        "chat-rig",
        cfg,
        &cfg.base_url,
        "chat",
        Some("rig"),
        "",
        Some("via rig"),
    )
    .await
}

async fn anthropic_rig_scenario(cfg: &codex_live_tests::LiveConfig) -> anyhow::Result<()> {
    let Some(anthropic_url) = codex_live_tests::anthropic_url_or_skip(cfg) else {
        return Ok(());
    };
    run_marker_turn(
        "anthropic-rig",
        cfg,
        &anthropic_url,
        "anthropic",
        Some("rig"),
        "",
        Some("via rig"),
    )
    .await
}

/// The fork default: without `experimental_bridge`, chat providers must be
/// served by the rig bridge (asserted via the dispatch log on stderr).
async fn chat_default_scenario(cfg: &codex_live_tests::LiveConfig) -> anyhow::Result<()> {
    run_marker_turn(
        "chat-default",
        cfg,
        &cfg.base_url,
        "chat",
        None,
        "",
        Some("via rig"),
    )
    .await
}

exec_matrix!(chat_genai, ["mimo", "glm", "step"]);
exec_matrix!(chat_rig, ["mimo", "glm", "step"]);
exec_matrix!(chat_default, ["mimo", "glm", "step"]);
exec_matrix!(responses_rig_default, ["mimo", "glm", "step"]);
exec_matrix!(responses_native, ["mimo", "glm", "step"]);
exec_matrix!(anthropic_genai, ["mimo", "glm", "step"]);
exec_matrix!(anthropic_rig, ["mimo", "glm", "step"]);
