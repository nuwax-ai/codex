//! B2 acceptance: queue ownership through the REAL CLI binary against real
//! app-server owners on the default control socket.
//!
//! Three ownership facts the enum-level unit tests cannot prove:
//! 1. A standalone owner that holds the active NUWAX environment IS the
//!    legitimate owner: it executes queued turns through its own env-seeded
//!    temporary provider, and the env never persists into config.toml.
//! 2. Enqueue is message submission, not model execution: a thread whose
//!    temporary provider the owner cannot resolve accepts the enqueue and
//!    then fails the TURN with a bounded error — enqueue success is never
//!    reported as execution success.
//! 3. A session name shared across providers is ambiguous for queue targeting
//!    and must be rejected with the candidate thread IDs.

use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use app_test_support::MockResponsesConfig;
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
use codex_app_server_protocol::TurnStatus;
use codex_protocol::config_types::NUWAX_ENV_PROVIDER_ID;
use codex_protocol::shell_environment::OPENAI_FEDERATION_RULE_ID_ENV_VAR;
use codex_protocol::shell_environment::OPENAI_IDENTITY_TOKEN_FILE_ENV_VAR;
use pretty_assertions::assert_eq;
use serde_json::Value;
use tokio::time::timeout;

#[cfg(unix)]
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
#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
async fn connect_owner(
    socket_path: &codex_utils_absolute_path::AbsolutePathBuf,
) -> Result<RemoteAppServerClient> {
    Ok(RemoteAppServerClient::connect(RemoteAppServerConnectArgs {
        endpoint: RemoteAppServerEndpoint::UnixSocket {
            socket_path: socket_path.clone(),
        },
        client_name: "queue-owner-matrix-test".to_string(),
        client_version: "0.1.0".to_string(),
        experimental_api: true,
        mcp_server_openai_form_elicitation: false,
        opt_out_notification_methods: Vec::new(),
        channel_capacity: DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
    })
    .await?)
}

#[cfg(unix)]
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

#[cfg(unix)]
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

/// A standalone owner holding the active NUWAX environment executes queued
/// turns through its own env-seeded provider: the wire carries the env model
/// and credential, and nothing persists into config.toml.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_executes_on_a_standalone_owner_holding_the_nuwax_environment() -> Result<()> {
    let codex_home = socket_test_home()?;
    let codex = codex_utils_cargo_bin::cargo_bin("codex")?;
    let model = create_mock_responses_server_sequence_unchecked(vec![
        create_final_assistant_message_sse_response("queued on env owner")?,
    ])
    .await;
    // Minimal config: no named provider, so the owner's model and provider
    // can only come from its environment.
    std::fs::write(
        codex_home.path().join("config.toml"),
        "features.plugins = false\nanalytics.enabled = false\n",
    )?;
    let config_before = std::fs::read(codex_home.path().join("config.toml"))?;
    let timestamp = "2025-03-01T09-00-00";
    let thread_id = create_fake_rollout(
        codex_home.path(),
        timestamp,
        "2025-03-01T09:00:00Z",
        "env-owner session",
        Some(NUWAX_ENV_PROVIDER_ID),
        /*git_info*/ None,
    )?;

    let env_base_url = format!("{}/v1", model.uri());
    let _server = TestAppServer::builder()
        .with_program(&codex)
        .with_codex_home(codex_home.path())
        .with_plugin_startup_tasks()
        .without_managed_config()
        .with_args(&["app-server", "--listen", "unix://"])
        .with_env_overrides(&cleaned_environment())
        .with_env_overrides(&[
            // The full active group lives ONLY in this owner process.
            ("NUWAX_BASE_URL", Some(env_base_url.as_str())),
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
    // Loading the thread first is the natural owner-side trigger: a queued
    // message dispatches when the thread becomes idle after resume.
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
    let output = enqueue_from_envless_client(
        &codex,
        codex_home.path(),
        thread_id.as_str(),
        "run on the env owner",
    )
    .await?;
    assert!(
        output.status.success(),
        "enqueue must be accepted by the env-holding owner: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8(output.stdout)?.contains(&format!("for thread {thread_id}.")));

    let completed = timeout(Duration::from_secs(/*secs*/ 30), async {
        while let Some(event) = app.next_event().await {
            if let AppServerEvent::ServerNotification(notification) = event
                && let ServerNotification::TurnCompleted(completed) = *notification
            {
                return Ok::<_, anyhow::Error>(completed);
            }
        }
        anyhow::bail!("env owner disconnected before the queued turn completed")
    })
    .await??;
    assert_eq!(completed.thread_id.to_string(), thread_id);
    assert_eq!(completed.turn.status, TurnStatus::Completed);

    assert_eq!(drain_queue(&mut app, &thread_id).await?, 0);
    let requests = model
        .received_requests()
        .await
        .context("owner model request capture unavailable")?;
    assert_eq!(requests.len(), 1, "exactly one execution request");
    let body = requests[0].body_json::<Value>()?;
    assert_eq!(body["model"], Value::from("nuwax-owner-model"));
    assert_eq!(requests[0].url.path(), "/v1/responses");
    assert_eq!(
        requests[0].headers["authorization"].to_str()?,
        "Bearer owner-env-key"
    );
    let metadata: Value =
        serde_json::from_str(requests[0].headers["x-codex-turn-metadata"].to_str()?)?;
    assert_eq!(metadata["turn_trigger"], Value::from("queue"));
    assert_eq!(
        std::fs::read(codex_home.path().join("config.toml"))?,
        config_before,
        "the owner's environment must not persist into config.toml"
    );
    app.shutdown().await?;
    Ok(())
}

/// Enqueue is submission: without the environment, the owner accepts the
/// message and then fails the TURN — enqueue success never masquerades as
/// execution success, no model request is made, and the failure is bounded.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_submission_succeeds_but_execution_fails_without_the_temporary_provider() -> Result<()>
{
    let codex_home = socket_test_home()?;
    let codex = codex_utils_cargo_bin::cargo_bin("codex")?;
    let model = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    MockResponsesConfig::new(&model.uri())
        .with_model("gpt-5.2-codex")
        .with_root_config("features.plugins = false\nanalytics.enabled = false")
        .with_provider_config("env_key = \"CODEX_QUEUE_OWNER_API_KEY\"")
        .write(codex_home.path())?;
    let config_before = std::fs::read(codex_home.path().join("config.toml"))?;
    let timestamp = "2025-03-02T08-00-00";
    let thread_id = create_fake_rollout(
        codex_home.path(),
        timestamp,
        "2025-03-02T08:00:00Z",
        "unresolvable temporary provider session",
        Some(NUWAX_ENV_PROVIDER_ID),
        /*git_info*/ None,
    )?;

    let _server = TestAppServer::builder()
        .with_program(&codex)
        .with_codex_home(codex_home.path())
        .with_plugin_startup_tasks()
        .without_managed_config()
        .with_args(&["app-server", "--listen", "unix://"])
        .with_env_overrides(&cleaned_environment())
        .with_env_overrides(&[
            ("CODEX_QUEUE_OWNER_API_KEY", Some("owner-named-key")),
            (
                "CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED",
                Some("1"),
            ),
        ])
        .build()
        .await?;
    let socket_path = wait_for_socket(codex_home.path()).await?;
    let mut app = connect_owner(&socket_path).await?;
    let output = enqueue_from_envless_client(
        &codex,
        codex_home.path(),
        thread_id.as_str(),
        "cannot run without the env",
    )
    .await?;
    assert!(
        output.status.success(),
        "the enqueue itself must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8(output.stdout)?.contains(&format!("for thread {thread_id}.")));

    // The execution trigger is the owner loading the thread; without the
    // environment it cannot resolve the temporary provider, so the resume is
    // rejected and the submission stays queued — enqueue success never
    // becomes execution success.
    let resume_result = timeout(
        Duration::from_secs(/*secs*/ 30),
        app.request_typed::<ThreadResumeResponse>(ClientRequest::ThreadResume {
            request_id: RequestId::Integer(2),
            params: ThreadResumeParams {
                thread_id: thread_id.clone(),
                ..Default::default()
            },
        }),
    )
    .await?;
    let error = resume_result
        .err()
        .context("owner must reject an unresolvable temporary provider")?;
    let message = format!("{error:#}");
    assert!(
        message.contains(&format!(
            "model_providers.{NUWAX_ENV_PROVIDER_ID} is reserved"
        )) && message.contains("environment group is not active"),
        "resume must fail for the missing provider rather than transport or timeout: {message}"
    );
    assert_eq!(
        drain_queue(&mut app, &thread_id).await?,
        1,
        "the accepted submission stays queued when execution cannot start"
    );
    let requests = model
        .received_requests()
        .await
        .context("owner model request capture unavailable")?;
    assert!(
        requests.is_empty(),
        "no model request may be made for an unresolvable provider"
    );
    assert_eq!(
        std::fs::read(codex_home.path().join("config.toml"))?,
        config_before,
        "the failure must not rewrite config.toml"
    );
    app.shutdown().await?;
    Ok(())
}

/// A client holding a DIFFERENT active NUWAX environment submits to an
/// existing owner: execution stays on the owner's environment end to end,
/// the client's credentials never reach the wire, and the client environment
/// must not leak into the shared config.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_from_a_differently_envd_client_runs_on_the_owner_environment() -> Result<()> {
    let codex_home = socket_test_home()?;
    let codex = codex_utils_cargo_bin::cargo_bin("codex")?;
    let owner_model = create_mock_responses_server_sequence_unchecked(vec![
        create_final_assistant_message_sse_response("owner env executes")?,
    ])
    .await;
    // The client's environment points at a SECOND mock whose request log must
    // stay empty: if any writer consumed the client environment, its requests
    // would land here.
    let client_model = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    std::fs::write(
        codex_home.path().join("config.toml"),
        "features.plugins = false\nanalytics.enabled = false\n",
    )?;
    let config_before = std::fs::read(codex_home.path().join("config.toml"))?;
    let thread_id = create_fake_rollout(
        codex_home.path(),
        "2025-03-04T06-00-00",
        "2025-03-04T06:00:00Z",
        "cross-client queue session",
        Some(NUWAX_ENV_PROVIDER_ID),
        /*git_info*/ None,
    )?;

    let owner_base_url = format!("{}/v1", owner_model.uri());
    let _server = TestAppServer::builder()
        .with_program(&codex)
        .with_codex_home(codex_home.path())
        .with_plugin_startup_tasks()
        .without_managed_config()
        .with_args(&["app-server", "--listen", "unix://"])
        .with_env_overrides(&cleaned_environment())
        .with_env_overrides(&[
            ("NUWAX_BASE_URL", Some(owner_base_url.as_str())),
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

    // The client carries a complete, DIFFERENT environment group.
    let mut command = tokio::process::Command::new(&codex);
    for (variable, _) in cleaned_environment() {
        command.env_remove(variable);
    }
    let output = timeout(
        Duration::from_secs(/*secs*/ 30),
        command
            .env("CODEX_HOME", codex_home.path())
            .current_dir(codex_home.path())
            .kill_on_drop(true)
            .env("NUWAX_BASE_URL", format!("{}/v1", client_model.uri()))
            .env("NUWAX_WIRE_API", "responses")
            .env("NUWAX_API_KEY", "client-side-key")
            .env("NUWAX_MODEL", "nuwax-client-model")
            .args([
                "queue",
                "--thread",
                thread_id.as_str(),
                "--message",
                "must run on the owner",
            ])
            .output(),
    )
    .await??;
    assert!(
        output.status.success(),
        "an env-holding client must still enqueue through the owner: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let completed = timeout(Duration::from_secs(/*secs*/ 30), async {
        while let Some(event) = app.next_event().await {
            if let AppServerEvent::ServerNotification(notification) = event
                && let ServerNotification::TurnCompleted(completed) = *notification
            {
                return Ok::<_, anyhow::Error>(completed);
            }
        }
        anyhow::bail!("owner disconnected before the queued turn completed")
    })
    .await??;
    assert_eq!(completed.thread_id.to_string(), thread_id);
    assert_eq!(completed.turn.status, TurnStatus::Completed);

    assert_eq!(drain_queue(&mut app, &thread_id).await?, 0);
    let owner_requests = owner_model
        .received_requests()
        .await
        .context("owner model request capture unavailable")?;
    assert_eq!(owner_requests.len(), 1, "exactly one execution request");
    let body = owner_requests[0].body_json::<Value>()?;
    assert_eq!(body["model"], Value::from("nuwax-owner-model"));
    assert_eq!(
        owner_requests[0].headers["authorization"].to_str()?,
        "Bearer owner-env-key"
    );
    let client_requests = client_model
        .received_requests()
        .await
        .context("client model request capture unavailable")?;
    assert!(
        client_requests.is_empty(),
        "the client environment must never execute the queued turn"
    );
    assert_eq!(
        std::fs::read(codex_home.path().join("config.toml"))?,
        config_before,
        "the client environment must not leak into config.toml"
    );
    app.shutdown().await?;
    Ok(())
}

/// While the owner daemon listens on the default socket, a queue invocation
/// whose configuration excludes the daemon (here: a non-allowlisted `-c`
/// override) would fall back to an EMBEDDED writer — the no-second-writer
/// guard must refuse it before any submission is written.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_refuses_an_embedded_writer_beside_the_running_owner_daemon() -> Result<()> {
    let codex_home = socket_test_home()?;
    let codex = codex_utils_cargo_bin::cargo_bin("codex")?;
    let model = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    std::fs::write(
        codex_home.path().join("config.toml"),
        "features.plugins = false\nanalytics.enabled = false\n",
    )?;
    let thread_id = create_fake_rollout(
        codex_home.path(),
        "2025-03-05T05-00-00",
        "2025-03-05T05:00:00Z",
        "second writer guard session",
        Some(NUWAX_ENV_PROVIDER_ID),
        /*git_info*/ None,
    )?;

    let base_url = format!("{}/v1", model.uri());
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

    let mut command = tokio::process::Command::new(&codex);
    for (variable, _) in cleaned_environment() {
        command.env_remove(variable);
    }
    let output = timeout(
        Duration::from_secs(/*secs*/ 30),
        command
            .env("CODEX_HOME", codex_home.path())
            .current_dir(codex_home.path())
            .kill_on_drop(true)
            // A non-allowlisted override excludes the shared daemon for this
            // invocation, so queue would fall back to an embedded writer.
            .args(["-c", "model_reasoning_summary=detailed"])
            .args([
                "queue",
                "--thread",
                thread_id.as_str(),
                "--message",
                "must not create a second writer",
            ])
            .output(),
    )
    .await??;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "an embedded writer beside the running daemon must be refused: {stderr}"
    );
    assert!(
        stderr.contains("cannot queue through an embedded app server"),
        "the refusal must be the second-writer guard: {stderr}"
    );
    assert_eq!(
        drain_queue(&mut app, &thread_id).await?,
        0,
        "the refused invocation must leave the queue untouched"
    );
    let requests = model
        .received_requests()
        .await
        .context("owner model request capture unavailable")?;
    assert!(
        requests.is_empty(),
        "no turn may start from a refused enqueue"
    );
    app.shutdown().await?;
    Ok(())
}

/// A session label shared by two threads across providers is ambiguous for
/// queue targeting: the CLI must refuse and name the candidates.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_by_name_rejects_an_ambiguous_label_across_providers() -> Result<()> {
    let codex_home = socket_test_home()?;
    let codex = codex_utils_cargo_bin::cargo_bin("codex")?;
    let model = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    MockResponsesConfig::new(&model.uri())
        .with_model("gpt-5.2-codex")
        .with_root_config("features.plugins = false\nanalytics.enabled = false")
        .with_provider_config("env_key = \"CODEX_QUEUE_OWNER_API_KEY\"")
        .write(codex_home.path())?;
    let shared_name = "shared-ambiguous-name";
    let first = create_fake_rollout(
        codex_home.path(),
        "2025-03-03T07-00-00",
        "2025-03-03T07:00:00Z",
        shared_name,
        Some(NUWAX_ENV_PROVIDER_ID),
        /*git_info*/ None,
    )?;
    let second = create_fake_rollout(
        codex_home.path(),
        "2025-03-03T07-30-00",
        "2025-03-03T07:30:00Z",
        shared_name,
        /*provider*/ None,
        /*git_info*/ None,
    )?;

    let _server = TestAppServer::builder()
        .with_program(&codex)
        .with_codex_home(codex_home.path())
        .with_plugin_startup_tasks()
        .without_managed_config()
        .with_args(&["app-server", "--listen", "unix://"])
        .with_env_overrides(&cleaned_environment())
        .with_env_overrides(&[(
            "CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED",
            Some("1"),
        )])
        .build()
        .await?;
    let _socket_path = wait_for_socket(codex_home.path()).await?;

    let output =
        enqueue_from_envless_client(&codex, codex_home.path(), shared_name, "ambiguous").await?;
    assert!(
        !output.status.success(),
        "an ambiguous label must be rejected"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ambig") || stderr.contains("multiple"),
        "the error must describe the ambiguity: {stderr}"
    );
    assert!(
        stderr.contains(first.as_str()) && stderr.contains(second.as_str()),
        "the error must list both candidate thread IDs ({first}, {second}): {stderr}"
    );
    let requests = model
        .received_requests()
        .await
        .context("owner model request capture unavailable")?;
    assert!(
        requests.is_empty(),
        "no turn may start from an ambiguous enqueue"
    );
    Ok(())
}
