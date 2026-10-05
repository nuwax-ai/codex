//! C2 acceptance: the NUWAX request/stream/idle environment controls drive
//! real behavior through the production `codex exec` binary — per-protocol
//! handshake attempt counts, the separate core sampling budget, idle-timeout
//! enforcement, cancellation during backoff and stream waits, concurrent
//! processes with distinct controls, and fail-fast parsing that names the
//! variable without echoing values.
//!
//! Two-layer budget contract (spec §S3): `NUWAX_REQUEST_MAX_RETRIES` is the
//! HTTP handshake budget (provider request_retry); `NUWAX_STREAM_MAX_RETRIES`
//! is the core sampling budget. Defaults: request 4, stream 5.

#![allow(clippy::unwrap_used)]
use anyhow::Result;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex_exec::test_codex_exec;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use wiremock::Mock;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const CHAT_SSE: &str = concat!(
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"nuwax-controls-model\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"ok\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"nuwax-controls-model\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":1,\"total_tokens\":5}}\n\n",
    "data: [DONE]\n\n",
);

const ANTHROPIC_SSE: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"nuwax-controls-model\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n",
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":1}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

const RESPONSES_SSE: &str = concat!(
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\",\"model\":\"nuwax-controls-model\"}}\n\n",
    "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"ok\"}],\"id\":\"o1\"}}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"usage\":{\"input_tokens\":4,\"output_tokens\":1,\"total_tokens\":5}}}\n\n",
);

/// A Chat SSE body whose stream dies mid-answer: one content delta, then the
/// body ends with no finish chunk and no `[DONE]` — the truncation class the
/// core sampling budget exists to resample.
const CHAT_TRUNCATED_SSE: &str = "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"nuwax-controls-model\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"par\"},\"finish_reason\":null}]}\n\n";

#[derive(Clone, Default)]
struct RequestLog(Arc<Mutex<Vec<wiremock::Request>>>);

impl wiremock::Match for RequestLog {
    fn matches(&self, request: &wiremock::Request) -> bool {
        self.0.lock().unwrap().push(request.clone());
        true
    }
}

impl RequestLog {
    fn count(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    fn paths(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.url.path().to_string())
            .collect()
    }

    fn bodies(&self) -> Vec<serde_json::Value> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|request| serde_json::from_slice(&request.body).expect("body json"))
            .collect()
    }
}

fn nuwax_command(
    test: &core_test_support::test_codex_exec::TestCodexExecBuilder,
    server_uri: &str,
    wire_api: &str,
    api_key: &str,
    model: &str,
) -> assert_cmd::Command {
    let repo_root = codex_utils_cargo_bin::repo_root().unwrap();
    let mut command = test.cmd();
    for variable in [
        "NUWAX_BASE_URL",
        "NUWAX_WIRE_API",
        "NUWAX_API_KEY",
        "NUWAX_MODEL",
        "NUWAX_MAX_OUTPUT_TOKENS",
        "NUWAX_REQUEST_MAX_RETRIES",
        "NUWAX_STREAM_MAX_RETRIES",
        "NUWAX_STREAM_IDLE_TIMEOUT_MS",
    ] {
        command.env_remove(variable);
    }
    command
        .arg("--skip-git-repo-check")
        .arg("-C")
        .arg(&repo_root)
        .arg("reply with ok")
        .env("NUWAX_BASE_URL", format!("{server_uri}/v1"))
        .env("NUWAX_WIRE_API", wire_api)
        .env("NUWAX_API_KEY", api_key)
        .env("NUWAX_MODEL", model);
    command
}

/// Handshake retry matrix on the Anthropic and Responses wires: 0 sends
/// exactly once and fails, 1 recovers on the second POST, unset uses the
/// default budget. Stream retries pinned to 0 so only the handshake layer
/// may resend.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nuwax_env_request_retries_reach_anthropic_and_responses_wires() -> Result<()> {
    // All six matrix cells run CONCURRENTLY: each cell is an independent
    // home/server pair, and cold-starting six exec children sequentially
    // overruns the per-test time budget on a loaded host.
    let mut cells = Vec::new();
    for (wire_api, expected_path, sse) in [
        ("anthropic", "/v1/messages", ANTHROPIC_SSE),
        ("responses", "/v1/responses", RESPONSES_SSE),
    ] {
        for (retries, expected_requests, expect_success) in
            [(Some("1"), 2, true), (Some("0"), 1, false), (None, 2, true)]
        {
            cells.push(async move {
                let test = test_codex_exec();
                let server = start_mock_server().await;
                let log = RequestLog::default();
                Mock::given(method("POST"))
                    .and(path(expected_path))
                    .and(log.clone())
                    .respond_with(ResponseTemplate::new(503).set_body_string("overloaded"))
                    .up_to_n_times(1)
                    .mount(&server)
                    .await;
                Mock::given(method("POST"))
                    .and(path(expected_path))
                    .and(log.clone())
                    .respond_with(
                        ResponseTemplate::new(200)
                            .set_body_raw(sse.to_string(), "text/event-stream"),
                    )
                    .mount(&server)
                    .await;
                let mut command = nuwax_command(
                    &test,
                    &server.uri(),
                    wire_api,
                    "nuwax-retry-key",
                    "nuwax-controls-model",
                );
                if let Some(retries) = retries {
                    command.env("NUWAX_REQUEST_MAX_RETRIES", retries);
                }
                command.env("NUWAX_STREAM_MAX_RETRIES", "0");
                let output = tokio::task::spawn_blocking(move || command.output()).await??;
                assert_eq!(
                    output.status.success(),
                    expect_success,
                    "wire {wire_api} retries={retries:?}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert_eq!(
                    log.count(),
                    expected_requests,
                    "wire {wire_api} retries={retries:?}: paths {:?}",
                    log.paths()
                );
                Ok::<(), anyhow::Error>(())
            });
        }
    }
    let handles: Vec<_> = cells.into_iter().map(tokio::spawn).collect();
    for handle in handles {
        handle.await??;
    }
    Ok(())
}

/// Core sampling budget: with the handshake succeeding (200), a stream that
/// dies mid-answer is resampled by the STREAM budget only — 0 sends once and
/// fails, 1 resends once, unset uses the default (5) for six POSTs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nuwax_env_stream_retries_resample_truncated_chat_streams() -> Result<()> {
    for (stream_retries, expected_requests) in [(Some("0"), 1), (Some("1"), 2), (None, 6)] {
        let test = test_codex_exec();
        let server = start_mock_server().await;
        let log = RequestLog::default();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(log.clone())
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(CHAT_TRUNCATED_SSE.to_string(), "text/event-stream"),
            )
            .mount(&server)
            .await;
        let mut command = nuwax_command(
            &test,
            &server.uri(),
            "chat",
            "nuwax-stream-key",
            "nuwax-controls-model",
        );
        command.env("NUWAX_REQUEST_MAX_RETRIES", "0");
        if let Some(stream_retries) = stream_retries {
            command.env("NUWAX_STREAM_MAX_RETRIES", stream_retries);
        }
        let output = tokio::task::spawn_blocking(move || command.output()).await??;
        assert!(
            !output.status.success(),
            "a truncated stream never completes the turn: retries={stream_retries:?}"
        );
        assert_eq!(
            log.count(),
            expected_requests,
            "stream retries={stream_retries:?}: paths {:?}",
            log.paths()
        );
    }
    Ok(())
}

/// Fail-fast parsing through the real binary: every invalid control value
/// exits non-zero, names the variable, never echoes the value, and sends no
/// request. Covers negative, non-numeric, overflow, blank, zero-timeout,
/// non-Unicode, and an orphaned control without the group.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nuwax_env_control_values_fail_fast_naming_the_variable() -> Result<()> {
    struct Case {
        label: &'static str,
        variable: &'static str,
        value: &'static str,
        expected_fragment: &'static str,
        complete_group: bool,
    }
    let cases = [
        Case {
            label: "negative",
            variable: "NUWAX_REQUEST_MAX_RETRIES",
            value: "-3",
            expected_fragment: "NUWAX_REQUEST_MAX_RETRIES",
            complete_group: true,
        },
        Case {
            label: "non-numeric",
            variable: "NUWAX_STREAM_MAX_RETRIES",
            value: "soon",
            expected_fragment: "NUWAX_STREAM_MAX_RETRIES",
            complete_group: true,
        },
        Case {
            label: "i64 overflow",
            variable: "NUWAX_REQUEST_MAX_RETRIES",
            value: "99999999999999999999999",
            expected_fragment: "NUWAX_REQUEST_MAX_RETRIES",
            complete_group: true,
        },
        Case {
            label: "blank",
            variable: "NUWAX_STREAM_IDLE_TIMEOUT_MS",
            value: "   ",
            expected_fragment: "NUWAX_STREAM_IDLE_TIMEOUT_MS",
            complete_group: true,
        },
        Case {
            label: "zero timeout",
            variable: "NUWAX_STREAM_IDLE_TIMEOUT_MS",
            value: "0",
            expected_fragment: "NUWAX_STREAM_IDLE_TIMEOUT_MS",
            complete_group: true,
        },
        Case {
            label: "orphaned control",
            variable: "NUWAX_REQUEST_MAX_RETRIES",
            value: "1",
            expected_fragment: "complete NUWAX temporary provider group",
            complete_group: false,
        },
    ];
    for case in cases {
        let test = test_codex_exec();
        let server = start_mock_server().await;
        let log = RequestLog::default();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(log.clone())
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(CHAT_SSE.to_string(), "text/event-stream"),
            )
            .mount(&server)
            .await;
        let secret = "nuwax-secret-value-never-echoed";
        let mut command =
            nuwax_command(&test, &server.uri(), "chat", secret, "nuwax-controls-model");
        if !case.complete_group {
            command.env_remove("NUWAX_BASE_URL");
            command.env_remove("NUWAX_WIRE_API");
            command.env_remove("NUWAX_API_KEY");
        }
        command.env(case.variable, case.value);
        let label = case.label;
        let output = tokio::task::spawn_blocking(move || command.output()).await??;
        assert!(
            !output.status.success(),
            "{label}: an invalid control must fail the process"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(case.expected_fragment),
            "{label}: expected `{}` in error, got: {stderr}",
            case.expected_fragment
        );
        assert!(
            !stderr.contains(secret),
            "{label}: the error must not echo the credential value"
        );
        assert!(log.count() == 0, "{label}: no request may be attempted");
    }
    Ok(())
}

/// A non-Unicode control value must be rejected by name even though OS env
/// values are not guaranteed UTF-8.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nuwax_env_non_unicode_control_fails_naming_the_variable() -> Result<()> {
    use std::os::unix::ffi::OsStringExt;
    let test = test_codex_exec();
    let server = start_mock_server().await;
    let log = RequestLog::default();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(log.clone())
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(CHAT_SSE.to_string(), "text/event-stream"),
        )
        .mount(&server)
        .await;
    let mut command = nuwax_command(
        &test,
        &server.uri(),
        "chat",
        "nuwax-test-key",
        "nuwax-controls-model",
    );
    command.env(
        "NUWAX_STREAM_MAX_RETRIES",
        std::ffi::OsString::from_vec(vec![0xff, 0xfe]),
    );
    let output = tokio::task::spawn_blocking(move || command.output()).await??;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("NUWAX_STREAM_MAX_RETRIES") && stderr.contains("Unicode"),
        "expected the non-Unicode error naming the variable, got: {stderr}"
    );
    assert!(log.count() == 0, "no request may be attempted");
    Ok(())
}

/// An explicitly selected named provider ignores the whole NUWAX group —
/// including control values that would otherwise fail parsing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nuwax_env_invalid_controls_are_ignored_for_explicit_named_provider() -> Result<()> {
    let test = test_codex_exec();
    let server = start_mock_server().await;
    let log = RequestLog::default();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(log.clone())
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(CHAT_SSE.to_string(), "text/event-stream"),
        )
        .mount(&server)
        .await;
    let config_path = test.home_path().join("config.toml");
    let original = std::fs::read_to_string(&config_path).unwrap_or_default();
    let provider_config = format!(
        "{}\nmodel = \"named-mock-model\"\nmodel_provider = \"named_mock\"\n\n[model_providers.named_mock]\nname = \"Named Mock\"\nbase_url = \"{}/v1\"\nwire_api = \"chat\"\nenv_key = \"NAMED_MOCK_API_KEY\"\n",
        if original.is_empty() {
            "# explicit provider\n".to_string()
        } else {
            original.clone()
        },
        server.uri(),
    );
    std::fs::write(&config_path, &provider_config)?;
    let repo_root = codex_utils_cargo_bin::repo_root()?;
    let mut command = test.cmd();
    for variable in [
        "NUWAX_BASE_URL",
        "NUWAX_WIRE_API",
        "NUWAX_API_KEY",
        "NUWAX_MODEL",
        "NUWAX_MAX_OUTPUT_TOKENS",
        "NUWAX_REQUEST_MAX_RETRIES",
        "NUWAX_STREAM_MAX_RETRIES",
        "NUWAX_STREAM_IDLE_TIMEOUT_MS",
    ] {
        command.env_remove(variable);
    }
    command
        .arg("--skip-git-repo-check")
        .arg("-C")
        .arg(&repo_root)
        // An explicit CLI provider override ignores the whole NUWAX group —
        // including control values that would otherwise fail parsing.
        .arg("-c")
        .arg("model_provider=\"named_mock\"")
        .arg("reply with ok")
        .env("NAMED_MOCK_API_KEY", "named-mock-key")
        // The whole group — invalid control included — must be ignored.
        .env("NUWAX_BASE_URL", "http://127.0.0.1:9/v1")
        .env("NUWAX_WIRE_API", "anthropic")
        .env("NUWAX_API_KEY", "nuwax-group-key")
        .env("NUWAX_MODEL", "nuwax-group-model")
        .env("NUWAX_REQUEST_MAX_RETRIES", "-1");
    let output = tokio::task::spawn_blocking(move || command.output()).await??;
    assert!(
        output.status.success(),
        "explicit provider must ignore the NUWAX group: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = log.0.lock().unwrap().clone();
    assert_eq!(
        requests.len(),
        1,
        "exactly one request via the named provider"
    );
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body)?;
    assert_eq!(body["model"], serde_json::json!("named-mock-model"));
    let credential = requests[0]
        .headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert_eq!(credential, "Bearer named-mock-key");
    assert_eq!(
        std::fs::read_to_string(&config_path)?,
        provider_config,
        "the named provider config must not be rewritten"
    );
    Ok(())
}
