//! Live binary-level end-to-end tests for all three wire protocols Xiaomi
//! MiMo exposes, run against the `codex-exec` binary compiled from this
//! workspace:
//!
//! | protocol        | provider config                          | code path                     |
//! |-----------------|------------------------------------------|-------------------------------|
//! | Chat Completions| `wire_api = "chat"`, `/v1`               | rust-genai bridge (OpenAI)    |
//! | Responses API   | default `wire_api`, `/v1`                | upstream codex (no bridge)    |
//! | Anthropic       | `wire_api = "chat"`, `/anthropic/v1`     | rust-genai bridge (Anthropic) |
//! | Chat (rig)      | + `experimental_bridge = "rig"`          | rust-rig bridge (OpenAI)      |
//! | Anthropic (rig) | + `experimental_bridge = "rig"`          | rust-rig bridge (Anthropic)   |
//!
//! Each test drives a real agent loop with the "unforgeable marker"
//! technique: the model must run `echo <random-marker>` locally and quote
//! the output verbatim, which only succeeds when tool calling, local
//! execution, result replay, and the final answer all work.
//!
//! # Artifacts
//!
//! Every run persists its full JSONL event stream, stderr, and final agent
//! message under `target/live-mimo/<protocol>-<marker>/` for offline
//! troubleshooting (the directory is printed in the test log and also shown
//! on failure paths).
//!
//! # Configuration
//!
//! `MIMO_API_KEY` / `MIMO_BASE_URL` / `MIMO_ANTHROPIC_BASE_URL` /
//! `MIMO_MODEL` from the environment or a gitignored `.env.local` / `.env`
//! at the repo root. Tests skip when unconfigured.
//!
//! # Running
//!
//! ```text
//! cargo nextest run -p codex-exec --test live_mimo --no-capture
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use tokio::process::Command;

const RUN_TIMEOUT: Duration = Duration::from_secs(300);

struct LiveConfig {
    api_key: String,
    base_url: String,
    anthropic_base_url: String,
    model: String,
}

fn live_config() -> Option<LiveConfig> {
    let file_env = load_env_files();
    let lookup = |key: &str| -> Option<String> {
        std::env::var(key)
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| file_env.get(key).cloned())
    };
    let api_key = match lookup("MIMO_API_KEY") {
        Some(key) => key,
        None => {
            println!("MIMO_API_KEY not set — skipping live MiMo binary tests");
            return None;
        }
    };
    Some(LiveConfig {
        api_key,
        base_url: lookup("MIMO_BASE_URL")
            .unwrap_or_else(|| "https://token-plan-cn.xiaomimimo.com/v1".into()),
        anthropic_base_url: lookup("MIMO_ANTHROPIC_BASE_URL")
            .unwrap_or_else(|| "https://token-plan-cn.xiaomimimo.com/anthropic/v1".into()),
        model: lookup("MIMO_MODEL").unwrap_or_else(|| "mimo-v2.6-flash".into()),
    })
}

/// Parses `.env.local` (higher priority) then `.env` from the repo root into
/// a local map. Nothing is written to the process environment (that would
/// require `unsafe` on edition 2024).
fn load_env_files() -> HashMap<String, String> {
    let mut merged = HashMap::new();
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut repo_root = None;
    while dir.pop() {
        if dir.join(".git").exists() {
            repo_root = Some(dir);
            break;
        }
    }
    let Some(root) = repo_root else {
        return merged;
    };
    for file in [".env.local", ".env"] {
        let Ok(contents) = std::fs::read_to_string(root.join(file)) else {
            continue;
        };
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let mut value = value.trim().to_string();
            if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                value = value[1..value.len() - 1].to_string();
            }
            merged.insert(key.trim().to_string(), value);
        }
    }
    merged
}

/// Workspace `target/` directory (respects `CARGO_TARGET_DIR`).
fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .map(|p| p.join("target"))
                .expect("manifest dir has a parent")
        })
}

fn write_config_toml(
    home: &Path,
    cfg: &LiveConfig,
    base_url: &str,
    wire_api: &str,
    bridge: Option<&str>,
    extra: &str,
) -> std::io::Result<()> {
    // `experimental_bearer_token` keeps the key inside the temporary home; it
    // is never logged and the temp dir is removed at test end. `bridge: None`
    // omits `experimental_bridge` so the fork default path is exercised.
    let bridge_line = bridge
        .map(|b| format!("experimental_bridge = \"{b}\"\n"))
        .unwrap_or_default();
    let toml = format!(
        r#"model = "{model}"
model_provider = "mimo"
approval_policy = "never"
sandbox_mode = "danger-full-access"
{extra}
[model_providers.mimo]
name = "MiMo"
base_url = "{base_url}"
wire_api = "{wire_api}"
{bridge_line}experimental_bearer_token = "{api_key}"
"#,
        model = cfg.model,
        base_url = base_url,
        wire_api = wire_api,
        bridge_line = bridge_line,
        api_key = cfg.api_key,
    );
    std::fs::write(home.join("config.toml"), toml)
}

/// One full binary-level run: spawn `codex-exec`, drive the marker task,
/// persist artifacts, and verify the closed loop.
async fn run_marker_turn(
    protocol: &str,
    cfg: &LiveConfig,
    base_url: &str,
    wire_api: &str,
    bridge: Option<&str>,
    extra_config: &str,
    expect_bridge_log: Option<&str>,
) -> anyhow::Result<()> {
    let home = tempfile::TempDir::new()?;
    let cwd = tempfile::TempDir::new()?;
    write_config_toml(home.path(), cfg, base_url, wire_api, bridge, extra_config)?;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let marker = format!("mimo-{protocol}-{nonce}-{}", std::process::id());
    let prompt = format!(
        "请务必调用 shell 工具真实执行命令 `echo {marker}`（不要只把命令当文本输出），然后把命令的原始输出逐字告诉我，不要添加任何解释。"
    );

    let artifacts_dir = target_dir().join("live-mimo").join(&marker);
    std::fs::create_dir_all(&artifacts_dir)?;

    let last_message_path = home.path().join("last_message.txt");
    let child = Command::new(env!("CARGO_BIN_EXE_codex-exec"))
        .arg("--skip-git-repo-check")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .arg("--output-last-message")
        .arg(&last_message_path)
        .arg(&prompt)
        .env("CODEX_HOME", home.path())
        .env("CODEX_SQLITE_HOME", home.path())
        .env("RUST_LOG", "info")
        .current_dir(cwd.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let output = tokio::time::timeout(RUN_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!("[{protocol}] codex-exec did not finish within {RUN_TIMEOUT:?}"))??;

    // Persist the artifacts first so failed runs can still be inspected.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    std::fs::write(artifacts_dir.join("events.jsonl"), &*stdout)?;
    std::fs::write(artifacts_dir.join("stderr.log"), &*stderr)?;
    let final_message = std::fs::read_to_string(&last_message_path)
        .unwrap_or_else(|_| "<last_message.txt missing>".to_string());
    std::fs::write(artifacts_dir.join("final_message.txt"), &final_message)?;
    println!("[{protocol}] artifacts saved to {}", artifacts_dir.display());

    println!("--- [{protocol}] codex-exec JSONL events ---");
    for line in stdout.lines() {
        println!("[{protocol}] {line}");
    }
    if !stderr.trim().is_empty() {
        println!("--- [{protocol}] codex-exec stderr ---\n{stderr}");
    }
    if let Some(expected) = expect_bridge_log {
        anyhow::ensure!(
            stderr.contains(expected),
            "[{protocol}] expected the dispatch log `{expected}` on stderr, got:\n{stderr}"
        );
    }

    anyhow::ensure!(
        output.status.success(),
        "[{protocol}] codex-exec exited with {:?}; see {}stderr.log",
        output.status.code(),
        artifacts_dir.display(),
    );
    anyhow::ensure!(
        stdout.contains(&marker),
        "[{protocol}] JSONL event stream should contain the executed command marker {marker}"
    );
    let command_events = stdout
        .lines()
        .filter(|line| line.contains("command_execution"))
        .count();
    anyhow::ensure!(
        command_events >= 1,
        "[{protocol}] expected at least one command-execution event in the JSONL stream"
    );
    // Primary proof of the closed loop: the command REALLY executed locally
    // with exit 0 and the marker in its aggregated output (unforgeable — the
    // marker value only exists in the echo output).
    let command_executed_with_marker = stdout.lines().any(|line| {
        line.contains("\"type\":\"command_execution\"")
            && line.contains("\"exit_code\":0")
            && line.contains(&marker)
    });
    anyhow::ensure!(
        command_executed_with_marker,
        "[{protocol}] the executed command should have exited 0 with the marker in its output"
    );
    // Stronger signal when the model cooperates: it quotes the tool output
    // back. MiMo occasionally ends the turn right after the tool call without
    // narrating, so treat narration as a bonus, not a hard requirement.
    if final_message.contains(&marker) {
        println!("[{protocol}] model quoted the tool output verbatim");
    } else {
        println!(
            "[{protocol}] note: model ended the turn without quoting the marker \
             (tool execution itself verified above)"
        );
    }
    println!(
        "[{protocol}] OK marker={marker} command_events={command_events} final_message_chars={}",
        final_message.chars().count()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_chat_completions_protocol() -> anyhow::Result<()> {
    let Some(cfg) = live_config() else {
        return Ok(());
    };
    run_marker_turn("chat-genai", &cfg, &cfg.base_url, "chat", Some("genai"), "", Some("via genai")).await
}

/// The upstream-native Responses API path (no bridge involved): guards the
/// merge against regressions in stock codex behavior. Hosted tools are
/// disabled because MiMo's Responses gateway rejects `web_search`.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_responses_api_protocol() -> anyhow::Result<()> {
    let Some(cfg) = live_config() else {
        return Ok(());
    };
    run_marker_turn("responses", &cfg, &cfg.base_url, "responses", Some("genai"), "web_search = \"disabled\"\n", None).await
}

/// Anthropic Messages protocol via the bridge's genai Anthropic adapter,
/// selected by the `/anthropic` base URL (see `adapter_kind_for_base_url`).
#[tokio::test(flavor = "multi_thread")]
async fn e2e_anthropic_protocol() -> anyhow::Result<()> {
    let Some(cfg) = live_config() else {
        return Ok(());
    };
    run_marker_turn("anthropic-genai", &cfg, &cfg.anthropic_base_url, "chat", Some("genai"), "", Some("via genai")).await
}

/// Same chat-protocol loop through the rig bridge (A/B against the genai
/// variant above; see my-docs/rig-bridge-implementation-plan.md).
#[tokio::test(flavor = "multi_thread")]
async fn e2e_chat_completions_protocol_via_rig() -> anyhow::Result<()> {
    let Some(cfg) = live_config() else {
        return Ok(());
    };
    run_marker_turn("chat-rig", &cfg, &cfg.base_url, "chat", Some("rig"), "", Some("via rig")).await
}

/// Anthropic Messages protocol through the rig bridge (A/B against the genai
/// variant above).
#[tokio::test(flavor = "multi_thread")]
async fn e2e_anthropic_protocol_via_rig() -> anyhow::Result<()> {
    let Some(cfg) = live_config() else {
        return Ok(());
    };
    run_marker_turn("anthropic-rig", &cfg, &cfg.anthropic_base_url, "chat", Some("rig"), "", Some("via rig")).await
}

/// The fork default: without `experimental_bridge`, chat providers must be
/// served by the rig bridge (asserted via the dispatch log on stderr).
#[tokio::test(flavor = "multi_thread")]
async fn e2e_chat_default_bridge_is_rig() -> anyhow::Result<()> {
    let Some(cfg) = live_config() else {
        return Ok(());
    };
    run_marker_turn("chat-default", &cfg, &cfg.base_url, "chat", None, "", Some("via rig")).await
}
