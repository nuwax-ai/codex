//! Real-subprocess contracts for memory claim cleanup on normal process exit.
//!
//! `codex-exec` drives an in-process app-server. On normal exit the app-server
//! shuts its threads down and then waits (bounded) for shutdown-bound memory
//! cleanup, so a claimed phase-2 consolidation job must be released before
//! the process is gone — even when the release transaction contends with a
//! brief writer lock on the memory database. A real agent failure must also
//! survive the exit: the later `Shutdown` status overwrite must not
//! reclassify the failure as a clean cancel.
//!
//! Every test here spawns the actual `codex-exec` binary against a localhost
//! mock vendor and inspects the persisted job row from this independent
//! process afterwards, mirroring the externally reproduced failures this
//! contract was written against.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Context;
use anyhow::Result;
use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::sync::watch;

/// How the mock vendor answers the consolidation agent's request.
#[derive(Clone, Copy)]
enum MemoryFault {
    /// Never answer: the agent is mid-request when the process exits.
    Hang,
    /// Answer with an HTTP 400 invalid_prompt error.
    InvalidPrompt,
}

#[derive(Clone, Copy)]
enum ForegroundGate {
    WaitForMemory,
    WaitForTest,
    Immediate,
}

struct VendorServer {
    _addr: std::net::SocketAddr,
    memory_arrived: watch::Receiver<bool>,
    foreground_release: watch::Sender<bool>,
    shutdown: watch::Sender<bool>,
}

impl VendorServer {
    async fn start(fault: MemoryFault, foreground_gate: ForegroundGate) -> VendorServer {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock vendor");
        let addr = listener.local_addr().expect("mock vendor address");
        let (memory_tx, memory_arrived) = watch::channel(false);
        let (foreground_release, foreground_ready) = watch::channel(false);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let arrived_handle = memory_arrived.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(value) => value,
                    Err(_) => return,
                };
                let memory_tx = memory_tx.clone();
                let gate = memory_arrived.clone();
                let foreground_ready = foreground_ready.clone();
                let mut conn_shutdown = shutdown_rx.clone();
                tokio::spawn(async move {
                    handle_connection(
                        stream,
                        fault,
                        foreground_gate,
                        memory_tx,
                        gate,
                        foreground_ready,
                        &mut conn_shutdown,
                    )
                    .await;
                });
            }
        });
        VendorServer {
            _addr: addr,
            memory_arrived: arrived_handle,
            foreground_release,
            shutdown: shutdown_tx,
        }
    }

    fn port(&self) -> u16 {
        self._addr.port()
    }
}

/// Reads one HTTP request (headers + content-length body) from the stream.
async fn read_http_request(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let separator = find_subsequence(&buffer, b"\r\n\r\n");
        if let Some(index) = separator {
            break index + 4;
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            anyhow::bail!("connection closed before request headers were complete");
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while buffer.len() < header_end + content_length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            anyhow::bail!("connection closed before request body was complete");
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Ok(buffer)
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn sse_ok_response() -> String {
    let frames: [(&str, &str); 5] = [
        (
            "response.created",
            r#"{"type":"response.created","response":{"id":"exit-review-r","object":"response","status":"in_progress","output":[]}}"#,
        ),
        (
            "response.output_item.added",
            r#"{"type":"response.output_item.added","output_index":0,"item":{"id":"exit-review-m","type":"message","role":"assistant","status":"in_progress","content":[]}}"#,
        ),
        (
            "response.output_text.delta",
            r#"{"type":"response.output_text.delta","item_id":"exit-review-m","output_index":0,"content_index":0,"delta":"ok"}"#,
        ),
        (
            "response.output_item.done",
            r#"{"type":"response.output_item.done","output_index":0,"item":{"id":"exit-review-m","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"ok","annotations":[]}]}}"#,
        ),
        (
            "response.completed",
            r#"{"type":"response.completed","response":{"id":"exit-review-r","object":"response","status":"completed","output":[],"usage":{"input_tokens":10,"output_tokens":1,"total_tokens":11}}}"#,
        ),
    ];
    let mut body = String::new();
    for (event, payload) in frames {
        body.push_str(&format!("event: {event}\ndata: {payload}\n\n"));
    }
    body
}

async fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

async fn handle_connection(
    mut stream: TcpStream,
    fault: MemoryFault,
    foreground_gate: ForegroundGate,
    memory_arrived: watch::Sender<bool>,
    mut memory_gate: watch::Receiver<bool>,
    mut foreground_ready: watch::Receiver<bool>,
    shutdown: &mut watch::Receiver<bool>,
) {
    let request = match read_http_request(&mut stream).await {
        Ok(request) => request,
        Err(_) => return,
    };
    let is_memory_agent = find_subsequence(&request, b"Memory Writing Agent").is_some();
    if is_memory_agent {
        let _ = memory_arrived.send(true);
        match fault {
            MemoryFault::Hang => {
                // Keep the request pending for the lifetime of the server so
                // the consolidation agent is mid-request at process exit.
                let _ = shutdown.wait_for(|closed| *closed).await;
            }
            MemoryFault::InvalidPrompt => {
                let body = r#"{"error":{"message":"exit review injected invalid prompt","type":"invalid_request_error","code":"invalid_prompt"}}"#;
                let _ =
                    write_http_response(&mut stream, "400 Bad Request", "application/json", body)
                        .await;
            }
        }
        return;
    }

    // Gate foreground responses on the consolidation request so the job is
    // always claimed before the process begins exiting.
    if matches!(
        foreground_gate,
        ForegroundGate::WaitForMemory | ForegroundGate::WaitForTest
    ) && !matches!(
        tokio::time::timeout(
            Duration::from_secs(20),
            memory_gate.wait_for(|arrived| *arrived),
        )
        .await,
        Ok(Ok(_))
    ) {
        return;
    }
    if matches!(foreground_gate, ForegroundGate::WaitForTest)
        && !matches!(
            tokio::time::timeout(
                Duration::from_secs(20),
                foreground_ready.wait_for(|ready| *ready),
            )
            .await,
            Ok(Ok(_))
        )
    {
        return;
    }
    let _ = write_http_response(
        &mut stream,
        "200 OK",
        "text/event-stream",
        &sse_ok_response(),
    )
    .await;
}

#[derive(Debug)]
struct JobRow {
    status: String,
    lease_until: Option<i64>,
    retry_at: Option<i64>,
    retry_remaining: i64,
    last_error: Option<String>,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

/// Opens the child's memory database directly, like the external inspector
/// this contract is written for: going through the codex-state shim would
/// reopen and migrate a database the child process just finalized.
#[expect(
    clippy::disallowed_methods,
    reason = "deliberate external inspection; the shim would mutate the database"
)]
async fn open_pool(home: &Path, read_only: bool) -> Option<sqlx::SqlitePool> {
    let mut dir = tokio::fs::read_dir(home).await.ok()?;
    while let Ok(Some(entry)) = dir.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("sqlite") {
            continue;
        }
        let mode = if read_only { "ro" } else { "rw" };
        let url = format!("sqlite://{}?mode={mode}", path.display());
        let Ok(options) = std::str::FromStr::from_str(&url) else {
            continue;
        };
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await;
        let Ok(pool) = pool else { continue };
        // Only the memory database carries the consolidation job row.
        let has_jobs_table = sqlx::query("SELECT 1 FROM jobs LIMIT 1")
            .execute(&pool)
            .await
            .is_ok();
        if has_jobs_table {
            return Some(pool);
        }
        let _ = pool.close().await;
    }
    None
}

async fn read_job_row_if_present(home: &Path) -> Option<JobRow> {
    use sqlx::Row;

    let Some(pool) = open_pool(home, /*read_only*/ true).await else {
        assert!(
            !home.join("memories_1.sqlite").exists(),
            "an existing memory database must be readable by the independent probe",
        );
        return None;
    };
    let row = sqlx::query(
        "SELECT status, lease_until, retry_at, retry_remaining, last_error \
         FROM jobs WHERE kind = 'memory_consolidate_global'",
    )
    .fetch_optional(&pool)
    .await
    .expect("read consolidation job row");
    let job = row.map(|row| JobRow {
        status: row.try_get("status").expect("status column"),
        lease_until: row.try_get("lease_until").expect("lease_until column"),
        retry_at: row.try_get("retry_at").expect("retry_at column"),
        retry_remaining: row
            .try_get("retry_remaining")
            .expect("retry_remaining column"),
        last_error: row.try_get("last_error").expect("last_error column"),
    });
    pool.close().await;
    job
}

async fn read_job_row(home: &Path) -> JobRow {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(job) = read_job_row_if_present(home).await {
            return job;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "job row never appeared"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Takes a `BEGIN IMMEDIATE` writer lock on the memory database for `hold`.
async fn hold_memory_db_write_lock(
    home: &Path,
    hold: Duration,
    foreground_release: &watch::Sender<bool>,
) -> Result<()> {
    let pool = open_pool(home, /*read_only*/ false)
        .await
        .context("memory database with jobs table not found for lock")?;
    let mut connection = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await?;
    foreground_release.send(true)?;
    tokio::time::sleep(hold).await;
    sqlx::query("ROLLBACK").execute(&mut *connection).await?;
    drop(connection);
    let _ = pool.close().await;
    Ok(())
}

struct ExecOutcome {
    returncode: i32,
}

async fn run_codex_exec(home: &Path, cwd: &Path, timeout: Duration) -> ExecOutcome {
    let binary = codex_utils_cargo_bin::cargo_bin("codex-exec").expect("codex-exec binary");
    let mut command = Command::new(binary);
    command
        .arg("--skip-git-repo-check")
        .arg("--dangerously-bypass-approvals-and-sandbox")
        .arg("--json")
        .arg("-C")
        .arg(cwd)
        .arg("Return ok without any tools.")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Keep the ambient environment (like the external reproduction) minus
    // proxy and sandbox overrides that would reroute the localhost vendor.
    for (key, _) in std::env::vars() {
        if key.to_ascii_lowercase().ends_with("_proxy")
            || key.starts_with("NUWAX_")
            || key == "CODEX_SANDBOX_NETWORK_DISABLED"
            || key == "CODEX_SANDBOX"
        {
            command.env_remove(&key);
        }
    }
    command
        .env("CODEX_HOME", home)
        .env("OFFLINE_MEMORY_REVIEW_KEY", "dummy-exit-contract-key");

    let child = command.spawn().expect("spawn codex-exec");
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .expect("codex-exec did not exit within the test timeout")
        .expect("wait for codex-exec");
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "codex-exec failed: {}\nstdout tail: {}\nstderr tail: {}",
            output.status,
            &stdout[stdout.len().saturating_sub(2000)..],
            &stderr[stderr.len().saturating_sub(2000)..]
        );
    }
    ExecOutcome {
        returncode: output.status.code().unwrap_or(-1),
    }
}

fn write_config(home: &Path, port: u16) -> Result<()> {
    let config = format!(
        r#"model = "offline-exit-review"
model_provider = "offline-exit-review"

[model_providers.offline-exit-review]
name = "offline-exit-review"
base_url = "http://127.0.0.1:{port}/v1"
wire_api = "responses"
env_key = "OFFLINE_MEMORY_REVIEW_KEY"
supports_websockets = false
requires_openai_auth = false

[features]
memories = true
sqlite = true

[memories]
generate_memories = true
use_memories = true
"#
    );
    std::fs::write(home.join("config.toml"), config)?;
    Ok(())
}

async fn wait_for_memory_request(server: &VendorServer) {
    let mut arrived = server.memory_arrived.clone();
    tokio::time::timeout(
        Duration::from_secs(30),
        arrived.wait_for(|arrived| *arrived),
    )
    .await
    .expect("consolidation request never reached the mock vendor")
    .expect("memory arrival channel alive");
}

/// A normal exit must release the claimed consolidation job even when the
/// release transaction contends with a brief writer lock on the memory
/// database: rc=0 with the job left `running` (a one-hour lease) is the
/// failure this contract forbids.
#[tokio::test(flavor = "multi_thread")]
async fn normal_exit_releases_memory_claim_despite_transient_db_write_lock() -> Result<()> {
    let run = TempDir::new()?;
    let home = run.path().join("home");
    let cwd = run.path().join("work");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let server = VendorServer::start(MemoryFault::Hang, ForegroundGate::WaitForTest).await;
    write_config(&home, server.port())?;

    let home_path = home.clone();
    let cwd_path = cwd.clone();
    let exec_task = tokio::spawn(async move {
        run_codex_exec(&home_path, &cwd_path, Duration::from_secs(60)).await
    });

    wait_for_memory_request(&server).await;
    // Hold a brief ordinary writer lock exactly like a concurrent writer
    // would; the release must land once the lock goes away, before exit.
    hold_memory_db_write_lock(&home, Duration::from_secs(1), &server.foreground_release).await?;

    let outcome = exec_task.await.expect("exec task joined");
    assert_eq!(outcome.returncode, 0);

    let job = read_job_row(&home).await;
    assert_eq!(job.status, "pending", "job must be released, got {job:?}");
    assert_eq!(job.last_error, None);
    assert_eq!(
        job.retry_remaining, 3,
        "clean cancel must not touch the budget"
    );
    assert!(
        job.lease_until.is_none(),
        "released job must not hold a lease"
    );
    // The retry delay class is what matters: the ~5s clean-cancel delay is
    // counted from the CHILD's release and this process reads the row some
    // time after the child exited (several seconds slower on Windows), so
    // only assert it stays far below the 3600s failure backoff.
    let now = now_unix();
    assert!(
        job.retry_at.is_some_and(|retry_at| retry_at - now < 1800),
        "clean-cancel retry delay must stay in the short class, got retry_at={:?} now={now}",
        job.retry_at
    );
    let _ = server.shutdown.send(true);
    Ok(())
}

/// A real agent failure (HTTP 400 invalid_prompt) immediately followed by the
/// normal process exit must keep the failure evidence: `error` status, the
/// failure reason, a consumed retry, and the long failure backoff — not the
/// clean-cancel signature of a 5s retry and a pristine budget.
#[tokio::test(flavor = "multi_thread")]
async fn normal_exit_after_real_agent_error_keeps_failure_budget_and_backoff() -> Result<()> {
    let run = TempDir::new()?;
    let home = run.path().join("home");
    let cwd = run.path().join("work");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let server =
        VendorServer::start(MemoryFault::InvalidPrompt, ForegroundGate::WaitForMemory).await;
    write_config(&home, server.port())?;

    let home_path = home.clone();
    let cwd_path = cwd.clone();
    let exec_task = tokio::spawn(async move {
        run_codex_exec(&home_path, &cwd_path, Duration::from_secs(60)).await
    });

    wait_for_memory_request(&server).await;
    let outcome = exec_task.await.expect("exec task joined");
    assert_eq!(outcome.returncode, 0);

    let job = read_job_row(&home).await;
    assert_eq!(
        job.status, "error",
        "real failure must not become a clean cancel: {job:?}"
    );
    assert_eq!(job.last_error.as_deref(), Some("failed_agent"));
    assert_eq!(
        job.retry_remaining, 2,
        "the failure budget must be consumed"
    );
    let now = now_unix();
    // 3600s class counted from the child's clock; allow generous read skew.
    assert!(
        job.retry_at
            .is_some_and(|retry_at| (3400..=3800).contains(&(retry_at - now))),
        "failure backoff must stay at the long retry delay, got retry_at={:?} now={now}",
        job.retry_at
    );
    let _ = server.shutdown.send(true);
    Ok(())
}

/// When the cleanup cannot finish (the write lock is held far beyond the
/// shutdown bound), the exit must still be bounded: the process leaves, the
/// job stays `running` under its lease, and lease expiry remains the
/// documented recovery backstop.
#[tokio::test(flavor = "multi_thread")]
async fn normal_exit_stays_bounded_when_cleanup_is_blocked_leaving_lease_backstop() -> Result<()> {
    let run = TempDir::new()?;
    let home = run.path().join("home");
    let cwd = run.path().join("work");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let server = VendorServer::start(MemoryFault::Hang, ForegroundGate::WaitForTest).await;
    write_config(&home, server.port())?;

    let home_path = home.clone();
    let cwd_path = cwd.clone();
    let exec_task = tokio::spawn(async move {
        run_codex_exec(&home_path, &cwd_path, Duration::from_secs(45)).await
    });

    wait_for_memory_request(&server).await;
    // Keep the writer lock until the test has observed process exit. Releasing
    // it on a wall-clock timer could race slower hosts and hide blocked cleanup.
    let lock_home = home.clone();
    let mut lock_shutdown = server.shutdown.subscribe();
    let (lock_ready, locked) = tokio::sync::oneshot::channel();
    let lock_task = tokio::spawn(async move {
        let pool = open_pool(&lock_home, /*read_only*/ false)
            .await
            .context("memory database with jobs table not found for lock")?;
        let mut connection = pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *connection)
            .await?;
        let _ = lock_ready.send(());
        let _ = lock_shutdown.wait_for(|closed| *closed).await;
        sqlx::query("ROLLBACK").execute(&mut *connection).await?;
        drop(connection);
        let _ = pool.close().await;
        Ok::<(), anyhow::Error>(())
    });

    // The memory request alone is not a writer-lock handshake: the child
    // can otherwise exit and release its claim before this task is polled.
    tokio::time::timeout(Duration::from_secs(10), locked)
        .await
        .context("waiting for the cleanup-blocking writer lock")?
        .context("writer lock task failed before acquiring the lock")?;
    server.foreground_release.send(true)?;
    let started = tokio::time::Instant::now();
    let outcome = exec_task.await.expect("exec task joined");
    let elapsed = started.elapsed();
    assert_eq!(outcome.returncode, 0);
    assert!(
        elapsed < Duration::from_secs(40),
        "exit must stay bounded while cleanup is blocked, took {elapsed:?}"
    );

    let job = read_job_row(&home).await;
    assert_eq!(
        job.status, "running",
        "blocked cleanup must leave the claim: {job:?}"
    );
    assert_eq!(job.retry_remaining, 3);
    let now = now_unix();
    assert!(
        job.lease_until.is_some_and(|lease| lease > now),
        "lease expiry must remain the recovery backstop, got lease_until={:?} now={now}",
        job.lease_until
    );
    // The lock is still held here, proving the exit did not wait for it to
    // release. Only the explicit signal below ends the blocking transaction.
    assert!(
        !lock_task.is_finished(),
        "the writer lock should still be held right after the bounded exit"
    );
    let _ = server.shutdown.send(true);
    let _ = lock_task.await;
    Ok(())
}

/// Foreground completion need not wait for the memory startup pipeline. A
/// shutdown fence must reject a late consolidation start and release any claim
/// it already owns, rather than waiting for a newly started hung agent.
#[tokio::test(flavor = "multi_thread")]
async fn fast_foreground_exit_does_not_leave_a_late_memory_agent_claim() -> Result<()> {
    let run = TempDir::new()?;
    let home = run.path().join("home");
    let cwd = run.path().join("work");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;
    let server = VendorServer::start(MemoryFault::Hang, ForegroundGate::Immediate).await;
    write_config(&home, server.port())?;

    let outcome = run_codex_exec(&home, &cwd, Duration::from_secs(45)).await;
    assert_eq!(outcome.returncode, 0);
    // No row is valid when shutdown fences startup before phase2 claims work.
    // If it has already claimed, normal cancellation must relinquish ownership.
    if let Some(job) = read_job_row_if_present(&home).await {
        assert_eq!(
            (
                job.status,
                job.lease_until,
                job.retry_remaining,
                job.last_error
            ),
            ("pending".to_owned(), None, 3, None),
        );
    }
    let _ = server.shutdown.send(true);
    Ok(())
}
