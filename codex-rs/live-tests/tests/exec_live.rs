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

use codex_live_tests::run_capped_marker_turn;
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

/// D2 evidence: an EXPLICIT provider output budget must reach the wire in
/// the protocol's cap field (Chat: `max_tokens` or `max_completion_tokens`),
/// asserted for every captured attempt.
async fn chat_rig_capped_scenario(cfg: &codex_live_tests::LiveConfig) -> anyhow::Result<()> {
    run_capped_marker_turn(
        "chat-rig-capped",
        cfg,
        &cfg.base_url,
        "chat",
        Some("rig"),
        /*output_cap*/ 512,
        Some("via rig"),
    )
    .await
}

/// D2 evidence on the Anthropic wire: `max_tokens` is required there, so the
/// explicit budget is asserted directly against the captured field.
async fn anthropic_rig_capped_scenario(cfg: &codex_live_tests::LiveConfig) -> anyhow::Result<()> {
    let Some(anthropic_url) = codex_live_tests::anthropic_url_or_skip(cfg) else {
        return Ok(());
    };
    run_capped_marker_turn(
        "anthropic-rig-capped",
        cfg,
        &anthropic_url,
        "anthropic",
        Some("rig"),
        /*output_cap*/ 512,
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

/// Auto-compact across all three wires: teach a unique passphrase, force a
/// local compaction on the resumed turn (token limit far below any real
/// turn), and recall it. See `run_compact_turn` for the full assertions.
async fn compact_chat_rig_scenario(cfg: &codex_live_tests::LiveConfig) -> anyhow::Result<()> {
    codex_live_tests::run_compact_turn(
        "compact-chat-rig",
        cfg,
        &cfg.base_url,
        "chat",
        Some("rig"),
        "",
    )
    .await
}

async fn compact_anthropic_rig_scenario(cfg: &codex_live_tests::LiveConfig) -> anyhow::Result<()> {
    let Some(anthropic_url) = codex_live_tests::anthropic_url_or_skip(cfg) else {
        return Ok(());
    };
    codex_live_tests::run_compact_turn(
        "compact-anthropic-rig",
        cfg,
        &anthropic_url,
        "anthropic",
        Some("rig"),
        "",
    )
    .await
}

async fn compact_responses_rig_scenario(cfg: &codex_live_tests::LiveConfig) -> anyhow::Result<()> {
    let Some(responses_url) = codex_live_tests::responses_url_or_skip(cfg) else {
        return Ok(());
    };
    codex_live_tests::run_compact_turn(
        "compact-responses-rig",
        cfg,
        &responses_url,
        "responses",
        None,
        // MiMo's Responses gateway rejects hosted tools (see
        // responses_rig_default_scenario); harmless for GLM.
        "web_search = \"disabled\"\n",
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
exec_matrix!(chat_rig_capped, ["mimo", "glm"]);
exec_matrix!(anthropic_rig_capped, ["mimo", "glm"]);
/// Cross-turn web_search on the Anthropic wire: turn 2 replays turn 1's
/// history with the WebSearchCall items dropped (pinned behavior); the live
/// gateway accepting both turns is the baseline for the phase-3 replay.
async fn websearch_anthropic_rig_scenario(
    cfg: &codex_live_tests::LiveConfig,
) -> anyhow::Result<()> {
    let Some(anthropic_url) = codex_live_tests::anthropic_url_or_skip(cfg) else {
        return Ok(());
    };
    codex_live_tests::run_websearch_turns(
        "websearch-anthropic-rig",
        cfg,
        &anthropic_url,
        Some("rig"),
    )
    .await
}

// Hosted web search on the Anthropic wire — vendor capability matrix
// (live-verified 2026-10-01, artifacts under logs/live-<vendor>/websearch-*):
//   glm:  executes server-side; result pairs replay on turn 2 (PASS).
//   mimo: echoes the declared server tool back as a CLIENT tool_use —
//         core rejects it ("unsupported call: web_search") and the model
//         falls back to shell lookups; no hosted search happens.
//   step: rejects the web_search_20250305 declaration outright
//         (400 input_invalid); the no-search anthropic scenario passes,
//         isolating the declaration as the trigger.
// Both are gateway capability boundaries, not bridge defects; re-add a
// vendor here once its gateway runs the tool server-side.
exec_matrix!(websearch_anthropic_rig, ["glm"]);
exec_matrix!(compact_chat_rig, ["mimo", "glm", "step"]);
exec_matrix!(compact_anthropic_rig, ["mimo", "glm", "step"]);
exec_matrix!(compact_responses_rig, ["mimo", "glm", "step"]);
