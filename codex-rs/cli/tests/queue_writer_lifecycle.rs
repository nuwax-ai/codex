//! Queue writer lifecycle evidence per the observation contract in
//! my-docs/queue-writer-lifecycle-contract-2026-10-06.md.
//!
//! A thread's writer is the process holding the exclusive file lock on
//! `CODEX_HOME/thread-writer-locks/{thread_id}.lock`. This test observes that
//! lock directly from the test process (per-home namespace, so other tests
//! cannot interfere) across the full lifecycle of a real CLI enqueue against
//! a live owner daemon:
//!
//! 1. an unloaded thread has no writer, and public thread/resume acquires one;
//! 2. samples taken while the enqueue client is demonstrably alive stay held;
//! 3. the owner's live recorder retains its lock after the client exits;
//! 4. public thread/unsubscribe unloads the thread and releases the lock.
//!
//! The probe takes the home coordination lock before opening a thread lock,
//! so publication/cleanup cannot replace its inode during a sample. Polling
//! cannot prove ownership at every instant or identify the holder's PID.
//!
//! The CLI refusing to spawn an embedded writer beside a live daemon is a
//! separate, already-covered guard; this file adds the lock-lifecycle half.

use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::create_fake_rollout;
use app_test_support::create_final_assistant_message_sse_response;
use app_test_support::create_mock_responses_server_sequence_unchecked;
use codex_app_server::app_server_control_socket_path;
use codex_app_server_client::AppServerEvent;
use codex_app_server_client::DEFAULT_IN_PROCESS_CHANNEL_CAPACITY;
use codex_app_server_client::RemoteAppServerClient;
use codex_app_server_client::RemoteAppServerConnectArgs;
use codex_app_server_client::RemoteAppServerEndpoint;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadQueueListParams;
use codex_app_server_protocol::ThreadQueueListResponse;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadUnsubscribeParams;
use codex_app_server_protocol::ThreadUnsubscribeResponse;
use codex_app_server_protocol::ThreadUnsubscribeStatus;
use codex_app_server_protocol::TurnStatus;
use codex_protocol::config_types::NUWAX_ENV_PROVIDER_ID;
use codex_protocol::shell_environment::OPENAI_FEDERATION_RULE_ID_ENV_VAR;
use codex_protocol::shell_environment::OPENAI_IDENTITY_TOKEN_FILE_ENV_VAR;
use pretty_assertions::assert_eq;

fn socket_test_home() -> Result<tempfile::TempDir> {
    // macOS temporary paths can exceed the Unix socket path limit.
    #[cfg(target_os = "macos")]
    let home = tempfile::tempdir_in("/tmp")?;
    #[cfg(not(target_os = "macos"))]
    let home = tempfile::TempDir::new()?;
    Ok(home)
}

fn cleaned_environment() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("CODEX_SQLITE_HOME", None),
        ("CODEX_MODEL_REASONING_EFFORT", None),
        ("CODEX_MODEL_CONTEXT_WINDOW", None),
        ("CODEX_AUTO_COMPACT_TOKEN_LIMIT", None),
        ("CODEX_AUTO_COMPACT_RATIO", None),
        ("CODEX_MANAGED_BY_VITE_PLUS", None),
        ("CODEX_MANAGED_BY_PNPM", None),
        ("CODEX_MANAGED_BY_NPM", None),
        ("CODEX_MANAGED_BY_BUN", None),
        ("CODEX_INSTALL_SOURCE", None),
        ("NUWAX_MODEL", None),
        ("NUWAX_BASE_URL", None),
        ("NUWAX_WIRE_API", None),
        ("NUWAX_API_KEY", None),
        ("NUWAX_MAX_OUTPUT_TOKENS", None),
        ("NUWAX_REQUEST_MAX_RETRIES", None),
        ("NUWAX_STREAM_MAX_RETRIES", None),
        ("NUWAX_STREAM_IDLE_TIMEOUT_MS", None),
        ("OPENAI_API_KEY", None),
        ("CODEX_API_KEY", None),
        ("CODEX_ACCESS_TOKEN", None),
        (OPENAI_FEDERATION_RULE_ID_ENV_VAR, None),
        (OPENAI_IDENTITY_TOKEN_FILE_ENV_VAR, None),
    ]
}

async fn wait_for_socket(
    home: &std::path::Path,
) -> Result<codex_utils_absolute_path::AbsolutePathBuf> {
    let socket_path = app_server_control_socket_path(home)?;
    tokio::time::timeout(Duration::from_secs(/*secs*/ 30), async {
        while !socket_path.as_path().try_exists()? {
            tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(socket_path)
}

async fn connect_owner(
    socket_path: &codex_utils_absolute_path::AbsolutePathBuf,
) -> Result<RemoteAppServerClient> {
    Ok(RemoteAppServerClient::connect(RemoteAppServerConnectArgs {
        endpoint: RemoteAppServerEndpoint::UnixSocket {
            socket_path: socket_path.clone(),
        },
        client_name: "queue-writer-lifecycle-test".to_string(),
        client_version: "0.1.0".to_string(),
        experimental_api: true,
        mcp_server_openai_form_elicitation: false,
        opt_out_notification_methods: Vec::new(),
        channel_capacity: DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
    })
    .await?)
}

#[derive(Debug, PartialEq, Eq)]
enum WriterLockState {
    Free,
    Held,
}

#[derive(Debug)]
struct WriterLockSample {
    started_at: Instant,
    finished_at: Instant,
    state: WriterLockState,
}

/// Coordinates with production lock-file replacement and releases any probe
/// lock before allowing a real writer to acquire the coordination lock.
fn sample_writer_lock(home: &std::path::Path, thread_id: &str) -> Result<WriterLockSample> {
    use std::fs::OpenOptions;
    let started_at = Instant::now();
    let directory = home.join("thread-writer-locks");
    std::fs::create_dir_all(&directory)?;
    let coordination = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(".coordination.lock"))?;
    coordination.lock().context("lock writer coordination")?;
    let path = directory.join(format!("{thread_id}.lock"));
    let state = match OpenOptions::new().read(true).write(true).open(&path) {
        Ok(file) => {
            let state = match file.try_lock() {
                Ok(()) => Ok(WriterLockState::Free),
                Err(std::fs::TryLockError::WouldBlock) => Ok(WriterLockState::Held),
                Err(std::fs::TryLockError::Error(error)) => {
                    Err(anyhow::anyhow!("try_lock {}: {error}", path.display()))
                }
            };
            drop(file);
            state?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => WriterLockState::Free,
        Err(error) => return Err(error).with_context(|| format!("open {}", path.display())),
    };
    drop(coordination);
    Ok(WriterLockSample {
        started_at,
        finished_at: Instant::now(),
        state,
    })
}

struct WriterLockProbe {
    ready: tokio::sync::oneshot::Receiver<()>,
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<Result<Vec<WriterLockSample>>>,
}

impl WriterLockProbe {
    fn start(mut sample: impl FnMut() -> Result<WriterLockSample> + Send + 'static) -> Self {
        let (ready_tx, ready) = tokio::sync::oneshot::channel();
        let (stop, mut stop_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let mut ready_tx = Some(ready_tx);
            let mut samples = Vec::new();
            loop {
                samples.push(sample()?);
                if let Some(ready_tx) = ready_tx.take() {
                    let _ = ready_tx.send(());
                }
                tokio::select! {
                    _ = &mut stop_rx => return Ok(samples),
                    _ = tokio::time::sleep(Duration::from_millis(/*millis*/ 50)) => {}
                }
            }
        });
        Self { ready, stop, task }
    }

    async fn finish(self) -> Result<Vec<WriterLockSample>> {
        let _ = self.stop.send(());
        self.task.await.context("writer lock probe task failed")?
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enqueue_keeps_loaded_owner_writer_until_thread_unloads() -> Result<()> {
    let home = socket_test_home()?;
    let model = create_mock_responses_server_sequence_unchecked(vec![
        create_final_assistant_message_sse_response("writer lifecycle complete")?,
    ])
    .await;
    std::fs::write(
        home.path().join("config.toml"),
        "features.plugins = false\nanalytics.enabled = false\nthread_unload_delay_secs = 1\n",
    )?;
    let codex = codex_utils_cargo_bin::cargo_bin("codex")?;
    let thread_id = create_fake_rollout(
        home.path(),
        "2025-05-01T05-00-00",
        "2025-05-01T05:00:00Z",
        "writer lifecycle session",
        Some(NUWAX_ENV_PROVIDER_ID),
        /*git_info*/ None,
    )?;

    let base_url = format!("{}/v1", model.uri());
    let _server = TestAppServer::builder()
        .with_program(&codex)
        .with_codex_home(home.path())
        .with_plugin_startup_tasks()
        .without_managed_config()
        .with_args(&["app-server", "--listen", "unix://"])
        .with_env_overrides(&cleaned_environment())
        .with_env_overrides(&[
            ("NUWAX_BASE_URL", Some(base_url.as_str())),
            ("NUWAX_WIRE_API", Some("responses")),
            ("NUWAX_API_KEY", Some("writer-lifecycle-key")),
            ("NUWAX_MODEL", Some("writer-lifecycle-model")),
            (
                "CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED",
                Some("1"),
            ),
        ])
        .build()
        .await?;
    let socket = wait_for_socket(home.path()).await?;
    let mut owner = connect_owner(&socket).await?;

    assert_eq!(
        sample_writer_lock(home.path(), &thread_id)?.state,
        WriterLockState::Free
    );
    let _: ThreadResumeResponse = tokio::time::timeout(
        Duration::from_secs(/*secs*/ 30),
        owner.request_typed(ClientRequest::ThreadResume {
            request_id: RequestId::Integer(1),
            params: ThreadResumeParams {
                thread_id: thread_id.clone(),
                ..Default::default()
            },
        }),
    )
    .await
    .context("owner resume did not finish")??;
    assert_eq!(
        sample_writer_lock(home.path(), &thread_id)?.state,
        WriterLockState::Held
    );

    let probe_home = home.path().to_path_buf();
    let probe_thread = thread_id.clone();
    let mut probe = WriterLockProbe::start(move || sample_writer_lock(&probe_home, &probe_thread));
    // Always stop and join the probe, including on CLI spawn/wait failures.
    let enqueue = async {
        (&mut probe.ready)
            .await
            .context("probe failed before its first sample")?;
        let mut command = tokio::process::Command::new(&codex);
        for (variable, _) in cleaned_environment() {
            command.env_remove(variable);
        }
        let mut child = command
            .env("CODEX_HOME", home.path())
            .current_dir(home.path())
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .args([
                "queue",
                "--thread",
                thread_id.as_str(),
                "--message",
                "writer lifecycle probe",
            ])
            .spawn()?;
        let client_started_at = Instant::now();
        let mut last_observed_alive = client_started_at;
        let mut immediate_samples = Vec::new();
        let output = tokio::time::timeout(Duration::from_secs(/*secs*/ 60), async {
            // This immediate sample also covers clients that finish before the
            // background probe's first 50ms interval. The subsequent try_wait
            // proves the child was still alive after the whole sample.
            let sample = sample_writer_lock(home.path(), &thread_id)?;
            if child.try_wait()?.is_none() {
                last_observed_alive = Instant::now();
                immediate_samples.push(sample);
            }
            while child.try_wait()?.is_none() {
                last_observed_alive = Instant::now();
                tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;
            }
            Ok::<_, anyhow::Error>(child.wait_with_output().await?)
        })
        .await
        .context("enqueue did not finish")??;
        Ok::<_, anyhow::Error>((
            output,
            client_started_at,
            last_observed_alive,
            immediate_samples,
        ))
    }
    .await;
    let mut samples = probe.finish().await?;
    let (output, client_started_at, last_observed_alive, immediate_samples) = enqueue?;
    samples.extend(immediate_samples);
    assert!(
        output.status.success(),
        "enqueue against the live owner must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let client_samples: Vec<_> = samples
        .iter()
        .filter(|sample| {
            sample.started_at >= client_started_at && sample.finished_at <= last_observed_alive
        })
        .collect();
    assert!(
        !client_samples.is_empty(),
        "no complete sample was taken while the enqueue client was alive: {samples:?}"
    );
    assert!(
        client_samples
            .iter()
            .all(|sample| sample.state == WriterLockState::Held),
        "the loaded owner's writer lock must stay held during enqueue: {client_samples:?}"
    );
    assert_eq!(
        sample_writer_lock(home.path(), &thread_id)?.state,
        WriterLockState::Held
    );

    let completed = tokio::time::timeout(Duration::from_secs(/*secs*/ 30), async {
        while let Some(event) = owner.next_event().await {
            if let AppServerEvent::ServerNotification(notification) = event
                && let ServerNotification::TurnCompleted(completed) = *notification
                && completed.thread_id == thread_id
            {
                return Ok::<_, anyhow::Error>(completed);
            }
        }
        anyhow::bail!("owner disconnected before the queued turn completed")
    })
    .await??;
    assert_eq!(completed.turn.status, TurnStatus::Completed);
    let queue: ThreadQueueListResponse = owner
        .request_typed(ClientRequest::ThreadQueueList {
            request_id: RequestId::Integer(3),
            params: ThreadQueueListParams {
                thread_id: thread_id.clone(),
                cursor: None,
                limit: None,
            },
        })
        .await?;
    assert_eq!(queue.data, Vec::new());
    assert_eq!(
        sample_writer_lock(home.path(), &thread_id)?.state,
        WriterLockState::Held
    );
    let unsubscribe: ThreadUnsubscribeResponse = owner
        .request_typed(ClientRequest::ThreadUnsubscribe {
            request_id: RequestId::Integer(4),
            params: ThreadUnsubscribeParams {
                thread_id: thread_id.clone(),
            },
        })
        .await?;
    assert_eq!(unsubscribe.status, ThreadUnsubscribeStatus::Unsubscribed);
    tokio::time::timeout(Duration::from_secs(/*secs*/ 30), async {
        while let Some(event) = owner.next_event().await {
            if let AppServerEvent::ServerNotification(notification) = event
                && let ServerNotification::ThreadClosed(closed) = *notification
                && closed.thread_id == thread_id
            {
                return Ok::<_, anyhow::Error>(());
            }
        }
        anyhow::bail!("owner disconnected before thread unload")
    })
    .await??;
    assert_eq!(
        sample_writer_lock(home.path(), &thread_id)?.state,
        WriterLockState::Free
    );
    owner.shutdown().await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn probe_finish_reports_a_failure_after_its_ready_sample() -> Result<()> {
    let mut samples = 0;
    let mut probe = WriterLockProbe::start(move || {
        samples += 1;
        anyhow::ensure!(samples == 1, "injected probe failure after readiness");
        let at = Instant::now();
        Ok(WriterLockSample {
            started_at: at,
            finished_at: at,
            state: WriterLockState::Held,
        })
    });
    (&mut probe.ready).await?;
    tokio::time::timeout(Duration::from_secs(/*secs*/ 10), async {
        while !probe.task.is_finished() {
            tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;
        }
    })
    .await?;
    let error = probe
        .finish()
        .await
        .expect_err("the probe failure must reach its caller");
    assert_eq!(error.to_string(), "injected probe failure after readiness");
    Ok(())
}
