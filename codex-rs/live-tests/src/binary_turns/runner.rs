//! The subprocess boundary and byte-preserving artifact persistence.

use super::*;
use anyhow::Context;
use std::future::Future;

pub(super) struct ExecTurn<'a> {
    pub(super) binary: &'a Path,
    pub(super) home: &'a Path,
    pub(super) cwd: &'a Path,
    pub(super) args: &'a [&'a str],
    pub(super) prompt: &'a str,
    pub(super) last_message: &'a Path,
    pub(super) request_capture: &'a Path,
}

/// Executes a turn without checking scene assertions. Implementations must
/// return all completed pipe reads even when waiting or draining fails.
pub(super) trait ExecRunner: Sync {
    fn run(&self, turn: &ExecTurn<'_>) -> impl Future<Output = Result<ExecCapture>> + Send;
}

pub(super) struct ProcessRunner;

pub(super) const MAX_PIPE_BYTES: usize = 16 * 1024 * 1024;

pub(super) struct ExecCapture {
    pub(super) stdout: PipeCapture,
    pub(super) stderr: PipeCapture,
    pub(super) completion: Completion,
}

pub(super) enum Completion {
    Exited { success: bool, description: String },
    TimedOut,
    WaitFailed(String),
    CaptureFailed(String),
}

pub(super) struct PipeCapture {
    pub(super) bytes: Vec<u8>,
    pub(super) error: Option<String>,
}

impl ExecRunner for ProcessRunner {
    async fn run(&self, turn: &ExecTurn<'_>) -> Result<ExecCapture> {
        let mut child = tokio::process::Command::new(turn.binary)
            .arg("--skip-git-repo-check")
            .arg("--json")
            .args(["--color", "never", "--output-last-message"])
            .arg(turn.last_message)
            .args(turn.args)
            .arg(turn.prompt)
            .env("CODEX_HOME", turn.home)
            .env("CODEX_SQLITE_HOME", turn.home)
            .env("RUST_LOG", "info")
            .env("CODEX_RIG_REQUEST_CAPTURE_FILE", turn.request_capture)
            .current_dir(turn.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("launch codex-exec {}", turn.binary.display()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("stdout pipe missing"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("stderr pipe missing"))?;
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let (failures, mut failure_rx) = tokio::sync::mpsc::channel(/*buffer*/ 2);
        let stdout_task = tokio::spawn(capture_with_failure_signal(
            stdout,
            stopped.clone(),
            failures.clone(),
            "stdout",
        ));
        let stderr_task = tokio::spawn(capture_with_failure_signal(
            stderr, stopped, failures, "stderr",
        ));
        let mut completion = tokio::select! {
            failure = async {
                match failure_rx.recv().await {
                    Some(failure) => failure,
                    None => std::future::pending().await,
                }
            } => {
                let _ = child.kill().await;
                Completion::CaptureFailed(failure)
            },
            result = tokio::time::timeout(EXEC_RUN_TIMEOUT, child.wait()) => match result {
                Ok(Ok(status)) => Completion::Exited {
                    success: status.success(), description: status.to_string(),
                },
                Ok(Err(error)) => Completion::WaitFailed(error.to_string()),
                Err(_elapsed) => {
                    let _ = child.kill().await;
                    Completion::TimedOut
                }
            },
        };
        let _ = stop.send(true);
        let stdout = stdout_task.await.context("stdout reader task")?;
        let stderr = stderr_task.await.context("stderr reader task")?;
        if let Completion::Exited {
            success: true,
            description,
        } = &completion
        {
            let failures: Vec<_> = [("stdout", &stdout), ("stderr", &stderr)]
                .into_iter()
                .filter_map(|(stream, capture)| {
                    capture
                        .error
                        .as_ref()
                        .map(|error| format!("{stream}: {error}"))
                })
                .collect();
            if !failures.is_empty() {
                completion = Completion::CaptureFailed(format!(
                    "{}; child {description}",
                    failures.join("; ")
                ));
            }
        }
        Ok(ExecCapture {
            stdout,
            stderr,
            completion,
        })
    }
}

/// Checks the prepared executable immediately around the injected execution
/// boundary. Persistence failures cannot replace a launch/exit/capture failure.
pub(super) async fn execute(
    runner: &impl ExecRunner,
    scene: &scenarios::Scene<'_>,
    args: &[&str],
    prompt: &str,
    last_message: &Path,
    label: &str,
) -> Result<(String, String)> {
    scene.prepared.verify_unchanged()?;
    let request_capture = scene.artifacts.join("requests.jsonl");
    let captured = runner
        .run(&ExecTurn {
            binary: &scene.prepared.path,
            home: scene.home,
            cwd: scene.cwd,
            args,
            prompt,
            last_message,
            request_capture: &request_capture,
        })
        .await;
    let prefix = if label.is_empty() {
        String::new()
    } else {
        format!("{label}.")
    };
    let capture = match captured {
        Ok(capture) => capture,
        Err(error) => {
            if let Err(persist_error) = std::fs::write(
                scene.artifacts.join(format!("{prefix}exit.txt")),
                format!("{error:#}\n"),
            ) {
                eprintln!("warn: persist launch failure: {persist_error}");
            }
            if let Err(identity_error) = scene.prepared.verify_unchanged() {
                eprintln!(
                    "warn: executable identity check after runner failure: {identity_error:#}"
                );
            }
            return Err(error);
        }
    };
    let mut exit = match &capture.completion {
        Completion::Exited { description, .. } => format!("{description}\n"),
        Completion::TimedOut => format!("timeout after {EXEC_RUN_TIMEOUT:?}\n"),
        Completion::WaitFailed(error) => format!("wait failed: {error}\n"),
        Completion::CaptureFailed(error) => format!("output capture failed: {error}\n"),
    };
    for (stream, pipe) in [("stdout", &capture.stdout), ("stderr", &capture.stderr)] {
        if let Some(error) = &pipe.error {
            exit.push_str(&format!("{stream} capture incomplete: {error}\n"));
        }
    }
    let process_outcome = match &capture.completion {
        Completion::Exited {
            success: false,
            description,
        } => Err(anyhow!(
            "[{}] {label} exited with {description}",
            scene.protocol
        )),
        Completion::TimedOut => Err(anyhow!(
            "[{}] {label} did not finish within {EXEC_RUN_TIMEOUT:?}",
            scene.protocol
        )),
        Completion::WaitFailed(error) => {
            Err(anyhow!("[{}] {label} wait failed: {error}", scene.protocol))
        }
        Completion::CaptureFailed(error) => Err(anyhow!(
            "[{}] {label} output capture failed: {error}",
            scene.protocol
        )),
        Completion::Exited { success: true, .. } => {
            if capture.stdout.error.is_some() || capture.stderr.error.is_some() {
                Err(anyhow!(
                    "[{}] {label} output capture incomplete: {exit}",
                    scene.protocol
                ))
            } else {
                Ok(())
            }
        }
    };
    // Save every completed pipe read before hashing the executable again.
    // An outer watchdog can terminate the test while this full-file check runs.
    let mut persistence_errors = Vec::new();
    for (name, bytes) in [
        ("events.jsonl", capture.stdout.bytes.as_slice()),
        ("stderr.log", capture.stderr.bytes.as_slice()),
        ("exit.txt", exit.as_bytes()),
    ] {
        if let Err(error) = std::fs::write(scene.artifacts.join(format!("{prefix}{name}")), bytes) {
            persistence_errors.push((name, error));
        }
    }
    let unchanged = scene.prepared.verify_unchanged();
    if let Err(error) = &unchanged {
        exit.push_str(&format!("executable identity check failed: {error:#}\n"));
        if let Err(error) = std::fs::write(scene.artifacts.join(format!("{prefix}exit.txt")), exit)
        {
            persistence_errors.push(("exit.txt identity diagnostic", error));
        }
    }
    let outcome = process_outcome.and(unchanged);
    if outcome.is_err() {
        for (name, error) in &persistence_errors {
            eprintln!("warn: persist {name}: {error}");
        }
    }
    outcome?;
    if let Some((_, error)) = persistence_errors.into_iter().next() {
        return Err(error.into());
    }
    let stdout = String::from_utf8_lossy(&capture.stdout.bytes).into_owned();
    let stderr = String::from_utf8_lossy(&capture.stderr.bytes).into_owned();
    println!("[{}] {label} codex-exec events:\n{stdout}", scene.protocol);
    if !stderr.trim().is_empty() {
        println!("[{}] {label} stderr:\n{stderr}", scene.protocol);
    }
    Ok((stdout, stderr))
}

/// Notifies the process owner immediately when a reader cannot keep complete
/// evidence, while returning its retained bytes for artifact persistence.
async fn capture_with_failure_signal(
    pipe: impl tokio::io::AsyncRead + Unpin + Send,
    stopped: tokio::sync::watch::Receiver<bool>,
    failures: tokio::sync::mpsc::Sender<String>,
    stream: &'static str,
) -> PipeCapture {
    let capture = capture_pipe(pipe, stopped, Duration::from_secs(/*secs*/ 5)).await;
    if let Some(error) = &capture.error {
        let _ = failures.try_send(format!("{stream}: {error}"));
    }
    capture
}

/// A descendant can hold a pipe after the child exits. Bound that drain while
/// preserving every completed read, including a trailing invalid UTF-8 byte.
pub(super) async fn capture_pipe(
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
                Ok(read) => {
                    let remaining = MAX_PIPE_BYTES.saturating_sub(capture.bytes.len());
                    capture.bytes.extend_from_slice(&chunk[..read.min(remaining)]);
                    if read > remaining {
                        capture.error = Some("pipe exceeded 16 MiB capture limit".to_string());
                        return capture;
                    }
                },
                Err(error) => { capture.error = Some(error.to_string()); return capture; }
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
