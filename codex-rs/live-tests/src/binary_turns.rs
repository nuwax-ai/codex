//! Binary-level (L3) end-to-end scenarios driving the compiled
//! `codex-exec`: marker turns, compaction recovery, web-search turns.

use super::*;
use artifacts::PreparedExec;
use runner::ProcessRunner;
use scenarios::BinaryScenario;
use scenarios::Scene;

mod rollouts;
mod runner;
mod scenarios;

// ================================================================
// Binary-level (L3) end-to-end
// ================================================================

/// Locates the `codex-exec` binary. Cargo does not build binaries of other
/// workspace members for this crate's tests, so resolution walks the target
/// directories (honoring `CARGO_TARGET_DIR`); callers fail fast with build
/// instructions when the binary has not been built yet.
///
/// Warns loudly when the binary is older than the sources that feed the
/// bridges — a stale binary silently testing old code burned us once
/// (unknown config variant), so the mtime check makes it visible.
pub fn codex_exec_binary() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_codex-exec") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    let Some(root) = repo_root() else {
        return Err(anyhow!("cannot locate repository root"));
    };
    // NOTE: the resolved binary is whatever the target dir holds. Building
    // it inside the test thrashes workspace feature unification (the test
    // graph and the binary graph alternate fingerprints, recompiling core
    // both ways), so freshness is the CALLER's contract: build codex-exec in
    // the same cargo invocation as the tests, or point CARGO_BIN_EXE_codex-exec
    // at a known binary. The manifest hashes the executable; its source
    // revision remains unknown without an executable build receipt.
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("codex-rs").join("target"));
    let binary = ["debug", "release"]
        .iter()
        .map(|profile| target.join(profile).join("codex-exec"))
        .find(|path| path.is_file())
        .ok_or_else(|| {
            anyhow!(
                "codex-exec binary not found under {}; run \
                 `cargo build -p codex-exec --bin codex-exec` first",
                target.display()
            )
        })?;
    warn_if_stale(&binary, &root);
    Ok(binary)
}

/// Prints a prominent warning when the binary predates recent changes in the
/// crates it embeds, so a red suite is not misread as a code regression.
fn warn_if_stale(binary: &Path, root: &Path) {
    let Ok(binary_mtime) = binary.metadata().and_then(|m| m.modified()) else {
        return;
    };
    let watched = [
        "codex-rs/core/src",
        "codex-rs/codex-rust-rig-bridge/src",
        "codex-rs/codex-rust-genai-bridge/src",
        "codex-rs/model-provider-info/src",
        "codex-rs/exec/src",
    ];
    let mut newer = Vec::new();
    for rel in watched {
        let dir = root.join(rel);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Ok(mtime) = entry.metadata().and_then(|m| m.modified())
                && mtime > binary_mtime
            {
                newer.push(entry.path());
            }
        }
    }
    if !newer.is_empty() {
        println!(
            "⚠️  codex-exec binary is older than {} source file(s) under the bridge crates \
             (e.g. {}); the binary-level suite may be testing stale code. \
             Rebuild with: cargo build -p codex-exec --bin codex-exec",
            newer.len(),
            newer[0].display()
        );
    }
}

/// Writes the provider config for one binary-level run into `home/config.toml`.
/// `bridge: None` omits `experimental_bridge` so the fork default path is
/// exercised. The bearer token only ever lives inside the temporary home.
#[allow(clippy::too_many_arguments)]
pub fn write_config_toml(
    home: &Path,
    cfg: &LiveConfig,
    base_url: &str,
    wire_api: &str,
    bridge: Option<&str>,
    extra: &str,
) -> std::io::Result<()> {
    let bridge_line = bridge
        .map(|b| format!("experimental_bridge = \"{b}\"\n"))
        .unwrap_or_default();
    let toml = format!(
        r#"model = "{model}"
model_provider = "{vendor}"
approval_policy = "never"
sandbox_mode = "danger-full-access"
{extra}
[model_providers.{vendor}]
name = "{vendor}"
base_url = "{base_url}"
wire_api = "{wire_api}"
{bridge_line}experimental_bearer_token = "{api_key}"
"#,
        model = cfg.model,
        vendor = cfg.vendor,
        base_url = base_url,
        wire_api = wire_api,
        bridge_line = bridge_line,
        api_key = cfg.api_key,
    );
    std::fs::write(home.join("config.toml"), toml)
}

/// Runs one executable scene and retains partial rollouts on every failure.
#[allow(clippy::too_many_arguments)]
pub async fn run_marker_turn(
    protocol: &str,
    cfg: &LiveConfig,
    base_url: &str,
    wire_api: &str,
    bridge: Option<&str>,
    extra_config: &str,
    expect_bridge_log: Option<&str>,
) -> Result<()> {
    run_binary_scene(
        protocol,
        cfg,
        base_url,
        wire_api,
        bridge,
        extra_config,
        BinaryScenario::Marker { expect_bridge_log },
    )
    .await
}

/// Teaches a unique passphrase and checks recall after a persisted compaction.
pub async fn run_compact_turn(
    protocol: &str,
    cfg: &LiveConfig,
    base_url: &str,
    wire_api: &str,
    bridge: Option<&str>,
    extra_config: &str,
) -> Result<()> {
    run_binary_scene(
        protocol,
        cfg,
        base_url,
        wire_api,
        bridge,
        &format!("{extra_config}model_auto_compact_token_limit = 200\n"),
        BinaryScenario::Compact,
    )
    .await
}

/// Requires a new completed hosted search on both resumed turns and records citations.
pub async fn run_websearch_turns(
    protocol: &str,
    cfg: &LiveConfig,
    base_url: &str,
    bridge: Option<&str>,
) -> Result<()> {
    run_binary_scene(
        protocol,
        cfg,
        base_url,
        "anthropic",
        bridge,
        "web_search = \"live\"\n",
        BinaryScenario::WebSearch,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_binary_scene(
    protocol: &str,
    cfg: &LiveConfig,
    base_url: &str,
    wire_api: &str,
    bridge: Option<&str>,
    extra_config: &str,
    scenario: BinaryScenario<'_>,
) -> Result<()> {
    anyhow::ensure!(
        cassette_mode() != CassetteMode::Replay,
        "exec_live cannot replay; use --test bridge_live for offline replay"
    );
    let home = tempfile::TempDir::new()?;
    let cwd = tempfile::TempDir::new()?;
    write_config_toml(home.path(), cfg, base_url, wire_api, bridge, extra_config)?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let marker = format!("{}-{protocol}-{nonce}-{}", cfg.vendor, std::process::id());
    let artifacts_dir = repo_root()
        .ok_or_else(|| anyhow!("cannot locate repository root"))?
        .join("logs")
        .join(format!("live-{}", cfg.vendor))
        .join(&marker);
    std::fs::create_dir_all(&artifacts_dir)?;
    let prepared = write_manifest(&artifacts_dir, cfg, protocol, bridge)?;
    if let Some(parent) = artifacts_dir.parent() {
        prune_artifacts(parent.to_path_buf());
    }
    scenarios::run(
        &ProcessRunner,
        &Scene {
            prepared: &prepared,
            home: home.path(),
            cwd: cwd.path(),
            artifacts: &artifacts_dir,
            protocol,
            marker: &marker,
            expected_model: &cfg.model,
            expected_url_prefix: base_url,
        },
        scenario,
    )
    .await
}

/// Retention is secondary evidence work: a failure never replaces the original error.
pub(crate) fn retain_rollouts_on_failure<T>(
    home: &Path,
    artifacts_rollout_dir: &Path,
    outcome: Result<T>,
) -> Result<T> {
    if outcome.is_err()
        && let Err(error) = rollouts::retain_best_effort(home, artifacts_rollout_dir)
    {
        eprintln!("warn: retain partial rollouts: {error}");
    }
    outcome
}

#[cfg(test)]
#[path = "binary_turns_tests/mod.rs"]
mod tests;
