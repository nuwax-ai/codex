//! End-to-end NUWAX_* environment startup coverage: the same process
//! environment must produce the same model, protocol and outbound request
//! through the real `codex exec` binary — one test per wire, each asserting
//! the actual HTTP path, the credential resolved from `NUWAX_API_KEY`
//! (referenced via `env_key`, never embedded in config), and the model from
//! `NUWAX_MODEL`.

#![allow(clippy::unwrap_used)]
use anyhow::Result;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex_exec::test_codex_exec;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::Mutex;
use wiremock::Mock;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;
use wiremock::matchers::path_regex;

const CHAT_SSE: &str = concat!(
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"nuwax-test-model\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"ok\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"nuwax-test-model\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":1,\"total_tokens\":5}}\n\n",
    "data: [DONE]\n\n",
);

const ANTHROPIC_SSE: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"nuwax-test-model\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n",
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":1}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

const RESPONSES_SSE: &str = concat!(
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\",\"model\":\"nuwax-test-model\"}}\n\n",
    "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"ok\"}],\"id\":\"o1\"}}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"usage\":{\"input_tokens\":4,\"output_tokens\":1,\"total_tokens\":5}}}\n\n",
);

/// Captures every matched request. The shared `mount_sse_once_match` helper
/// hardcodes the `/responses` path, so the chat and anthropic wires need
/// their own mount.
#[derive(Clone, Default)]
struct RequestLog(Arc<Mutex<Vec<wiremock::Request>>>);

impl wiremock::Match for RequestLog {
    fn matches(&self, request: &wiremock::Request) -> bool {
        self.0.lock().unwrap().push(request.clone());
        true
    }
}

impl RequestLog {
    fn all(&self) -> Vec<wiremock::Request> {
        self.0.lock().unwrap().clone()
    }

    fn single(&self) -> wiremock::Request {
        let requests = self.0.lock().unwrap();
        assert_eq!(
            requests.len(),
            1,
            "expected exactly one request, got paths {:?}",
            requests
                .iter()
                .map(|request| request.url.path().to_string())
                .collect::<Vec<_>>()
        );
        requests[0].clone()
    }
}

async fn run_nuwax_env_turn(
    wire_api: &str,
    expected_path: &str,
    sse_body: &str,
    model: &str,
    api_key: &str,
) -> Result<()> {
    let test = test_codex_exec();
    let config_path = test.home_path().join("config.toml");
    let original_config = "# NUWAX environment settings must remain ephemeral.\n";
    std::fs::write(&config_path, original_config)?;
    let server = start_mock_server().await;
    let repo_root = codex_utils_cargo_bin::repo_root()?;
    let log = RequestLog::default();
    Mock::given(method("POST"))
        .and(path(expected_path))
        .and(log.clone())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(
                    sse_body.replace("nuwax-test-model", model),
                    "text/event-stream",
                ),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let mut command = test.cmd();
    for variable in [
        "NUWAX_BASE_URL",
        "NUWAX_WIRE_API",
        "NUWAX_API_KEY",
        "NUWAX_MODEL",
        "NUWAX_MAX_OUTPUT_TOKENS",
    ] {
        command.env_remove(variable);
    }
    command
        .arg("--skip-git-repo-check")
        .arg("-C")
        .arg(&repo_root)
        .arg("reply with ok")
        .env("NUWAX_BASE_URL", format!("{}/v1", server.uri()))
        .env("NUWAX_WIRE_API", wire_api)
        .env("NUWAX_API_KEY", api_key)
        .env("NUWAX_MODEL", model);
    command.env("NUWAX_MAX_OUTPUT_TOKENS", "2048");
    let output = tokio::task::spawn_blocking(move || command.output()).await??;
    assert!(
        output.status.success(),
        "wire {wire_api} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let request = log.single();
    assert_eq!(request.url.path(), expected_path, "wire {wire_api}");
    // The credential must resolve from NUWAX_API_KEY through env_key.
    // Anthropic authenticates with x-api-key; the other wires use Bearer.
    let credential = if wire_api == "anthropic" {
        request
            .headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    } else {
        request
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    let expected_credential = if wire_api == "anthropic" {
        api_key.to_string()
    } else {
        format!("Bearer {api_key}")
    };
    assert_eq!(credential, Some(expected_credential), "wire {wire_api}");
    let body: serde_json::Value = serde_json::from_slice(&request.body).expect("request body json");
    assert_eq!(body["model"], model, "wire {wire_api}");
    let output_cap_field = if wire_api == "responses" {
        "max_output_tokens"
    } else {
        "max_tokens"
    };
    assert_eq!(body[output_cap_field], 2048, "wire {wire_api}");
    assert_eq!(
        std::fs::read_to_string(&config_path)?,
        original_config,
        "wire {wire_api} must not persist its temporary provider or credential"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nuwax_env_chat_wire_reaches_the_chat_endpoint() -> Result<()> {
    run_nuwax_env_turn(
        "chat",
        "/v1/chat/completions",
        CHAT_SSE,
        "nuwax-test-model",
        "nuwax-test-key",
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nuwax_env_anthropic_wire_reaches_the_messages_endpoint() -> Result<()> {
    run_nuwax_env_turn(
        "anthropic",
        "/v1/messages",
        ANTHROPIC_SSE,
        "nuwax-test-model",
        "nuwax-test-key",
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nuwax_env_responses_wire_reaches_the_responses_endpoint() -> Result<()> {
    run_nuwax_env_turn(
        "responses",
        "/v1/responses",
        RESPONSES_SSE,
        "nuwax-test-model",
        "nuwax-test-key",
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nuwax_env_parallel_exec_processes_keep_independent_provider_settings() -> Result<()> {
    // Both helpers yield while their independently configured child commands run.
    let (chat, anthropic) = tokio::join!(
        run_nuwax_env_turn(
            "chat",
            "/v1/chat/completions",
            CHAT_SSE,
            "nuwax-parallel-chat-model",
            "nuwax-parallel-chat-key",
        ),
        run_nuwax_env_turn(
            "anthropic",
            "/v1/messages",
            ANTHROPIC_SSE,
            "nuwax-parallel-anthropic-model",
            "nuwax-parallel-anthropic-key",
        ),
    );
    chat?;
    anthropic?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nuwax_env_partial_group_fails_fast_without_a_request() -> anyhow::Result<()> {
    let test = test_codex_exec();
    let server = start_mock_server().await;
    let repo_root = codex_utils_cargo_bin::repo_root()?;
    let log = RequestLog::default();
    Mock::given(method("POST"))
        .and(path_regex(".*"))
        .and(log.clone())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(RESPONSES_SSE.to_string(), "text/event-stream"),
        )
        .mount(&server)
        .await;
    // BASE_URL and API_KEY set, WIRE_API missing: the process must exit with
    // the group error before any request is attempted.
    let output = test
        .cmd()
        .arg("--skip-git-repo-check")
        .arg("-C")
        .arg(&repo_root)
        .arg("hello")
        .env_remove("NUWAX_MAX_OUTPUT_TOKENS")
        .env_remove("NUWAX_WIRE_API")
        .env("NUWAX_BASE_URL", format!("{}/v1", server.uri()))
        .env("NUWAX_API_KEY", "nuwax-test-key")
        .env("NUWAX_MODEL", "nuwax-test-model")
        .output()?;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("NUWAX_WIRE_API") && stderr.contains("together"),
        "expected the partial-group error, got: {stderr}"
    );
    assert!(
        log.0.lock().unwrap().is_empty(),
        "no request may be attempted for a partial group"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nuwax_env_reserved_provider_conflict_fails_before_any_request() -> anyhow::Result<()> {
    let test = test_codex_exec();
    let server = start_mock_server().await;
    let repo_root = codex_utils_cargo_bin::repo_root()?;
    let log = RequestLog::default();
    Mock::given(method("POST"))
        .and(path_regex(".*"))
        .and(log.clone())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(RESPONSES_SSE.to_string(), "text/event-stream"),
        )
        .mount(&server)
        .await;
    // A stale reserved-id definition in the user config file carries old
    // credentials toward the environment endpoint; loading must fail before
    // any request, and the file must stay untouched.
    let home_config = test.home_path().join("config.toml");
    let conflicting = std::fs::read_to_string(&home_config).unwrap_or_default();
    let conflicting = if conflicting.is_empty() {
        r#"[model_providers.nuwax_env]
name = "stale definition"
http_headers = { x-old-gateway = "stale-credential" }
"#
        .to_string()
    } else {
        format!(
            "{conflicting}\n[model_providers.nuwax_env]\nname = \"stale definition\"\nhttp_headers = {{ x-old-gateway = \"stale-credential\" }}\n"
        )
    };
    std::fs::write(&home_config, &conflicting)?;
    let output = test
        .cmd()
        .arg("--skip-git-repo-check")
        .arg("-C")
        .arg(&repo_root)
        .arg("hello")
        .env_remove("NUWAX_MAX_OUTPUT_TOKENS")
        .env("NUWAX_BASE_URL", format!("{}/v1", server.uri()))
        .env("NUWAX_WIRE_API", "responses")
        .env("NUWAX_API_KEY", "nuwax-test-key")
        .env("NUWAX_MODEL", "nuwax-test-model")
        .output()?;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("nuwax_env") && stderr.contains("http_headers"),
        "expected the reserved-provider conflict error, got: {stderr}"
    );
    assert!(
        !stderr.contains("stale-credential"),
        "the error must not echo config values: {stderr}"
    );
    assert!(
        log.0.lock().unwrap().is_empty(),
        "no request may be attempted while the provider definition conflicts"
    );
    assert_eq!(
        std::fs::read_to_string(&home_config)?,
        conflicting,
        "the config file must not be rewritten"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nuwax_env_cli_provider_subkeys_and_parent_tables_fail_before_requests() -> Result<()> {
    for override_key in [
        "model_providers.nuwax_env.env_key=\"OLD_AUTH_KEY\"",
        "model_providers.nuwax_env.base_url=\"https://old.example/v1\"",
        "model_providers={nuwax_env={name=\"stale definition\"}}",
    ] {
        let test = test_codex_exec();
        let server = start_mock_server().await;
        let output = test
            .cmd()
            .arg("--skip-git-repo-check")
            .arg("-c")
            .arg(override_key)
            .arg("hello")
            .env_remove("NUWAX_MAX_OUTPUT_TOKENS")
            .env("NUWAX_BASE_URL", format!("{}/v1", server.uri()))
            .env("NUWAX_WIRE_API", "responses")
            .env("NUWAX_API_KEY", "nuwax-test-key")
            .env("NUWAX_MODEL", "nuwax-test-model")
            .output()?;
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("nuwax_env") && stderr.contains("reserved"),
            "{stderr}"
        );
        assert!(
            server
                .received_requests()
                .await
                .expect("received requests")
                .is_empty()
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nuwax_env_request_retries_reach_the_chat_wire() -> Result<()> {
    // The environment retry controls must drive the real HTTP attempt count
    // through the whole binary: one 503, then SSE. With one configured
    // retry the wire sees exactly two POSTs; with zero it sees one and the
    // turn fails.
    // Core sampling retries (stream_max_retries) would mask the handshake
    // layer, so the test pins them to zero: only the provider HTTP retry
    // budget may resend. Default request retries (4) and an explicit 1 both
    // recover on the second attempt; an explicit 0 sends exactly once and
    // the turn fails without any resample.
    for (retries, expected_requests, expect_success) in
        [(Some("1"), 2, true), (Some("0"), 1, false), (None, 2, true)]
    {
        let test = test_codex_exec();
        let server = start_mock_server().await;
        let log = RequestLog::default();
        // First reply: 503 (retryable). Following replies: valid SSE.
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(log.clone())
            .respond_with(ResponseTemplate::new(503).set_body_string("overloaded"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(log.clone())
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(CHAT_SSE.to_string(), "text/event-stream"),
            )
            .mount(&server)
            .await;
        let repo_root = codex_utils_cargo_bin::repo_root()?;
        let mut command = test.cmd();
        for variable in [
            "NUWAX_BASE_URL",
            "NUWAX_WIRE_API",
            "NUWAX_API_KEY",
            "NUWAX_MODEL",
            "NUWAX_REQUEST_MAX_RETRIES",
            "NUWAX_MAX_OUTPUT_TOKENS",
        ] {
            command.env_remove(variable);
        }
        command
            .arg("--skip-git-repo-check")
            .arg("-C")
            .arg(&repo_root)
            .arg("reply with ok")
            .env("NUWAX_BASE_URL", format!("{}/v1", server.uri()))
            .env("NUWAX_WIRE_API", "chat")
            .env("NUWAX_API_KEY", "nuwax-retry-key")
            .env("NUWAX_MODEL", "nuwax-test-model");
        if let Some(retries) = retries {
            command.env("NUWAX_REQUEST_MAX_RETRIES", retries);
        }
        command.env("NUWAX_STREAM_MAX_RETRIES", "0");
        let output = tokio::task::spawn_blocking(move || command.output()).await??;
        assert_eq!(
            output.status.success(),
            expect_success,
            "retries={retries:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let requests = log.all();
        assert_eq!(
            requests.len(),
            expected_requests,
            "retries={retries:?}: paths {:?}",
            requests
                .iter()
                .map(|request| request.url.path().to_string())
                .collect::<Vec<_>>()
        );
    }
    Ok(())
}
