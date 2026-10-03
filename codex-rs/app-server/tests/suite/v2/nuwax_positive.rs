//! Fork (nuwax-codex): the positive control for the `NUWAX_*` environment
//! group over a REAL child-process app-server. Isolation tests prove that
//! rejected configurations never emit a model request; this proves the
//! complement — a legitimate group WITH credentials, started through the
//! production environment channel, issues exactly the one expected model
//! request with the referenced credential, and the rejection paths hold in
//! the same child form.
//!
//! The child disables remote-control startup via the daemon's internal env
//! marker (see the spawn site) — without it the standalone server's
//! remote-control resolution retries auth discovery on unreachable networks
//! and the initialize handshake cannot complete in isolated environments.

use anyhow::Result;
use app_test_support::TestAppServer;
use codex_app_server_protocol::ConfigBatchWriteParams;
use codex_app_server_protocol::ConfigEdit;
use codex_app_server_protocol::ConfigReadParams;
use codex_app_server_protocol::ConfigReadResponse;
use codex_app_server_protocol::ConfigWriteResponse;
use codex_app_server_protocol::MergeStrategy;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadForkParams;
use codex_app_server_protocol::ThreadForkResponse;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadReadParams;
use codex_app_server_protocol::ThreadReadResponse;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::UserInput as V2UserInput;
use codex_app_server_transport::REMOTE_CONTROL_DISABLED_ENV_VAR;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::matchers::method;
use wiremock::matchers::path;

const READ_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 30);
const TEST_MODEL: &str = "nuwax-test-model";
const TEST_KEY: &str = "nuwax-positive-key";
const ROTATED_KEY: &str = "nuwax-rotated-key";
const FALLBACK_MODEL: &str = "independent-model";
const FALLBACK_KEY: &str = "independent-provider-key";
const ANSWER: &str = "positive control answered";

fn anthropic_completion_sse() -> String {
    concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg-pos\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"positive control answered\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":5}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    )
    .to_string()
}

enum StartupGroup<'a> {
    Seed {
        server_uri: &'a str,
        api_key: &'a str,
    },
    Absent,
}

/// Environment changes apply only to this explicitly controlled child.
async fn child(codex_home: &TempDir, group: StartupGroup<'_>) -> Result<TestAppServer> {
    let mut overrides = vec![
        ("NUWAX_BASE_URL", None),
        ("NUWAX_WIRE_API", None),
        ("NUWAX_API_KEY", None),
        ("NUWAX_MODEL", None),
        ("N1_FALLBACK_API_KEY", Some(FALLBACK_KEY)),
        (REMOTE_CONTROL_DISABLED_ENV_VAR, Some("1")),
    ];
    let base_url;
    if let StartupGroup::Seed {
        server_uri,
        api_key,
    } = group
    {
        base_url = format!("{server_uri}/v1");
        overrides.extend([
            ("NUWAX_BASE_URL", Some(base_url.as_str())),
            ("NUWAX_WIRE_API", Some("anthropic")),
            ("NUWAX_API_KEY", Some(api_key)),
            ("NUWAX_MODEL", Some(TEST_MODEL)),
        ]);
    }
    TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_managed_config()
        .with_env_overrides(&overrides)
        .build_initialized_with_timeout(READ_TIMEOUT)
        .await
}

async fn successful_turn(app: &mut TestAppServer, thread_id: &str, prompt: &str) -> Result<()> {
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
    let answers: Vec<_> = completed
        .turn
        .items
        .iter()
        .filter_map(|item| match item {
            ThreadItem::AgentMessage { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        (completed.turn.status, completed.turn.error, answers),
        (TurnStatus::Completed, None, vec![ANSWER])
    );
    Ok(())
}

fn message_texts(body: &Value, role: &str) -> Vec<String> {
    body["messages"]
        .as_array()
        .expect("Anthropic messages")
        .iter()
        .filter(|message| message["role"] == role)
        .flat_map(|message| match &message["content"] {
            Value::String(text) => vec![text.clone()],
            Value::Array(parts) => parts
                .iter()
                .filter_map(|part| part["text"].as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        })
        .collect()
}

async fn provider_requests(
    server: &MockServer,
    expected: &[(&str, &str, &str)],
) -> Result<Vec<Value>> {
    let requests = server.received_requests().await.expect("provider requests");
    assert_eq!(
        requests.len(),
        expected.len(),
        "only the expected model attempts reach this endpoint"
    );
    let mut bodies = Vec::new();
    for (request, (prompt, model, key)) in requests.iter().zip(expected) {
        let body: Value = serde_json::from_slice(&request.body)?;
        assert_eq!(
            (
                request.method.as_str(),
                request.url.path(),
                body["model"].as_str(),
                request
                    .headers
                    .get("x-api-key")
                    .and_then(|value| value.to_str().ok())
            ),
            ("POST", "/v1/messages", Some(*model), Some(*key))
        );
        assert!(
            message_texts(&body, "user")
                .iter()
                .any(|text| text.as_str() == *prompt),
            "this HTTP attempt must contain its initiating user prompt"
        );
        bodies.push(body);
    }
    Ok(bodies)
}

async fn read_config(app: &mut TestAppServer) -> Result<ConfigReadResponse> {
    let id = app
        .send_config_read_request(ConfigReadParams {
            include_layers: true,
            cwd: None,
        })
        .await?;
    timeout(READ_TIMEOUT, app.read_response(id)).await?
}

#[tokio::test]
async fn legal_environment_group_sends_exactly_one_credentialed_model_request() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_raw(anthropic_completion_sse(), "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let codex_home = TempDir::new()?;
    let mut app = child(
        &codex_home,
        StartupGroup::Seed {
            server_uri: &server.uri(),
            api_key: TEST_KEY,
        },
    )
    .await?;
    let thread = app.start_thread(ThreadStartParams::default()).await?.thread;
    let completed = timeout(
        READ_TIMEOUT,
        app.start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: thread.id.clone(),
            client_user_message_id: None,
            input: vec![V2UserInput::Text {
                text: "answer with one sentence".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        }),
    )
    .await??;
    anyhow::ensure!(
        completed.turn.error.is_none(),
        "the legal group's turn must succeed: {:?}",
        completed.turn.error
    );

    let requests = server.received_requests().await.expect("recorded requests");
    assert_eq!(
        requests.len(),
        1,
        "exactly one model request leaves the child process"
    );
    let request = &requests[0];
    let body: serde_json::Value = serde_json::from_slice(&request.body)?;
    assert_eq!(body["model"], json!(TEST_MODEL));
    let key = request
        .headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok());
    assert_eq!(
        key,
        Some(TEST_KEY),
        "the referenced credential reaches the provider endpoint"
    );
    let answer = serde_json::to_string(&completed)?;
    assert!(
        answer.contains("positive control answered"),
        "the mocked answer completed the turn: {answer}"
    );
    app.shutdown_gracefully().await?;
    Ok(())
}

#[tokio::test]
async fn child_thread_start_still_rejects_reserved_table_writes() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let codex_home = TempDir::new()?;
    let mut app = child(
        &codex_home,
        StartupGroup::Seed {
            server_uri: &server.uri(),
            api_key: TEST_KEY,
        },
    )
    .await?;
    let config: HashMap<String, serde_json::Value> =
        json!({"model_providers": {"nuwax_env": {"base_url": "https://evil.example/v1"}}})
            .as_object()
            .expect("config object")
            .clone()
            .into_iter()
            .collect();
    let params = ThreadStartParams {
        config: Some(config),
        ..Default::default()
    };
    let request_id = app
        .send_raw_request("thread/start", Some(serde_json::to_value(&params)?))
        .await?;
    let error = timeout(
        READ_TIMEOUT,
        app.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert!(
        error.error.message.contains("nuwax_env"),
        "the reserved provider is named: {}",
        error.error.message
    );
    server.verify().await;
    app.shutdown_gracefully().await?;
    Ok(())
}

#[tokio::test]
async fn seed_lifecycle_reloads_current_endpoint_and_never_persists_into_an_unseeded_child()
-> Result<()> {
    let original = MockServer::start().await;
    let rotated = MockServer::start().await;
    let fallback = MockServer::start().await;
    for (server, expected) in [(&original, 2), (&rotated, 3), (&fallback, 3)] {
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(anthropic_completion_sse(), "text/event-stream"),
            )
            .expect(expected)
            .mount(server)
            .await;
    }
    let codex_home = TempDir::new()?;
    std::fs::write(
        codex_home.path().join("config.toml"),
        format!(
            r#"
model = "{FALLBACK_MODEL}"
model_provider = "independent"
approval_policy = "never"
sandbox_mode = "read-only"
[model_providers.independent]
name = "Independent provider"
base_url = "{}/v1"
wire_api = "anthropic"
env_key = "N1_FALLBACK_API_KEY"
request_max_retries = 0
stream_max_retries = 0
"#,
            fallback.uri()
        ),
    )?;

    let mut first = child(
        &codex_home,
        StartupGroup::Seed {
            server_uri: &original.uri(),
            api_key: TEST_KEY,
        },
    )
    .await?;
    let started = first.start_thread(ThreadStartParams::default()).await?;
    assert_eq!(
        (started.model.as_str(), started.model_provider.as_str()),
        (TEST_MODEL, "nuwax_env")
    );
    let source_id = started.thread.id;
    successful_turn(&mut first, &source_id, "N1 original seeded turn").await?;

    // Explicit selection on another thread must not copy the seed's endpoint
    // or credentials, or modify the first thread's selection.
    let independent = first
        .start_thread(ThreadStartParams {
            model: Some(FALLBACK_MODEL.to_string()),
            model_provider: Some("independent".to_string()),
            ..Default::default()
        })
        .await?;
    successful_turn(
        &mut first,
        &independent.thread.id,
        "N1 independent second thread",
    )
    .await?;
    successful_turn(&mut first, &source_id, "N1 original remains isolated").await?;
    let read_id = first
        .send_thread_read_request(ThreadReadParams {
            thread_id: source_id.clone(),
            include_turns: true,
        })
        .await?;
    let persisted: ThreadReadResponse =
        timeout(READ_TIMEOUT, first.read_response(read_id)).await??;
    assert_eq!(persisted.thread.turns.len(), 2);
    let source_path = persisted.thread.path.expect("durable seeded rollout");
    first.shutdown_gracefully().await?;

    // A new process has no loaded thread and a different legitimate endpoint
    // and key. Its cold resume must rebuild the provider from this startup.
    let mut resumed_child = child(
        &codex_home,
        StartupGroup::Seed {
            server_uri: &rotated.uri(),
            api_key: ROTATED_KEY,
        },
    )
    .await?;
    let resume_id = resumed_child
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: source_id.clone(),
            ..Default::default()
        })
        .await?;
    let resumed: ThreadResumeResponse =
        timeout(READ_TIMEOUT, resumed_child.read_response(resume_id)).await??;
    assert_eq!(
        (
            resumed.thread.id.as_str(),
            resumed.model.as_str(),
            resumed.model_provider.as_str()
        ),
        (source_id.as_str(), TEST_MODEL, "nuwax_env")
    );
    successful_turn(
        &mut resumed_child,
        &source_id,
        "N1 cold resume uses rotated seed",
    )
    .await?;
    resumed_child.shutdown_gracefully().await?;

    // Another process cold-forks persisted history, then exercises the public
    // reload entry point before sending its next model request.
    let mut fork_child = child(
        &codex_home,
        StartupGroup::Seed {
            server_uri: &rotated.uri(),
            api_key: ROTATED_KEY,
        },
    )
    .await?;
    let fork_id = fork_child
        .send_thread_fork_request(ThreadForkParams {
            thread_id: source_id.clone(),
            ..Default::default()
        })
        .await?;
    let forked: ThreadForkResponse =
        timeout(READ_TIMEOUT, fork_child.read_response(fork_id)).await??;
    assert_ne!(forked.thread.id, source_id);
    assert_eq!(
        (forked.model.as_str(), forked.model_provider.as_str()),
        (TEST_MODEL, "nuwax_env")
    );
    let forked_id = forked.thread.id;
    successful_turn(
        &mut fork_child,
        &forked_id,
        "N1 cold fork uses rotated seed",
    )
    .await?;
    let write_id = fork_child
        .send_config_batch_write_request(ConfigBatchWriteParams {
            edits: vec![ConfigEdit {
                key_path: "features.enable_mcp_apps".to_string(),
                value: json!(false),
                merge_strategy: MergeStrategy::Upsert,
            }],
            file_path: None,
            expected_version: None,
            reload_user_config: true,
        })
        .await?;
    let _: ConfigWriteResponse =
        timeout(READ_TIMEOUT, fork_child.read_response(write_id)).await??;
    let reloaded = read_config(&mut fork_child).await?;
    assert_eq!(
        reloaded.config.additional["features"]["enable_mcp_apps"],
        json!(false)
    );
    assert_eq!(reloaded.config.model_provider.as_deref(), Some("nuwax_env"));
    successful_turn(&mut fork_child, &forked_id, "N1 reload keeps rotated seed").await?;
    let read_id = fork_child
        .send_thread_read_request(ThreadReadParams {
            thread_id: forked_id,
            include_turns: true,
        })
        .await?;
    let forked_history: ThreadReadResponse =
        timeout(READ_TIMEOUT, fork_child.read_response(read_id)).await??;
    let fork_path = forked_history.thread.path.expect("durable fork rollout");
    fork_child.shutdown_gracefully().await?;

    // Saved metadata can remember the selected provider ID; its definition,
    // endpoint, and credentials must not survive without a startup group.
    let mut unseeded = child(&codex_home, StartupGroup::Absent).await?;
    let clean_config = read_config(&mut unseeded).await?;
    assert_eq!(
        (
            clean_config.config.model.as_deref(),
            clean_config.config.model_provider.as_deref()
        ),
        (Some(FALLBACK_MODEL), Some("independent"))
    );
    assert!(
        clean_config.config.additional["model_providers"]
            .get("nuwax_env")
            .is_none()
    );
    assert!(
        !serde_json::to_string(&clean_config)?.contains("nuwax_env"),
        "no seed definition may survive in effective config or its layers"
    );
    let resume_id = unseeded
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: source_id.clone(),
            ..Default::default()
        })
        .await?;
    let error = timeout(
        READ_TIMEOUT,
        unseeded.read_stream_until_error_message(RequestId::Integer(resume_id)),
    )
    .await??;
    assert!(
        error.error.message.contains("nuwax_env"),
        "the absent provider definition must fail cold resume: {}",
        error.error.message
    );
    let fresh = unseeded.start_thread(ThreadStartParams::default()).await?;
    assert_eq!(
        (fresh.model.as_str(), fresh.model_provider.as_str()),
        (FALLBACK_MODEL, "independent")
    );
    successful_turn(
        &mut unseeded,
        &fresh.thread.id,
        "N1 no-env fresh thread is independent",
    )
    .await?;
    let retarget_id = unseeded
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: source_id,
            model: Some(FALLBACK_MODEL.to_string()),
            model_provider: Some("independent".to_string()),
            ..Default::default()
        })
        .await?;
    let retargeted: ThreadResumeResponse =
        timeout(READ_TIMEOUT, unseeded.read_response(retarget_id)).await??;
    assert_eq!(retargeted.model_provider, "independent");
    successful_turn(
        &mut unseeded,
        &retargeted.thread.id,
        "N1 explicit resume is independent",
    )
    .await?;
    unseeded.shutdown_gracefully().await?;

    provider_requests(
        &original,
        &[
            ("N1 original seeded turn", TEST_MODEL, TEST_KEY),
            ("N1 original remains isolated", TEST_MODEL, TEST_KEY),
        ],
    )
    .await?;
    let rotated_bodies = provider_requests(
        &rotated,
        &[
            ("N1 cold resume uses rotated seed", TEST_MODEL, ROTATED_KEY),
            ("N1 cold fork uses rotated seed", TEST_MODEL, ROTATED_KEY),
            ("N1 reload keeps rotated seed", TEST_MODEL, ROTATED_KEY),
        ],
    )
    .await?;
    provider_requests(
        &fallback,
        &[
            ("N1 independent second thread", FALLBACK_MODEL, FALLBACK_KEY),
            (
                "N1 no-env fresh thread is independent",
                FALLBACK_MODEL,
                FALLBACK_KEY,
            ),
            (
                "N1 explicit resume is independent",
                FALLBACK_MODEL,
                FALLBACK_KEY,
            ),
        ],
    )
    .await?;
    // Both cold paths send actual persisted assistant replies and prompts,
    // so selecting the expected model slug alone cannot satisfy this test.
    for (body, replies) in [(&rotated_bodies[0], 2), (&rotated_bodies[1], 3)] {
        let users = message_texts(body, "user");
        assert!(users.iter().any(|text| text == "N1 original seeded turn"));
        assert!(
            users
                .iter()
                .any(|text| text == "N1 original remains isolated")
        );
        assert_eq!(
            message_texts(body, "assistant"),
            vec![ANSWER.to_string(); replies]
        );
    }
    let original_uri = original.uri();
    let rotated_uri = rotated.uri();
    for path in [
        codex_home.path().join("config.toml"),
        source_path,
        fork_path,
    ] {
        let persisted = std::fs::read_to_string(&path)?;
        for forbidden in [
            TEST_KEY,
            ROTATED_KEY,
            FALLBACK_KEY,
            original_uri.as_str(),
            rotated_uri.as_str(),
        ] {
            assert!(
                !persisted.contains(forbidden),
                "startup provider data persisted into {}",
                path.display()
            );
        }
    }
    original.verify().await;
    rotated.verify().await;
    fallback.verify().await;
    Ok(())
}
