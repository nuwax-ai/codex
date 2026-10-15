//! Fork (nuwax-codex): LIVE acceptance for the typed `NUWAX_*` environment
//! startup through a REAL app-server child against a REAL vendor gateway.
//!
//! Gating: the `LIVE_MIMO_CHAT_URL` / `LIVE_MIMO_API_KEY` / `LIVE_MIMO_MODEL`
//! group must be present in the test process environment (source the private
//! `.env.local` before running); otherwise every scenario reports
//! "not configured" and returns. Credentials never appear in assertions,
//! artifacts or logs — the request capture redacts by construction and the
//! non-leak scan below treats the key as untouchable output.
//!
//! Every scenario runs under a hard pre-dispatch HTTP attempt budget
//! (`CODEX_RIG_MAX_HTTP_ATTEMPTS`) shared across ALL processes of the scene
//! through the same capture ledger: sampling, continuation, resume and
//! compact requests all count, and an over-budget dispatch fails before
//! reaching the vendor.
//!
//! ```text
//! set -a; source .env.local; set +a
//! cargo nextest run -p codex-app-server -E 'test(nuwax_positive_wires_live)'
//! ```

use anyhow::Context;
use anyhow::Result;
use app_test_support::TestAppServer;
use codex_app_server_protocol::ThreadCompactStartParams;
use codex_app_server_protocol::ThreadCompactStartResponse;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::TurnCompletedNotification;
use codex_app_server_protocol::TurnInterruptParams;
use codex_app_server_protocol::TurnInterruptResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::UserInput as V2UserInput;
use codex_app_server_transport::REMOTE_CONTROL_DISABLED_ENV_VAR;
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tempfile::TempDir;
use tokio::time::Instant;
use tokio::time::sleep;
use tokio::time::timeout;

const READ_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 90);

struct LiveVendor {
    base_url: String,
    api_key: String,
    model: String,
}

/// LIVE gating: the private MiMo group must already be in the process env.
fn live_vendor() -> Option<LiveVendor> {
    Some(LiveVendor {
        base_url: std::env::var("LIVE_MIMO_CHAT_URL").ok()?,
        api_key: std::env::var("LIVE_MIMO_API_KEY").ok()?,
        model: std::env::var("LIVE_MIMO_MODEL").ok()?,
    })
}

/// The live suite exercises the provider wire, not approvals; see the
/// offline sibling for why the policy is pinned.
fn never_approval_thread() -> ThreadStartParams {
    ThreadStartParams {
        approval_policy: Some(codex_app_server_protocol::AskForApproval::Never),
        ..Default::default()
    }
}

fn marker(tag: &str) -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("live-mimo-{tag}-{nonce}")
}

/// Spawns a REAL app-server child whose provider group arrives ONLY through
/// the typed `NUWAX_*` environment; the request capture ledger and the hard
/// attempt budget are wired through the same env, so every process of the
/// scene (initial child and cold-resume child) shares one ledger.
async fn live_child(
    codex_home: &TempDir,
    vendor: &LiveVendor,
    ledger: &Path,
    http_attempt_budget: usize,
    output_cap: u64,
) -> Result<TestAppServer> {
    let ledger_str = ledger.to_str().expect("ledger path is UTF-8").to_string();
    let budget_str = http_attempt_budget.to_string();
    let cap_str = output_cap.to_string();
    let overrides: Vec<(&str, Option<&str>)> = [
        ("NUWAX_BASE_URL", Some(vendor.base_url.as_str())),
        ("NUWAX_WIRE_API", Some("chat")),
        ("NUWAX_API_KEY", Some(vendor.api_key.as_str())),
        ("NUWAX_MODEL", Some(vendor.model.as_str())),
        ("NUWAX_REQUEST_MAX_RETRIES", Some("0")),
        ("NUWAX_STREAM_MAX_RETRIES", Some("0")),
        ("NUWAX_STREAM_IDLE_TIMEOUT_MS", Some("45000")),
        ("CODEX_RIG_REQUEST_CAPTURE_FILE", Some(ledger_str.as_str())),
        ("CODEX_RIG_MAX_HTTP_ATTEMPTS", Some(budget_str.as_str())),
        // The per-scene output cap (lowered only by the exhaustion case).
        ("NUWAX_MAX_OUTPUT_TOKENS", Some(cap_str.as_str())),
        (REMOTE_CONTROL_DISABLED_ENV_VAR, Some("1")),
    ]
    .into_iter()
    .collect();
    TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_managed_config()
        .with_env_overrides(&overrides)
        .build_initialized_with_timeout(READ_TIMEOUT)
        .await
}

fn ledger_attempts(ledger: &Path) -> usize {
    std::fs::read_to_string(ledger)
        .map(|contents| {
            contents
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count()
        })
        .unwrap_or(0)
}

/// Every recorded attempt must hit the configured gateway with the
/// configured model — asserted from the pre-send capture, never from logs.
fn assert_ledger_shape(ledger: &Path, vendor: &LiveVendor) -> Result<()> {
    let contents = std::fs::read_to_string(ledger).context("capture ledger")?;
    let attempts: Vec<&str> = contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    anyhow::ensure!(!attempts.is_empty(), "the capture ledger is empty");
    for (index, line) in attempts.iter().enumerate() {
        let entry: Value =
            serde_json::from_str(line).with_context(|| format!("ledger entry {index}"))?;
        let url = entry["url"].as_str().unwrap_or_default();
        anyhow::ensure!(
            url.starts_with(&vendor.base_url),
            "entry {index} targets {url}, expected the configured gateway"
        );
        anyhow::ensure!(
            entry["body"]["model"].as_str() == Some(vendor.model.as_str()),
            "entry {index} model mismatch"
        );
        anyhow::ensure!(
            !line.contains(&vendor.api_key),
            "entry {index} leaked the credential"
        );
    }
    Ok(())
}

/// The typed-env non-leak contract: the credential must never persist into
/// the child's home or the scene's artifacts.
fn assert_no_credential_leak(home: &Path, key: &str) -> Result<()> {
    let mut hits = Vec::new();
    let mut pending = vec![home.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.is_file()
                && std::fs::read_to_string(&path)
                    .map(|text| text.contains(key))
                    .unwrap_or(false)
            {
                hits.push(path);
            }
        }
    }
    anyhow::ensure!(hits.is_empty(), "credential leaked to: {hits:?}");
    Ok(())
}

async fn start_turn_and_wait(
    app: &mut TestAppServer,
    thread_id: &str,
    prompt: &str,
) -> Result<TurnCompletedNotification> {
    let environment = app.auto_env_params()?;
    let completed = timeout(
        READ_TIMEOUT,
        app.start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: thread_id.to_string(),
            input: vec![V2UserInput::Text {
                text: prompt.to_string(),
                text_elements: Vec::new(),
            }],
            environments: Some(vec![environment]),
            ..Default::default()
        }),
    )
    .await??;
    Ok(completed)
}

fn agent_text(completed: &TurnCompletedNotification) -> String {
    completed
        .turn
        .items
        .iter()
        .filter_map(|item| match item {
            codex_app_server_protocol::ThreadItem::AgentMessage { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn command_executed_with_marker(completed: &TurnCompletedNotification, marker: &str) -> bool {
    completed.turn.items.iter().any(|item| {
        matches!(
            item,
            codex_app_server_protocol::ThreadItem::CommandExecution { .. }
        ) && serde_json::to_string(item)
            .map(|text| text.contains(marker))
            .unwrap_or(false)
    })
}

#[tokio::test]
async fn live_env_tool_round_trip_usage_and_interrupt_with_two_resumes() -> Result<()> {
    let Some(vendor) = live_vendor() else {
        eprintln!("live vendor not configured — skipping");
        return Ok(());
    };
    let codex_home = TempDir::new()?;
    let artifacts: PathBuf = std::env::temp_dir().join(format!(
        "nuwax-live-{}",
        marker("wires").replace("live-mimo-", "")
    ));
    std::fs::create_dir_all(&artifacts)?;
    let ledger = artifacts.join("requests.jsonl");
    let marker = marker("roundtrip");

    let mut app = live_child(
        &codex_home,
        &vendor,
        &ledger,
        /*budget*/ 6,
        /*cap*/ 2048,
    )
    .await?;
    let thread = app.start_thread(never_approval_thread()).await?.thread;

    // 1. Tool round trip with a random marker: the command must really run,
    //    its output must come back, the answer must carry the marker, and the
    //    live usage must reach the shared v2 accounting.
    let mut token_usage_params = Vec::new();
    let mut command_items = Vec::new();
    let completed = {
        let environment = app.auto_env_params()?;
        let request_id = app
            .send_turn_start_request(TurnStartParams {
                thread_id: thread.id.clone(),
                input: vec![V2UserInput::Text {
                    text: format!(
                        "请务必调用 shell 工具真实执行命令 `echo {marker}`（不要只把命令当文本输出），然后把命令的原始输出逐字告诉我，不要添加任何解释。"
                    ),
                    text_elements: Vec::new(),
                }],
                environments: Some(vec![environment]),
                ..Default::default()
            })
            .await?;
        let _: TurnStartResponse = timeout(READ_TIMEOUT, app.read_response(request_id)).await??;
        let mut completed = None;
        timeout(READ_TIMEOUT, async {
            loop {
                let notification = app
                    .read_stream_until_matching_notification(
                        "token usage, command item or turn completion",
                        |notification| {
                            matches!(
                                notification.method.as_str(),
                                "thread/tokenUsage/updated" | "turn/completed" | "item/completed"
                            )
                        },
                    )
                    .await?;
                if notification.method.as_str() == "thread/tokenUsage/updated" {
                    token_usage_params.push(notification.params.clone().context("params")?);
                } else if notification.method.as_str() == "item/completed" {
                    command_items.push(notification.params.clone().context("params")?);
                } else {
                    completed = Some(
                        serde_json::from_value::<TurnCompletedNotification>(
                            notification.params.context("params")?,
                        )
                        .context("turn/completed shape")?,
                    );
                    return Ok::<(), anyhow::Error>(());
                }
            }
        })
        .await??;
        completed.expect("turn/completed observed")
    };
    assert_eq!(completed.turn.status, TurnStatus::Completed);
    assert!(completed.turn.error.is_none());
    let usage: codex_app_server_protocol::ThreadTokenUsageUpdatedNotification =
        serde_json::from_value(
            token_usage_params
                .last()
                .context("live usage must reach thread/tokenUsage/updated")?
                .clone(),
        )?;
    assert!(
        usage.token_usage.last.input_tokens > 0 && usage.token_usage.last.output_tokens > 0,
        "live usage must be nonzero: {:?}",
        usage.token_usage.last
    );
    let executed = command_items.iter().any(|item| {
        item["item"]["type"].as_str() == Some("commandExecution")
            && item["item"]["exitCode"].as_i64() == Some(0)
            && item["item"]["aggregatedOutput"]
                .as_str()
                .is_some_and(|output| output.contains(&marker))
    });
    assert!(
        executed,
        "the shell tool must really execute the marker command: {command_items:?}"
    );
    let answer = agent_text(&completed);
    assert!(
        answer.contains(&marker),
        "the final answer must carry the executed marker: {answer}"
    );

    // 2. Real interrupt mid-generation; the in-flight attempt must be the
    //    last wire request until the resume.
    let environment = app.auto_env_params()?;
    let TurnStartResponse { turn } = timeout(
        READ_TIMEOUT,
        app.request(
            |request_id| codex_app_server_protocol::ClientRequest::TurnStart {
                request_id,
                params: TurnStartParams {
                    thread_id: thread.id.clone(),
                    client_user_message_id: None,
                    input: vec![V2UserInput::Text {
                        text: "请写一篇至少六百字的文章，主题是远海的信标，不要提前结束。"
                            .to_string(),
                        text_elements: Vec::new(),
                    }],
                    environments: Some(vec![environment]),
                    ..Default::default()
                },
            },
        ),
    )
    .await??;
    let _: codex_app_server_protocol::TurnStartedNotification =
        timeout(READ_TIMEOUT, app.read_notification("turn/started")).await??;
    sleep(Duration::from_millis(1500)).await;
    let _: TurnInterruptResponse = timeout(
        READ_TIMEOUT,
        app.request(
            |request_id| codex_app_server_protocol::ClientRequest::TurnInterrupt {
                request_id,
                params: TurnInterruptParams {
                    thread_id: thread.id.clone(),
                    turn_id: turn.id.clone(),
                },
            },
        ),
    )
    .await??;
    let interrupted: TurnCompletedNotification =
        timeout(READ_TIMEOUT, app.read_notification("turn/completed")).await??;
    assert_eq!(
        (interrupted.turn.status, interrupted.turn.error.is_none()),
        (TurnStatus::Interrupted, true)
    );
    let attempts_after_interrupt = ledger_attempts(&ledger);
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        sleep(Duration::from_millis(250)).await;
        anyhow::ensure!(
            ledger_attempts(&ledger) <= attempts_after_interrupt,
            "a confirmed interrupt must not be followed by new sampling"
        );
    }

    // 3. Warm resume in the same child.
    let resume_id = app
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: thread.id.clone(),
            ..Default::default()
        })
        .await?;
    let _: ThreadResumeResponse = timeout(READ_TIMEOUT, app.read_response(resume_id)).await??;
    let recalled = start_turn_and_wait(
        &mut app,
        &thread.id,
        "刚才我让你执行的命令的原始输出是什么？逐字回答。",
    )
    .await?;
    assert_eq!(recalled.turn.status, TurnStatus::Completed);
    assert!(
        agent_text(&recalled).contains(&marker),
        "warm resume must recall the marker: {}",
        agent_text(&recalled)
    );

    // 4. Cold resume in a fresh child sharing the SAME ledger and budget.
    app.shutdown_gracefully().await?;
    let mut cold = live_child(
        &codex_home,
        &vendor,
        &ledger,
        /*budget*/ 6,
        /*cap*/ 2048,
    )
    .await?;
    let resume_id = cold
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: thread.id.clone(),
            ..Default::default()
        })
        .await?;
    let resumed: ThreadResumeResponse =
        timeout(READ_TIMEOUT, cold.read_response(resume_id)).await??;
    assert_eq!(
        (resumed.model.as_str(), resumed.model_provider.as_str()),
        (vendor.model.as_str(), "nuwax_env")
    );
    let after_cold = start_turn_and_wait(
        &mut cold,
        &thread.id,
        "请用一句话概括：本会话至今执行过什么命令？",
    )
    .await?;
    assert_eq!(after_cold.turn.status, TurnStatus::Completed);
    cold.shutdown_gracefully().await?;

    assert_ledger_shape(&ledger, &vendor)?;
    assert!(
        ledger_attempts(&ledger) <= 6,
        "the shared budget bounds the whole scene: {}",
        ledger_attempts(&ledger)
    );
    assert_no_credential_leak(codex_home.path(), &vendor.api_key)?;
    Ok(())
}

#[tokio::test]
async fn live_env_compact_and_post_compact_recall() -> Result<()> {
    let Some(vendor) = live_vendor() else {
        eprintln!("live vendor not configured — skipping");
        return Ok(());
    };
    let codex_home = TempDir::new()?;
    let artifacts = std::env::temp_dir().join(marker("compact"));
    std::fs::create_dir_all(&artifacts)?;
    let ledger = artifacts.join("requests.jsonl");
    let passphrase = marker("secret");

    let mut app = live_child(
        &codex_home,
        &vendor,
        &ledger,
        /*budget*/ 6,
        /*cap*/ 2048,
    )
    .await?;
    let thread = app.start_thread(never_approval_thread()).await?.thread;
    let learned = start_turn_and_wait(
        &mut app,
        &thread.id,
        &format!("请记住暗号：{passphrase}。只回复OK。"),
    )
    .await?;
    assert_eq!(learned.turn.status, TurnStatus::Completed);

    let compact_id = app
        .send_thread_compact_start_request(ThreadCompactStartParams {
            thread_id: thread.id.clone(),
        })
        .await?;
    let _: ThreadCompactStartResponse =
        timeout(READ_TIMEOUT, app.read_response(compact_id)).await??;
    // The compact turn runs to completion before the recall turn can start
    // (compaction is itself a turn; steering into it is rejected).
    let before_compact = ledger_attempts(&ledger);
    // Stale notifications from earlier turns must not satisfy the wait. The
    // compaction ITEM completes while the compact TURN is still finalizing
    // (rollout migration): wait for both, pinned to the compact turn's id.
    app.clear_message_buffer();
    let compaction = timeout(
        READ_TIMEOUT,
        app.read_stream_until_matching_notification("compaction item completed", |notification| {
            notification.method.as_str() == "item/completed"
                && notification
                    .params
                    .as_ref()
                    .and_then(|params| params["item"]["type"].as_str())
                    .is_some_and(|kind| kind == "contextCompaction")
        }),
    )
    .await??;
    let compact_turn_id = compaction
        .params
        .as_ref()
        .and_then(|params| params["turnId"].as_str())
        .context("compaction turn id")?
        .to_string();
    let _ = timeout(
        READ_TIMEOUT,
        app.read_stream_until_matching_notification("compact turn completed", |notification| {
            notification.method.as_str() == "turn/completed"
                && notification
                    .params
                    .as_ref()
                    .and_then(|params| params["turn"]["id"].as_str())
                    .is_some_and(|turn_id| turn_id == compact_turn_id)
        }),
    )
    .await??;
    anyhow::ensure!(
        ledger_attempts(&ledger) > before_compact,
        "compaction never sampled the live wire"
    );

    let recalled = start_turn_and_wait(
        &mut app,
        &thread.id,
        "我们前面约定了一个核对用的关键词，请逐字复述那个关键词，不要任何其他内容。",
    )
    .await?;
    assert_eq!(recalled.turn.status, TurnStatus::Completed);
    assert!(
        agent_text(&recalled).contains(&passphrase),
        "the passphrase must survive live compaction: {}",
        agent_text(&recalled)
    );
    app.shutdown_gracefully().await?;

    assert_ledger_shape(&ledger, &vendor)?;
    assert!(
        ledger_attempts(&ledger) <= 6,
        "compact scene stays inside the budget: {}",
        ledger_attempts(&ledger)
    );
    assert_no_credential_leak(codex_home.path(), &vendor.api_key)?;
    Ok(())
}

#[tokio::test]
async fn live_env_output_cap_exhaustion_fails_without_resampling() -> Result<()> {
    let Some(vendor) = live_vendor() else {
        eprintln!("live vendor not configured — skipping");
        return Ok(());
    };
    let codex_home = TempDir::new()?;
    let artifacts = std::env::temp_dir().join(marker("cap"));
    std::fs::create_dir_all(&artifacts)?;
    let ledger = artifacts.join("requests.jsonl");

    let mut app = live_child(
        &codex_home,
        &vendor,
        &ledger,
        /*budget*/ 2,
        /*cap*/ 64,
    )
    .await?;
    let thread = app.start_thread(never_approval_thread()).await?.thread;
    let completed = start_turn_and_wait(
        &mut app,
        &thread.id,
        "请直接输出一段至少两百字的故事，主题是远航的灯塔，不要任何解释。",
    )
    .await;
    app.shutdown_gracefully().await?;
    let completed = completed?;
    assert_ne!(completed.turn.status, TurnStatus::Completed);
    let error_text = completed
        .turn
        .error
        .map(|error| error.message)
        .unwrap_or_default();
    assert!(
        error_text.contains("Output token limit reached"),
        "a real exhausted cap must surface the cap terminal: {error_text}"
    );
    assert_eq!(
        ledger_attempts(&ledger),
        1,
        "an exhausted cap must not resample"
    );
    assert_ledger_shape(&ledger, &vendor)?;
    assert_no_credential_leak(codex_home.path(), &vendor.api_key)?;
    Ok(())
}
