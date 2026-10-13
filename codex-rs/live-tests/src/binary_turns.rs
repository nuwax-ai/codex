//! Binary-level (L3) end-to-end scenarios driving the compiled
//! `codex-exec`: marker turns, compaction recovery, web-search turns.

use super::*;
use artifacts::PreparedExec;
use runner::ProcessRunner;
use scenarios::BinaryScenario;
use scenarios::Scene;

mod capture_validation;
mod credential_scan;
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
    // Windows executables carry the .exe suffix the resolver would otherwise
    // miss (CI run 38249075905: the build succeeded, the finder found
    // nothing).
    let binary_name = if cfg!(windows) {
        "codex-exec.exe"
    } else {
        "codex-exec"
    };
    let binary = ["debug", "release"]
        .iter()
        .map(|profile| target.join(profile).join(binary_name))
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
    provider_cap: Option<u64>,
) -> std::io::Result<()> {
    let bridge_line = bridge
        .map(|b| format!("experimental_bridge = \"{b}\"\n"))
        .unwrap_or_default();
    let cap_line = provider_cap
        .map(|cap| format!("max_output_tokens = {cap}\n"))
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
{bridge_line}{cap_line}experimental_bearer_token = "{api_key}"
"#,
        model = cfg.model,
        vendor = cfg.vendor,
        base_url = base_url,
        wire_api = wire_api,
        bridge_line = bridge_line,
        cap_line = cap_line,
        api_key = cfg.api_key,
    );
    std::fs::write(home.join("config.toml"), toml)
}

/// How the child process learns its provider: the temporary config.toml
/// (existing mode) or the typed `NUWAX_*` subprocess environment only.
#[derive(Debug)]
pub enum Startup<'a> {
    /// `model_providers.<vendor>` + `experimental_bearer_token` written into
    /// the temporary home's config.toml.
    Config {
        wire_api: &'a str,
        bridge: Option<&'a str>,
        output_cap: Option<u64>,
    },
    /// The `NUWAX_*` startup group passed to the child environment only;
    /// config.toml carries no provider section and no credential.
    NuwaxEnv {
        wire_api: &'a str,
        output_cap: Option<u64>,
        request_max_retries: Option<i64>,
        stream_max_retries: Option<i64>,
        stream_idle_timeout_ms: Option<i64>,
    },
}

impl Startup<'_> {
    fn env_vars(&self, cfg: &LiveConfig, base_url: &str) -> Vec<(String, String)> {
        match self {
            Startup::Config { .. } => Vec::new(),
            Startup::NuwaxEnv {
                wire_api,
                output_cap,
                request_max_retries,
                stream_max_retries,
                stream_idle_timeout_ms,
            } => {
                let mut vars = vec![
                    ("NUWAX_BASE_URL".to_string(), base_url.to_string()),
                    ("NUWAX_WIRE_API".to_string(), wire_api.to_string()),
                    ("NUWAX_API_KEY".to_string(), cfg.api_key.clone()),
                    ("NUWAX_MODEL".to_string(), cfg.model.clone()),
                ];
                let mut optional = [
                    (
                        "NUWAX_MAX_OUTPUT_TOKENS",
                        output_cap.map(|cap| cap.to_string()),
                    ),
                    (
                        "NUWAX_REQUEST_MAX_RETRIES",
                        request_max_retries.map(|value| value.to_string()),
                    ),
                    (
                        "NUWAX_STREAM_MAX_RETRIES",
                        stream_max_retries.map(|value| value.to_string()),
                    ),
                    (
                        "NUWAX_STREAM_IDLE_TIMEOUT_MS",
                        stream_idle_timeout_ms.map(|value| value.to_string()),
                    ),
                ];
                vars.extend(optional.iter_mut().filter_map(|(name, value)| {
                    value.take().map(|value| (name.to_string(), value))
                }));
                vars
            }
        }
    }
}

/// Writes the NON-provider settings shared by both startup modes into
/// `home/config.toml`. In typed environment mode this is everything the
/// child reads from config — no provider table, no credential.
fn write_base_config_toml(home: &Path, extra: &str) -> std::io::Result<()> {
    let toml = format!(
        r#"approval_policy = "never"
sandbox_mode = "danger-full-access"
{extra}
"#
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
        /*output_cap*/ None,
        BinaryScenario::Marker { expect_bridge_log },
    )
    .await
}

/// Marker turn with an EXPLICIT provider output budget: the wire must carry
/// the cap in the protocol's field (Chat may spell it `max_tokens` or
/// `max_completion_tokens`), asserted per captured attempt.
#[allow(clippy::too_many_arguments)]
pub async fn run_capped_marker_turn(
    protocol: &str,
    cfg: &LiveConfig,
    base_url: &str,
    wire_api: &str,
    bridge: Option<&str>,
    output_cap: u64,
    expect_bridge_log: Option<&str>,
) -> Result<()> {
    run_binary_scene(
        protocol,
        cfg,
        base_url,
        wire_api,
        bridge,
        "",
        Some(output_cap),
        BinaryScenario::Marker { expect_bridge_log },
    )
    .await
}

/// Live cap EXHAUSTION: an output budget small enough that the real vendor
/// truncates the turn; the wire cap is asserted per captured attempt and the
/// process must fail through the product's cap terminal instead of
/// completing. Counted separately from cap-on-wire (`run_capped_marker_turn`)
/// and plain marker evidence.
#[allow(clippy::too_many_arguments)]
pub async fn run_cap_exhausted_turn(
    protocol: &str,
    cfg: &LiveConfig,
    base_url: &str,
    wire_api: &str,
    bridge: Option<&str>,
    output_cap: u64,
) -> Result<()> {
    run_binary_scene(
        protocol,
        cfg,
        base_url,
        wire_api,
        bridge,
        "",
        Some(output_cap),
        BinaryScenario::CapExhausted,
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
        /*output_cap*/ None,
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
        /*output_cap*/ None,
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
    output_cap: Option<u64>,
    scenario: BinaryScenario<'_>,
) -> Result<()> {
    run_scene_with_startup(
        protocol,
        cfg,
        base_url,
        &Startup::Config {
            wire_api,
            bridge,
            output_cap,
        },
        extra_config,
        scenario,
    )
    .await
}

/// Typed `NUWAX_*` environment startup: the provider group (base URL, wire,
/// credential, model and the optional cap/retry/idle budget) reaches the
/// child ONLY through its process environment — config.toml carries no
/// provider section and no credential. Shares the manifest, bounded capture,
/// artifacts and scene assertions with the config mode; additionally proves
/// after a successful scene that neither the credential nor the ephemeral
/// provider definition leaked into config, rollouts or artifacts.
pub async fn run_nuwax_env_marker_turn(
    protocol: &str,
    cfg: &LiveConfig,
    base_url: &str,
    wire_api: &str,
    output_cap: Option<u64>,
    extra_config: &str,
    expect_bridge_log: Option<&str>,
    http_attempt_budget: Option<usize>,
) -> Result<()> {
    let home = tempfile::TempDir::new()?;
    let cwd = tempfile::TempDir::new()?;
    let startup = Startup::NuwaxEnv {
        wire_api,
        output_cap,
        // Live acceptance sends each sampling once. Provider continuation
        // requests are a separate budget; these controls do not cap them.
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        stream_idle_timeout_ms: Some(45_000),
    };
    run_scene(
        protocol,
        cfg,
        base_url,
        &startup,
        extra_config,
        BinaryScenario::Marker { expect_bridge_log },
        &home,
        &cwd,
        http_attempt_budget,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_scene_with_startup(
    protocol: &str,
    cfg: &LiveConfig,
    base_url: &str,
    startup: &Startup<'_>,
    extra_config: &str,
    scenario: BinaryScenario<'_>,
) -> Result<()> {
    let home = tempfile::TempDir::new()?;
    let cwd = tempfile::TempDir::new()?;
    run_scene(
        protocol,
        cfg,
        base_url,
        startup,
        extra_config,
        scenario,
        &home,
        &cwd,
        /*http_attempt_budget*/ None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_scene(
    protocol: &str,
    cfg: &LiveConfig,
    base_url: &str,
    startup: &Startup<'_>,
    extra_config: &str,
    scenario: BinaryScenario<'_>,
    home: &tempfile::TempDir,
    cwd: &tempfile::TempDir,
    http_attempt_budget: Option<usize>,
) -> Result<()> {
    anyhow::ensure!(
        cassette_mode() != CassetteMode::Replay,
        "exec_live cannot replay; use --test bridge_live for offline replay"
    );
    let startup_env = startup.env_vars(cfg, base_url);
    let output_cap = match startup {
        Startup::Config { output_cap, .. } => *output_cap,
        Startup::NuwaxEnv { output_cap, .. } => *output_cap,
    };
    let wire_api = match startup {
        Startup::Config { wire_api, .. } => {
            write_config_toml(
                home.path(),
                cfg,
                base_url,
                wire_api,
                bridge_of(startup),
                extra_config,
                output_cap,
            )?;
            *wire_api
        }
        Startup::NuwaxEnv { wire_api, .. } => {
            write_base_config_toml(home.path(), extra_config)?;
            wire_api
        }
    };
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let marker = format!("{}-{protocol}-{nonce}-{}", cfg.vendor, std::process::id());
    let artifacts_dir = repo_root()
        .ok_or_else(|| anyhow!("cannot locate repository root"))?
        .join("logs")
        .join(format!("live-{}", cfg.vendor))
        .join(&marker);
    std::fs::create_dir_all(&artifacts_dir)?;
    let prepared = write_manifest(&artifacts_dir, cfg, protocol, bridge_of(startup))?;
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
            wire: match wire_api {
                "responses" => codex_rust_rig_bridge::RigProtocol::Responses,
                "chat" => codex_rust_rig_bridge::RigProtocol::Chat,
                "anthropic" => codex_rust_rig_bridge::RigProtocol::Anthropic,
                _ => anyhow::bail!("unsupported live scene wire API"),
            },
            capture_requirement: match startup {
                // The environment group never sets experimental_bridge, so
                // every attempt goes through rig and must leave a capture.
                Startup::NuwaxEnv { .. } => capture_validation::CaptureRequirement::Required,
                Startup::Config {
                    bridge: Some("native" | "genai"),
                    ..
                } => capture_validation::CaptureRequirement::Optional,
                Startup::Config { bridge: None, .. } if cfg.vendor == "openai" => {
                    capture_validation::CaptureRequirement::Optional
                }
                Startup::Config { .. } => capture_validation::CaptureRequirement::Required,
            },
            expected_cap: match (wire_api, output_cap) {
                (_, Some(cap)) => capture_validation::CapExpectation::Explicit(cap),
                ("anthropic", None) => capture_validation::CapExpectation::AnthropicDefault,
                (_, None) => capture_validation::CapExpectation::Absent,
            },
            startup_env: &startup_env,
            http_attempt_budget,
        },
        scenario,
    )
    .await?;
    // The typed environment mode's own contract: after a SUCCESSFUL scene,
    // the credential and the ephemeral provider definition must be absent
    // from the child's config, its rollouts AND the retained artifacts.
    // Failure paths return the scene's own error untouched — evidence
    // always beats a secondary scan failure.
    if matches!(startup, Startup::NuwaxEnv { .. }) {
        assert_no_credential_leak(home.path(), cwd.path(), &artifacts_dir, cfg)?;
    }
    Ok(())
}

fn bridge_of<'a>(startup: &Startup<'a>) -> Option<&'a str> {
    match startup {
        Startup::Config { bridge, .. } => *bridge,
        Startup::NuwaxEnv { .. } => None,
    }
}

/// The typed environment mode's non-leak contract, executed after a
/// successful scene: the credential and the ephemeral provider definition
/// must be absent from the child's home, its working directory and every
/// retained artifact (manifest, events, stderr, request capture).
fn assert_no_credential_leak(
    home: &Path,
    cwd: &Path,
    artifacts: &Path,
    cfg: &LiveConfig,
) -> Result<()> {
    credential_scan::verify(&[home, cwd, artifacts], &cfg.api_key)
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
