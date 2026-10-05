//! C2 acceptance for the stream-wait side of the NUWAX controls: cancelling
//! a child that is waiting for its first SSE response bytes must close the
//! in-flight request with no further attempt, and two PARALLEL children must
//! enforce their DISTINCT idle budgets on identically stalled streams (the
//! failure timings diverge, proving the values reach real behavior rather
//! than only config state).

use anyhow::Context;
use anyhow::Result;
use core_test_support::test_codex_exec::test_codex_exec;
use pretty_assertions::assert_eq;
use std::time::Duration;
use std::time::Instant;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

/// One Chat SSE frame and nothing else: the stream truncates immediately, so
/// the only live budget on the connection is the idle timeout.
const TRUNCATED_CHAT_FRAME: &str = "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"nuwax-controls-model\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"par\"},\"finish_reason\":null}]}\n\n";

const CONFIG_ENV_VARIABLES: [&str; 12] = [
    "NUWAX_BASE_URL",
    "NUWAX_WIRE_API",
    "NUWAX_API_KEY",
    "NUWAX_MODEL",
    "NUWAX_MAX_OUTPUT_TOKENS",
    "NUWAX_REQUEST_MAX_RETRIES",
    "NUWAX_STREAM_MAX_RETRIES",
    "NUWAX_STREAM_IDLE_TIMEOUT_MS",
    "CODEX_MODEL_REASONING_EFFORT",
    "CODEX_MODEL_CONTEXT_WINDOW",
    "CODEX_AUTO_COMPACT_TOKEN_LIMIT",
    "CODEX_AUTO_COMPACT_RATIO",
];

/// Reads one full HTTP request (header block plus Content-Length body) from
/// the socket, returning the raw headers and body bytes.
async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Result<(String, Vec<u8>)> {
    let mut data = Vec::new();
    let header_end = loop {
        let mut chunk = [0u8; 4096];
        let read = socket.read(&mut chunk).await?;
        anyhow::ensure!(read != 0, "client closed before sending the request");
        data.extend_from_slice(&chunk[..read]);
        if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&data[..header_end]).into_owned();
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then_some(value.trim())
        })
        .context("request must carry content-length")?
        .parse::<usize>()?;
    while data.len() < header_end + length {
        let mut chunk = [0u8; 4096];
        let read = socket.read(&mut chunk).await?;
        anyhow::ensure!(read != 0, "client closed mid-request");
        data.extend_from_slice(&chunk[..read]);
    }
    Ok((headers, data[header_end..header_end + length].to_vec()))
}

fn assert_chat_post(headers: &str, body: &[u8], model: &str, bearer: &str) -> Result<()> {
    assert!(
        headers.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"),
        "unexpected request line: {headers}"
    );
    assert!(
        headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case(&format!("authorization: Bearer {bearer}"))),
        "request must carry this child's bearer token"
    );
    let body: serde_json::Value = serde_json::from_slice(body)?;
    assert_eq!(body["model"], serde_json::json!(model));
    Ok(())
}

/// Keep accepting after the first connection closes: a late retry must
/// remain observable until the child has exited and its output was collected.
async fn reject_late_requests_until_child_exits(
    listener: &tokio::net::TcpListener,
    child_exited: tokio::sync::oneshot::Receiver<()>,
) -> Result<()> {
    tokio::select! {
        biased;
        extra = listener.accept() => {
            extra?;
            anyhow::bail!("the child opened another connection after its first request closed");
        }
        exited = child_exited => {
            exited.context("the child's completion signal was dropped")?;
        }
    }
    Ok(())
}

/// Negative control for the post-EOF observation phase: the simulated child
/// has closed its first socket, but has not reported process completion when
/// another connection arrives. The same guard used by both gateways must fail.
#[tokio::test]
async fn stream_wait_gateway_rejects_a_late_connection_before_child_exit() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(/*secs*/ 40), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let first_client = tokio::net::TcpStream::connect(address).await?;
        let (mut first_socket, _) = listener.accept().await?;
        drop(first_client);
        let mut chunk = [0u8; 1];
        assert_eq!(first_socket.read(&mut chunk).await?, 0);

        let (child_exited, child_exited_rx) = tokio::sync::oneshot::channel::<()>();
        let gateway = tokio::spawn(async move {
            reject_late_requests_until_child_exits(&listener, child_exited_rx).await
        });
        let _late_client = tokio::net::TcpStream::connect(address).await?;
        let error = gateway
            .await?
            .err()
            .context("gateway accepted a late connection before child exit")?;
        assert!(
            error
                .to_string()
                .contains("another connection after its first request closed"),
            "the late connection must trigger the request guard: {error}"
        );
        drop(child_exited);
        Ok::<_, anyhow::Error>(())
    })
    .await?
}

/// Interrupting the child while it awaits the FIRST response bytes of the
/// SSE stream (the gateway has consumed the request and answered nothing)
/// must close the in-flight connection, exit through the interruption path
/// — not the idle budget — and never send a second request. The idle budget
/// is pinned high so the signal is the only prompt exit. Windows Ctrl-C
/// cancellation needs a native console fixture and remains unexecuted by
/// this Unix signal test.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nuwax_env_interrupt_while_awaiting_sse_response_closes_the_request() -> Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (request_read, request_read_rx) = tokio::sync::oneshot::channel::<()>();
    let (child_exited, child_exited_rx) = tokio::sync::oneshot::channel::<()>();
    let gateway = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(40), async move {
            let (mut socket, _) = listener.accept().await?;
            let (headers, body) = read_http_request(&mut socket).await?;
            assert_chat_post(&headers, &body, "nuwax-wait-model", "nuwax-wait-key")?;
            request_read
                .send(())
                .map_err(|_| anyhow::anyhow!("test dropped the request-read signal"))?;
            // Answer nothing; the child's only prompt exit is the interrupt.
            let mut chunk = [0u8; 256];
            tokio::select! {
                biased;
                extra = listener.accept() => {
                    extra?;
                    anyhow::bail!("a client waiting for SSE must send exactly one POST");
                }
                read = socket.read(&mut chunk) => {
                    anyhow::ensure!(
                        read? == 0,
                        "unexpected bytes on the unanswered connection"
                    );
                }
            }
            reject_late_requests_until_child_exits(&listener, child_exited_rx).await?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    });

    let repo_root = codex_utils_cargo_bin::repo_root()?;
    let test = test_codex_exec();
    let mut command = tokio::process::Command::new(
        codex_utils_cargo_bin::cargo_bin("codex-exec").context("codex-exec binary")?,
    );
    for variable in CONFIG_ENV_VARIABLES {
        command.env_remove(variable);
    }
    command
        .current_dir(test.cwd_path())
        .env("CODEX_HOME", test.home_path())
        .env("CODEX_SQLITE_HOME", test.home_path())
        .env("CODEX_API_KEY", "dummy")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .arg("--skip-git-repo-check")
        .args(["--color", "never"])
        .arg("-C")
        .arg(&repo_root)
        .arg("reply with ok")
        .env("NUWAX_BASE_URL", format!("http://{address}/v1"))
        .env("NUWAX_WIRE_API", "chat")
        .env("NUWAX_API_KEY", "nuwax-wait-key")
        .env("NUWAX_MODEL", "nuwax-wait-model")
        .env("NUWAX_REQUEST_MAX_RETRIES", "0")
        .env("NUWAX_STREAM_MAX_RETRIES", "0")
        .env("NUWAX_STREAM_IDLE_TIMEOUT_MS", "60000");
    let child = command.spawn()?;
    tokio::time::timeout(Duration::from_secs(30), request_read_rx)
        .await
        .context("the child never sent its SSE request")??;
    // Give the child a beat to settle into the response wait, then interrupt.
    tokio::time::sleep(Duration::from_millis(750)).await;
    let pid = child.id().context("child exited before the interrupt")?;
    let interrupted_at = Instant::now();
    let signal_status = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new("kill")
            .args(["-INT", &pid.to_string()])
            .kill_on_drop(true)
            .status(),
    )
    .await
    .context("sending SIGINT exceeded the process budget")??;
    anyhow::ensure!(
        signal_status.success(),
        "SIGINT command failed: {signal_status}"
    );
    let (exited, stderr) = tokio::time::timeout(Duration::from_secs(30), async {
        let output = child.wait_with_output().await?;
        Ok::<_, anyhow::Error>((
            output.status,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))
    })
    .await
    .context("the interrupted exec must exit within the process budget")??;
    // A gateway failure may already have dropped its receiver; awaiting the
    // task below preserves that failure instead of masking it as a send error.
    let _ = child_exited.send(());
    assert_eq!(
        exited.code(),
        Some(1),
        "exec must handle turn interruption and exit normally: {stderr}"
    );
    assert!(
        interrupted_at.elapsed() < Duration::from_secs(15),
        "cancellation must be prompt, not the idle budget ({:?})",
        interrupted_at.elapsed()
    );
    assert!(
        !stderr.contains("request timed out"),
        "the exit must come from cancellation, not the idle budget: {stderr}"
    );
    // The gateway observes exactly one connection through child exit.
    gateway.await??;
    Ok(())
}

/// One stalled-stream gateway: answer one Chat request with the truncated
/// frame, then hold the connection open until the client's idle budget
/// closes it, and measure how long the stream stayed stalled.
async fn stalled_chat_gateway(
    listener: tokio::net::TcpListener,
    model: &'static str,
    bearer: &'static str,
    child_exited: tokio::sync::oneshot::Receiver<()>,
) -> Result<Duration> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let (mut socket, _) = tokio::time::timeout_at(deadline, listener.accept())
        .await
        .context("gateway never saw its child connect")??;
    let (headers, body) = read_http_request(&mut socket).await?;
    assert_chat_post(&headers, &body, model, bearer)?;
    let first_frame_at = Instant::now();
    socket
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await?;
    socket.write_all(TRUNCATED_CHAT_FRAME.as_bytes()).await?;
    socket.flush().await?;
    let mut chunk = [0u8; 256];
    tokio::select! {
        biased;
        extra = listener.accept() => {
            extra?;
            anyhow::bail!("a stalled stream must send exactly one POST (model {model})");
        }
        read = socket.read(&mut chunk) => {
            anyhow::ensure!(
                read? == 0,
                "unexpected request bytes on the stalled connection (model {model})"
            );
        }
    }
    let stalled = first_frame_at.elapsed();
    tokio::time::timeout_at(
        deadline,
        reject_late_requests_until_child_exits(&listener, child_exited),
    )
    .await
    .context("gateway did not observe child exit within its process budget")??;
    Ok(stalled)
}

/// Two children run CONCURRENTLY against identically stalled streams with
/// different `NUWAX_STREAM_IDLE_TIMEOUT_MS` values: both turns must fail on
/// their own idle budget, and the budgets must actually diverge — the short
/// budget fails well before the long one — proving the parallel processes
/// enforce their distinct idle values in behavior, not just in config.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nuwax_env_parallel_processes_enforce_distinct_idle_budgets() -> Result<()> {
    let fast_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let slow_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let fast_address = fast_listener.local_addr()?;
    let slow_address = slow_listener.local_addr()?;
    let (fast_child_exited, fast_child_exited_rx) = tokio::sync::oneshot::channel::<()>();
    let (slow_child_exited, slow_child_exited_rx) = tokio::sync::oneshot::channel::<()>();
    let fast_gateway = tokio::spawn(stalled_chat_gateway(
        fast_listener,
        "nuwax-idle-fast-model",
        "nuwax-idle-fast-key",
        fast_child_exited_rx,
    ));
    let slow_gateway = tokio::spawn(stalled_chat_gateway(
        slow_listener,
        "nuwax-idle-slow-model",
        "nuwax-idle-slow-key",
        slow_child_exited_rx,
    ));

    let run_child = |address: std::net::SocketAddr,
                     model: &'static str,
                     api_key: &'static str,
                     idle_ms: &'static str,
                     child_exited: tokio::sync::oneshot::Sender<()>| {
        tokio::task::spawn_blocking(move || {
            let test = test_codex_exec();
            let repo_root = codex_utils_cargo_bin::repo_root()?;
            let mut command = test.cmd();
            for variable in CONFIG_ENV_VARIABLES {
                command.env_remove(variable);
            }
            command
                .arg("--skip-git-repo-check")
                .arg("-C")
                .arg(&repo_root)
                .arg("reply with ok")
                .env("NUWAX_BASE_URL", format!("http://{address}/v1"))
                .env("NUWAX_WIRE_API", "chat")
                .env("NUWAX_API_KEY", api_key)
                .env("NUWAX_MODEL", model)
                .env("NUWAX_REQUEST_MAX_RETRIES", "0")
                .env("NUWAX_STREAM_MAX_RETRIES", "0")
                .env("NUWAX_STREAM_IDLE_TIMEOUT_MS", idle_ms)
                .timeout(Duration::from_secs(40));
            let started = Instant::now();
            let output = command.output();
            let elapsed = started.elapsed();
            let _ = child_exited.send(());
            anyhow::Ok((output?, elapsed))
        })
    };
    let (fast, slow) = tokio::join!(
        run_child(
            fast_address,
            "nuwax-idle-fast-model",
            "nuwax-idle-fast-key",
            "1500",
            fast_child_exited
        ),
        run_child(
            slow_address,
            "nuwax-idle-slow-model",
            "nuwax-idle-slow-key",
            "12000",
            slow_child_exited
        ),
    );
    let ((fast_output, fast_elapsed), (slow_output, slow_elapsed)) = (fast??, slow??);
    let fast_stderr = String::from_utf8_lossy(&fast_output.stderr);
    let slow_stderr = String::from_utf8_lossy(&slow_output.stderr);
    assert!(
        !fast_output.status.success(),
        "the short-budget child must fail the turn: {fast_stderr}"
    );
    assert!(
        !slow_output.status.success(),
        "the long-budget child must fail the turn: {slow_stderr}"
    );
    assert!(
        fast_stderr.contains("request timed out"),
        "the short-budget child must fail on its idle budget: {fast_stderr}"
    );
    assert!(
        slow_stderr.contains("request timed out"),
        "the long-budget child must fail on its idle budget: {slow_stderr}"
    );
    let (fast_stalled, slow_stalled) = (fast_gateway.await??, slow_gateway.await??);
    assert!(
        fast_stalled >= Duration::from_millis(1500),
        "the short budget must elapse after the first frame (took {fast_stalled:?})"
    );
    assert!(
        slow_stalled >= Duration::from_millis(12000),
        "the long budget must elapse after the first frame (took {slow_stalled:?})"
    );
    assert!(
        fast_stalled < slow_stalled,
        "parallel children must enforce distinct idle budgets: fast {fast_stalled:?} vs slow {slow_stalled:?}"
    );
    assert!(
        fast_elapsed < slow_elapsed,
        "the short-budget child must fail before the long-budget one: {fast_elapsed:?} vs {slow_elapsed:?}"
    );
    Ok(())
}
