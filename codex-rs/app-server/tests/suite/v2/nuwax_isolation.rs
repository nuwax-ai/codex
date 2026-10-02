//! Fork (nuwax-codex): the `NUWAX_*` environment group's source isolation
//! over real JSON-RPC. An app-server started with a legitimate environment
//! group must reject client-supplied `thread/start`, `resume` and `fork`
//! config that writes into the reserved provider's table — same-named legal
//! keys included — without a single model request leaving the process.
//!
//! In-process (not child-process) because this environment cannot complete
//! the standalone app-server's startup network dependencies; the JSON-RPC
//! surface under test is identical.

use anyhow::Result;
use codex_app_server::in_process;
use codex_app_server::in_process::InProcessStartArgs;
use codex_app_server_protocol::ClientInfo;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::InitializeCapabilities;
use codex_app_server_protocol::InitializeParams;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::SessionSource;
use codex_app_server_protocol::ThreadForkParams;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::UserInput as V2UserInput;
use codex_config::CloudConfigBundleLoader;
use codex_config::LoaderOverrides;
use codex_core::config::ConfigBuilder;
use codex_exec_server::EnvironmentManager;
use codex_utils_cli::NuwaxEnvInput;
use codex_utils_cli::nuwax_env_overrides;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::MockServer;

const READ_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 30);

/// Seeds the environment group from structured input — no process-env
/// mutation — pointing the provider at the capturing mock so any model
/// request that escapes the rejected configuration is recorded instead of
/// silently failing to connect.
fn env_group_seeds(server_uri: &str) -> Vec<(String, toml::Value)> {
    nuwax_env_overrides(
        NuwaxEnvInput {
            model: Some("nuwax-test-model".into()),
            base_url: Some(server_uri.into()),
            wire_api: Some("responses".into()),
            api_key: Some("nuwax-test-key".into()),
        },
        /*cli_model*/ None,
        /*cli_provider*/ None,
        &[],
    )
    .expect("valid environment group")
}

fn config_override(value: Value) -> Option<HashMap<String, Value>> {
    let object = value.as_object().expect("config object").clone();
    Some(object.into_iter().collect())
}

async fn nuwax_in_process(
    codex_home: &TempDir,
    seeds: Vec<(String, toml::Value)>,
) -> Result<in_process::InProcessClientHandle> {
    let loader_overrides = LoaderOverrides::without_managed_config_for_tests();
    let config = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .fallback_cwd(Some(codex_home.path().to_path_buf()))
        .loader_overrides(loader_overrides.clone())
        .build()
        .await?;
    // Resume needs a thread store that supports listing turns; the default
    // in-memory one does not.
    let state_db = codex_rollout::state_db::try_init(&config).await?;
    let client = in_process::start(InProcessStartArgs {
        arg0_paths: Default::default(),
        config: Arc::new(config),
        cli_overrides: Vec::new(),
        env_seed_overrides: seeds,
        loader_overrides,
        strict_config: /*strict_config*/ false,
        cloud_config_bundle: CloudConfigBundleLoader::default(),
        embedded_network_policy: Default::default(),
        thread_config_loader: Arc::new(codex_config::NoopThreadConfigLoader),
        feedback: codex_feedback::CodexFeedback::new(),
        log_db: None,
        state_db: Some(state_db),
        environment_manager: Arc::new(EnvironmentManager::default_for_tests()),
        config_warnings: Vec::new(),
        session_source: SessionSource::Cli.into(),
        enable_codex_api_key_env: false,
        initialize: InitializeParams {
            client_info: ClientInfo {
                name: "codex-app-server-tests".to_string(),
                title: None,
                version: "0.1.0".to_string(),
            },
            capabilities: Some(InitializeCapabilities {
                experimental_api: true,
                ..Default::default()
            }),
        },
        channel_capacity: in_process::DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
    })
    .await?;
    Ok(client)
}

async fn expect_reserved_error(
    client: &in_process::InProcessClientHandle,
    request: ClientRequest,
) -> Result<()> {
    let error: JSONRPCErrorError = match timeout(READ_TIMEOUT, client.request(request)).await?? {
        Ok(_result) => anyhow::bail!("reserved-provider override must fail the request"),
        Err(error) => error,
    };
    let message = error.message;
    assert!(
        message.contains("nuwax_env"),
        "the rejection must name the reserved provider: {message}"
    );
    assert!(
        !message.contains("nuwax-test-key") && !message.contains("evil.example"),
        "errors must name keys, never values: {message}"
    );
    Ok(())
}

async fn assert_no_model_requests(server: &MockServer) -> Result<()> {
    let requests = server.received_requests().await.expect("request capture");
    assert!(
        requests.is_empty(),
        "rejected configuration must not produce model requests: {requests:?}"
    );
    Ok(())
}

#[tokio::test]
async fn thread_start_config_cannot_override_the_environment_provider() -> Result<()> {
    let server = MockServer::start().await;
    let codex_home = TempDir::new()?;
    let seeds = env_group_seeds(&server.uri());
    let client = nuwax_in_process(&codex_home, seeds).await?;
    for config in [
        // Same-named legal keys — the merged-table whitelist hole.
        json!({"model_providers": {"nuwax_env": {"base_url": "https://evil.example/v1"}}}),
        json!({"model_providers": {"nuwax_env": {"env_key": "OLD_AUTH_KEY"}}}),
        // Foreign keys are equally rejected.
        json!({"model_providers": {"nuwax_env": {"http_headers": {"x-old": "1"}}}}),
    ] {
        let mut params = ThreadStartParams::default();
        params.config = config_override(config);
        expect_reserved_error(
            &client,
            ClientRequest::ThreadStart {
                request_id: RequestId::Integer(1),
                params,
            },
        )
        .await?;
    }
    assert_no_model_requests(&server).await?;
    client.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn thread_resume_and_fork_config_cannot_override_the_environment_provider() -> Result<()> {
    let server = MockServer::start().await;
    let codex_home = TempDir::new()?;
    let seeds = env_group_seeds(&server.uri());
    let mut client = nuwax_in_process(&codex_home, seeds).await?;
    let response = client
        .request(ClientRequest::ThreadStart {
            request_id: RequestId::Integer(1),
            params: ThreadStartParams::default(),
        })
        .await?
        .expect("clean thread/start should succeed");
    let ThreadStartResponse { thread, .. } = serde_json::from_value(response)?;
    // One turn attempt gives the thread a rollout, so resume and fork reach
    // configuration loading instead of failing on a missing rollout first.
    // The credentials stay environment-referenced and unset in this process,
    // so the turn itself stops before any HTTP — exactly the zero-request
    // property under test.
    client
        .request(ClientRequest::TurnStart {
            request_id: RequestId::Integer(2),
            params: TurnStartParams {
                thread_id: thread.id.clone(),
                client_user_message_id: None,
                input: vec![V2UserInput::Text {
                    text: "seed the rollout".to_string(),
                    text_elements: Vec::new(),
                }],
                ..Default::default()
            },
        })
        .await?
        .expect("turn/start should succeed");
    timeout(READ_TIMEOUT, async {
        loop {
            let Some(event) = client.next_event().await else {
                anyhow::bail!("in-process app-server stopped before turn/completed");
            };
            if let in_process::InProcessServerEvent::ServerNotification(notification) = event
                && let ServerNotification::TurnCompleted(completed) = notification.as_ref()
                && completed.thread_id == thread.id
            {
                return Ok::<(), anyhow::Error>(());
            }
        }
    })
    .await??;
    client.shutdown().await?;
    let thread_id = thread.id.to_string();
    // A fresh runtime holds no live thread, so resume and fork take the cold
    // path that reloads configuration from the client-supplied overrides.
    let seeds = env_group_seeds(&server.uri());
    let client = nuwax_in_process(&codex_home, seeds).await?;

    expect_reserved_error(
        &client,
        ClientRequest::ThreadResume {
            request_id: RequestId::Integer(2),
            params: ThreadResumeParams {
                thread_id: thread_id.clone(),
                config: config_override(json!({
                    "model_providers": {"nuwax_env": {"base_url": "https://evil.example/v1"}}
                })),
                ..Default::default()
            },
        },
    )
    .await?;

    expect_reserved_error(
        &client,
        ClientRequest::ThreadFork {
            request_id: RequestId::Integer(3),
            params: ThreadForkParams {
                thread_id,
                config: config_override(json!({
                    "model_providers": {"nuwax_env": {"env_key": "OLD_AUTH_KEY"}}
                })),
                ..Default::default()
            },
        },
    )
    .await?;

    // No request may ever leave: the turn stops at credential resolution and
    // the rejected resume/fork config fails the load.
    assert_no_model_requests(&server).await?;
    client.shutdown().await?;
    Ok(())
}
