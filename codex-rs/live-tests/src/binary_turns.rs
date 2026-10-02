//! Binary-level (L3) end-to-end scenarios driving the compiled
//! `codex-exec`: marker turns, compaction recovery, web-search turns.

use super::*;

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
    write_manifest(&artifacts_dir, cfg, protocol, bridge)?;
    prune_artifacts(artifacts_dir.parent().expect("vendor dir").to_path_buf());

    let last_message_path = home.path().join("last_message.txt");
    let binary = codex_exec_binary()?;
    let (stdout, stderr) = spawn_exec_turn(
        &binary,
        home.path(),
        cwd.path(),
        &[],
        &prompt,
        &last_message_path,
        &artifacts_dir,
        "",
        protocol,
    )
    .await?;

    let final_message = std::fs::read_to_string(&last_message_path)
        .unwrap_or_else(|_| "<last_message.txt missing>".to_string());
    std::fs::write(artifacts_dir.join("final_message.txt"), &final_message)?;
    println!(
        "[{protocol}] artifacts saved to {}",
        artifacts_dir.display()
    );

    if let Some(expected) = expect_bridge_log {
        anyhow::ensure!(
            stderr.contains(expected),
            "[{protocol}] expected the dispatch log `{expected}` on stderr, got:\n{stderr}"
        );
    }

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

/// Two-turn binary-level auto-compact run: teach a unique passphrase, then
/// resume with a token limit far below any real turn so codex must compact
/// the context locally before the second model call, and finally recall the
/// passphrase. Verifies three things: the recall answer carries the
/// passphrase, the rollout contains a `compacted` record (guarding against
/// the no-compaction false pass where the full history would also answer),
/// and that record precedes the recalling answer while carrying the
/// passphrase itself.
pub async fn run_compact_turn(
    protocol: &str,
    cfg: &LiveConfig,
    base_url: &str,
    wire_api: &str,
    bridge: Option<&str>,
    extra_config: &str,
) -> Result<()> {
    anyhow::ensure!(
        cassette_mode() != CassetteMode::Replay,
        "exec_live cannot replay; use --test bridge_live for offline replay"
    );
    let home = tempfile::TempDir::new()?;
    let cwd = tempfile::TempDir::new()?;
    // The tiny limit forces one local compaction before the resumed turn's
    // model call on every wire: bridge providers report remote compaction
    // unsupported, so codex always summarizes locally.
    write_config_toml(
        home.path(),
        cfg,
        base_url,
        wire_api,
        bridge,
        &format!("{extra_config}model_auto_compact_token_limit = 200\n"),
    )?;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let marker = format!("PASSPHRASE-{protocol}-{nonce}-{}", std::process::id());

    let artifacts_dir = repo_root()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("logs")
        .join(format!("live-{}", cfg.vendor))
        .join(format!("compact-{marker}"));
    std::fs::create_dir_all(&artifacts_dir)?;
    write_manifest(&artifacts_dir, cfg, protocol, bridge)?;

    let binary = codex_exec_binary()?;
    let first_prompt = format!("请记住暗号：{marker}。只回复OK。");
    let last_message_1 = home.path().join("last_message_1.txt");
    spawn_exec_turn(
        &binary,
        home.path(),
        cwd.path(),
        &[],
        &first_prompt,
        &last_message_1,
        &artifacts_dir,
        "turn1",
        protocol,
    )
    .await?;

    let last_message_2 = home.path().join("last_message_2.txt");
    spawn_exec_turn(
        &binary,
        home.path(),
        cwd.path(),
        &["resume", "--last"],
        "暗号是什么？直接回答暗号本身，不要任何其他内容。",
        &last_message_2,
        &artifacts_dir,
        "turn2",
        protocol,
    )
    .await?;
    let answer = std::fs::read_to_string(&last_message_2)
        .unwrap_or_else(|_| "<last_message_2.txt missing>".to_string());
    anyhow::ensure!(
        answer.contains(&marker),
        "[{protocol}] passphrase lost after compaction; answer: {answer}"
    );

    // The compacted record must exist, carry the passphrase (summary or
    // retained original message), and precede the recalling answer.
    let rollouts = session_rollouts(home.path());
    anyhow::ensure!(
        !rollouts.is_empty(),
        "[{protocol}] no session rollout under {}",
        home.path().display()
    );
    let mut compacted_at = None;
    let mut answer_at = None;
    let mut compacted_carries_marker = false;
    for rollout in &rollouts {
        for (index, line) in std::fs::read_to_string(rollout)
            .unwrap_or_default()
            .lines()
            .enumerate()
        {
            let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if record.get("type").and_then(Value::as_str) == Some("compacted") {
                if compacted_at.is_none() {
                    compacted_at = Some(index);
                }
                if serde_json::to_string(&record).is_ok_and(|encoded| encoded.contains(&marker)) {
                    compacted_carries_marker = true;
                }
            }
            // The recalling answer: an assistant message that carries the
            // passphrase and is not itself a compaction record.
            if compacted_at.is_some()
                && answer_at.is_none()
                && record.get("type").and_then(Value::as_str) == Some("response_item")
                && record["payload"]["type"].as_str() == Some("message")
                && record["payload"]["role"].as_str() == Some("assistant")
                && serde_json::to_string(&record).is_ok_and(|encoded| encoded.contains(&marker))
            {
                answer_at = Some(index);
            }
        }
    }
    let compacted_index =
        compacted_at.ok_or_else(|| anyhow!("[{protocol}] no compacted record in the rollout"))?;
    anyhow::ensure!(
        answer_at.is_some_and(|index| index > compacted_index),
        "[{protocol}] the recall answer must follow the compacted record"
    );
    anyhow::ensure!(
        compacted_carries_marker,
        "[{protocol}] the compacted record must carry the passphrase (summary or retained message)"
    );
    println!(
        "[{protocol}] OK compact marker={marker} answer_chars={}",
        answer.chars().count()
    );
    Ok(())
}

/// Spawns one `codex-exec` turn with the shared live-test environment,
/// persists its stdout/stderr artifacts (label-prefixed unless the label is
/// empty, so failed runs stay inspectable), echoes the JSONL event stream,
/// and returns the captured `(stdout, stderr)` for caller-specific checks.
#[allow(
    clippy::too_many_arguments,
    reason = "The shared subprocess runner accepts explicit launch settings and artifact paths"
)]
async fn spawn_exec_turn(
    binary: &Path,
    home: &Path,
    cwd: &Path,
    subcommand_args: &[&str],
    prompt: &str,
    last_message_path: &Path,
    artifacts_dir: &Path,
    label: &str,
    protocol: &str,
) -> Result<(String, String)> {
    let prefix = if label.is_empty() {
        String::new()
    } else {
        format!("{label}.")
    };
    let mut child = tokio::process::Command::new(binary)
        .arg("--skip-git-repo-check")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .arg("--output-last-message")
        .arg(last_message_path)
        .args(subcommand_args)
        .arg(prompt)
        .env("CODEX_HOME", home)
        .env("CODEX_SQLITE_HOME", home)
        .env("RUST_LOG", "info")
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    // Pipe the streams through readers we own: on a timeout the buffered
    // output still lands in the artifact directory with the partial exit
    // status, instead of vanishing with the child.
    let stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("stdout pipe missing"))?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("stderr pipe missing"))?;
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let drain_timeout = Duration::from_secs(5);
    let stdout_task = tokio::spawn(capture_pipe(stdout_pipe, stopped.clone(), drain_timeout));
    let stderr_task = tokio::spawn(capture_pipe(stderr_pipe, stopped, drain_timeout));
    let (status, timed_out) = match tokio::time::timeout(EXEC_RUN_TIMEOUT, child.wait()).await {
        Ok(status) => (Some(status?), /*timed_out*/ false),
        Err(_elapsed) => {
            let _ = child.kill().await;
            (/*status*/ None, /*timed_out*/ true)
        }
    };
    let _ = stop.send(true);
    let stdout_capture = stdout_task
        .await
        .map_err(|error| anyhow!("stdout reader: {error}"))?;
    let stderr_capture = stderr_task
        .await
        .map_err(|error| anyhow!("stderr reader: {error}"))?;
    let stdout = String::from_utf8_lossy(&stdout_capture.bytes).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_capture.bytes).into_owned();
    std::fs::write(artifacts_dir.join(format!("{prefix}events.jsonl")), &stdout)?;
    std::fs::write(artifacts_dir.join(format!("{prefix}stderr.log")), &stderr)?;
    let mut exit = match &status {
        Some(status) => format!("{status}\n"),
        None => format!("timeout after {EXEC_RUN_TIMEOUT:?}\n"),
    };
    for (stream, capture) in [("stdout", &stdout_capture), ("stderr", &stderr_capture)] {
        if let Some(error) = &capture.error {
            exit.push_str(&format!("{stream} capture incomplete: {error}\n"));
        }
    }
    std::fs::write(artifacts_dir.join(format!("{prefix}exit.txt")), &exit)?;
    if timed_out {
        anyhow::bail!("[{protocol}] {label} did not finish within {EXEC_RUN_TIMEOUT:?}");
    }
    anyhow::ensure!(
        stdout_capture.error.is_none() && stderr_capture.error.is_none(),
        "[{protocol}] {label} output capture incomplete: {exit}"
    );
    println!("--- [{protocol}] {label} codex-exec JSONL events ---");
    for line in stdout.lines() {
        println!("[{protocol}] {line}");
    }
    if !stderr.trim().is_empty() {
        println!("--- [{protocol}] {label} codex-exec stderr ---\n{stderr}");
    }
    anyhow::ensure!(
        status.is_some_and(|status| status.success()),
        "[{protocol}] {label} exited with {}",
        status.map(|status| status.to_string()).unwrap_or_default()
    );
    Ok((stdout, stderr))
}

struct PipeCapture {
    bytes: Vec<u8>,
    error: Option<String>,
}

/// After the child exits, descendants may still hold its pipes. Bound the
/// remaining drain and retain every completed read, even when EOF never arrives.
async fn capture_pipe(
    mut pipe: impl tokio::io::AsyncRead + Unpin,
    mut stopped: tokio::sync::watch::Receiver<bool>,
    drain_timeout: Duration,
) -> PipeCapture {
    use tokio::io::AsyncReadExt;
    let mut capture = PipeCapture {
        bytes: Vec::new(),
        error: None,
    };
    let mut chunk = [0u8; 8192];
    let mut deadline = None;
    loop {
        tokio::select! {
            result = pipe.read(&mut chunk) => match result {
                Ok(0) => return capture,
                Ok(read) => capture.bytes.extend_from_slice(&chunk[..read]),
                Err(error) => {
                    capture.error = Some(error.to_string());
                    return capture;
                }
            },
            _ = stopped.changed(), if deadline.is_none() => {
                deadline = Some(tokio::time::Instant::now() + drain_timeout);
            }
            _ = async {
                match deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            } => {
                capture.error = Some(format!("pipe did not close within {drain_timeout:?}"));
                return capture;
            }
        }
    }
}

/// Two-turn live web-search run on the Anthropic wire: the second turn's
/// request replays turn 1's versioned, same-source search envelopes.
/// Completing turn 2 against the real gateway verifies resumed history is
/// accepted; retained rollouts supply inspectable envelope evidence.
///
/// Both prompts explicitly demand a web search, so a passing run must show
/// a NEW completed `web_search_call` rollout item per turn — a non-empty
/// answer alone proves nothing about search execution.
pub async fn run_websearch_turns(
    protocol: &str,
    cfg: &LiveConfig,
    base_url: &str,
    bridge: Option<&str>,
) -> Result<()> {
    anyhow::ensure!(
        cassette_mode() != CassetteMode::Replay,
        "exec_live cannot replay; use --test bridge_live for offline replay"
    );
    let home = tempfile::TempDir::new()?;
    let cwd = tempfile::TempDir::new()?;
    // Both prompts demand a web search, and the fork fails the DEFAULT
    // cached mode closed on chat-family bridge wires — request live search
    // explicitly so the hosted tool is actually advertised.
    write_config_toml(
        home.path(),
        cfg,
        base_url,
        "anthropic",
        bridge,
        "web_search = \"live\"\n",
    )?;

    // Unique per run (a fixed directory silently overwrote earlier evidence,
    // including the failure trail of a flaky retry) and manifest-bound so
    // index-logs can identify the harness and executable bytes separately.
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let artifacts_dir = repo_root()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("logs")
        .join(format!("live-{}", cfg.vendor))
        .join(format!(
            "websearch-{protocol}-{nonce}-{}",
            std::process::id()
        ));
    std::fs::create_dir_all(&artifacts_dir)?;
    write_manifest(&artifacts_dir, cfg, protocol, bridge)?;
    prune_artifacts(artifacts_dir.parent().expect("vendor dir").to_path_buf());

    let binary = codex_exec_binary()?;
    let outcome =
        run_websearch_turns_inner(&binary, home.path(), cwd.path(), &artifacts_dir, protocol).await;
    // D3/N6: a failing run still leaves its PARTIAL rollouts beside the
    // stdout/stderr/exit artifacts — the failure scene must include what the
    // run actually persisted, not only the success path's evidence.
    let (answer1_chars, answer2_chars, searches) = match outcome {
        Ok(summary) => summary,
        Err(error) => {
            if let Err(retain_error) =
                retain_rollouts_best_effort(home.path(), &artifacts_dir.join("rollout"))
            {
                eprintln!("warn: {retain_error}");
            }
            return Err(error.context(format!("[{protocol}] web-search turns failed")));
        }
    };
    println!(
        "[{protocol}] OK websearch turns answer1_chars={} answer2_chars={} searches={}/{}",
        answer1_chars, answer2_chars, searches.0, searches.1
    );
    Ok(())
}

type WebsearchTurnsSummary = (usize, usize, (usize, usize));

async fn run_websearch_turns_inner(
    binary: &Path,
    home: &Path,
    cwd: &Path,
    artifacts_dir: &Path,
    protocol: &str,
) -> Result<WebsearchTurnsSummary> {
    let last1 = home.join("last_message_1.txt");
    spawn_exec_turn(
        binary,
        home,
        cwd,
        &[],
        "用 web 搜索今天北京的天气，然后用一句话总结。",
        &last1,
        artifacts_dir,
        "turn1",
        protocol,
    )
    .await?;
    let answer1 = std::fs::read_to_string(&last1).unwrap_or_default();
    anyhow::ensure!(!answer1.trim().is_empty(), "turn 1 produced no answer");
    let searches_after_turn1 = completed_web_search_calls(home);
    anyhow::ensure!(
        searches_after_turn1 >= 1,
        "turn 1 answered without any completed web_search_call in the rollout — \
         a non-empty answer is not evidence that the hosted search ran"
    );

    let last2 = home.join("last_message_2.txt");
    spawn_exec_turn(
        binary,
        home,
        cwd,
        &["resume", "--last"],
        "再用 web 搜索今天上海的天气，然后用一句话总结。",
        &last2,
        artifacts_dir,
        "turn2",
        protocol,
    )
    .await?;
    let answer2 = std::fs::read_to_string(&last2).unwrap_or_default();
    anyhow::ensure!(
        !answer2.trim().is_empty(),
        "turn 2 produced no answer — gateway may be rejecting resumed search history"
    );
    let searches_after_turn2 = completed_web_search_calls(home);
    anyhow::ensure!(
        searches_after_turn2 > searches_after_turn1,
        "turn 2 added no completed web_search_call \
         ({searches_after_turn2} total after {searches_after_turn1} from turn 1) — \
         the second search must actually execute, not just answer"
    );
    // Keep the rollouts with the artifacts: the persisted search items (and
    // their replay envelopes) are the evidence the next turn's request was
    // built from — the outbound body itself is not observable against a real
    // gateway without a MITM proxy (registered boundary).
    let retained_pairs = retain_search_rollouts(home, &artifacts_dir.join("rollout"))?;
    anyhow::ensure!(
        retained_pairs >= 2,
        "both search turns must retain completed call/result envelopes"
    );
    Ok((
        answer1.chars().count(),
        answer2.chars().count(),
        (searches_after_turn1, searches_after_turn2),
    ))
}

/// Best-effort partial-rollout retention for failure scenes; parse errors in
/// truncated files are tolerated (whatever copies through is kept).
fn retain_rollouts_best_effort(home: &Path, rollout_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(rollout_dir)?;
    for path in session_rollouts(home) {
        let name = path
            .file_name()
            .ok_or_else(|| anyhow!("rollout filename missing"))?;
        match std::fs::copy(&path, rollout_dir.join(name)) {
            Ok(_) => {}
            Err(error) => eprintln!("warn: retain rollout {}: {error}", path.display()),
        }
    }
    Ok(())
}

fn retain_search_rollouts(home: &Path, rollout_dir: &Path) -> Result<usize> {
    std::fs::create_dir_all(rollout_dir)?;
    let mut retained_envelopes = 0;
    for path in session_rollouts(home) {
        let name = path
            .file_name()
            .ok_or_else(|| anyhow!("rollout filename missing"))?;
        let retained = rollout_dir.join(name);
        std::fs::copy(&path, &retained)
            .map_err(|error| anyhow!("retain rollout {}: {error}", path.display()))?;
        let contents = std::fs::read_to_string(&retained)?;
        for (index, line) in contents.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let record: Value = serde_json::from_str(line)
                .map_err(|error| anyhow!("{} line {}: {error}", retained.display(), index + 1))?;
            if record["type"].as_str() != Some("response_item")
                || record["payload"]["type"].as_str() != Some("web_search_call")
                || record["payload"]["status"].as_str() != Some("completed")
                || record["payload"]["wire_blocks"].is_null()
            {
                continue;
            }
            let envelope = &record["payload"]["wire_blocks"];
            anyhow::ensure!(
                envelope["version"].as_u64() == Some(1)
                    && envelope["source"]
                        .as_str()
                        .is_some_and(|source| !source.trim().is_empty())
                    && envelope["blocks"].is_array(),
                "{} line {}: invalid search replay envelope",
                retained.display(),
                index + 1
            );
            let blocks = envelope["blocks"]
                .as_array()
                .ok_or_else(|| anyhow!("missing search blocks"))?;
            let call_id = blocks
                .iter()
                .find(|block| block["type"] == "server_tool_use")
                .and_then(|block| block["id"].as_str())
                .filter(|id| !id.is_empty());
            anyhow::ensure!(
                call_id.is_some_and(|id| blocks.iter().any(|block| matches!(
                    block["type"].as_str(),
                    Some("web_search_tool_result" | "tool_result")
                ) && block["tool_use_id"]
                    .as_str()
                    == Some(id))),
                "retained completed search envelope has no matching call/result pair"
            );
            retained_envelopes += 1;
        }
    }
    anyhow::ensure!(
        retained_envelopes > 0,
        "no persisted search replay envelope found in the retained rollout"
    );
    Ok(retained_envelopes)
}

/// Completed `web_search_call` items across every rollout under the codex
/// home: the rollout is the durable proof that the hosted search executed
/// (turn prompts explicitly demand it).
fn completed_web_search_calls(home: &Path) -> usize {
    session_rollouts(home)
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .map(|contents| {
            contents
                .lines()
                .filter(|line| !line.trim().is_empty())
                .filter(|line| {
                    serde_json::from_str::<Value>(line).is_ok_and(|record| {
                        record.get("type").and_then(Value::as_str) == Some("response_item")
                            && record["payload"]["type"].as_str() == Some("web_search_call")
                            && record["payload"]["status"].as_str() == Some("completed")
                    })
                })
                .count()
        })
        .sum()
}

/// All session rollout files recorded under a codex home, oldest first.
fn session_rollouts(home: &Path) -> Vec<PathBuf> {
    fn visit(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                out.push(path);
            }
        }
    }
    let mut rollouts = Vec::new();
    visit(&home.join("sessions"), &mut rollouts);
    rollouts.sort();
    rollouts
}

#[cfg(test)]
#[path = "binary_turns_tests.rs"]
mod tests;
