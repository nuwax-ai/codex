//! B2 acceptance: queue DISPATCH semantics on the real CLI and a real
//! default-socket owner.
//!
//! 1. A message enqueued while the owner has a turn IN FLIGHT (the model
//!    request is on the wire and its response is held by a real gate)
//!    dispatches only after that turn completes, executed by the SAME owner
//!    writer — no second writer, no extra requests, client credentials
//!    irrelevant.
//! 2. A cold enqueue with NO owner running is still a durable submission:
//!    a later env-holding owner loads the thread and drains it, executing
//!    through its own environment.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::create_fake_rollout;
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
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::UserInput;
use codex_core::find_archived_thread_path_by_id_str;
use codex_core::find_thread_path_by_id_str;
use codex_protocol::config_types::NUWAX_ENV_PROVIDER_ID;
use codex_protocol::shell_environment::OPENAI_FEDERATION_RULE_ID_ENV_VAR;
use codex_protocol::shell_environment::OPENAI_IDENTITY_TOKEN_FILE_ENV_VAR;
use pretty_assertions::assert_eq;
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::time::timeout;

fn socket_test_home() -> Result<tempfile::TempDir> {
    // macOS temporary paths can exceed the Unix socket path limit.
    #[cfg(target_os = "macos")]
    let home = tempfile::tempdir_in("/tmp")?;
    #[cfg(not(target_os = "macos"))]
    let home = tempfile::TempDir::new()?;
    Ok(home)
}

/// Environment scrub applied to every process in these tests so inherited
/// developer shells cannot leak NUWAX groups or OpenAI auth into either side.
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
    timeout(Duration::from_secs(/*secs*/ 30), async {
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
        client_name: "queue-owner-dispatch-test".to_string(),
        client_version: "0.1.0".to_string(),
        experimental_api: true,
        mcp_server_openai_form_elicitation: false,
        opt_out_notification_methods: Vec::new(),
        channel_capacity: DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
    })
    .await?)
}

async fn drain_queue(app: &mut RemoteAppServerClient, thread_id: &str) -> Result<usize> {
    let queue: ThreadQueueListResponse = app
        .request_typed(ClientRequest::ThreadQueueList {
            request_id: RequestId::Integer(99),
            params: ThreadQueueListParams {
                thread_id: thread_id.to_string(),
                cursor: None,
                limit: None,
            },
        })
        .await?;
    Ok(queue.data.len())
}

async fn enqueue_from_envless_client(
    codex: &std::path::Path,
    home: &std::path::Path,
    target: &str,
    message: &str,
) -> Result<std::process::Output> {
    let mut command = tokio::process::Command::new(codex);
    for (variable, _) in cleaned_environment() {
        command.env_remove(variable);
    }
    let output = timeout(
        Duration::from_secs(/*secs*/ 30),
        command
            .env("CODEX_HOME", home)
            .current_dir(home)
            .kill_on_drop(true)
            .args(["queue", "--thread", target, "--message", message])
            .output(),
    )
    .await??;
    Ok(output)
}

/// One captured owner-model request: path, auth header, body, turn trigger.
#[derive(Clone)]
struct CapturedRequest {
    path: String,
    authorization: String,
    body: Value,
    turn_trigger: Option<String>,
}

/// Clones the captured requests; a poisoned lock is a test failure, not
/// empty evidence.
fn snapshot_requests(captured: &Arc<Mutex<Vec<CapturedRequest>>>) -> Result<Vec<CapturedRequest>> {
    Ok(captured
        .lock()
        .map_err(|_| anyhow::anyhow!("capture lock poisoned"))?
        .clone())
}

/// A raw Responses-gateway whose FIRST request is held behind a real gate:
/// the request is fully received (the owner's turn is provably in flight on
/// the wire) and only answered when the test releases the gate. The second
/// request and all later requests are recorded and answered immediately;
/// each test checks the captured request count after its expected completions.
async fn gated_owner_model_server() -> Result<(
    std::net::SocketAddr,
    Arc<Mutex<Vec<CapturedRequest>>>,
    tokio::sync::watch::Sender<bool>,
)> {
    const FIRST_RESPONSE: &str = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\",\"model\":\"nuwax-owner-model\"}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"first turn done\"}],\"id\":\"o1\"}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"usage\":{\"input_tokens\":4,\"output_tokens\":3,\"total_tokens\":7}}}\n\n",
    );
    const SECOND_RESPONSE: &str = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r2\",\"model\":\"nuwax-owner-model\"}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"queued turn done\"}],\"id\":\"o2\"}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r2\",\"usage\":{\"input_tokens\":4,\"output_tokens\":3,\"total_tokens\":7}}}\n\n",
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let captured: Arc<Mutex<Vec<CapturedRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let (release, mut task_release) = tokio::sync::watch::channel(false);
    let requests = captured.clone();
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(/*secs*/ 120);
        while tokio::time::Instant::now() < deadline {
            let (mut socket, _) = match tokio::time::timeout_at(deadline, listener.accept()).await {
                Ok(Ok(accepted)) => accepted,
                _ => break,
            };
            let Ok((headers, body)) = read_http_request(&mut socket).await else {
                continue;
            };
            let request_line = headers.lines().next().unwrap_or_default().to_string();
            let authorization = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("authorization")
                        .then(|| value.trim().to_string())
                })
                .unwrap_or_default();
            let turn_trigger = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("x-codex-turn-metadata")
                        .then(|| value.trim().to_string())
                })
                .and_then(|metadata| serde_json::from_str::<Value>(&metadata).ok())
                .and_then(|metadata| metadata["turn_trigger"].as_str().map(str::to_string));
            let index = {
                let mut guard = match requests.lock() {
                    Ok(guard) => guard,
                    Err(_) => break,
                };
                let index = guard.len();
                guard.push(CapturedRequest {
                    path: request_line
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_string(),
                    authorization,
                    body: serde_json::from_slice(&body).unwrap_or(Value::Null),
                    turn_trigger,
                });
                index
            };
            // Only the first request is gated; later ones answer immediately.
            if index == 0 {
                let _ = task_release.changed().await;
            }
            let body_bytes = if index == 0 {
                FIRST_RESPONSE
            } else {
                SECOND_RESPONSE
            };
            let _ = socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                )
                .await;
            let _ = socket.write_all(body_bytes.as_bytes()).await;
            let _ = socket.flush().await;
            let _ = socket.shutdown().await;
        }
    });
    Ok((address, captured, release))
}

/// Reads one full HTTP request (header block plus Content-Length body).
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

/// Enqueue while the owner's turn is provably RUNNING (its model request is
/// held by the gateway's gate): the submission is accepted, dispatches only
/// after the in-flight turn completes, and the SAME owner writer executes
/// both turns — exactly two requests, both on the owner's credential, the
/// second tagged `turn_trigger=queue`.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_enqueued_while_the_owner_turn_runs_dispatches_after_without_a_second_writer()
-> Result<()> {
    let codex_home = socket_test_home()?;
    let codex = codex_utils_cargo_bin::cargo_bin("codex")?;
    let (address, captured, release) = gated_owner_model_server().await?;
    std::fs::write(
        codex_home.path().join("config.toml"),
        "features.plugins = false\nanalytics.enabled = false\n",
    )?;
    let config_before = std::fs::read(codex_home.path().join("config.toml"))?;
    let thread_id = create_fake_rollout(
        codex_home.path(),
        "2025-03-06T04-00-00",
        "2025-03-06T04:00:00Z",
        "running owner queue session",
        Some(NUWAX_ENV_PROVIDER_ID),
        /*git_info*/ None,
    )?;

    let base_url = format!("http://{address}/v1");
    let _server = TestAppServer::builder()
        .with_program(&codex)
        .with_codex_home(codex_home.path())
        .with_plugin_startup_tasks()
        .without_managed_config()
        .with_args(&["app-server", "--listen", "unix://"])
        .with_env_overrides(&cleaned_environment())
        .with_env_overrides(&[
            ("NUWAX_BASE_URL", Some(base_url.as_str())),
            ("NUWAX_WIRE_API", Some("responses")),
            ("NUWAX_API_KEY", Some("owner-env-key")),
            ("NUWAX_MODEL", Some("nuwax-owner-model")),
            (
                "CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED",
                Some("1"),
            ),
        ])
        .build()
        .await?;
    let socket_path = wait_for_socket(codex_home.path()).await?;
    let mut app = connect_owner(&socket_path).await?;
    let _: ThreadResumeResponse = timeout(
        Duration::from_secs(/*secs*/ 30),
        app.request_typed(ClientRequest::ThreadResume {
            request_id: RequestId::Integer(1),
            params: ThreadResumeParams {
                thread_id: thread_id.clone(),
                ..Default::default()
            },
        }),
    )
    .await??;

    // Start a turn whose model response the gateway holds: the turn is
    // in flight exactly while the request sits unanswered on the wire.
    let _: Value = timeout(
        Duration::from_secs(/*secs*/ 30),
        app.request_typed(ClientRequest::TurnStart {
            request_id: RequestId::Integer(2),
            params: TurnStartParams {
                thread_id: thread_id.clone(),
                input: vec![UserInput::Text {
                    text: "run slowly".to_string(),
                    text_elements: Vec::new(),
                }],
                ..Default::default()
            },
        }),
    )
    .await??;
    timeout(Duration::from_secs(/*secs*/ 30), async {
        while snapshot_requests(&captured)?.is_empty() {
            tokio::time::sleep(Duration::from_millis(/*millis*/ 20)).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    assert_eq!(
        drain_queue(&mut app, &thread_id).await?,
        0,
        "nothing is queued yet"
    );

    // Enqueue while the owner's turn is in flight on the wire.
    let output = enqueue_from_envless_client(
        &codex,
        codex_home.path(),
        thread_id.as_str(),
        "dispatch after the running turn",
    )
    .await?;
    assert!(
        output.status.success(),
        "enqueue must be accepted while the owner turn runs: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(drain_queue(&mut app, &thread_id).await?, 1);

    // Releasing the gate completes the first turn; the queue then drains
    // through the SAME writer. Two completions, two requests, nothing else.
    release.send(true).ok();
    let mut completions = 0;
    timeout(Duration::from_secs(/*secs*/ 60), async {
        while let Some(event) = app.next_event().await {
            if let AppServerEvent::ServerNotification(notification) = event
                && let ServerNotification::TurnCompleted(completed) = *notification
            {
                assert_eq!(completed.turn.status, TurnStatus::Completed);
                completions += 1;
                if completions == 2 {
                    return Ok::<_, anyhow::Error>(());
                }
            }
        }
        anyhow::bail!("owner disconnected before both turns completed")
    })
    .await??;

    let requests = snapshot_requests(&captured)?;
    assert_eq!(
        requests.len(),
        2,
        "exactly two requests: no second writer, no resample"
    );
    for request in &requests {
        assert_eq!(request.path, "/v1/responses");
        assert_eq!(request.body["model"], Value::from("nuwax-owner-model"));
        assert_eq!(request.authorization, "Bearer owner-env-key");
    }
    let triggers: Vec<_> = requests.iter().map(|r| r.turn_trigger.clone()).collect();
    assert_ne!(
        triggers[0],
        Some("queue".to_string()),
        "the first turn is the interactive one"
    );
    assert_eq!(
        triggers[1],
        Some("queue".to_string()),
        "the drained submission must run as the queue turn"
    );
    assert_eq!(drain_queue(&mut app, &thread_id).await?, 0);
    assert_eq!(
        std::fs::read(codex_home.path().join("config.toml"))?,
        config_before
    );
    app.shutdown().await?;
    Ok(())
}

/// In the npm_nuwax single-binary installation environment, with no daemon
/// running, a cold enqueue and an administrative archive must both succeed
/// through the embedded server, execute nothing, and leave no control socket.
/// Queue still prefers an existing owner independently of installation;
/// this cell checks cold behavior and the persisted queue/archive results.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn npm_install_method_keeps_cold_queue_and_archive_on_the_embedded_server() -> Result<()> {
    let codex_home = socket_test_home()?;
    let codex = codex_utils_cargo_bin::cargo_bin("codex")?;
    let (address, captured, _release) = gated_owner_model_server().await?;
    MockResponsesConfig::new(&format!("http://{address}"))
        .with_root_config("features.plugins = false\nanalytics.enabled = false")
        .write(codex_home.path())?;
    let config_before = std::fs::read(codex_home.path().join("config.toml"))?;
    let queue_thread = create_fake_rollout(
        codex_home.path(),
        "2025-03-08T02-00-00",
        "2025-03-08T02:00:00Z",
        "npm cold enqueue session",
        Some("mock_provider"),
        /*git_info*/ None,
    )?;
    let archive_thread = create_fake_rollout(
        codex_home.path(),
        "2025-03-08T02-30-00",
        "2025-03-08T02:30:00Z",
        "npm archive session",
        Some("mock_provider"),
        /*git_info*/ None,
    )?;

    async fn run_cli(
        codex: &std::path::Path,
        home: &std::path::Path,
        args: &[&str],
    ) -> Result<std::process::Output> {
        let mut command = tokio::process::Command::new(codex);
        for (variable, _) in cleaned_environment() {
            command.env_remove(variable);
        }
        Ok(timeout(
            Duration::from_secs(/*secs*/ 60),
            command
                .env("CODEX_HOME", home)
                .env("CODEX_INSTALL_SOURCE", "npm_nuwax")
                .current_dir(home)
                .kill_on_drop(true)
                .args(args)
                .output(),
        )
        .await??)
    }

    let enqueue = run_cli(
        &codex,
        codex_home.path(),
        &[
            "queue",
            "--thread",
            queue_thread.as_str(),
            "--message",
            "npm embedded submission",
        ],
    )
    .await?;
    assert!(
        enqueue.status.success(),
        "the npm_nuwax install method must accept a cold enqueue embedded: {}",
        String::from_utf8_lossy(&enqueue.stderr)
    );
    let archive = run_cli(
        &codex,
        codex_home.path(),
        &["archive", archive_thread.as_str()],
    )
    .await?;
    assert!(
        archive.status.success(),
        "the npm_nuwax install method must archive through the embedded server: {}",
        String::from_utf8_lossy(&archive.stderr)
    );
    assert!(
        find_thread_path_by_id_str(
            codex_home.path(),
            &archive_thread,
            /*state_db_ctx*/ None
        )
        .await?
        .is_none(),
        "archive must remove the active rollout"
    );
    assert!(
        find_archived_thread_path_by_id_str(
            codex_home.path(),
            &archive_thread,
            /*state_db_ctx*/ None
        )
        .await?
        .is_some(),
        "archive must persist the archived rollout"
    );
    // Both CLI processes have exited. A new server must read the submission
    // from durable storage, without loading the thread or executing it.
    let mut verifier = TestAppServer::builder()
        .with_program(&codex)
        .with_codex_home(codex_home.path())
        .with_plugin_startup_tasks()
        .without_managed_config()
        .with_args(&["app-server"])
        .with_env_overrides(&cleaned_environment())
        .build_initialized()
        .await?;
    let queue: ThreadQueueListResponse = verifier
        .request(|request_id| ClientRequest::ThreadQueueList {
            request_id,
            params: ThreadQueueListParams {
                thread_id: queue_thread,
                cursor: None,
                limit: None,
            },
        })
        .await?;
    assert_eq!(
        queue
            .data
            .into_iter()
            .map(|item| item.input)
            .collect::<Vec<_>>(),
        vec![vec![UserInput::Text {
            text: "npm embedded submission".to_string(),
            text_elements: Vec::new(),
        }]],
        "cold queue must retain the exact submission after CLI exit"
    );
    timeout(
        Duration::from_secs(/*secs*/ 30),
        verifier.shutdown_gracefully(),
    )
    .await??;
    assert!(
        snapshot_requests(&captured)?.is_empty(),
        "no model request may happen: {address} saw none"
    );
    let leftover_socket = app_server_control_socket_path(codex_home.path())?;
    anyhow::ensure!(
        !leftover_socket.as_path().try_exists()?,
        "embedded writers must not leave a control socket behind"
    );
    assert_eq!(
        std::fs::read(codex_home.path().join("config.toml"))?,
        config_before
    );
    Ok(())
}

/// With NO owner running anywhere, a cold enqueue is still a durable
/// submission: the CLI accepts it (the embedded writer persists it), nothing
/// executes, and a later env-holding owner on the default socket loads the
/// thread and drains the queue through its own environment.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_without_a_running_owner_persists_and_a_later_owner_drains_it() -> Result<()> {
    let codex_home = socket_test_home()?;
    let codex = codex_utils_cargo_bin::cargo_bin("codex")?;
    let (address, captured, release) = gated_owner_model_server().await?;
    std::fs::write(
        codex_home.path().join("config.toml"),
        "features.plugins = false\nanalytics.enabled = false\n",
    )?;
    let config_before = std::fs::read(codex_home.path().join("config.toml"))?;
    let thread_id = create_fake_rollout(
        codex_home.path(),
        "2025-03-07T03-00-00",
        "2025-03-07T03:00:00Z",
        "cold enqueue session",
        Some(NUWAX_ENV_PROVIDER_ID),
        /*git_info*/ None,
    )?;

    // No owner exists yet: the cold enqueue must still succeed as a
    // submission and must not execute anything.
    let output = enqueue_from_envless_client(
        &codex,
        codex_home.path(),
        thread_id.as_str(),
        "cold submission without an owner",
    )
    .await?;
    assert!(
        output.status.success(),
        "a cold enqueue is a submission and must be accepted: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8(output.stdout)?.contains(&format!("for thread {thread_id}.")));
    assert!(
        snapshot_requests(&captured)?.is_empty(),
        "no model request may happen without an owner environment"
    );
    let leftover_socket = app_server_control_socket_path(codex_home.path())?;
    anyhow::ensure!(
        !leftover_socket.as_path().try_exists()?,
        "the in-process embedded writer must not leave a control socket behind"
    );
    // The drained turn's model request is the gateway's first request; open
    // its gate so the later owner's dispatch completes normally.
    release.send(true).ok();

    // The env-holding owner appears later and drains the persisted queue.
    let base_url = format!("http://{address}/v1");
    let _server = TestAppServer::builder()
        .with_program(&codex)
        .with_codex_home(codex_home.path())
        .with_plugin_startup_tasks()
        .without_managed_config()
        .with_args(&["app-server", "--listen", "unix://"])
        .with_env_overrides(&cleaned_environment())
        .with_env_overrides(&[
            ("NUWAX_BASE_URL", Some(base_url.as_str())),
            ("NUWAX_WIRE_API", Some("responses")),
            ("NUWAX_API_KEY", Some("owner-env-key")),
            ("NUWAX_MODEL", Some("nuwax-owner-model")),
            (
                "CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED",
                Some("1"),
            ),
        ])
        .build()
        .await?;
    let socket_path = wait_for_socket(codex_home.path()).await?;
    let mut app = connect_owner(&socket_path).await?;
    let _: ThreadResumeResponse = timeout(
        Duration::from_secs(/*secs*/ 30),
        app.request_typed(ClientRequest::ThreadResume {
            request_id: RequestId::Integer(1),
            params: ThreadResumeParams {
                thread_id: thread_id.clone(),
                ..Default::default()
            },
        }),
    )
    .await??;
    let completed = timeout(Duration::from_secs(/*secs*/ 30), async {
        while let Some(event) = app.next_event().await {
            if let AppServerEvent::ServerNotification(notification) = event
                && let ServerNotification::TurnCompleted(completed) = *notification
            {
                return Ok::<_, anyhow::Error>(completed);
            }
        }
        anyhow::bail!("owner disconnected before the drained turn completed")
    })
    .await??;
    assert_eq!(completed.turn.status, TurnStatus::Completed);

    let requests = snapshot_requests(&captured)?;
    assert_eq!(requests.len(), 1, "exactly one drained execution request");
    assert_eq!(requests[0].body["model"], Value::from("nuwax-owner-model"));
    assert_eq!(requests[0].authorization, "Bearer owner-env-key");
    assert_eq!(requests[0].turn_trigger, Some("queue".to_string()));
    assert_eq!(drain_queue(&mut app, &thread_id).await?, 0);
    assert_eq!(
        std::fs::read(codex_home.path().join("config.toml"))?,
        config_before,
        "the cold submission must not rewrite config.toml"
    );
    app.shutdown().await?;
    Ok(())
}
