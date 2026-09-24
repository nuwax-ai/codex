//! Shared support for the live integration tests of the chat bridges.
//!
//! Everything here talks to REAL providers and is vendor-agnostic: configure
//! one via `LIVE_VENDOR_*` (environment, or a gitignored `.env.local` /
//! `.env` at the repository root; Xiaomi MiMo is the default and the
//! `MIMO_*` variables remain as fallbacks). Adding a vendor is pure
//! configuration — no code changes. Two suites live in this crate:
//!
//! - `tests/bridge_live.rs` — bridge level: `ResponsesApiRequest` → bridge →
//!   provider SSE → codex `ResponseEvent`s, with protocol invariants asserted
//!   on real streams; runs the same cases through both bridges for A/B.
//! - `tests/exec_live.rs` — binary level: the compiled `codex-exec` driven
//!   end to end with the unforgeable-marker technique, covering both
//!   `wire_api` values: chat (always via a bridge) and responses (fork
//!   default: third-party providers also go through the rig bridge;
//!   `experimental_bridge = "native"` opts back into the upstream
//!   transport).
//!
//! Every run persists artifacts under `<repo>/logs/live-<vendor>/`
//! (gitignored): per-turn bridge event logs in `bridge/`, and per-run
//! `events.jsonl` / `final_message.txt` / `stderr.log` for the binary suite.
//!
//! NOTE: rebuild the binary after changing core/provider code, or the
//! binary-level suite tests the stale executable:
//! `cargo build -p codex-exec --bin codex-exec`.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::anyhow;
use anyhow::Result;
use codex_api::ApiError;
use codex_api::AuthProvider;
use codex_api::Provider;
use codex_api::ResponseEvent;
use codex_api::ResponsesApiRequest;
use codex_api::ResponseStream;
use codex_api::RetryConfig;
use codex_api::SharedAuthProvider;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use http::HeaderMap;
use http::header::AUTHORIZATION;
use tokio::time::timeout;

const TURN_TIMEOUT: Duration = Duration::from_secs(180);
const EXEC_RUN_TIMEOUT: Duration = Duration::from_secs(300);

// ================================================================
// Configuration
// ================================================================

pub struct LiveConfig {
    /// Vendor tag: names the provider in generated config and the artifact
    /// directory (`logs/live-<vendor>/`). Defaults to `mimo`.
    pub vendor: String,
    pub api_key: String,
    pub base_url: String,
    pub anthropic_base_url: String,
    pub model: String,
}

/// Loads `MIMO_*` configuration: environment variables first, then
/// `.env.local` / `.env` at the repository root. Returns `None` (caller
/// skips the test) when `MIMO_API_KEY` is unset.
pub fn live_config() -> Option<LiveConfig> {
    let file_env = load_env_files();
    let lookup = |key: &str| -> Option<String> {
        std::env::var(key)
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| file_env.get(key).cloned())
    };
    let vendor = lookup("LIVE_VENDOR_NAME").unwrap_or_else(|| "mimo".into());
    let api_key = lookup("LIVE_VENDOR_API_KEY")
        .or_else(|| lookup("MIMO_API_KEY"));
    let api_key = match api_key {
        Some(key) => key,
        None => {
            println!("LIVE_VENDOR_API_KEY (or MIMO_API_KEY) not set — skipping live test");
            return None;
        }
    };
    // Adding a vendor is pure configuration: set LIVE_VENDOR_* in .env.local
    // (key, chat URL, optional Anthropic URL, model) — no code changes. The
    // MIMO_* fallbacks keep the original Xiaomi MiMo defaults working.
    Some(LiveConfig {
        api_key,
        base_url: lookup("LIVE_VENDOR_CHAT_URL")
            .or_else(|| lookup("MIMO_BASE_URL"))
            .unwrap_or_else(|| "https://token-plan-cn.xiaomimimo.com/v1".into()),
        anthropic_base_url: lookup("LIVE_VENDOR_ANTHROPIC_URL")
            .or_else(|| lookup("MIMO_ANTHROPIC_BASE_URL"))
            .unwrap_or_else(|| "https://token-plan-cn.xiaomimimo.com/anthropic/v1".into()),
        model: lookup("LIVE_VENDOR_MODEL")
            .or_else(|| lookup("MIMO_MODEL"))
            .unwrap_or_else(|| "mimo-v2.6-flash".into()),
        vendor,
    })
}

/// Parses `.env.local` (higher priority) then `.env` from the repository
/// root into a local map. Nothing is written to the process environment
/// (that would require `unsafe` on edition 2024).
pub fn load_env_files() -> HashMap<String, String> {
    let mut merged = HashMap::new();
    let Some(root) = repo_root() else {
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

/// Nearest ancestor of this crate's manifest containing `.git`.
pub fn repo_root() -> Option<PathBuf> {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    while dir.pop() {
        if dir.join(".git").exists() {
            return Some(dir);
        }
    }
    None
}

// ================================================================
// Shared fixtures
// ================================================================

/// Bearer-token [`AuthProvider`] backed by the static key from `.env.local`.
pub struct StaticBearerAuth(pub String);

impl AuthProvider for StaticBearerAuth {
    fn add_auth_headers(&self, headers: &mut HeaderMap) {
        let value = format!("Bearer {}", self.0)
            .parse()
            .expect("valid authorization header value");
        headers.insert(AUTHORIZATION, value);
    }
}

pub fn shared_auth(api_key: &str) -> SharedAuthProvider {
    Arc::new(StaticBearerAuth(api_key.to_string()))
}

pub fn vendor_provider(vendor: &str, base_url: &str) -> Provider {
    Provider {
        name: vendor.to_string(),
        base_url: base_url.to_string(),
        query_params: None,
        headers: HeaderMap::new(),
        retry: RetryConfig {
            max_attempts: 1,
            base_delay: Duration::ZERO,
            retry_429: false,
            retry_5xx: false,
            retry_transport: false,
        },
        stream_idle_timeout: Duration::from_secs(120),
    }
}

pub fn user_message(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "user".into(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

// ================================================================
// Bridge-level (L1) streaming
// ================================================================

/// Drains a codex `ResponseStream`, printing and persisting every event
/// under `<repo>/logs/live-mimo/bridge/<tag>-<nonce>.log`.
pub async fn drain_stream(stream: ResponseStream, vendor: &str, tag: &str) -> Vec<ResponseEvent> {
    let mut stream = stream;
    let mut events = Vec::new();
    let mut log_lines = Vec::new();
    loop {
        let event = timeout(TURN_TIMEOUT, stream.rx_event.recv())
            .await
            .expect("next event within timeout");
        match event {
            Some(Ok(event)) => {
                let line = format!("[{tag}] {event:?}");
                println!("{line}");
                log_lines.push(line);
                events.push(event);
            }
            Some(Err(err)) => {
                let line = format!("[{tag}] stream error: {err:#}");
                println!("{line}");
                log_lines.push(line);
                panic!("stream error from bridge: {err:#}");
            }
            None => break,
        }
    }
    persist_lines(vendor, "bridge", tag, &log_lines);
    events
}

/// One bridge-level turn through the genai bridge.
pub async fn run_turn_genai(
    cfg: &LiveConfig,
    base_url: &str,
    request: &ResponsesApiRequest,
    adapter_kind: genai::adapter::AdapterKind,
    tag: &str,
) -> Vec<ResponseEvent> {
    let provider = vendor_provider(&cfg.vendor, base_url);
    let stream: ResponseStream = timeout(
        TURN_TIMEOUT,
        codex_rust_genai_bridge::stream_via_genai(
            request,
            &provider,
            &shared_auth(&cfg.api_key),
            HeaderMap::new(),
            adapter_kind,
            provider.stream_idle_timeout,
        ),
    )
    .await
    .expect("stream_via_genai started within timeout")
    .expect("stream_via_genai succeeded");
    drain_stream(stream, &cfg.vendor, tag).await
}

/// One bridge-level turn through the rig bridge (protocol picked from the
/// provider base URL inside the bridge).
pub async fn run_turn_rig(
    cfg: &LiveConfig,
    base_url: &str,
    request: &ResponsesApiRequest,
    tag: &str,
) -> Vec<ResponseEvent> {
    let provider = vendor_provider(&cfg.vendor, base_url);
    let stream: ResponseStream = timeout(
        TURN_TIMEOUT,
        codex_rust_rig_bridge::stream_via_rig(
            request,
            &provider,
            &shared_auth(&cfg.api_key),
            HeaderMap::new(),
            provider.stream_idle_timeout,
        ),
    )
    .await
    .expect("stream_via_rig started within timeout")
    .expect("stream_via_rig succeeded");
    drain_stream(stream, &cfg.vendor, tag).await
}

// ================================================================
// Binary-level (L3) end-to-end
// ================================================================

/// Locates the `codex-exec` binary. Cargo does not build binaries of other
/// workspace members for this crate's tests, so resolution walks the usual
/// target directories; callers fail fast with build instructions when the
/// binary has not been built yet.
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
    let target = root.join("codex-rs").join("target");
    for profile in ["debug", "release"] {
        let path = target.join(profile).join("codex-exec");
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(anyhow!(
        "codex-exec binary not found under {}; run `cargo build -p codex-exec --bin codex-exec` first",
        target.display()
    ))
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

/// One full binary-level run: spawn `codex-exec`, drive the unforgeable
/// marker task, persist artifacts, and verify the closed loop.
///
/// The marker task proves the whole chain — tool calling, local execution,
/// result replay, final answer — because the marker value only exists in
/// the executed `echo` output (asserted via the command-execution event
/// with exit 0). Model narration of the marker is logged as a bonus; MiMo
/// occasionally ends a turn without narrating.
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

    let artifacts_dir = repo_root()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("logs")
        .join(format!("live-{}", cfg.vendor))
        .join(&marker);
    std::fs::create_dir_all(&artifacts_dir)?;

    let last_message_path = home.path().join("last_message.txt");
    let binary = codex_exec_binary()?;
    let child = tokio::process::Command::new(&binary)
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

    let output = tokio::time::timeout(EXEC_RUN_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| anyhow!("[{protocol}] codex-exec did not finish within {EXEC_RUN_TIMEOUT:?}"))??;

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
    // with exit 0 and the marker in its aggregated output.
    let command_executed_with_marker = stdout.lines().any(|line| {
        line.contains("\"type\":\"command_execution\"")
            && line.contains("\"exit_code\":0")
            && line.contains(&marker)
    });
    anyhow::ensure!(
        command_executed_with_marker,
        "[{protocol}] the executed command should have exited 0 with the marker in its output"
    );
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

// ================================================================
// Assertions shared by the bridge-level suites
// ================================================================

pub fn assert_completed_with_usage(events: &[ResponseEvent], context: &str) {
    let completed = events
        .iter()
        .filter(|e| matches!(e, ResponseEvent::Completed { .. }))
        .count();
    assert_eq!(completed, 1, "{context}: expected exactly one Completed event");
    assert!(
        events.iter().any(|e| matches!(
            e,
            ResponseEvent::Completed {
                token_usage: Some(_),
                ..
            }
        )),
        "{context}: Completed event should carry token usage"
    );
}

pub fn text_len(events: &[ResponseEvent]) -> usize {
    events
        .iter()
        .map(|e| match e {
            ResponseEvent::OutputTextDelta(delta) => delta.chars().count(),
            _ => 0,
        })
        .sum()
}

pub fn reasoning_len(events: &[ResponseEvent]) -> usize {
    events
        .iter()
        .map(|e| match e {
            ResponseEvent::ReasoningContentDelta { delta, .. } => delta.chars().count(),
            _ => 0,
        })
        .sum()
}

/// v0.17.4 event-ordering fix must hold on the wire: reasoning starts
/// streaming before message text, and the reasoning item completes first.
pub fn assert_reasoning_before_message(events: &[ResponseEvent], context: &str) {
    let position_of = |pred: &dyn Fn(&ResponseEvent) -> bool| events.iter().position(|e| pred(e));
    if let (Some(r), Some(t)) = (
        position_of(&|e| matches!(e, ResponseEvent::ReasoningContentDelta { .. })),
        position_of(&|e| matches!(e, ResponseEvent::OutputTextDelta(_))),
    ) {
        assert!(
            r < t,
            "{context}: reasoning deltas must start before text deltas (got {r} vs {t})"
        );
    }
    if let (Some(r), Some(m)) = (
        position_of(&|e| {
            matches!(e, ResponseEvent::OutputItemDone(ResponseItem::Reasoning { .. }))
        }),
        position_of(&|e| {
            matches!(e, ResponseEvent::OutputItemDone(ResponseItem::Message { .. }))
        }),
    ) {
        assert!(
            r < m,
            "{context}: reasoning item must complete before the message item (got {r} vs {m})"
        );
    }
}

/// Concatenated `ToolCallInputDelta`s must reassemble into exactly the final
/// `FunctionCall.arguments` string — validates delta computation on a real
/// stream.
pub fn assert_tool_deltas_reassemble(events: &[ResponseEvent], context: &str) {
    let reassembled: String = events
        .iter()
        .filter_map(|e| match e {
            ResponseEvent::ToolCallInputDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    let final_args = events
        .iter()
        .find_map(|e| match e {
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { arguments, .. }) => {
                Some(arguments.clone())
            }
            _ => None,
        })
        .unwrap_or_default();
    assert_eq!(
        reassembled, final_args,
        "{context}: concatenated tool-call deltas must equal the final arguments"
    );
}

pub fn end_turn_of(events: &[ResponseEvent]) -> Option<bool> {
    events.iter().find_map(|e| match e {
        ResponseEvent::Completed { end_turn, .. } => *end_turn,
        _ => None,
    })
}

/// Convenience for tests that only care about `ApiError` type compatibility.
#[allow(dead_code)]
pub fn api_error_display(err: &ApiError) -> String {
    format!("{err:#}")
}

// ================================================================
// Artifact persistence
// ================================================================

fn persist_lines(vendor: &str, subdir: &str, tag: &str, lines: &[String]) {
    let Some(root) = repo_root() else {
        return;
    };
    let dir = root.join("logs").join(format!("live-{vendor}")).join(subdir);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let path = dir.join(format!("{tag}-{nonce}.log"));
    if std::fs::write(&path, lines.join("\n") + "\n").is_ok() {
        println!("[artifacts] saved {}", path.display());
    }
}
