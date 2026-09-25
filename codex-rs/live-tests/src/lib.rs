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
//!   on real streams; defaults to the rig bridge, with genai variants opt-in
//!   via `LIVE_INCLUDE_GENAI=1` (bridge shelved; A/B comparison).
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

// This crate is test infrastructure only (consumed exclusively by its own
// integration tests under tests/); panicking helpers are the intended
// fail-fast behavior inside a test runner. Mirrors the lmstudio precedent.
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Result;
use anyhow::anyhow;
use codex_api::AuthProvider;
use codex_api::Provider;
use codex_api::ResponseEvent;
use codex_api::ResponseStream;
use codex_api::ResponsesApiRequest;
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
    /// Anthropic-protocol gateway, when this vendor exposes one. Anthropic
    /// suites skip when `None` — there must never be a cross-vendor default
    /// here (sending vendor A's key to vendor B's endpoint).
    pub anthropic_base_url: Option<String>,
    /// Responses-API endpoint, when it differs from the chat endpoint
    /// (GLM: chat at `/api/coding/paas/v4`, responses at `/api/v1`).
    /// `None` defaults to the chat URL (same-origin deployments like MiMo).
    pub responses_base_url: Option<String>,
    pub model: String,
}

/// Enumerates every vendor under test.
///
/// Primary mode — matrix: `LIVE_VENDORS=mimo,glm` in `.env.local`, with
/// per-vendor variables `LIVE_<NAME>_API_KEY` / `LIVE_<NAME>_CHAT_URL` /
/// `LIVE_<NAME>_ANTHROPIC_URL` (optional) / `LIVE_<NAME>_MODEL`. Tests are
/// generated per vendor (see the `vendor_matrix!` macros in the suites), so
/// each vendor reports its own pass/fail granularity.
///
/// Fallback mode — single vendor: when `LIVE_VENDORS` is unset, the legacy
/// `LIVE_VENDOR_*` (and `MIMO_*`) variables configure exactly one vendor,
/// defaulting to `mimo`.
///
/// URL rules: no cross-vendor fallbacks ever. The `mimo` vendor has built-in
/// defaults; every other vendor must declare its URLs explicitly (missing
/// declarations skip that vendor with a notice instead of guessing).
pub fn vendors() -> Vec<LiveConfig> {
    let file_env = load_env_files();
    let lookup = |key: &str| -> Option<String> {
        std::env::var(key)
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| file_env.get(key).cloned())
    };
    let names: Vec<String> = match lookup("LIVE_VENDORS") {
        Some(list) => list
            .split(',')
            .map(|n| n.trim().to_lowercase())
            .filter(|n| !n.is_empty())
            .collect(),
        None => vec![lookup("LIVE_VENDOR_NAME").unwrap_or_else(|| "mimo".into())],
    };
    names
        .iter()
        .filter_map(|name| vendor_from_env(&lookup, name))
        .collect()
}

/// The configuration for one named vendor, when it is configured.
pub fn vendor(name: &str) -> Option<LiveConfig> {
    vendors().into_iter().find(|v| v.vendor == name)
}

fn vendor_from_env(lookup: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<LiveConfig> {
    let upper = name.to_uppercase().replace('-', "_");
    let (key_var, chat_var, anthropic_var, model_var) = if lookup("LIVE_VENDORS").is_some() {
        (
            format!("LIVE_{upper}_API_KEY"),
            format!("LIVE_{upper}_CHAT_URL"),
            format!("LIVE_{upper}_ANTHROPIC_URL"),
            format!("LIVE_{upper}_MODEL"),
        )
    } else {
        // Legacy single-vendor variables (mimo keeps its built-in defaults).
        (
            "LIVE_VENDOR_API_KEY".to_string(),
            "LIVE_VENDOR_CHAT_URL".to_string(),
            "LIVE_VENDOR_ANTHROPIC_URL".to_string(),
            "LIVE_VENDOR_MODEL".to_string(),
        )
    };
    let is_mimo = name == "mimo";
    // MIMO_* stays as a fallback so the original .env.local keeps working.
    let api_key = lookup(&key_var).or_else(|| is_mimo.then(|| lookup("MIMO_API_KEY")).flatten());
    let api_key = match api_key {
        Some(key) => key,
        // Replay mode is fully offline: cassette fixtures carry everything
        // the assertions need, so a placeholder key keeps the vendor active.
        None if cassette_mode() == CassetteMode::Replay => "(replay-placeholder)".to_string(),
        None => {
            println!("no API key configured for vendor `{name}` ({key_var}) — skipping vendor");
            return None;
        }
    };
    let base_url = lookup(&chat_var).or_else(|| is_mimo.then(|| lookup("MIMO_BASE_URL")).flatten());
    let base_url = match base_url {
        Some(url) => url,
        // Replay mode is fully offline: fixtures carry everything the
        // conversion needs, so placeholder URLs/models keep the vendor
        // active for bridge-boundary replay.
        None if cassette_mode() == CassetteMode::Replay => "(replay-placeholder-url)".to_string(),
        None => {
            println!("no chat URL configured for vendor `{name}` ({chat_var}) — skipping vendor");
            return None;
        }
    };
    let model = lookup(&model_var).or_else(|| is_mimo.then(|| lookup("MIMO_MODEL")).flatten());
    let model = match model {
        Some(m) => m,
        None if cassette_mode() == CassetteMode::Replay => "(replay-placeholder-model)".to_string(),
        None => {
            println!("no model configured for vendor `{name}` ({model_var}) — skipping vendor");
            return None;
        }
    };
    let anthropic_base_url = lookup(&anthropic_var)
        .or_else(|| is_mimo.then(|| lookup("MIMO_ANTHROPIC_BASE_URL")).flatten());
    let responses_var = if lookup("LIVE_VENDORS").is_some() {
        format!("LIVE_{upper}_RESPONSES_URL")
    } else {
        "LIVE_VENDOR_RESPONSES_URL".to_string()
    };
    let responses_base_url = lookup(&responses_var).or(Some(base_url.clone()));
    Some(LiveConfig {
        vendor: name.to_string(),
        api_key,
        base_url,
        anthropic_base_url,
        responses_base_url,
        model,
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

/// Endpoint for the native Responses transport. Defaults to the chat URL
/// (same-origin deployments); some vendors (GLM) serve it from a different
/// path, configured via `LIVE_<NAME>_RESPONSES_URL`.
pub fn responses_url(cfg: &LiveConfig) -> String {
    cfg.responses_base_url
        .clone()
        .unwrap_or_else(|| cfg.base_url.clone())
}

/// Returns the vendor's Anthropic gateway, or `None` after a skip notice
/// when the vendor does not expose one — never falls back across vendors.
pub fn anthropic_url_or_skip(cfg: &LiveConfig) -> Option<String> {
    if cfg.anthropic_base_url.is_none() {
        println!(
            "vendor `{}` has no Anthropic gateway configured (set LIVE_VENDOR_ANTHROPIC_URL) — skipping",
            cfg.vendor
        );
    }
    cfg.anthropic_base_url.clone()
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

/// Which bridge a suite exercises.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Bridge {
    Genai,
    Rig,
}

/// The wire a scenario drives: explicit, mirroring `wire_api` in provider
/// config. `Chat` keeps the URL heuristic as fallback (MiMo-style
/// `/anthropic` gateways); `Anthropic` is the explicit protocol for
/// gateways like StepFun whose URL carries no marker.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LiveWire {
    Chat,
    Anthropic,
}

impl Bridge {
    pub fn name(self) -> &'static str {
        match self {
            Self::Genai => "genai",
            Self::Rig => "rig",
        }
    }
}

/// Whether the shelved genai bridge participates in this run. Rig is the
/// fork default for every wire; genai live/replay tests are opt-in via
/// `LIVE_INCLUDE_GENAI=1` (set on the command line like `LIVE_CASSETTE`),
/// e.g. to re-validate genai before resurrecting it or for A/B comparison
/// runs. Default runs exercise rig only, halving vendor-quota usage.
pub fn genai_bridge_enabled() -> bool {
    std::env::var("LIVE_INCLUDE_GENAI").as_deref() == Ok("1")
}

fn replay_rig_turn(cfg: &LiveConfig, tag: &str) -> Vec<ResponseEvent> {
    let fixture = load_rig_event_fixture(&cfg.vendor, tag)
        .unwrap_or_else(|error| panic!("[cassette-rig] {error}"));
    println!(
        "[cassette-rig] replaying {}/{} through current bridge conversion ({} rig events, offline)",
        cfg.vendor,
        tag,
        fixture.rig_events.len()
    );
    codex_rust_rig_bridge::replay_fixture_events(&fixture)
        .unwrap_or_else(|error| panic!("[cassette-rig] {}/{}: {error}", cfg.vendor, tag))
}

/// One bridge-level turn through the selected bridge (the vendor's chat URL;
/// Anthropic gateways are passed by the anthropic scenarios).
pub async fn run_turn(
    cfg: &LiveConfig,
    base_url: &str,
    wire: LiveWire,
    bridge: Bridge,
    request: &ResponsesApiRequest,
    tag: &str,
) -> Vec<ResponseEvent> {
    if cassette_mode() == CassetteMode::Replay {
        // Rig replay must execute current conversion. Never fall back to the
        // already-converted ResponseEvent cassette when parsing fails.
        if bridge == Bridge::Rig {
            return replay_rig_turn(cfg, tag);
        }
        let fixture = load_fixture(&cfg.vendor, bridge.name(), tag).unwrap_or_else(|| {
            // Fail fast: silently falling back to live would consume vendor
            // quota in what the operator explicitly declared an offline run,
            // and a green suite could then be evidence of a live call rather
            // than of the recorded fixture.
            panic!(
                "[cassette] replay: no fixture for {}/{}-{} \
                 (record with LIVE_CASSETTE=record)",
                cfg.vendor,
                bridge.name(),
                tag
            );
        });
        println!(
            "[cassette] replaying {}/{}-{} ({} events, offline)",
            cfg.vendor,
            bridge.name(),
            tag,
            fixture.events.len()
        );
        return fixture.events;
    }
    match bridge {
        Bridge::Genai => {
            let adapter_kind = match wire {
                LiveWire::Anthropic => genai::adapter::AdapterKind::Anthropic,
                LiveWire::Chat
                    if codex_rust_rig_bridge::RigProtocol::from_base_url(base_url)
                        == codex_rust_rig_bridge::RigProtocol::Anthropic =>
                {
                    genai::adapter::AdapterKind::Anthropic
                }
                LiveWire::Chat => genai::adapter::AdapterKind::OpenAI,
            };
            run_turn_genai(cfg, base_url, request, adapter_kind, tag).await
        }
        Bridge::Rig => {
            let protocol = match wire {
                LiveWire::Anthropic => codex_rust_rig_bridge::RigProtocol::Anthropic,
                LiveWire::Chat => codex_rust_rig_bridge::RigProtocol::from_base_url(base_url),
            };
            run_turn_rig(cfg, base_url, protocol, request, tag).await
        }
    }
}

/// One bridge-level turn through the genai bridge.
async fn run_turn_genai(
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
    let events = drain_stream(stream, &cfg.vendor, tag).await;
    record_turn(cfg, Bridge::Genai, tag, request, &events);
    events
}

/// One bridge-level turn through the rig bridge (protocol picked from the
/// provider base URL inside the bridge).
pub async fn run_turn_rig(
    cfg: &LiveConfig,
    base_url: &str,
    protocol: codex_rust_rig_bridge::RigProtocol,
    request: &ResponsesApiRequest,
    tag: &str,
) -> Vec<ResponseEvent> {
    // Replay recorded Rig events through the current conversion. Missing or
    // malformed fixtures must never fall through to a real provider request.
    if cassette_mode() == CassetteMode::Replay {
        return replay_rig_turn(cfg, tag);
    }

    let provider = vendor_provider(&cfg.vendor, base_url);
    let recorder: codex_rust_rig_bridge::RigEventRecorder =
        if cassette_mode() == CassetteMode::Record {
            Some(Arc::new(std::sync::Mutex::new(Vec::new())))
        } else {
            None
        };
    let (stream, recorder): (ResponseStream, codex_rust_rig_bridge::RigEventRecorder) = timeout(
        TURN_TIMEOUT,
        codex_rust_rig_bridge::stream_via_rig_with_recording(
            request,
            &provider,
            &shared_auth(&cfg.api_key),
            HeaderMap::new(),
            protocol,
            provider.stream_idle_timeout,
            recorder,
        ),
    )
    .await
    .expect("stream_via_rig started within timeout")
    .expect("stream_via_rig succeeded");
    let events = drain_stream(stream, &cfg.vendor, tag).await;
    record_turn(cfg, Bridge::Rig, tag, request, &events);
    // Save the rig-event fixture alongside the event-level one.
    if let Some(rec) = recorder
        && let Ok(rig_events) = rec.lock()
    {
        let custom_tools = codex_rust_rig_bridge::extract_custom_tool_names(request);
        save_rig_event_fixture(&cfg.vendor, tag, &rig_events, &custom_tools);
    }
    events
}

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
    anyhow::ensure!(
        cassette_mode() != CassetteMode::Replay,
        "exec_live cannot replay; use --test bridge_live for offline replay"
    );
    let home = tempfile::TempDir::new()?;
    let cwd = tempfile::TempDir::new()?;
    write_config_toml(home.path(), cfg, base_url, wire_api, bridge, extra_config)?;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let marker = format!("{}-{protocol}-{nonce}-{}", cfg.vendor, std::process::id());
    let prompt = format!(
        "请务必调用 shell 工具真实执行命令 `echo {marker}`（不要只把命令当文本输出），然后把命令的原始输出逐字告诉我，不要添加任何解释。"
    );

    let artifacts_dir = repo_root()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("logs")
        .join(format!("live-{}", cfg.vendor))
        .join(&marker);
    std::fs::create_dir_all(&artifacts_dir)?;
    write_manifest(&artifacts_dir, cfg, protocol, bridge);
    prune_artifacts(artifacts_dir.parent().expect("vendor dir").to_path_buf());

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
        .map_err(|_| {
            anyhow!("[{protocol}] codex-exec did not finish within {EXEC_RUN_TIMEOUT:?}")
        })??;

    // Persist the artifacts first so failed runs can still be inspected.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    std::fs::write(artifacts_dir.join("events.jsonl"), &*stdout)?;
    std::fs::write(artifacts_dir.join("stderr.log"), &*stderr)?;
    let final_message = std::fs::read_to_string(&last_message_path)
        .unwrap_or_else(|_| "<last_message.txt missing>".to_string());
    std::fs::write(artifacts_dir.join("final_message.txt"), &final_message)?;
    println!(
        "[{protocol}] artifacts saved to {}",
        artifacts_dir.display()
    );

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
    // Primary proof of the closed loop: parse the completed
    // command-execution event and check the ACTUAL aggregated output —
    // matching the raw line would also hit the command string itself
    // (which contains the marker) even if stdout capture was broken.
    let command_executed_with_marker = stdout.lines().any(|line| {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            return false;
        };
        let item = event.get("item").unwrap_or(&event);
        item.get("type").and_then(|t| t.as_str()) == Some("command_execution")
            && item.get("exit_code").and_then(serde_json::Value::as_i64) == Some(0)
            && item
                .get("aggregated_output")
                .and_then(|o| o.as_str())
                .is_some_and(|o| o.contains(&marker))
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
    assert_eq!(
        completed, 1,
        "{context}: expected exactly one Completed event"
    );
    let usage = events.iter().find_map(|e| match e {
        ResponseEvent::Completed {
            token_usage: Some(usage),
            ..
        } => Some(usage.clone()),
        _ => None,
    });
    let Some(usage) = usage else {
        panic!("{context}: Completed event should carry token usage");
    };
    // Plausibility invariants: a mis-mapped usage counter (e.g. swapped
    // input/output or a missing normalization) shows up here immediately.
    assert!(
        usage.input_tokens > 0,
        "{context}: input_tokens should be positive, got {usage:?}"
    );
    assert!(
        usage.total_tokens >= usage.input_tokens + usage.output_tokens,
        "{context}: total_tokens should cover input+output, got {usage:?}"
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
    let position_of = |pred: &dyn Fn(&ResponseEvent) -> bool| events.iter().position(pred);
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
            matches!(
                e,
                ResponseEvent::OutputItemDone(ResponseItem::Reasoning { .. })
            )
        }),
        position_of(&|e| {
            matches!(
                e,
                ResponseEvent::OutputItemDone(ResponseItem::Message { .. })
            )
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

// ================================================================
// Bridge-boundary cassette (record / replay)
// ================================================================

/// Cassette mode from `LIVE_CASSETTE`: unset = live only, `record` = live
/// and persist fixtures, `replay` = serve recorded fixtures when present
/// (falls back to live with a notice when a fixture is missing).
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

/// One recorded bridge-boundary turn: the exact request and the exact event
/// stream, serialized to `tests/fixtures/<vendor>/<bridge>-<tag>.json`.
/// Fixtures contain prompts and model text only — never credentials.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct TurnFixture {
    pub vendor: String,
    pub bridge: String,
    pub tag: String,
    /// The request, serialized as plain JSON for documentation (replay only
    /// consumes `events`, so no typed round-trip is required here).
    pub request: serde_json::Value,
    pub events: Vec<ResponseEvent>,
}

pub fn fixture_path(vendor: &str, bridge: &str, tag: &str) -> Option<PathBuf> {
    let root = repo_root()?;
    Some(
        root.join("codex-rs")
            .join("live-tests")
            .join("tests")
            .join("fixtures")
            .join(vendor)
            .join(format!("{bridge}-{tag}.json")),
    )
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
) {
    let Some(path) = rig_event_fixture_path(vendor, tag) else {
        return;
    };
    let fixture = codex_rust_rig_bridge::RigEventFixture {
        vendor: vendor.to_string(),
        tag: tag.to_string(),
        rig_events: rig_events.to_vec(),
        custom_tools: custom_tools.iter().cloned().collect(),
    };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_ok()
        && let Ok(json) = serde_json::to_string_pretty(&fixture)
        && std::fs::write(&path, json).is_ok()
    {
        println!("[cassette-rig] recorded {}", path.display());
    }
}

fn rig_event_fixture_path(vendor: &str, tag: &str) -> Option<PathBuf> {
    let root = repo_root()?;
    Some(
        root.join("codex-rs")
            .join("live-tests")
            .join("tests")
            .join("fixtures")
            .join(vendor)
            .join(format!("rig-events-{tag}.json")),
    )
}

pub fn load_fixture(vendor: &str, bridge: &str, tag: &str) -> Option<TurnFixture> {
    let path = fixture_path(vendor, bridge, tag)?;
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

pub fn store_fixture(fixture: &TurnFixture) {
    let Some(path) = fixture_path(&fixture.vendor, &fixture.bridge, &fixture.tag) else {
        return;
    };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_ok()
        && let Ok(json) = serde_json::to_string_pretty(fixture)
        && std::fs::write(&path, json).is_ok()
    {
        println!("[cassette] recorded {}", path.display());
    }
}

/// Stable kind tag per event, for A/B sequence diffs between bridges.
pub fn event_kind(event: &ResponseEvent) -> &'static str {
    match event {
        ResponseEvent::Created { .. } => "created",
        ResponseEvent::OutputItemAdded(item) => match item {
            ResponseItem::Message { .. } => "added.message",
            ResponseItem::Reasoning { .. } => "added.reasoning",
            ResponseItem::FunctionCall { .. } => "added.function_call",
            _ => "added.other",
        },
        ResponseEvent::OutputTextDelta(_) => "delta.text",
        ResponseEvent::ReasoningContentDelta { .. } => "delta.reasoning",
        ResponseEvent::ToolCallInputDelta { .. } => "delta.tool",
        ResponseEvent::OutputItemDone(item) => match item {
            ResponseItem::Message { .. } => "done.message",
            ResponseItem::Reasoning { .. } => "done.reasoning",
            ResponseItem::FunctionCall { .. } => "done.function_call",
            _ => "done.other",
        },
        ResponseEvent::Completed { .. } => "completed",
        _ => "other",
    }
}

// ================================================================
// Error-path probing
// ================================================================

/// Starts a turn and returns the bridge's start error (if any) without
/// draining — used by the auth-rejected scenario to assert HTTP status
/// passthrough (401 must reach codex-core's re-login loop as Http{401}).
pub async fn turn_start_error(
    cfg: &LiveConfig,
    base_url: &str,
    bridge: Bridge,
    request: &ResponsesApiRequest,
) -> Option<String> {
    let bad_auth: SharedAuthProvider = Arc::new(StaticBearerAuth(format!(
        "invalid-key-{}",
        cfg.api_key.len()
    )));
    let provider = vendor_provider(&cfg.vendor, base_url);
    let result = match bridge {
        Bridge::Genai => {
            let adapter_kind = if codex_rust_rig_bridge::protocol_for_base_url(base_url)
                == codex_rust_rig_bridge::RigProtocol::Anthropic
            {
                genai::adapter::AdapterKind::Anthropic
            } else {
                genai::adapter::AdapterKind::OpenAI
            };
            timeout(
                TURN_TIMEOUT,
                codex_rust_genai_bridge::stream_via_genai(
                    request,
                    &provider,
                    &bad_auth,
                    HeaderMap::new(),
                    adapter_kind,
                    provider.stream_idle_timeout,
                ),
            )
            .await
        }
        Bridge::Rig => {
            timeout(
                TURN_TIMEOUT,
                codex_rust_rig_bridge::stream_via_rig(
                    request,
                    &provider,
                    &bad_auth,
                    HeaderMap::new(),
                    codex_rust_rig_bridge::RigProtocol::from_base_url(base_url),
                    provider.stream_idle_timeout,
                ),
            )
            .await
        }
    };
    match result {
        Ok(Ok(mut stream)) => {
            // genai (and any bridge that defers HTTP failures into the
            // stream) surfaces a bad key as the first error EVENT — drain
            // until it arrives so the scenario can assert on it either way.
            loop {
                match timeout(TURN_TIMEOUT, stream.rx_event.recv()).await {
                    Ok(Some(Err(api_error))) => return Some(format!("{api_error:#}")),
                    // A completed turn means the request unexpectedly
                    // succeeded despite the invalid key.
                    Ok(Some(Ok(ResponseEvent::Completed { .. }))) => return None,
                    Ok(None) => return None,
                    Err(_elapsed) => return Some("drain timed out".to_string()),
                    Ok(Some(Ok(_))) => {}
                }
            }
        }
        Ok(Err(api_error)) => Some(format!("{api_error:#}")),
        Err(_elapsed) => Some("start timed out".to_string()),
    }
}

// ================================================================
// Artifact persistence
// ================================================================

/// Records what code produced this run: git revision, binary mtime, and the
/// vendor/scenario — so any artifact directory can be traced back to a
/// build. Written into every binary-level run directory.
fn write_manifest(dir: &Path, cfg: &LiveConfig, scenario: &str, bridge: Option<&str>) {
    let root = repo_root();
    let git_rev = root
        .and_then(|root| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .arg("rev-parse")
                .arg("--short")
                .arg("HEAD")
                .output()
                .ok()
        })
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
    let binary_mtime = codex_exec_binary()
        .ok()
        .and_then(|p| p.metadata().ok())
        .and_then(|m| m.modified().ok())
        .map(|t| {
            t.duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default()
        });
    let manifest = serde_json::json!({
        "timestamp": SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default(),
        "git_rev": git_rev,
        "binary_mtime_unix": binary_mtime,
        "vendor": cfg.vendor,
        "model": cfg.model,
        "scenario": scenario,
        "experimental_bridge": bridge,
    });
    let _ = std::fs::write(dir.join("manifest.json"), manifest.to_string());
}

/// Keeps the newest `KEEP_RUNS` run directories (and `KEEP_BRIDGE_LOGS`
/// bridge event logs) so `logs/` cannot grow unboundedly.
const KEEP_RUNS: usize = 25;
const KEEP_BRIDGE_LOGS: usize = 120;

fn prune_artifacts(vendor_dir: PathBuf) {
    fn prune_dir(dir: &Path, keep: usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let mtime = e.metadata().ok()?.modified().ok()?;
                Some((mtime, path))
            })
            .collect();
        if dirs.len() <= keep {
            return;
        }
        dirs.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime)); // newest first
        for (_, path) in dirs.iter().skip(keep) {
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    prune_dir(&vendor_dir, KEEP_RUNS);
    prune_dir(&vendor_dir.join("bridge"), KEEP_BRIDGE_LOGS);
}

fn record_turn(
    cfg: &LiveConfig,
    bridge: Bridge,
    tag: &str,
    request: &ResponsesApiRequest,
    events: &[ResponseEvent],
) {
    if cassette_mode() != CassetteMode::Record {
        return;
    }
    store_fixture(&TurnFixture {
        vendor: cfg.vendor.clone(),
        bridge: bridge.name().to_string(),
        tag: tag.to_string(),
        request: serde_json::to_value(request).unwrap_or(serde_json::Value::Null),
        events: events.to_vec(),
    });
}

fn persist_lines(vendor: &str, subdir: &str, tag: &str, lines: &[String]) {
    let Some(root) = repo_root() else {
        return;
    };
    let dir = root
        .join("logs")
        .join(format!("live-{vendor}"))
        .join(subdir);
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
